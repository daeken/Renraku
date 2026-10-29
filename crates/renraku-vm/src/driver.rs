//! Bounded host coordination. Delivery stays outside guest instruction execution.
use crate::{
    checkpoint::CheckpointStore,
    effects::{EffectState, ModelProvider},
    error::{Error, Result, require},
    execution::ExecutionStatus,
    identity::EffectId,
    runtime::Vm,
    value::Value,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DriverLimits {
    pub instructions: u64,
    /// Counts both submissions and receipt lookups, including missing receipts.
    pub provider_operations: u64,
    pub checkpoint_interval: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DriverStop {
    Completed(Option<Value>),
    InstructionBudget,
    ProviderBudget { effect: EffectId },
    OutcomeUnknown { effect: EffectId },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DriverReport {
    pub stop: DriverStop,
    pub instructions: u64,
    pub provider_operations: u64,
}

impl Vm<'_> {
    /// Run one bounded invocation, committing the final state on every successful
    /// return. Limits reset on the next explicit invocation; they are host policy.
    /// Only the active guest's waiting request is delivered. A zero instruction
    /// budget still permits delivery to an already waiting guest.
    pub fn drive_model(
        &mut self,
        store: &mut CheckpointStore,
        provider: &mut impl ModelProvider,
        limits: DriverLimits,
    ) -> Result<DriverReport> {
        require(
            self.model_calls_enabled,
            "model driver is disabled by the host",
        )?;
        // Supplying a host provider grants that capability for this invocation.
        // Each waiting effect must still match its capability and model exactly.
        crate::model::validate_identifier(provider.capability())?;
        require(
            limits.checkpoint_interval > 0,
            "checkpoint interval must be positive",
        )?;
        require(
            self.instructions_executed
                .checked_add(limits.instructions)
                .is_some(),
            "driver instruction budget exceeds the cumulative counter range",
        )?;
        let initial_instructions = self.instructions_executed;
        let mut operations = 0;
        let stop = loop {
            match self.context.status {
                ExecutionStatus::Completed { result } => break DriverStop::Completed(result),
                ExecutionStatus::Waiting { effect } => {
                    if operations == limits.provider_operations {
                        break DriverStop::ProviderBudget { effect };
                    }
                    operations += 1;
                    match self.effect(effect)?.state {
                        EffectState::Prepared => {
                            self.dispatch_model(store, effect, provider)?;
                        }
                        EffectState::InFlight | EffectState::OutcomeUnknown { .. } => {
                            if self.reconcile_model(store, effect, provider)?.is_none() {
                                break DriverStop::OutcomeUnknown { effect };
                            }
                        }
                        EffectState::Completed { .. } => {
                            return Err(Error(
                                "waiting guest already has a completed effect".into(),
                            ));
                        }
                    }
                }
                ExecutionStatus::Runnable | ExecutionStatus::Paused => {
                    let remaining =
                        limits.instructions - (self.instructions_executed - initial_instructions);
                    if remaining == 0 {
                        self.context.status = ExecutionStatus::Paused;
                        break DriverStop::InstructionBudget;
                    }
                    if let Err(error) =
                        self.run_checkpointed(store, remaining, limits.checkpoint_interval)
                    {
                        // Fault diagnostics are durable too. A checkpoint failure
                        // is propagated rather than acknowledging an unsaved fault.
                        if matches!(self.context.status, ExecutionStatus::Faulted { .. }) {
                            store.save(self)?;
                        }
                        return Err(error);
                    }
                }
                ExecutionStatus::Faulted { ref message } => {
                    let error = Error(message.clone());
                    store.save(self)?;
                    return Err(error);
                }
            }
        };
        store.save(self)?;
        Ok(DriverReport {
            stop,
            instructions: self.instructions_executed - initial_instructions,
            provider_operations: operations,
        })
    }
}
