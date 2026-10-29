//! Durable request intent and reconciliation for bounded host capabilities.
//! Delivery is explicitly separate from the universe's transaction.
pub use crate::model::ModelRequest;
use crate::{
    checkpoint::CheckpointStore,
    error::{Error, Result, require},
    execution::{Continuation, ExecutionStatus},
    heap::MAX_STRING_UNITS,
    identity::{EffectId, ObjectId, TypeId},
    model::{
        DEFAULT_MODEL, MAX_MESSAGES, MessageReference, MessageRole, ModelMessage,
        validate_identifier,
    },
    program::SlotType,
    runtime::Vm,
    value::Value,
};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use std::{collections::BTreeSet, future::Future, path::Path, time::Duration};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum EffectState {
    Prepared,
    InFlight,
    OutcomeUnknown { reason: String },
    Completed { response: ObjectId },
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EffectRecord {
    pub id: EffectId,
    pub capability: String,
    pub request: ObjectId,
    pub model: String,
    pub messages: Vec<MessageReference>,
    pub state: EffectState,
}
impl EffectRecord {
    pub fn references(&self) -> impl Iterator<Item = ObjectId> {
        let response = match self.state {
            EffectState::Completed { response } => Some(response),
            _ => None,
        };
        std::iter::once(self.request)
            .chain(self.messages.iter().map(|message| message.content))
            .chain(response)
    }
}
/// A submission error means an uncertain outcome. Reconciliation returning None
/// is not evidence that a provider did not accept or charge for a request.
pub trait ModelProvider {
    fn capability(&self) -> &str;
    fn supports_model(&self, model: &str) -> bool {
        model == DEFAULT_MODEL
    }
    /// Local/authorization checks only; must not submit inference. A rejection
    /// leaves the effect Prepared. Delivery starts after the dispatch checkpoint.
    fn validate_request(&mut self, _request: &ModelRequest) -> Result<()> {
        Ok(())
    }
    fn submit(&mut self, request: &ModelRequest) -> Result<Vec<u16>>;
    fn reconcile(&mut self, request: &ModelRequest) -> Result<Option<Vec<u16>>>;
}

/// Host futures are transient. Dropping delivery after its dispatch checkpoint
/// leaves InFlight/OutcomeUnknown; a later invocation must reconcile, not resend.
/// Implementations must yield while waiting rather than block an executor worker.
pub trait AsyncModelProvider: Send {
    fn capability(&self) -> &str;
    fn supports_model(&self, model: &str) -> bool {
        model == DEFAULT_MODEL
    }
    fn validate_request(
        &mut self,
        _request: &ModelRequest,
    ) -> impl Future<Output = Result<()>> + Send {
        std::future::ready(Ok(()))
    }
    fn submit(&mut self, request: &ModelRequest) -> impl Future<Output = Result<Vec<u16>>> + Send;
    fn reconcile(
        &mut self,
        request: &ModelRequest,
    ) -> impl Future<Output = Result<Option<Vec<u16>>>> + Send;
}

impl Vm<'_> {
    pub fn new_string(&mut self, units: Vec<u16>) -> Result<ObjectId> {
        require(
            units.len() <= MAX_STRING_UNITS,
            "string exceeds the UTF-16 unit limit",
        )?;
        crate::endowment::text(self, units.len())?;
        let mut id = ObjectId::fresh();
        while self.heap.object(id).is_ok() {
            id = ObjectId::fresh();
        }
        let allocation = crate::endowment::allocation(self, id, self.program.string_type()?)?;
        self.heap
            .allocate_string(id, self.program.string_type()?, units)?;
        crate::endowment::allocated(self, allocation)?;
        Ok(id)
    }
    pub fn effects(&self) -> impl Iterator<Item = &EffectRecord> {
        self.effects.values()
    }
    pub fn effect(&self, id: EffectId) -> Result<&EffectRecord> {
        self.effects
            .get(&id)
            .ok_or_else(|| Error(format!("unknown effect {id}")))
    }
    pub fn queue_model_request(&mut self, request: ObjectId) -> Result<EffectId> {
        let effect = self.prepare_model_request(request)?;
        let id = effect.id;
        self.effects.insert(id, effect);
        Ok(id)
    }
    pub(crate) fn prepare_model_request(&self, request: ObjectId) -> Result<EffectRecord> {
        self.prepare_chat_request(
            "local.echo".into(),
            DEFAULT_MODEL.into(),
            vec![MessageReference {
                role: MessageRole::User,
                content: request,
            }],
        )
    }
    pub fn queue_chat_request(
        &mut self,
        capability: String,
        model: String,
        messages: Vec<MessageReference>,
    ) -> Result<EffectId> {
        let effect = self.prepare_chat_request(capability, model, messages)?;
        let id = effect.id;
        self.effects.insert(id, effect);
        Ok(id)
    }
    fn prepare_chat_request(
        &self,
        capability: String,
        model: String,
        messages: Vec<MessageReference>,
    ) -> Result<EffectRecord> {
        validate_identifier(&capability)?;
        validate_identifier(&model)?;
        crate::endowment::effect(self, &capability, &model)?;
        require(
            !messages.is_empty() && messages.len() <= MAX_MESSAGES,
            "invalid model message count",
        )?;
        for message in &messages {
            crate::endowment::value(self, Value::Reference(message.content))?;
        }
        let request = messages.last().unwrap().content;
        let body = self.materialize_request(EffectId::fresh(), &capability, &model, &messages)?;
        body.validate()?;
        require(
            self.effects.len() < 1_000_000,
            "effect outbox exceeds its record limit",
        )?;
        let mut id = EffectId::fresh();
        while self.effects.contains_key(&id) {
            id = EffectId::fresh();
        }
        Ok(EffectRecord {
            id,
            capability,
            request,
            model,
            messages,
            state: EffectState::Prepared,
        })
    }
    fn model_field(
        &self,
        object: ObjectId,
        owner: TypeId,
        name: &str,
        ty: SlotType,
    ) -> Result<Value> {
        crate::endowment::value(self, Value::Reference(object))?;
        let object = self.heap.object(object)?;
        require(
            object.type_id() == owner,
            "model argument has the wrong nominal type",
        )?;
        let field = self
            .program
            .fields()
            .find(|f| f.owner == owner && f.name == name && f.value_type == ty && !f.is_static)
            .ok_or_else(|| Error(format!("invalid model argument field {name}")))?;
        crate::endowment::field(self, field.id, false)?;
        let value = object
            .fields()
            .get(&field.id)
            .copied()
            .ok_or_else(|| Error("missing model argument field".into()))?;
        crate::endowment::value(self, value)?;
        Ok(value)
    }
    pub(crate) fn prepare_guest_chat(
        &self,
        binding: ObjectId,
        head: ObjectId,
        binding_type: TypeId,
        message_type: TypeId,
    ) -> Result<EffectRecord> {
        let string = SlotType::Reference(self.program.string_type()?);
        let read_name = |field| -> Result<String> {
            let Value::Reference(id) = self.model_field(binding, binding_type, field, string)?
            else {
                return Err(Error("model binding names must be non-null strings".into()));
            };
            String::from_utf16(
                self.heap
                    .object(id)?
                    .string_units()
                    .ok_or_else(|| Error("model binding name is not a string".into()))?,
            )
            .map_err(|_| Error("invalid model binding UTF-16".into()))
        };
        let capability = read_name("Capability")?;
        let model = read_name("Model")?;
        let mut messages = Vec::new();
        let mut visited = BTreeSet::new();
        let mut next = Some(head);
        while let Some(id) = next {
            require(visited.insert(id), "model message chain contains a cycle")?;
            require(
                messages.len() < MAX_MESSAGES,
                "model message chain exceeds its limit",
            )?;
            let Value::I32(role) = self.model_field(id, message_type, "Role", SlotType::I32)?
            else {
                return Err(Error("model role is not Int32".into()));
            };
            let role = MessageRole::from_tag(
                u8::try_from(role).map_err(|_| Error("unsupported model message role".into()))?,
            )?;
            let Value::Reference(content) =
                self.model_field(id, message_type, "Content", string)?
            else {
                return Err(Error(
                    "model message content must be a non-null string".into(),
                ));
            };
            messages.push(MessageReference { role, content });
            next = match self.model_field(
                id,
                message_type,
                "Next",
                SlotType::Reference(message_type),
            )? {
                Value::Null => None,
                Value::Reference(id) => Some(id),
                _ => return Err(Error("invalid model message link".into())),
            };
        }
        self.prepare_chat_request(capability, model, messages)
    }
    fn model_request(&self, id: EffectId) -> Result<ModelRequest> {
        let record = self.effect(id)?;
        self.materialize_request(id, &record.capability, &record.model, &record.messages)
    }
    pub(crate) fn materialize_request(
        &self,
        id: EffectId,
        capability: &str,
        model: &str,
        messages: &[MessageReference],
    ) -> Result<ModelRequest> {
        let mut total = 0usize;
        let messages = messages
            .iter()
            .map(|message| {
                let object = self.heap.object(message.content)?;
                require(
                    object.type_id() == self.program.string_type()?,
                    "model message must be a managed string",
                )?;
                let content = object
                    .string_units()
                    .ok_or_else(|| Error("model message payload is not a string".into()))?;
                total = total
                    .checked_add(content.len())
                    .ok_or_else(|| Error("model request size overflow".into()))?;
                require(
                    total <= MAX_STRING_UNITS,
                    "model request exceeds the UTF-16 unit limit",
                )?;
                Ok(ModelMessage {
                    role: message.role,
                    content: content.to_vec(),
                })
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(ModelRequest {
            id,
            capability: capability.into(),
            model: model.into(),
            messages,
        })
    }
    /// Commit intent and the dispatch marker before calling external code.
    /// Completed and uncertain effects are never automatically resubmitted.
    pub fn dispatch_model(
        &mut self,
        store: &mut CheckpointStore,
        id: EffectId,
        provider: &mut impl ModelProvider,
    ) -> Result<ObjectId> {
        self.check_model_provider(
            id,
            provider.capability(),
            provider.supports_model(&self.effect(id)?.model),
        )?;
        let Some(request) = self.dispatch_request(id)? else {
            let EffectState::Completed { response } = self.effect(id)?.state else {
                unreachable!()
            };
            return Ok(response);
        };
        provider.validate_request(&request)?;
        self.mark_model_in_flight(store, id)?;
        let result = provider.submit(&request);
        self.finish_model_delivery(store, id, result)
    }
    pub async fn dispatch_model_async(
        &mut self,
        store: &mut CheckpointStore,
        id: EffectId,
        provider: &mut impl AsyncModelProvider,
    ) -> Result<ObjectId> {
        self.check_model_provider(
            id,
            provider.capability(),
            provider.supports_model(&self.effect(id)?.model),
        )?;
        let Some(request) = self.dispatch_request(id)? else {
            let EffectState::Completed { response } = self.effect(id)?.state else {
                unreachable!()
            };
            return Ok(response);
        };
        provider.validate_request(&request).await?;
        self.mark_model_in_flight(store, id)?;
        let result = provider.submit(&request).await;
        self.finish_model_delivery(store, id, result)
    }
    fn check_model_provider(
        &self,
        id: EffectId,
        capability: &str,
        model_supported: bool,
    ) -> Result<()> {
        require(
            self.effect(id)?.capability == capability,
            "provider does not match the effect capability",
        )?;
        require(model_supported, "provider does not support the saved model")
    }
    fn dispatch_request(&self, id: EffectId) -> Result<Option<ModelRequest>> {
        match self.effect(id)?.state {
            EffectState::Completed { .. } => return Ok(None),
            EffectState::Prepared => {}
            _ => {
                return Err(Error(
                    "effect outcome is uncertain; reconcile before continuing".into(),
                ));
            }
        }
        self.model_request(id).map(Some)
    }
    fn mark_model_in_flight(&mut self, store: &mut CheckpointStore, id: EffectId) -> Result<()> {
        self.effects.get_mut(&id).unwrap().state = EffectState::InFlight;
        // On a save error do not submit, and keep the memory state conservative.
        store.save(self).map(|_| ())
    }
    fn finish_model_delivery(
        &mut self,
        store: &mut CheckpointStore,
        id: EffectId,
        result: Result<Vec<u16>>,
    ) -> Result<ObjectId> {
        match result {
            Ok(response) => self.complete_model(store, id, response),
            Err(error) => {
                self.effects.get_mut(&id).unwrap().state = EffectState::OutcomeUnknown {
                    reason: error.to_string(),
                };
                store.save(self)?;
                Err(Error(format!(
                    "model request outcome is uncertain: {error}"
                )))
            }
        }
    }
    fn complete_model(
        &mut self,
        store: &mut CheckpointStore,
        id: EffectId,
        units: Vec<u16>,
    ) -> Result<ObjectId> {
        let response = self.new_string(units)?;
        let previous_context = self.context.clone();
        if self.context.status == (ExecutionStatus::Waiting { effect: id }) {
            let mut frame = self
                .context
                .frames
                .last()
                .ok_or_else(|| Error("waiting context has no frame".into()))?
                .clone();
            let Continuation::AwaitEffect {
                effect,
                request,
                next,
                ..
            } = frame.continuation
            else {
                return Err(Error("waiting context has no effect continuation".into()));
            };
            require(
                effect == id && request == self.effect(id)?.request,
                "waiting effect does not match its request",
            )?;
            require(
                frame.evaluation_stack.len() < self.current_method(&frame)?.max_stack,
                "model result exceeds maxstack",
            )?;
            frame.evaluation_stack.push(Value::Reference(response));
            frame.continuation = Continuation::Execute { next };
            *self.context.frames.last_mut().unwrap() = frame;
            self.context.status = ExecutionStatus::Runnable;
        }
        self.effects.get_mut(&id).unwrap().state = EffectState::Completed { response };
        if let Err(error) = store.save(self) {
            self.context = previous_context;
            self.effects.get_mut(&id).unwrap().state = EffectState::OutcomeUnknown {
                reason: format!("response checkpoint failed: {error}"),
            };
            return Err(error);
        }
        Ok(response)
    }
    /// Look up a receipt using the same stable effect ID. Absence leaves the
    /// request uncertain; this method never falls back to a second submission.
    pub fn reconcile_model(
        &mut self,
        store: &mut CheckpointStore,
        id: EffectId,
        provider: &mut impl ModelProvider,
    ) -> Result<Option<ObjectId>> {
        self.check_model_provider(
            id,
            provider.capability(),
            provider.supports_model(&self.effect(id)?.model),
        )?;
        if let Some(response) = self.reconciliation_response(id)? {
            return Ok(Some(response));
        }
        let request = self.model_request(id)?;
        let result = provider.reconcile(&request)?;
        self.finish_model_reconciliation(store, id, result)
    }
    pub async fn reconcile_model_async(
        &mut self,
        store: &mut CheckpointStore,
        id: EffectId,
        provider: &mut impl AsyncModelProvider,
    ) -> Result<Option<ObjectId>> {
        self.check_model_provider(
            id,
            provider.capability(),
            provider.supports_model(&self.effect(id)?.model),
        )?;
        if let Some(response) = self.reconciliation_response(id)? {
            return Ok(Some(response));
        }
        let request = self.model_request(id)?;
        let result = provider.reconcile(&request).await?;
        self.finish_model_reconciliation(store, id, result)
    }
    fn reconciliation_response(&self, id: EffectId) -> Result<Option<ObjectId>> {
        match self.effect(id)?.state {
            EffectState::Completed { response } => return Ok(Some(response)),
            EffectState::InFlight | EffectState::OutcomeUnknown { .. } => {}
            EffectState::Prepared => {
                return Err(Error("prepared effect has not been dispatched".into()));
            }
        }
        Ok(None)
    }
    fn finish_model_reconciliation(
        &mut self,
        store: &mut CheckpointStore,
        id: EffectId,
        result: Option<Vec<u16>>,
    ) -> Result<Option<ObjectId>> {
        match result {
            Some(response) => self.complete_model(store, id, response).map(Some),
            None => Ok(None),
        }
    }
    pub fn forget_completed_effect(&mut self, id: EffectId) -> Result<()> {
        require(
            matches!(self.effect(id)?.state, EffectState::Completed { .. }),
            "only completed effects can be forgotten",
        )?;
        self.effects.remove(&id);
        Ok(())
    }
}

const PROVIDER_ID: i64 = 0x52454d4f;

/// A deterministic local model substitute. Receipts commit in a separate
/// database, deliberately outside the universe checkpoint transaction.
pub struct LocalModelProvider {
    connection: Connection,
}
impl LocalModelProvider {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let mut connection = Connection::open(path)?;
        connection.busy_timeout(Duration::from_secs(5))?;
        let application: i64 =
            connection.pragma_query_value(None, "application_id", |r| r.get(0))?;
        let version: i64 = connection.pragma_query_value(None, "user_version", |r| r.get(0))?;
        if application == 0 && version == 0 {
            let tables: i64 = connection.query_row(
                "SELECT count(*) FROM sqlite_master WHERE name NOT LIKE 'sqlite_%'",
                [],
                |r| r.get(0),
            )?;
            require(tables == 0, "provider ledger is not an empty database")?;
            let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            tx.execute_batch(
                "CREATE TABLE receipt (
                effect BLOB PRIMARY KEY CHECK(length(effect)=16),
                request BLOB NOT NULL,
                response BLOB NOT NULL,
                attempts INTEGER NOT NULL CHECK(attempts>0)
            );",
            )?;
            tx.pragma_update(None, "application_id", PROVIDER_ID)?;
            tx.pragma_update(None, "user_version", 1)?;
            tx.commit()?;
        } else {
            require(
                application == PROVIDER_ID && matches!(version, 1 | 2),
                "not a supported local model ledger",
            )?;
        }
        let mode: String = connection.query_row("PRAGMA journal_mode=WAL", [], |r| r.get(0))?;
        require(
            mode.eq_ignore_ascii_case("wal"),
            "provider ledger requires WAL mode",
        )?;
        connection.pragma_update(None, "synchronous", "FULL")?;
        if connection.pragma_query_value(None, "user_version", |r| r.get::<_, i64>(0))? == 1 {
            let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            tx.execute_batch("ALTER TABLE receipt ADD COLUMN request_format INTEGER NOT NULL DEFAULT 1 CHECK(request_format IN (1,2));")?;
            tx.pragma_update(None, "user_version", 2)?;
            tx.commit()?;
        }
        Ok(Self { connection })
    }
    pub fn submission_attempts(&self, id: EffectId) -> Result<u64> {
        let attempts: Option<i64> = self
            .connection
            .query_row(
                "SELECT attempts FROM receipt WHERE effect=?1",
                params![id.as_bytes().as_slice()],
                |r| r.get(0),
            )
            .optional()?;
        u64::try_from(attempts.unwrap_or(0))
            .map_err(|_| Error("invalid provider attempt count".into()))
    }
}
fn encode_units(units: &[u16]) -> Result<Vec<u8>> {
    require(
        units.len() <= MAX_STRING_UNITS,
        "provider string exceeds the UTF-16 unit limit",
    )?;
    Ok(units.iter().flat_map(|u| u.to_le_bytes()).collect())
}
fn decode_units(bytes: &[u8]) -> Result<Vec<u16>> {
    require(
        bytes.len().is_multiple_of(2) && bytes.len() / 2 <= MAX_STRING_UNITS,
        "invalid provider UTF-16 payload",
    )?;
    Ok(bytes
        .as_chunks::<2>()
        .0
        .iter()
        .copied()
        .map(u16::from_le_bytes)
        .collect())
}
impl ModelProvider for LocalModelProvider {
    fn capability(&self) -> &str {
        "local.echo"
    }
    fn submit(&mut self, request: &ModelRequest) -> Result<Vec<u16>> {
        require(
            request.capability == self.capability() && self.supports_model(&request.model),
            "unsupported local model binding",
        )?;
        let encoded = request.encode()?;
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let existing: Option<(Vec<u8>, Vec<u8>, i64)> = tx
            .query_row(
                "SELECT request,response,request_format FROM receipt WHERE effect=?1",
                params![request.id.as_bytes().as_slice()],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?;
        let response = if let Some((prompt, response, format)) = existing {
            require(
                receipt_matches(request, &encoded, &prompt, format)?,
                "effect ID was reused with a different request",
            )?;
            tx.execute(
                "UPDATE receipt SET attempts=attempts+1 WHERE effect=?1",
                params![request.id.as_bytes().as_slice()],
            )?;
            decode_units(&response)?
        } else {
            let mut response: Vec<u16> = "echo: ".encode_utf16().collect();
            response.extend(&request.messages.last().unwrap().content);
            let encoded_response = encode_units(&response)?;
            tx.execute(
                "INSERT INTO receipt(effect,request,response,attempts,request_format) VALUES(?1,?2,?3,1,2)",
                params![request.id.as_bytes().as_slice(), encoded, encoded_response],
            )?;
            response
        };
        tx.commit()?;
        Ok(response)
    }
    fn reconcile(&mut self, request: &ModelRequest) -> Result<Option<Vec<u16>>> {
        require(
            request.capability == self.capability() && self.supports_model(&request.model),
            "unsupported local model binding",
        )?;
        let encoded = request.encode()?;
        let existing: Option<(Vec<u8>, Vec<u8>, i64)> = self
            .connection
            .query_row(
                "SELECT request,response,request_format FROM receipt WHERE effect=?1",
                params![request.id.as_bytes().as_slice()],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?;
        match existing {
            Some((prompt, response, format)) => {
                require(
                    receipt_matches(request, &encoded, &prompt, format)?,
                    "effect receipt does not match the request",
                )?;
                decode_units(&response).map(Some)
            }
            None => Ok(None),
        }
    }
}
fn receipt_matches(
    request: &ModelRequest,
    encoded: &[u8],
    saved: &[u8],
    format: i64,
) -> Result<bool> {
    match format {
        1 => match request.legacy_prompt() {
            Some(prompt) => Ok(encode_units(prompt)? == saved),
            None => Ok(false),
        },
        2 => Ok(encoded == saved),
        _ => Err(Error("unsupported model receipt request format".into())),
    }
}
