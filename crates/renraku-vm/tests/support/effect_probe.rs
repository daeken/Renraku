//! Gates actual subprocess crashes before and after provider acceptance.
use renraku_vm::{
    checkpoint::CheckpointStore,
    driver::DriverLimits,
    effects::{EffectState, LocalModelProvider, ModelProvider, ModelRequest},
    error::{Error, Result},
    runtime::RunOutcome,
};
use std::io::{Read, Write};
struct GatedProvider {
    inner: LocalModelProvider,
    gate: String,
    submissions: usize,
}
fn gate(label: &str) -> Result<()> {
    println!("READY {label}");
    std::io::stdout().flush()?;
    std::io::stdin().read_exact(&mut [0u8; 1])?;
    Ok(())
}
impl ModelProvider for GatedProvider {
    fn capability(&self) -> &str {
        self.inner.capability()
    }
    fn submit(&mut self, request: &ModelRequest) -> Result<Vec<u16>> {
        self.submissions += 1;
        if self.gate == "before" {
            gate("before acceptance")?;
        }
        let response = self.inner.submit(request)?;
        if self.gate == "after" || self.gate == "second-after" && self.submissions == 2 {
            gate("after acceptance")?;
        }
        Ok(response)
    }
    fn reconcile(&mut self, request: &ModelRequest) -> Result<Option<Vec<u16>>> {
        self.inner.reconcile(request)
    }
}
fn main() {
    if let Err(error) = run() {
        eprintln!("effect probe: {error}");
        std::process::exit(1);
    }
}
fn run() -> Result<()> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.len() != 3
        || !matches!(
            args[2].as_str(),
            "intent"
                | "before"
                | "after"
                | "committed"
                | "driver-before"
                | "driver-after"
                | "driver-second-after"
                | "driver-completed"
        )
    {
        return Err(Error(
            "effect probe requires universe, provider ledger, and gate".into(),
        ));
    }
    let mut store = CheckpointStore::open(&args[0])?;
    let saved = store.load()?;
    let mut vm = saved.snapshot.restore()?;
    if let Some(mode) = args[2].strip_prefix("driver-") {
        vm.set_model_calls_enabled(true);
        let mut provider = GatedProvider {
            inner: LocalModelProvider::open(&args[1])?,
            gate: mode.to_owned(),
            submissions: 0,
        };
        vm.drive_model(
            &mut store,
            &mut provider,
            DriverLimits {
                instructions: 1000,
                provider_operations: 8,
                checkpoint_interval: 7,
            },
        )?;
        if mode == "completed" {
            gate("after driver completion commit")?;
        }
        return Ok(());
    }
    if args[2] == "intent" {
        vm.set_model_calls_enabled(true);
        if !matches!(vm.run(10_000)?, RunOutcome::Waiting(_)) {
            return Err(Error("guest did not reach an effect call".into()));
        }
        gate("before intent commit")?;
        return Ok(());
    }
    let id = vm
        .effects()
        .find(|effect| effect.state == EffectState::Prepared)
        .ok_or_else(|| Error("no prepared effect".into()))?
        .id;
    let mut provider = GatedProvider {
        inner: LocalModelProvider::open(&args[1])?,
        gate: args[2].clone(),
        submissions: 0,
    };
    vm.dispatch_model(&mut store, id, &mut provider)?;
    if args[2] == "committed" {
        gate("after response and wakeup commit")?;
    }
    Ok(())
}
