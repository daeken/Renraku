//! Durable request intent and reconciliation for bounded host capabilities.
//! Delivery is explicitly separate from the universe's transaction.
use crate::{
    checkpoint::CheckpointStore,
    error::{Error, Result, require},
    execution::{Continuation, ExecutionStatus},
    heap::MAX_STRING_UNITS,
    identity::{EffectId, ObjectId},
    runtime::Vm,
    value::Value,
};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use std::{path::Path, time::Duration};

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
    pub state: EffectState,
}
impl EffectRecord {
    pub fn references(&self) -> impl Iterator<Item = ObjectId> {
        let response = match self.state {
            EffectState::Completed { response } => Some(response),
            _ => None,
        };
        std::iter::once(self.request).chain(response)
    }
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ModelRequest {
    pub id: EffectId,
    pub prompt: Vec<u16>,
}

/// A submission error means an uncertain outcome. Reconciliation returning None
/// is not evidence that a provider did not accept or charge for a request.
pub trait ModelProvider {
    fn capability(&self) -> &str;
    fn submit(&mut self, request: &ModelRequest) -> Result<Vec<u16>>;
    fn reconcile(&mut self, request: &ModelRequest) -> Result<Option<Vec<u16>>>;
}

impl Vm<'_> {
    pub fn new_string(&mut self, units: Vec<u16>) -> Result<ObjectId> {
        require(
            units.len() <= MAX_STRING_UNITS,
            "string exceeds the UTF-16 unit limit",
        )?;
        let mut id = ObjectId::fresh();
        while self.heap.object(id).is_ok() {
            id = ObjectId::fresh();
        }
        self.heap
            .allocate_string(id, self.program.string_type()?, units)?;
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
        require(
            self.heap.object(request)?.string_units().is_some(),
            "model request must be a managed string",
        )?;
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
            capability: "local.echo".into(),
            request,
            state: EffectState::Prepared,
        })
    }
    fn model_request(&self, id: EffectId) -> Result<ModelRequest> {
        let record = self.effect(id)?;
        let prompt = self
            .heap
            .object(record.request)?
            .string_units()
            .ok_or_else(|| Error("model request payload is not a string".into()))?
            .to_vec();
        Ok(ModelRequest { id, prompt })
    }
    /// Commit intent and the dispatch marker before calling external code.
    /// Completed and uncertain effects are never automatically resubmitted.
    pub fn dispatch_model(
        &mut self,
        store: &mut CheckpointStore,
        id: EffectId,
        provider: &mut impl ModelProvider,
    ) -> Result<ObjectId> {
        require(
            self.effect(id)?.capability == provider.capability(),
            "provider does not match the effect capability",
        )?;
        match self.effect(id)?.state {
            EffectState::Completed { response } => return Ok(response),
            EffectState::Prepared => {}
            _ => {
                return Err(Error(
                    "effect outcome is uncertain; reconcile before continuing".into(),
                ));
            }
        }
        let request = self.model_request(id)?;
        self.effects.get_mut(&id).unwrap().state = EffectState::InFlight;
        // On a save error do not submit, and keep the memory state conservative.
        store.save(self)?;
        match provider.submit(&request) {
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
        require(
            self.effect(id)?.capability == provider.capability(),
            "provider does not match the effect capability",
        )?;
        match self.effect(id)?.state {
            EffectState::Completed { response } => return Ok(Some(response)),
            EffectState::InFlight | EffectState::OutcomeUnknown { .. } => {}
            EffectState::Prepared => {
                return Err(Error("prepared effect has not been dispatched".into()));
            }
        }
        let request = self.model_request(id)?;
        match provider.reconcile(&request)? {
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
                application == PROVIDER_ID && version == 1,
                "not a supported local model ledger",
            )?;
        }
        let mode: String = connection.query_row("PRAGMA journal_mode=WAL", [], |r| r.get(0))?;
        require(
            mode.eq_ignore_ascii_case("wal"),
            "provider ledger requires WAL mode",
        )?;
        connection.pragma_update(None, "synchronous", "FULL")?;
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
        let encoded = encode_units(&request.prompt)?;
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let existing: Option<(Vec<u8>, Vec<u8>)> = tx
            .query_row(
                "SELECT request,response FROM receipt WHERE effect=?1",
                params![request.id.as_bytes().as_slice()],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        let response = if let Some((prompt, response)) = existing {
            require(
                prompt == encoded,
                "effect ID was reused with a different request",
            )?;
            tx.execute(
                "UPDATE receipt SET attempts=attempts+1 WHERE effect=?1",
                params![request.id.as_bytes().as_slice()],
            )?;
            decode_units(&response)?
        } else {
            let mut response: Vec<u16> = "echo: ".encode_utf16().collect();
            response.extend(&request.prompt);
            let encoded_response = encode_units(&response)?;
            tx.execute(
                "INSERT INTO receipt(effect,request,response,attempts) VALUES(?1,?2,?3,1)",
                params![request.id.as_bytes().as_slice(), encoded, encoded_response],
            )?;
            response
        };
        tx.commit()?;
        Ok(response)
    }
    fn reconcile(&mut self, request: &ModelRequest) -> Result<Option<Vec<u16>>> {
        let existing: Option<(Vec<u8>, Vec<u8>)> = self
            .connection
            .query_row(
                "SELECT request,response FROM receipt WHERE effect=?1",
                params![request.id.as_bytes().as_slice()],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        match existing {
            Some((prompt, response)) => {
                require(
                    prompt == encode_units(&request.prompt)?,
                    "effect receipt does not match the request",
                )?;
                decode_units(&response).map(Some)
            }
            None => Ok(None),
        }
    }
}
