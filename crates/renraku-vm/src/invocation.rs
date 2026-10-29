//! Durable host invocation receipts. Request IDs bind one call, not a retry policy.
use crate::{
    checkpoint::CheckpointStore,
    error::{Result, require},
    execution::{ExecutionStatus, MethodVersion},
    identity::{ExecutionId, MethodId, ObjectId},
    runtime::{RunOutcome, Vm},
    value::Value,
};
use std::collections::BTreeSet;

pub const MAX_INVOCATIONS: usize = 10_000;

/// Explicit receiver and exact code binding for a trusted host instance entry.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InstanceCall {
    pub method: MethodVersion,
    pub receiver: ObjectId,
    pub arguments: Vec<Value>,
}
impl InstanceCall {
    pub(crate) fn frame_arguments(self) -> Vec<Value> {
        let mut values = vec![Value::Reference(self.receiver)];
        values.extend(self.arguments);
        values
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum InvocationState {
    Running,
    Completed { result: Option<Value> },
    Faulted { message: String },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InvocationRecord {
    pub method: MethodVersion,
    pub arguments: Vec<Value>,
    pub execution: ExecutionId,
    pub state: InvocationState,
}
impl InvocationRecord {
    pub fn receiver(&self, vm: &Vm<'_>) -> Option<ObjectId> {
        vm.program
            .methods
            .get(&self.method.code)
            .filter(|m| m.signature.instance)
            .and_then(|_| self.arguments.first().and_then(Value::referenced_object))
    }
    pub fn references(&self) -> impl Iterator<Item = ObjectId> + '_ {
        let result = match self.state {
            InvocationState::Completed { result } => result.and_then(|v| v.referenced_object()),
            _ => None,
        };
        self.arguments
            .iter()
            .filter_map(Value::referenced_object)
            .chain(result)
    }
}

pub(crate) fn validate_request_id(id: &str) -> Result<()> {
    require(
        !id.is_empty()
            && id.len() <= 128
            && id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"._-/:".contains(&b)),
        "workbench request ID must contain 1-128 ASCII identifier characters",
    )
}

impl Vm<'_> {
    pub(crate) fn validate_invocations(&self) -> Result<()> {
        require(
            self.invocations.len() <= MAX_INVOCATIONS,
            "invocation receipt limit reached",
        )?;
        let mut executions = BTreeSet::new();
        for (request, record) in &self.invocations {
            validate_request_id(request)?;
            require(
                executions.insert(record.execution),
                "duplicate invocation execution identity",
            )?;
            let method = self.entry_method(record.method)?;
            self.check_entry_arguments(method, &record.arguments)?;
            match &record.state {
                InvocationState::Running => require(
                    record.execution == self.context.id
                        && !matches!(
                            self.context.status,
                            ExecutionStatus::Completed { .. } | ExecutionStatus::Faulted { .. }
                        ),
                    "running invocation does not own the active execution",
                )?,
                InvocationState::Completed { result } => match (result, method.signature.result) {
                    (Some(value), Some(slot)) => self.check_value(*value, slot)?,
                    (None, None) => {}
                    _ => {
                        return Err(crate::error::Error(
                            "invalid invocation completion result".into(),
                        ));
                    }
                },
                InvocationState::Faulted { message } => {
                    require(!message.is_empty(), "empty invocation diagnostic")?
                }
            }
            if record.execution == self.context.id {
                require(
                    self.entry == Some(record.method),
                    "invocation execution has a different entry binding",
                )?;
                let matching = match (&record.state, &self.context.status) {
                    (
                        InvocationState::Running,
                        ExecutionStatus::Runnable
                        | ExecutionStatus::Paused
                        | ExecutionStatus::Waiting { .. }
                        | ExecutionStatus::WaitingRemote { .. },
                    ) => true,
                    (
                        InvocationState::Completed { result: a },
                        ExecutionStatus::Completed { result: b },
                    ) => a == b,
                    (
                        InvocationState::Faulted { message: a },
                        ExecutionStatus::Faulted { message: b },
                    ) => a == b,
                    _ => false,
                };
                require(matching, "invocation receipt does not match its execution")?;
            }
        }
        Ok(())
    }
    pub fn invocations(&self) -> impl Iterator<Item = (&str, &InvocationRecord)> {
        self.invocations
            .iter()
            .map(|(id, record)| (id.as_str(), record))
    }
    pub fn invocation(&self, id: &str) -> Option<&InvocationRecord> {
        self.invocations.get(id)
    }

    /// Persist intent before executing. A repeated ID resumes the same execution
    /// or returns its committed terminal receipt; changed calls are rejected.
    /// On any storage error reload the last checkpoint before continuing.
    pub fn invoke_once(
        &mut self,
        store: &mut CheckpointStore,
        request_id: &str,
        entry: MethodId,
        arguments: Vec<Value>,
        budget: u64,
        interval: u64,
    ) -> Result<InvocationRecord> {
        let method = self.program.method(entry)?;
        require(
            !method.signature.instance,
            "invoke_once requires a static method",
        )?;
        self.invoke_pinned_once(
            store,
            request_id,
            method.binding,
            arguments,
            budget,
            interval,
        )
    }
    /// Durable instance entry: the receiver occupies argument zero in both the
    /// frame and receipt, as determined by the retained method signature.
    pub fn invoke_instance_once(
        &mut self,
        store: &mut CheckpointStore,
        request_id: &str,
        call: InstanceCall,
        budget: u64,
        interval: u64,
    ) -> Result<InvocationRecord> {
        let method = self.entry_method(call.method)?;
        require(
            method.signature.instance,
            "instance invocation requires an instance method",
        )?;
        let binding = call.method;
        self.invoke_pinned_once(
            store,
            request_id,
            binding,
            call.frame_arguments(),
            budget,
            interval,
        )
    }
    fn invoke_pinned_once(
        &mut self,
        store: &mut CheckpointStore,
        request_id: &str,
        binding: MethodVersion,
        arguments: Vec<Value>,
        budget: u64,
        interval: u64,
    ) -> Result<InvocationRecord> {
        validate_request_id(request_id)?;
        require(
            !self.installations.contains_key(request_id),
            "request ID is already bound to an installation",
        )?;
        require(interval > 0, "checkpoint interval must be positive")?;
        let method = self.entry_method(binding)?;
        self.check_entry_arguments(method, &arguments)?;
        if let Some(record) = self.invocations.get(request_id) {
            require(
                record.method == method.binding && record.arguments == arguments,
                "invocation request ID is already bound to a different call",
            )?;
            if record.state == InvocationState::Running {
                require(
                    record.execution == self.context.id,
                    "invocation has lost its execution",
                )?;
            }
        } else {
            require(
                self.invocations.len() < MAX_INVOCATIONS,
                "invocation receipt limit reached",
            )?;
            let binding = method.binding;
            self.start_pinned(binding, arguments.clone())?;
            self.invocations.insert(
                request_id.into(),
                InvocationRecord {
                    method: binding,
                    arguments,
                    execution: self.context.id,
                    state: InvocationState::Running,
                },
            );
        }
        // Also fence replay and any retry after a failed save: no uncommitted
        // in-memory result or unsaved intent may escape as a durable receipt.
        store.save(self)?;
        if self.invocations[request_id].state != InvocationState::Running {
            return Ok(self.invocations[request_id].clone());
        }
        let mut remaining = budget;
        loop {
            let chunk = remaining.min(interval);
            let outcome = self.run(chunk);
            remaining -= chunk;
            // Fault diagnostics are receipts too, saved with the faulting state.
            store.save(self)?;
            match outcome {
                Ok(RunOutcome::Paused) if remaining > 0 => continue,
                Err(error) if !matches!(self.context.status, ExecutionStatus::Faulted { .. }) => {
                    return Err(error);
                }
                _ => return Ok(self.invocations[request_id].clone()),
            }
        }
    }

    pub(crate) fn finish_invocation(&mut self) {
        let terminal = match &self.context.status {
            ExecutionStatus::Completed { result } => InvocationState::Completed { result: *result },
            ExecutionStatus::Faulted { message } => InvocationState::Faulted {
                message: message.clone(),
            },
            _ => return,
        };
        if let Some(record) = self.invocations.values_mut().find(|record| {
            record.execution == self.context.id && record.state == InvocationState::Running
        }) {
            record.state = terminal;
        }
    }
}
