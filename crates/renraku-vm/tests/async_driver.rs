#[allow(dead_code)]
mod common;
use common::{entry, fixtures, program};
use renraku_vm::{
    checkpoint::CheckpointStore,
    driver::{DriverLimits, DriverStop},
    effects::{AsyncModelProvider, EffectState, LocalModelProvider, ModelProvider, ModelRequest},
    error::{Error, Result},
    execution::ExecutionStatus,
    identity::UniverseId,
    program::Program,
    runtime::{RunOutcome, Vm},
    value::Value,
};
use std::{path::PathBuf, time::Duration};
use tokio::sync::oneshot;

struct Files(PathBuf);
impl Files {
    fn new() -> Self {
        let path = fixtures().output.join(UniverseId::fresh().to_string());
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn store(&self) -> CheckpointStore {
        CheckpointStore::open(self.0.join("vm.db")).unwrap()
    }
    fn provider(&self) -> Provider {
        Provider {
            inner: LocalModelProvider::open(self.0.join("receipts.db")).unwrap(),
            gate: None,
            submissions: 0,
            lookups: 0,
            reject: false,
        }
    }
}
impl Drop for Files {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
#[derive(Clone, Copy, Eq, PartialEq)]
enum Stage {
    Preflight,
    BeforeAccept,
    AfterAccept,
}
struct Gate {
    stage: Stage,
    entered: oneshot::Sender<()>,
    release: oneshot::Receiver<()>,
}
struct Provider {
    inner: LocalModelProvider,
    gate: Option<Gate>,
    submissions: usize,
    lookups: usize,
    reject: bool,
}
impl Provider {
    fn gate(&mut self, stage: Stage) -> (oneshot::Receiver<()>, oneshot::Sender<()>) {
        let (entered, received) = oneshot::channel();
        let (release, wait) = oneshot::channel();
        self.gate = Some(Gate {
            stage,
            entered,
            release: wait,
        });
        (received, release)
    }
    async fn pause(&mut self, stage: Stage) {
        if self.gate.as_ref().is_some_and(|g| g.stage == stage) {
            let gate = self.gate.take().unwrap();
            gate.entered.send(()).unwrap();
            let _ = gate.release.await;
        }
    }
}
impl AsyncModelProvider for Provider {
    fn capability(&self) -> &str {
        "local.echo"
    }
    async fn validate_request(&mut self, _: &ModelRequest) -> Result<()> {
        self.pause(Stage::Preflight).await;
        if self.reject {
            Err(Error("preflight rejected".into()))
        } else {
            Ok(())
        }
    }
    async fn submit(&mut self, request: &ModelRequest) -> Result<Vec<u16>> {
        self.submissions += 1;
        self.pause(Stage::BeforeAccept).await;
        let response = self.inner.submit(request)?;
        self.pause(Stage::AfterAccept).await;
        Ok(response)
    }
    async fn reconcile(&mut self, request: &ModelRequest) -> Result<Option<Vec<u16>>> {
        self.lookups += 1;
        self.inner.reconcile(request)
    }
}
fn start<'a>(program: &'a Program, method: &str) -> Vm<'a> {
    let mut vm = Vm::new(program);
    vm.start(entry(program, method), Vec::new()).unwrap();
    vm.set_model_calls_enabled(true);
    vm
}
fn limits(instructions: u64, operations: u64) -> DriverLimits {
    DriverLimits {
        instructions,
        provider_operations: operations,
        checkpoint_interval: 7,
    }
}
fn completed(stop: DriverStop) {
    assert_eq!(stop, DriverStop::Completed(Some(Value::I32(417))));
}

#[tokio::test(flavor = "current_thread")]
async fn another_universe_completes_on_the_same_executor_while_delivery_waits() {
    let program = program();
    let slow_files = Files::new();
    let fast_files = Files::new();
    let mut slow = start(&program, "ModelHarness");
    let mut slow_store = slow_files.store();
    let mut provider = slow_files.provider();
    let (entered, release) = provider.gate(Stage::BeforeAccept);
    let mut fast = start(&program, "Run");
    let mut fast_store = fast_files.store();
    let mut fast_provider = fast_files.provider();
    let delivering = slow.drive_model_async(&mut slow_store, &mut provider, limits(1000, 2));
    let independent = async {
        entered.await.unwrap();
        let report = fast
            .drive_model_async(&mut fast_store, &mut fast_provider, limits(1000, 0))
            .await
            .unwrap();
        assert_eq!(report.stop, DriverStop::Completed(Some(Value::I32(48))));
        assert_eq!(report.provider_operations, 0);
        release.send(()).unwrap();
    };
    let (report, ()) = tokio::time::timeout(Duration::from_secs(5), async {
        tokio::join!(delivering, independent)
    })
    .await
    .unwrap();
    completed(report.unwrap().stop);
    assert_eq!((provider.submissions, provider.lookups), (2, 0));
}

#[tokio::test(flavor = "current_thread")]
async fn dropping_delivery_never_resends_and_a_committed_receipt_wakes_the_original_frame() {
    for stage in [Stage::BeforeAccept, Stage::AfterAccept] {
        let program = program();
        let files = Files::new();
        let mut store = files.store();
        let mut provider = files.provider();
        let mut vm = start(&program, "ModelHarness");
        let (entered, release) = provider.gate(stage);
        {
            let delivery = vm.drive_model_async(&mut store, &mut provider, limits(1000, 2));
            tokio::pin!(delivery);
            tokio::select! { result = &mut delivery => panic!("delivery unexpectedly stopped: {result:?}"), result = entered => result.unwrap() }
        }
        drop(release);
        assert!(matches!(
            vm.context().status,
            ExecutionStatus::Waiting { .. }
        ));
        assert_eq!(vm.effects().next().unwrap().state, EffectState::InFlight);
        let frames = vm.context().frames.clone();
        let execution = vm.context().id;
        let saved = store.load().unwrap();
        let mut vm = saved.snapshot.restore().unwrap();
        vm.set_model_calls_enabled(true);
        assert_eq!(vm.context().id, execution);
        assert_eq!(vm.context().frames, frames);
        assert!(matches!(
            vm.effects().next().unwrap().state,
            EffectState::OutcomeUnknown { .. }
        ));
        let report = vm
            .drive_model_async(&mut store, &mut provider, limits(1000, 2))
            .await
            .unwrap();
        if stage == Stage::AfterAccept {
            completed(report.stop);
            assert_eq!(provider.submissions, 2);
        } else {
            assert!(matches!(report.stop, DriverStop::OutcomeUnknown { .. }));
            assert_eq!(provider.submissions, 1);
        }
        assert_eq!(provider.lookups, 1);
    }
}

#[tokio::test(flavor = "current_thread")]
async fn abandoned_preflight_is_prepared_and_rejection_does_not_mark_a_dispatch() {
    let program = program();
    let files = Files::new();
    let mut store = files.store();
    let mut provider = files.provider();
    let mut vm = start(&program, "ModelHarness");
    let (entered, release) = provider.gate(Stage::Preflight);
    {
        let delivery = vm.drive_model_async(&mut store, &mut provider, limits(1000, 2));
        tokio::pin!(delivery);
        tokio::select! { result = &mut delivery => panic!("preflight unexpectedly stopped: {result:?}"), result = entered => result.unwrap() }
    }
    drop(release);
    let saved = store.load().unwrap();
    let mut vm = saved.snapshot.restore().unwrap();
    vm.set_model_calls_enabled(true);
    assert_eq!(vm.effects().next().unwrap().state, EffectState::Prepared);
    provider.reject = true;
    assert!(
        vm.drive_model_async(&mut store, &mut provider, limits(1000, 2))
            .await
            .unwrap_err()
            .0
            .contains("preflight")
    );
    assert_eq!(vm.effects().next().unwrap().state, EffectState::Prepared);
    assert_eq!(provider.submissions, 0);
    provider.reject = false;
    completed(
        vm.drive_model_async(&mut store, &mut provider, limits(1000, 2))
            .await
            .unwrap()
            .stop,
    );
}

#[tokio::test(flavor = "current_thread")]
async fn async_budgets_and_restore_account_for_every_instruction_and_operation() {
    let program = program();
    let files = Files::new();
    let mut store = files.store();
    let mut provider = files.provider();
    let vm = start(&program, "ModelHarness");
    store.save(&vm).unwrap();
    drop(vm);
    let mut instructions = 0;
    let mut operations = 0;
    for _ in 0..200 {
        let saved = store.load().unwrap();
        let mut vm = saved.snapshot.restore().unwrap();
        vm.set_model_calls_enabled(true);
        let report = vm
            .drive_model_async(&mut store, &mut provider, limits(1, 1))
            .await
            .unwrap();
        assert!(report.instructions <= 1 && report.provider_operations <= 1);
        instructions += report.instructions;
        operations += report.provider_operations;
        if matches!(report.stop, DriverStop::Completed(_)) {
            completed(report.stop);
            assert_eq!(instructions, 158);
            assert_eq!(operations, 2);
            return;
        }
    }
    panic!("bounded driver did not complete");
}

#[tokio::test(flavor = "current_thread")]
async fn stale_async_drivers_are_fenced_before_submission() {
    let program = program();
    let files = Files::new();
    let mut store = files.store();
    let mut provider = files.provider();
    let mut vm = start(&program, "ModelHarness");
    assert!(matches!(
        vm.run_checkpointed(&mut store, 1000, 7).unwrap(),
        RunOutcome::Waiting(_)
    ));
    let mut stale_store = files.store();
    let saved = stale_store.load().unwrap();
    let mut stale = saved.snapshot.restore().unwrap();
    stale.set_model_calls_enabled(true);
    store.save(&vm).unwrap();
    assert!(
        stale
            .drive_model_async(&mut stale_store, &mut provider, limits(1000, 2))
            .await
            .is_err()
    );
    assert_eq!(provider.submissions, 0);
}

#[tokio::test(flavor = "current_thread")]
async fn cpu_guest_work_yields_even_with_a_large_checkpoint_interval() {
    let program = program();
    let files = Files::new();
    let mut store = files.store();
    let mut provider = files.provider();
    let mut vm = start(&program, "Forever");
    store.save(&vm).unwrap();
    let mut inspector = files.store();
    let driving = vm.drive_model_async(
        &mut store,
        &mut provider,
        DriverLimits {
            instructions: 10_000,
            provider_operations: 0,
            checkpoint_interval: 10_000,
        },
    );
    let observer = async {
        let saved = inspector.load().unwrap();
        let vm = saved.snapshot.restore().unwrap();
        assert!(
            vm.instructions_executed() < 10_000,
            "guest exhausted its budget before yielding to independent work"
        );
    };
    let (report, ()) = tokio::join!(driving, observer);
    let report = report.unwrap();
    assert_eq!(report.stop, DriverStop::InstructionBudget);
    assert_eq!(report.instructions, 10_000);
}
