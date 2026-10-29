#[allow(dead_code)]
mod common;
use common::{entry, fixtures, program};
use renraku_vm::{
    checkpoint::CheckpointStore,
    driver::{DriverLimits, DriverStop},
    effects::LocalModelProvider,
    identity::UniverseId,
    invocation::InvocationState,
    runtime::{RunOutcome, Vm},
    value::Value,
};
use rusqlite::{Connection, params};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{BufRead, BufReader},
    path::PathBuf,
    process::{Command, Stdio},
    sync::mpsc,
    time::Duration,
};

struct Database {
    directory: PathBuf,
    path: PathBuf,
}
impl Database {
    fn new() -> Self {
        let directory = fixtures().output.join(UniverseId::fresh().to_string());
        fs::create_dir(&directory).unwrap();
        Self {
            path: directory.join("universe.db"),
            directory,
        }
    }
}
impl Drop for Database {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.directory);
    }
}
fn completed(value: i32) -> InvocationState {
    InvocationState::Completed {
        result: Some(Value::I32(value)),
    }
}

#[test]
fn every_instruction_and_completed_replays_use_one_execution_and_one_mutation() {
    let program = program();
    let database = Database::new();
    let mut store = CheckpointStore::open(&database.path).unwrap();
    let method = entry(&program, "StaticRoundTrip");
    let mut vm = Vm::new(&program);
    let first = vm
        .invoke_once(&mut store, "turn/1", method, vec![], 0, 1)
        .unwrap();
    assert_eq!(first.state, InvocationState::Running);
    assert_eq!(vm.instructions_executed(), 0);
    let mut terminal = false;
    for _ in 0..100 {
        let snapshot = store.load().unwrap();
        let mut vm = snapshot.snapshot.restore().unwrap();
        let result = vm
            .invoke_once(&mut store, "turn/1", method, vec![], 1, 1)
            .unwrap();
        assert_eq!(result.execution, first.execution);
        if result.state == completed(4) {
            terminal = true;
            break;
        }
        assert_eq!(result.state, InvocationState::Running);
    }
    assert!(terminal);
    let snapshot = store.load().unwrap();
    let mut vm = snapshot.snapshot.restore().unwrap();
    let instructions = vm.instructions_executed();
    for _ in 0..3 {
        assert_eq!(
            vm.invoke_once(&mut store, "turn/1", method, vec![], 1000, 5)
                .unwrap()
                .state,
            completed(4)
        );
    }
    assert_eq!(vm.instructions_executed(), instructions);
    assert_eq!(vm.context().id, first.execution);
    assert_eq!(vm.invocations().count(), 1);
    assert_eq!(
        vm.invoke_once(&mut store, "turn/2", method, vec![], 1000, 5)
            .unwrap()
            .state,
        completed(8)
    );
    assert_eq!(
        vm.invoke_once(&mut store, "turn/1", method, vec![], 1000, 5)
            .unwrap()
            .state,
        completed(4)
    );
}

#[test]
fn receipt_arguments_and_object_results_remain_roots_after_unrelated_calls_and_gc() {
    let program = program();
    let database = Database::new();
    let mut store = CheckpointStore::open(&database.path).unwrap();
    let mut vm = Vm::new(&program);
    let created = vm
        .invoke_once(
            &mut store,
            "create",
            entry(&program, "ReturnObject"),
            vec![],
            1000,
            3,
        )
        .unwrap();
    let InvocationState::Completed {
        result: Some(Value::Reference(object)),
    } = created.state
    else {
        panic!()
    };
    let arguments = vec![Value::Reference(object), Value::I32(7)];
    assert_eq!(
        vm.invoke_once(
            &mut store,
            "add",
            entry(&program, "Advance"),
            arguments.clone(),
            1000,
            3
        )
        .unwrap()
        .state,
        completed(30)
    );
    vm.start(entry(&program, "Reset"), vec![]).unwrap();
    vm.run_checkpointed(&mut store, 1000, 3).unwrap();
    vm.collect_garbage().unwrap();
    store.save(&vm).unwrap();
    let snapshot = store.load().unwrap();
    let mut vm = snapshot.snapshot.restore().unwrap();
    assert!(vm.heap().object(object).is_ok());
    vm.start(
        entry(&program, "Advance"),
        vec![Value::Reference(object), Value::I32(3)],
    )
    .unwrap();
    assert_eq!(
        vm.run_checkpointed(&mut store, 1000, 3).unwrap(),
        RunOutcome::Completed(Some(Value::I32(33)))
    );
    assert_eq!(
        vm.invoke_once(
            &mut store,
            "add",
            entry(&program, "Advance"),
            arguments,
            1000,
            3
        )
        .unwrap()
        .state,
        completed(30)
    );
    assert_eq!(
        vm.invoke_once(
            &mut store,
            "create",
            entry(&program, "ReturnObject"),
            vec![],
            1000,
            3
        )
        .unwrap()
        .state,
        created.state
    );
}

#[test]
fn faults_are_durable_receipts_and_conflicting_or_invalid_requests_do_not_progress() {
    let program = program();
    let database = Database::new();
    let mut store = CheckpointStore::open(&database.path).unwrap();
    let mut vm = Vm::new(&program);
    let method = entry(&program, "NullReceiver");
    let result = vm
        .invoke_once(&mut store, "fault", method, vec![], 1000, 3)
        .unwrap();
    assert!(matches!(result.state, InvocationState::Faulted { .. }));
    let instructions = vm.instructions_executed();
    assert_eq!(
        vm.invoke_once(&mut store, "fault", method, vec![], 1000, 3)
            .unwrap(),
        result
    );
    assert_eq!(vm.instructions_executed(), instructions);
    let generation = store.generation();
    assert!(
        vm.invoke_once(&mut store, "fault", entry(&program, "Run"), vec![], 1000, 3)
            .is_err()
    );
    assert!(
        vm.invoke_once(&mut store, "fault", method, vec![Value::I32(3)], 1000, 3)
            .is_err()
    );
    for id in ["", "spaces forbidden", "snowman☃"] {
        assert!(
            vm.invoke_once(&mut store, id, method, vec![], 1000, 3)
                .is_err()
        );
    }
    assert!(
        vm.invoke_once(&mut store, "invalid-interval", method, vec![], 1000, 0)
            .is_err()
    );
    assert_eq!(store.generation(), generation);
    assert_eq!(vm.instructions_executed(), instructions);
    let snapshot = store.load().unwrap();
    assert_eq!(
        snapshot.snapshot.restore().unwrap().invocation("fault"),
        Some(&result)
    );
}

#[test]
fn failed_intent_never_executes_and_failed_completion_never_returns_an_unsaved_receipt() {
    for fail_generation in [2, 3] {
        let program = program();
        let database = Database::new();
        let mut store = CheckpointStore::open(&database.path).unwrap();
        let mut vm = Vm::new(&program);
        store.save(&vm).unwrap();
        let connection = Connection::open(&database.path).unwrap();
        connection.execute_batch(&format!("CREATE TRIGGER reject_save BEFORE UPDATE ON checkpoint WHEN NEW.generation={fail_generation} BEGIN SELECT RAISE(ABORT,'injected fault'); END;")).unwrap();
        let method = entry(&program, "StaticRoundTrip");
        assert!(
            vm.invoke_once(&mut store, "mutation", method, vec![], 1000, 1000)
                .is_err()
        );
        let generation = store.generation();
        assert_eq!(generation, fail_generation - 1);
        if fail_generation == 2 {
            assert_eq!(vm.instructions_executed(), 0);
        }
        assert!(
            vm.invoke_once(&mut store, "mutation", method, vec![], 1000, 1000)
                .is_err()
        );
        assert_eq!(store.generation(), generation);
        connection
            .execute_batch("DROP TRIGGER reject_save")
            .unwrap();
        let snapshot = store.load().unwrap();
        let mut restored = snapshot.snapshot.restore().unwrap();
        assert_eq!(
            restored
                .invoke_once(&mut store, "mutation", method, vec![], 1000, 1000)
                .unwrap()
                .state,
            completed(4)
        );
        assert!(
            vm.invoke_once(&mut store, "mutation", method, vec![], 1000, 1000)
                .is_err()
        );
        assert_eq!(
            store
                .load()
                .unwrap()
                .snapshot
                .restore()
                .unwrap()
                .invocation("mutation")
                .unwrap()
                .state,
            completed(4)
        );
    }
}

#[test]
fn stale_invokers_are_fenced_before_the_first_instruction() {
    let program = program();
    let database = Database::new();
    let mut store = CheckpointStore::open(&database.path).unwrap();
    let vm = Vm::new(&program);
    store.save(&vm).unwrap();
    let snapshot = store.load().unwrap();
    let mut stale = snapshot.snapshot.restore().unwrap();
    store.save(&vm).unwrap();
    assert!(
        stale
            .invoke_once(
                &mut store,
                "stale",
                entry(&program, "StaticRoundTrip"),
                vec![],
                1000,
                5
            )
            .is_err()
    );
    assert_eq!(stale.instructions_executed(), 0);
    assert_eq!(
        store
            .load()
            .unwrap()
            .snapshot
            .restore()
            .unwrap()
            .invocations()
            .count(),
        0
    );
}

#[test]
fn model_wait_and_driver_completion_retain_the_original_invocation_receipt() {
    let program = program();
    let database = Database::new();
    let mut store = CheckpointStore::open(&database.path).unwrap();
    let mut vm = Vm::new(&program);
    vm.set_model_calls_enabled(true);
    let first = vm
        .invoke_once(
            &mut store,
            "conversation",
            entry(&program, "ModelHarness"),
            vec![],
            1000,
            5,
        )
        .unwrap();
    assert_eq!(first.state, InvocationState::Running);
    assert_eq!(vm.effects().count(), 1);
    let snapshot = store.load().unwrap();
    let mut vm = snapshot.snapshot.restore().unwrap();
    assert_eq!(
        vm.invoke_once(
            &mut store,
            "conversation",
            entry(&program, "ModelHarness"),
            vec![],
            1000,
            5
        )
        .unwrap(),
        first
    );
    assert_eq!(vm.effects().count(), 1);
    vm.set_model_calls_enabled(true);
    let mut provider = LocalModelProvider::open(database.directory.join("provider.db")).unwrap();
    let report = vm
        .drive_model(
            &mut store,
            &mut provider,
            DriverLimits {
                instructions: 1000,
                provider_operations: 2,
                checkpoint_interval: 5,
            },
        )
        .unwrap();
    assert_eq!(report.stop, DriverStop::Completed(Some(Value::I32(417))));
    assert_eq!(vm.invocation("conversation").unwrap().state, completed(417));
    assert_eq!(
        vm.invocation("conversation").unwrap().execution,
        first.execution
    );
    assert_eq!(vm.effects().count(), 2);
    let snapshot = store.load().unwrap();
    let mut vm = snapshot.snapshot.restore().unwrap();
    assert_eq!(
        vm.invoke_once(
            &mut store,
            "conversation",
            entry(&program, "ModelHarness"),
            vec![],
            1000,
            5
        )
        .unwrap()
        .state,
        completed(417)
    );
}

#[test]
fn sigkill_at_invocation_commit_boundaries_does_not_duplicate_the_mutation() {
    for stage in ["prepared", "written", "committed"] {
        let program = program();
        let database = Database::new();
        let mut store = CheckpointStore::open(&database.path).unwrap();
        store.save(&Vm::new(&program)).unwrap();
        drop(store);
        let mut child = Command::new(env!("CARGO_BIN_EXE_renraku-checkpoint-probe"))
            .arg("invoke")
            .arg(&database.path)
            .args(["mutation", stage])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let stdout = child.stdout.take().unwrap();
        let (send, receive) = mpsc::channel();
        let reader = std::thread::spawn(move || {
            let mut line = String::new();
            let result = BufReader::new(stdout).read_line(&mut line);
            let _ = send.send((result, line));
        });
        let ready = receive.recv_timeout(Duration::from_secs(20));
        child.kill().unwrap();
        child.wait().unwrap();
        reader.join().unwrap();
        let (read, line) = ready.expect("probe did not reach commit boundary");
        assert!(read.unwrap() > 0);
        assert!(line.starts_with("READY"), "{line}");
        let mut store = CheckpointStore::open(&database.path).unwrap();
        let snapshot = store.load().unwrap();
        let mut vm = snapshot.snapshot.restore().unwrap();
        let recorded = vm.invocation("mutation").unwrap().clone();
        assert_eq!(
            recorded.state,
            if stage == "committed" {
                completed(4)
            } else {
                InvocationState::Running
            }
        );
        assert_eq!(
            vm.invoke_once(
                &mut store,
                "mutation",
                entry(&program, "StaticRoundTrip"),
                vec![],
                1000,
                3
            )
            .unwrap()
            .state,
            completed(4)
        );
        assert_eq!(
            vm.invocation("mutation").unwrap().execution,
            recorded.execution
        );
        assert_eq!(
            vm.invoke_once(
                &mut store,
                "mutation",
                entry(&program, "StaticRoundTrip"),
                vec![],
                1000,
                3
            )
            .unwrap()
            .state,
            completed(4)
        );
    }
}

#[test]
fn legacy_format_five_structured_wait_upgrades_without_invocation_receipts() {
    let database = Database::new();
    let mut store = CheckpointStore::open(&database.path).unwrap();
    let payload = include_bytes!("fixtures/checkpoint-v5.bin");
    Connection::open(&database.path)
        .unwrap()
        .execute(
            "INSERT INTO checkpoint VALUES(1,1,?1,5,?2,?3)",
            params![
                &payload[12..28],
                Sha256::digest(payload).as_slice(),
                payload.as_slice()
            ],
        )
        .unwrap();
    let snapshot = store.load().unwrap();
    let mut vm = snapshot.snapshot.restore().unwrap();
    assert_eq!(vm.instructions_executed(), 54);
    assert_eq!(vm.invocations().count(), 0);
    assert_eq!(vm.effects().count(), 1);
    vm.set_model_calls_enabled(true);
    let mut provider = LocalModelProvider::open(database.directory.join("provider.db")).unwrap();
    assert_eq!(
        vm.drive_model(
            &mut store,
            &mut provider,
            DriverLimits {
                instructions: 1000,
                provider_operations: 2,
                checkpoint_interval: 5
            }
        )
        .unwrap()
        .stop,
        DriverStop::Completed(Some(Value::I32(11)))
    );
    let format: u32 = Connection::open(&database.path)
        .unwrap()
        .query_row("SELECT format FROM checkpoint", [], |row| row.get(0))
        .unwrap();
    assert_eq!(format, 6);
}

#[test]
fn malformed_invocation_records_are_rejected_even_with_repaired_checksums() {
    for case in 0..7 {
        let program = program();
        let database = Database::new();
        let mut store = CheckpointStore::open(&database.path).unwrap();
        let mut vm = Vm::new(&program);
        let request = "invocation-corruption-golden";
        vm.invoke_once(
            &mut store,
            request,
            entry(&program, "StaticRoundTrip"),
            vec![],
            0,
            1,
        )
        .unwrap();
        let connection = Connection::open(&database.path).unwrap();
        let mut payload: Vec<u8> = connection
            .query_row("SELECT payload FROM checkpoint", [], |row| row.get(0))
            .unwrap();
        let name = payload
            .windows(request.len())
            .position(|bytes| bytes == request.as_bytes())
            .unwrap();
        let method = name + request.len();
        let state = method + 32 + 16 + 4;
        match case {
            0 => payload[name] = b' ',
            1 => payload[method..method + 16].fill(0),
            2 => payload[method + 16..method + 32].fill(0),
            3 => payload[method + 32..method + 48].fill(0),
            4 => payload[state] = 255,
            5 => {
                payload[state] = 1;
                payload.insert(state + 1, 0);
            }
            6 => {
                let copy = payload[name - 4..state + 1].to_vec();
                payload[name - 8..name - 4].copy_from_slice(&2u32.to_le_bytes());
                payload.splice(state + 1..state + 1, copy);
            }
            _ => unreachable!(),
        }
        connection
            .execute(
                "UPDATE checkpoint SET payload=?1,digest=?2",
                params![&payload, Sha256::digest(&payload).as_slice()],
            )
            .unwrap();
        assert!(
            store.load().is_err(),
            "case {case} accepted malformed invocation"
        );
    }
}
