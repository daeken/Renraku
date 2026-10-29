mod common;
use common::{entry, fixtures, program};
use renraku_vm::{
    checkpoint::CheckpointStore,
    driver::{DriverLimits, DriverReport, DriverStop},
    effects::{EffectState, LocalModelProvider, ModelProvider, ModelRequest},
    error::{Error, Result},
    execution::ExecutionStatus,
    identity::UniverseId,
    program::Program,
    runtime::{RunOutcome, Vm},
    value::Value,
};
use std::{
    io::{BufRead, BufReader},
    path::PathBuf,
    process::{Child, Command, Stdio},
    sync::mpsc,
    time::Duration,
};

struct Databases {
    directory: PathBuf,
    universe: PathBuf,
    provider: PathBuf,
}
impl Databases {
    fn new() -> Self {
        let directory = fixtures().output.join(UniverseId::fresh().to_string());
        std::fs::create_dir(&directory).unwrap();
        Self {
            universe: directory.join("universe.db"),
            provider: directory.join("provider.db"),
            directory,
        }
    }
}
impl Drop for Databases {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.directory);
    }
}
fn limits(instructions: u64, provider_operations: u64) -> DriverLimits {
    DriverLimits {
        instructions,
        provider_operations,
        checkpoint_interval: 7,
    }
}
fn start<'a>(program: &'a Program, name: &str) -> Vm<'a> {
    let mut vm = Vm::new(program);
    vm.set_model_calls_enabled(true);
    vm.start(entry(program, name), Vec::new()).unwrap();
    vm
}
fn assert_completed(report: DriverReport) {
    assert_eq!(
        report.stop,
        DriverStop::Completed(Some(Value::I32(
            fixtures().reference["ModelHarness"] as i32
        )))
    );
}
struct CountingProvider {
    inner: LocalModelProvider,
    submissions: usize,
    lookups: usize,
    lose_reply: bool,
}
impl CountingProvider {
    fn new(databases: &Databases) -> Self {
        Self {
            inner: LocalModelProvider::open(&databases.provider).unwrap(),
            submissions: 0,
            lookups: 0,
            lose_reply: false,
        }
    }
}
impl ModelProvider for CountingProvider {
    fn capability(&self) -> &str {
        self.inner.capability()
    }
    fn submit(&mut self, request: &ModelRequest) -> Result<Vec<u16>> {
        self.submissions += 1;
        let response = self.inner.submit(request)?;
        if self.lose_reply {
            return Err(Error("reply lost after acceptance".into()));
        }
        Ok(response)
    }
    fn reconcile(&mut self, request: &ModelRequest) -> Result<Option<Vec<u16>>> {
        self.lookups += 1;
        self.inner.reconcile(request)
    }
}

#[test]
fn driver_completes_and_checkpoints_the_linked_harness_without_manual_delivery() {
    let program = program();
    let databases = Databases::new();
    let mut store = CheckpointStore::open(&databases.universe).unwrap();
    let mut provider = CountingProvider::new(&databases);
    let mut vm = start(&program, "ModelHarness");
    let report = vm
        .drive_model(&mut store, &mut provider, limits(1000, 2))
        .unwrap();
    assert_completed(report);
    assert_eq!(report.instructions, 158);
    assert_eq!(report.provider_operations, 2);
    assert_eq!((provider.submissions, provider.lookups), (2, 0));
    let saved = store.load().unwrap();
    let mut recovered = saved.snapshot.restore().unwrap();
    assert_eq!(recovered.context(), vm.context());
    assert_eq!(recovered.universe_id(), vm.universe_id());
    assert_eq!(recovered.effects().count(), 2);
    recovered.set_model_calls_enabled(true);
    let report = recovered
        .drive_model(&mut store, &mut provider, limits(1000, 0))
        .unwrap();
    assert_completed(report);
    assert_eq!((report.instructions, report.provider_operations), (0, 0));
    for effect in recovered.effects() {
        assert_eq!(provider.inner.submission_attempts(effect.id).unwrap(), 1);
    }
}

#[test]
fn independent_budgets_resume_across_checkpoints_and_charge_actual_instructions() {
    let program = program();
    let databases = Databases::new();
    let mut store = CheckpointStore::open(&databases.universe).unwrap();
    let mut provider = CountingProvider::new(&databases);
    let mut vm = start(&program, "ModelHarness");
    let report = vm
        .drive_model(&mut store, &mut provider, limits(43, 2))
        .unwrap();
    assert_eq!(
        report,
        DriverReport {
            stop: DriverStop::InstructionBudget,
            instructions: 43,
            provider_operations: 0
        }
    );
    assert_eq!(vm.effects().count(), 0);
    let mut total = 43;
    for (instructions, operations, expected_instructions, expected_operations, final_call) in [
        (1, 0, 1, 0, false),
        (0, 1, 0, 1, false),
        (72, 0, 72, 0, false),
        (1000, 1, 42, 1, true),
    ] {
        let saved = store.load().unwrap();
        let mut vm = saved.snapshot.restore().unwrap();
        vm.set_model_calls_enabled(true);
        let report = vm
            .drive_model(&mut store, &mut provider, limits(instructions, operations))
            .unwrap();
        assert_eq!(report.instructions, expected_instructions);
        assert_eq!(report.provider_operations, expected_operations);
        total += report.instructions;
        if final_call {
            assert_completed(report);
        } else if instructions == 0 {
            assert_eq!(report.stop, DriverStop::InstructionBudget);
            assert_eq!(vm.context().status, ExecutionStatus::Paused);
        } else {
            let ExecutionStatus::Waiting { effect } = vm.context().status else {
                panic!()
            };
            assert_eq!(report.stop, DriverStop::ProviderBudget { effect });
            assert_eq!(provider.inner.submission_attempts(effect).unwrap(), 0);
        }
    }
    assert_eq!(total, 158);
    assert_eq!((provider.submissions, provider.lookups), (2, 0));
}

#[test]
fn invalid_limits_and_disabled_or_wrong_capabilities_fail_before_progress() {
    struct WrongProvider;
    impl ModelProvider for WrongProvider {
        fn capability(&self) -> &str {
            "wrong.provider"
        }
        fn submit(&mut self, _: &ModelRequest) -> Result<Vec<u16>> {
            panic!("wrong provider invoked")
        }
        fn reconcile(&mut self, _: &ModelRequest) -> Result<Option<Vec<u16>>> {
            panic!("wrong provider invoked")
        }
    }
    let program = program();
    let databases = Databases::new();
    let mut store = CheckpointStore::open(&databases.universe).unwrap();
    let mut provider = CountingProvider::new(&databases);
    let mut vm = start(&program, "ModelHarness");
    vm.set_model_calls_enabled(false);
    assert!(
        vm.drive_model(&mut store, &mut provider, limits(1000, 2))
            .unwrap_err()
            .0
            .contains("disabled")
    );
    vm.set_model_calls_enabled(true);
    assert!(
        vm.drive_model(&mut store, &mut WrongProvider, limits(1000, 2))
            .unwrap_err()
            .0
            .contains("capability")
    );
    let mut invalid = limits(1000, 2);
    invalid.checkpoint_interval = 0;
    assert!(
        vm.drive_model(&mut store, &mut provider, invalid)
            .unwrap_err()
            .0
            .contains("interval")
    );
    assert_eq!(store.generation(), 0);
    assert_eq!(vm.instructions_executed(), 0);
    assert_eq!(vm.effects().count(), 0);
    vm.run(1).unwrap();
    let before = vm.context().clone();
    assert!(
        vm.drive_model(&mut store, &mut provider, limits(u64::MAX, 2))
            .unwrap_err()
            .0
            .contains("counter range")
    );
    assert_eq!(vm.context(), &before);
    assert_eq!((provider.submissions, provider.lookups), (0, 0));
}

#[test]
fn zero_budgets_checkpoint_the_entry_without_guest_or_provider_progress() {
    let program = program();
    let databases = Databases::new();
    let mut store = CheckpointStore::open(&databases.universe).unwrap();
    let mut provider = CountingProvider::new(&databases);
    let mut vm = start(&program, "ModelHarness");
    let report = vm
        .drive_model(&mut store, &mut provider, limits(0, 0))
        .unwrap();
    assert_eq!(
        report,
        DriverReport {
            stop: DriverStop::InstructionBudget,
            instructions: 0,
            provider_operations: 0
        }
    );
    let saved = store.load().unwrap();
    let restored = saved.snapshot.restore().unwrap();
    assert_eq!(restored.context(), vm.context());
    assert_eq!(restored.context().status, ExecutionStatus::Paused);
    assert_eq!(restored.effects().count(), 0);
    assert_eq!((provider.submissions, provider.lookups), (0, 0));
}

#[test]
fn submission_errors_stop_and_the_next_invocation_reconciles_without_resending() {
    let program = program();
    let databases = Databases::new();
    let mut store = CheckpointStore::open(&databases.universe).unwrap();
    let mut provider = CountingProvider::new(&databases);
    provider.lose_reply = true;
    let mut vm = start(&program, "ModelHarness");
    assert!(
        vm.drive_model(&mut store, &mut provider, limits(1000, 8))
            .unwrap_err()
            .0
            .contains("reply lost")
    );
    assert_eq!((provider.submissions, provider.lookups), (1, 0));
    let saved = store.load().unwrap();
    let mut vm = saved.snapshot.restore().unwrap();
    let ExecutionStatus::Waiting { effect } = vm.context().status else {
        panic!()
    };
    assert!(matches!(
        vm.effect(effect).unwrap().state,
        EffectState::OutcomeUnknown { .. }
    ));
    vm.set_model_calls_enabled(true);
    provider.lose_reply = false;
    let report = vm
        .drive_model(&mut store, &mut provider, limits(1000, 2))
        .unwrap();
    assert_completed(report);
    assert_eq!((report.instructions, report.provider_operations), (114, 2));
    assert_eq!((provider.submissions, provider.lookups), (2, 1));
    assert_eq!(provider.inner.submission_attempts(effect).unwrap(), 1);
}

#[test]
fn automatic_delivery_leaves_unrelated_host_requests_untouched() {
    let program = program();
    let databases = Databases::new();
    let mut store = CheckpointStore::open(&databases.universe).unwrap();
    let mut provider = CountingProvider::new(&databases);
    let mut vm = start(&program, "ModelHarness");
    let prompt = vm.new_string(vec![7]).unwrap();
    let uncertain = vm.queue_model_request(prompt).unwrap();
    provider.lose_reply = true;
    assert!(
        vm.dispatch_model(&mut store, uncertain, &mut provider)
            .is_err()
    );
    let record = vm.effect(uncertain).unwrap().clone();
    let prepared = vm.queue_model_request(prompt).unwrap();
    provider.lose_reply = false;
    let report = vm
        .drive_model(&mut store, &mut provider, limits(1000, 2))
        .unwrap();
    assert_completed(report);
    assert_eq!(report.provider_operations, 2);
    assert_eq!(vm.effect(uncertain).unwrap(), &record);
    assert_eq!(vm.effect(prepared).unwrap().state, EffectState::Prepared);
    assert_eq!((provider.submissions, provider.lookups), (3, 0));
    assert_eq!(provider.inner.submission_attempts(prepared).unwrap(), 0);
}

#[test]
fn driver_preserves_fault_diagnostics_and_the_failing_frames_durably() {
    let program = program();
    let databases = Databases::new();
    let mut store = CheckpointStore::open(&databases.universe).unwrap();
    let mut provider = CountingProvider::new(&databases);
    let mut vm = start(&program, "StringBadIndex");
    let error = vm
        .drive_model(&mut store, &mut provider, limits(1000, 2))
        .unwrap_err();
    assert!(error.0.contains("string index out of range"));
    let saved = store.load().unwrap();
    let restored = saved.snapshot.restore().unwrap();
    assert_eq!(restored.context(), vm.context());
    assert!(matches!(
        restored.context().status,
        ExecutionStatus::Faulted { .. }
    ));
    assert_eq!((provider.submissions, provider.lookups), (0, 0));
}

#[test]
fn stale_waiting_drivers_cannot_invoke_the_provider() {
    let program = program();
    let databases = Databases::new();
    let mut store = CheckpointStore::open(&databases.universe).unwrap();
    let mut provider = CountingProvider::new(&databases);
    let mut vm = start(&program, "ModelHarness");
    assert!(matches!(
        vm.run_checkpointed(&mut store, 1000, 7).unwrap(),
        RunOutcome::Waiting(_)
    ));
    let saved = store.load().unwrap();
    let mut stale = saved.snapshot.restore().unwrap();
    stale.set_model_calls_enabled(true);
    store.save(&vm).unwrap();
    assert!(
        stale
            .drive_model(&mut store, &mut provider, limits(1000, 2))
            .unwrap_err()
            .0
            .contains("conflict")
    );
    assert_eq!((provider.submissions, provider.lookups), (0, 0));
}

struct Probe(Option<Child>);
impl Probe {
    fn start(databases: &Databases, mode: &str) -> Self {
        let child = Command::new(env!("CARGO_BIN_EXE_renraku-effect-probe"))
            .args([
                databases.universe.as_os_str(),
                databases.provider.as_os_str(),
                mode.as_ref(),
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap();
        let mut probe = Self(Some(child));
        let stdout = probe.0.as_mut().unwrap().stdout.take().unwrap();
        let (sender, receiver) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let line = line.unwrap();
                if line.starts_with("READY ") {
                    let _ = sender.send(());
                    break;
                }
            }
        });
        receiver
            .recv_timeout(Duration::from_secs(20))
            .expect("driver probe did not reach gate");
        probe
    }
    fn kill(&mut self) {
        let mut child = self.0.take().unwrap();
        child.kill().unwrap();
        let status = child.wait().unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::process::ExitStatusExt;
            assert_eq!(status.signal(), Some(9));
        }
        #[cfg(not(unix))]
        assert!(!status.success());
    }
}
impl Drop for Probe {
    fn drop(&mut self) {
        if let Some(child) = &mut self.0 {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

#[test]
fn missing_receipts_stop_each_invocation_without_retry_or_guest_progress() {
    let program = program();
    let databases = Databases::new();
    let mut store = CheckpointStore::open(&databases.universe).unwrap();
    let vm = start(&program, "ModelHarness");
    store.save(&vm).unwrap();
    Probe::start(&databases, "driver-before").kill();
    let mut provider = CountingProvider::new(&databases);
    for attempt in 1..=2 {
        let saved = store.load().unwrap();
        let mut vm = saved.snapshot.restore().unwrap();
        vm.set_model_calls_enabled(true);
        let before = vm.context().clone();
        let ExecutionStatus::Waiting { effect } = before.status else {
            panic!()
        };
        let report = vm
            .drive_model(&mut store, &mut provider, limits(1000, 4))
            .unwrap();
        assert_eq!(
            report,
            DriverReport {
                stop: DriverStop::OutcomeUnknown { effect },
                instructions: 0,
                provider_operations: 1
            }
        );
        assert_eq!(vm.context(), &before);
        assert_eq!((provider.submissions, provider.lookups), (0, attempt));
        assert_eq!(provider.inner.submission_attempts(effect).unwrap(), 0);
    }
}

#[test]
fn crashed_drivers_reconcile_accepted_requests_and_never_resend_completed_work() {
    let program = program();
    for (mode, instructions, operations, submissions, lookups) in [
        ("driver-after", 114, 2, 1, 1),
        ("driver-second-after", 42, 1, 0, 1),
        ("driver-completed", 0, 0, 0, 0),
    ] {
        let databases = Databases::new();
        let mut store = CheckpointStore::open(&databases.universe).unwrap();
        let vm = start(&program, "ModelHarness");
        store.save(&vm).unwrap();
        Probe::start(&databases, mode).kill();
        let saved = store.load().unwrap();
        let mut restored = saved.snapshot.restore().unwrap();
        assert_eq!(restored.universe_id(), vm.universe_id());
        assert_eq!(restored.context().id, vm.context().id);
        restored.set_model_calls_enabled(true);
        let mut provider = CountingProvider::new(&databases);
        let report = restored
            .drive_model(&mut store, &mut provider, limits(1000, 2))
            .unwrap();
        assert_completed(report);
        assert_eq!(
            (report.instructions, report.provider_operations),
            (instructions, operations)
        );
        assert_eq!(
            (provider.submissions, provider.lookups),
            (submissions, lookups)
        );
        assert_eq!(restored.effects().count(), 2);
        for effect in restored.effects() {
            assert_eq!(provider.inner.submission_attempts(effect.id).unwrap(), 1);
        }
    }
}

#[test]
fn cli_driver_requires_opt_in_and_resumes_after_its_provider_budget() {
    let program = program();
    let databases = Databases::new();
    let mut store = CheckpointStore::open(&databases.universe).unwrap();
    let vm = start(&program, "ModelHarness");
    store.save(&vm).unwrap();
    let command = |operations: &str, enabled: bool| {
        let mut command = Command::new(env!("CARGO_BIN_EXE_renraku-vm"));
        command
            .arg("drive-local")
            .arg(&databases.universe)
            .arg(&databases.provider)
            .args(["1000", operations, "7"]);
        if enabled {
            command.arg("--allow-model-calls");
        }
        command.output().unwrap()
    };
    let rejected = command("2", false);
    assert!(!rejected.status.success());
    assert!(String::from_utf8_lossy(&rejected.stderr).contains("requires --allow-model-calls"));
    assert!(!databases.provider.exists());
    let first = command("1", true);
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    let text = String::from_utf8(first.stdout).unwrap();
    assert!(text.contains("provider operation budget exhausted"));
    assert!(text.contains("instructions this run: 116"));
    let second = command("1", true);
    assert!(
        second.status.success(),
        "{}",
        String::from_utf8_lossy(&second.stderr)
    );
    let text = String::from_utf8(second.stdout).unwrap();
    assert!(text.contains("result: 417"));
    assert!(text.contains("instructions: 158"));
    assert!(text.contains("instructions this run: 42"));
    assert!(text.contains("provider operations: 1"));
    let saved = store.load().unwrap();
    assert_eq!(
        saved.snapshot.restore().unwrap().context().id,
        vm.context().id
    );
}

#[test]
fn cli_reports_an_unresolved_outcome_without_resending() {
    let program = program();
    let databases = Databases::new();
    let mut store = CheckpointStore::open(&databases.universe).unwrap();
    store.save(&start(&program, "ModelHarness")).unwrap();
    Probe::start(&databases, "driver-before").kill();
    let output = Command::new(env!("CARGO_BIN_EXE_renraku-vm"))
        .arg("drive-local")
        .arg(&databases.universe)
        .arg(&databases.provider)
        .args(["1000", "8", "--allow-model-calls"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("guest remains waiting"));
    assert!(String::from_utf8_lossy(&output.stdout).contains("provider operations: 1"));
    let saved = store.load().unwrap();
    let vm = saved.snapshot.restore().unwrap();
    let ExecutionStatus::Waiting { effect } = vm.context().status else {
        panic!()
    };
    assert_eq!(
        LocalModelProvider::open(&databases.provider)
            .unwrap()
            .submission_attempts(effect)
            .unwrap(),
        0
    );
}
