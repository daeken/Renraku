//! Bounded host coordination. Delivery stays outside guest instruction execution.
use crate::{
    checkpoint::CheckpointStore,
    effects::{AsyncModelProvider, EffectState, ModelProvider},
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

// One state machine keeps blocking and asynchronous drivers on the same budget,
// fault, dispatch, and recovery rules. It never contains a native provider future.
struct Invocation {
    limits: DriverLimits,
    initial_instructions: u64,
    operations: u64,
}
enum Action {
    Dispatch(EffectId),
    Reconcile(EffectId),
    Stop(DriverStop),
    Yield,
}
impl Invocation {
    fn new(vm: &Vm<'_>, capability: &str, limits: DriverLimits) -> Result<Self> {
        require(
            vm.model_calls_enabled,
            "model driver is disabled by the host",
        )?;
        crate::model::validate_identifier(capability)?;
        require(
            limits.checkpoint_interval > 0,
            "checkpoint interval must be positive",
        )?;
        require(
            vm.instructions_executed
                .checked_add(limits.instructions)
                .is_some(),
            "driver instruction budget exceeds the cumulative counter range",
        )?;
        Ok(Self {
            limits,
            initial_instructions: vm.instructions_executed,
            operations: 0,
        })
    }
    fn next(
        &mut self,
        vm: &mut Vm<'_>,
        store: &mut CheckpointStore,
        cooperative: bool,
    ) -> Result<Action> {
        loop {
            match vm.context.status {
                ExecutionStatus::Completed { result } => {
                    return Ok(Action::Stop(DriverStop::Completed(result)));
                }
                ExecutionStatus::Waiting { effect } => {
                    if self.operations == self.limits.provider_operations {
                        return Ok(Action::Stop(DriverStop::ProviderBudget { effect }));
                    }
                    self.operations += 1;
                    return match vm.effect(effect)?.state {
                        EffectState::Prepared => Ok(Action::Dispatch(effect)),
                        EffectState::InFlight | EffectState::OutcomeUnknown { .. } => {
                            Ok(Action::Reconcile(effect))
                        }
                        EffectState::Completed { .. } => {
                            Err(Error("waiting guest already has a completed effect".into()))
                        }
                    };
                }
                ExecutionStatus::Runnable | ExecutionStatus::Paused => {
                    let remaining = self.limits.instructions
                        - (vm.instructions_executed - self.initial_instructions);
                    if remaining == 0 {
                        vm.context.status = ExecutionStatus::Paused;
                        return Ok(Action::Stop(DriverStop::InstructionBudget));
                    }
                    // Limit each async poll's guest work even when the caller's
                    // instruction/checkpoint budgets are much larger.
                    let quantum = if cooperative {
                        remaining.min(self.limits.checkpoint_interval).min(1024)
                    } else {
                        remaining
                    };
                    if let Err(error) =
                        vm.run_checkpointed(store, quantum, self.limits.checkpoint_interval)
                    {
                        if matches!(vm.context.status, ExecutionStatus::Faulted { .. }) {
                            store.save(vm)?;
                        }
                        return Err(error);
                    }
                    if cooperative {
                        return Ok(Action::Yield);
                    }
                }
                ExecutionStatus::Faulted { ref message } => {
                    let error = Error(message.clone());
                    store.save(vm)?;
                    return Err(error);
                }
            }
        }
    }
    fn finish(
        self,
        vm: &Vm<'_>,
        store: &mut CheckpointStore,
        stop: DriverStop,
    ) -> Result<DriverReport> {
        store.save(vm)?;
        Ok(DriverReport {
            stop,
            instructions: vm.instructions_executed - self.initial_instructions,
            provider_operations: self.operations,
        })
    }
}
async fn yield_once() {
    let mut yielded = false;
    std::future::poll_fn(|context| {
        if yielded {
            std::task::Poll::Ready(())
        } else {
            yielded = true;
            context.waker().wake_by_ref();
            std::task::Poll::Pending
        }
    })
    .await;
}
impl Vm<'_> {
    /// Run one bounded invocation. Limits are per-invocation host policy. Only
    /// the active guest's waiting request is delivered; every successful stop saves.
    pub fn drive_model(
        &mut self,
        store: &mut CheckpointStore,
        provider: &mut impl ModelProvider,
        limits: DriverLimits,
    ) -> Result<DriverReport> {
        let mut invocation = Invocation::new(self, provider.capability(), limits)?;
        let stop = loop {
            match invocation.next(self, store, false)? {
                Action::Dispatch(effect) => {
                    self.dispatch_model(store, effect, provider)?;
                }
                Action::Reconcile(effect) => {
                    if self.reconcile_model(store, effect, provider)?.is_none() {
                        break DriverStop::OutcomeUnknown { effect };
                    }
                }
                Action::Stop(stop) => break stop,
                Action::Yield => unreachable!(),
            }
        };
        invocation.finish(self, store, stop)
    }
    /// Await host delivery without blocking an executor worker. Independent
    /// universes can be driven concurrently. This VM still has one guest context.
    /// Native futures are never saved. Dropping this future after dispatch keeps
    /// its request InFlight, requiring reconciliation on the next invocation.
    pub async fn drive_model_async(
        &mut self,
        store: &mut CheckpointStore,
        provider: &mut impl AsyncModelProvider,
        limits: DriverLimits,
    ) -> Result<DriverReport> {
        let mut invocation = Invocation::new(self, provider.capability(), limits)?;
        let stop = loop {
            match invocation.next(self, store, true)? {
                Action::Dispatch(effect) => {
                    self.dispatch_model_async(store, effect, provider).await?;
                }
                Action::Reconcile(effect) => {
                    if self
                        .reconcile_model_async(store, effect, provider)
                        .await?
                        .is_none()
                    {
                        break DriverStop::OutcomeUnknown { effect };
                    }
                }
                Action::Stop(stop) => break stop,
                Action::Yield => yield_once().await,
            }
        };
        invocation.finish(self, store, stop)
    }
}
