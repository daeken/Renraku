mod common;
use common::{entry, fixtures, program};
use renraku_vm::{
    checkpoint::{CheckpointStore, CommitStage},
    execution::ExecutionContext,
    heap::Object,
    identity::{FieldId, ObjectId, UniverseId},
    runtime::{RunOutcome, Vm},
    value::Value,
};
use rusqlite::{Connection, params};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    io::{BufRead, BufReader},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
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
        std::fs::create_dir(&directory).unwrap();
        let path = directory.join("universe.db");
        Self { directory, path }
    }
}
impl Drop for Database {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.directory);
    }
}
#[derive(Debug, Eq, PartialEq)]
struct Trace {
    universe: UniverseId,
    context: ExecutionContext,
    instructions: u64,
    objects: Vec<Object>,
    roots: BTreeMap<String, ObjectId>,
    statics: BTreeMap<FieldId, Value>,
}
fn trace(vm: &Vm<'_>) -> Trace {
    let retained = vm.retained_objects().unwrap();
    Trace {
        universe: vm.universe_id(),
        context: vm.context().clone(),
        instructions: vm.instructions_executed(),
        objects: vm
            .heap()
            .objects()
            .filter(|o| retained.contains(&o.id()))
            .cloned()
            .collect(),
        roots: vm.heap().roots().clone(),
        statics: vm.static_fields().clone(),
    }
}
fn payload(path: &Path) -> Vec<u8> {
    Connection::open(path)
        .unwrap()
        .query_row("SELECT payload FROM checkpoint", [], |r| r.get(0))
        .unwrap()
}
fn rewrite(path: &Path, change: impl FnOnce(&mut Vec<u8>), repair_checksum: bool) {
    let connection = Connection::open(path).unwrap();
    let mut bytes = payload(path);
    change(&mut bytes);
    if repair_checksum {
        connection
            .execute(
                "UPDATE checkpoint SET payload=?1,digest=?2",
                params![bytes, Sha256::digest(&bytes).as_slice()],
            )
            .unwrap();
    } else {
        connection
            .execute("UPDATE checkpoint SET payload=?1", params![bytes])
            .unwrap();
    }
}

#[test]
fn every_instruction_survives_a_checkpoint_with_identical_frames_heap_and_code() {
    let program = program();
    let database = Database::new();
    let mut store = CheckpointStore::open(&database.path).unwrap();
    let versions: BTreeMap<_, _> = program
        .methods()
        .map(|m| (m.binding.method, m.binding.code))
        .collect();
    let mut vm = Vm::new(&program);
    vm.start(entry(&program, "Run"), vec![]).unwrap();
    store.save(&vm).unwrap();
    let mut saved = store.load().unwrap();
    for step in 1..=155 {
        let mut vm = saved.snapshot.restore().unwrap();
        let result = vm.run(1).unwrap();
        let expected = trace(&vm);
        store.save(&vm).unwrap();
        drop(vm);
        saved = store.load().unwrap();
        let restored = saved.snapshot.restore().unwrap();
        assert_eq!(
            trace(&restored),
            expected,
            "state mismatch at instruction {step}"
        );
        assert_eq!(restored.instructions_executed(), step);
        assert_eq!(
            saved
                .snapshot
                .program
                .methods()
                .map(|m| (m.binding.method, m.binding.code))
                .collect::<BTreeMap<_, _>>(),
            versions
        );
        if step == 155 {
            assert_eq!(result, RunOutcome::Completed(Some(Value::I32(48))));
        } else {
            assert_eq!(result, RunOutcome::Paused);
        }
    }
}

#[test]
fn failed_writes_recover_the_previous_complete_checkpoint() {
    let program = program();
    let database = Database::new();
    let mut store = CheckpointStore::open(&database.path).unwrap();
    let mut vm = Vm::new(&program);
    vm.start(entry(&program, "DurableRun"), vec![]).unwrap();
    vm.run(100).unwrap();
    store.save(&vm).unwrap();
    let before = trace(&vm);
    let bytes = payload(&database.path);
    vm.run(70).unwrap();
    assert_ne!(trace(&vm), before);
    for stop in [CommitStage::Prepared, CommitStage::Written] {
        let error = store
            .save_with_observer(&vm, |stage| {
                if stage == stop {
                    Err(renraku_vm::error::Error("injected failure".into()))
                } else {
                    Ok(())
                }
            })
            .unwrap_err();
        assert!(error.0.contains("injected failure"));
        assert_eq!(store.generation(), 1);
        let saved = store.load().unwrap();
        assert_eq!(trace(&saved.snapshot.restore().unwrap()), before);
        assert_eq!(payload(&database.path), bytes);
    }
    assert_eq!(store.save(&vm).unwrap(), 2);
    assert_eq!(
        trace(&store.load().unwrap().snapshot.restore().unwrap()),
        trace(&vm)
    );
}

#[test]
fn stale_writers_and_other_universes_cannot_replace_a_checkpoint() {
    let program = program();
    let database = Database::new();
    let mut first = CheckpointStore::open(&database.path).unwrap();
    let mut vm = Vm::new(&program);
    vm.start(entry(&program, "Run"), vec![]).unwrap();
    vm.run(30).unwrap();
    first.save(&vm).unwrap();
    let mut stale = CheckpointStore::open(&database.path).unwrap();
    let old = stale.load().unwrap();
    let mut stale_vm = old.snapshot.restore().unwrap();
    stale_vm.run(10).unwrap();
    vm.run(20).unwrap();
    first.save(&vm).unwrap();
    let committed = payload(&database.path);
    assert!(stale.save(&stale_vm).unwrap_err().0.contains("conflict"));
    // Reusing the current connection must not bypass the VM's own origin.
    assert!(
        first
            .save(&stale_vm)
            .unwrap_err()
            .0
            .contains("older generation")
    );
    let _ = stale.load().unwrap();
    let other = Vm::new(&program);
    assert!(
        stale
            .save(&other)
            .unwrap_err()
            .0
            .contains("different universe")
    );
    assert_eq!(payload(&database.path), committed);
}

#[test]
fn roots_gc_and_returned_objects_survive_recovery() {
    let program = program();
    let database = Database::new();
    let mut vm = Vm::new(&program);
    vm.start(entry(&program, "Run"), vec![]).unwrap();
    vm.run(1000).unwrap();
    assert_eq!(vm.collect_garbage().unwrap(), 1);
    vm.start(entry(&program, "ReturnObject"), vec![]).unwrap();
    let RunOutcome::Completed(Some(Value::Reference(id))) = vm.run(100).unwrap() else {
        panic!("missing object")
    };
    vm.set_root("world", id).unwrap();
    vm.start(entry(&program, "Reset"), vec![]).unwrap();
    vm.run(100).unwrap();
    assert_eq!(vm.collect_garbage().unwrap(), 0);
    let mut store = CheckpointStore::open(&database.path).unwrap();
    store.save(&vm).unwrap();
    let saved = store.load().unwrap();
    let mut restored = saved.snapshot.restore().unwrap();
    assert_eq!(restored.heap().roots()["world"], id);
    assert_eq!(trace(&restored), trace(&vm));
    restored.remove_root("world");
    assert_eq!(restored.collect_garbage().unwrap(), 1);
}

#[test]
fn periodic_checkpoints_restore_completion_and_fault_diagnostics() {
    let program = program();
    let database = Database::new();
    let mut store = CheckpointStore::open(&database.path).unwrap();
    let mut vm = Vm::new(&program);
    vm.start(entry(&program, "Run"), vec![]).unwrap();
    assert_eq!(
        vm.run_checkpointed(&mut store, 1000, 17).unwrap(),
        RunOutcome::Completed(Some(Value::I32(48)))
    );
    assert_eq!(store.generation(), 10);
    let saved = store.load().unwrap();
    let mut restored = saved.snapshot.restore().unwrap();
    assert_eq!(
        restored.run(1).unwrap(),
        RunOutcome::Completed(Some(Value::I32(48)))
    );
    assert_eq!(restored.instructions_executed(), 155);
    vm.start(entry(&program, "NullReceiver"), vec![]).unwrap();
    assert!(vm.run(100).is_err());
    store.save(&vm).unwrap();
    let saved = store.load().unwrap();
    assert_eq!(trace(&saved.snapshot.restore().unwrap()), trace(&vm));
}

#[test]
fn checksums_formats_bindings_frames_and_dangling_references_are_validated() {
    let program = program();
    for case in 0..10 {
        let database = Database::new();
        let mut store = CheckpointStore::open(&database.path).unwrap();
        let mut vm = Vm::new(&program);
        let name = if case == 6 { "ReturnObject" } else { "Run" };
        vm.start(entry(&program, name), vec![]).unwrap();
        vm.run(if case == 5 || case == 6 { 1000 } else { 30 })
            .unwrap();
        store.save(&vm).unwrap();
        rewrite(
            &database.path,
            |bytes| {
                let length = bytes.len();
                match case {
                    0 => bytes[0] ^= 1,
                    1 => bytes[8..12].copy_from_slice(&99u32.to_le_bytes()),
                    2 => {
                        // The first assembly is followed by its TypeDef bindings.
                        let image_size =
                            u32::from_le_bytes(bytes[48..52].try_into().unwrap()) as usize;
                        let count_offset = 52 + image_size;
                        let count = u32::from_le_bytes(
                            bytes[count_offset..count_offset + 4].try_into().unwrap(),
                        );
                        assert!(count >= 2);
                        let start = count_offset + 4;
                        let first = bytes[start + 4..start + 20].to_vec();
                        bytes[start + 24..start + 40].copy_from_slice(&first);
                    }
                    3 => {
                        let active = vm.context().frames.last().unwrap().method;
                        let key = [
                            active.method.as_bytes().as_slice(),
                            active.code.as_bytes().as_slice(),
                        ]
                        .concat();
                        let position = bytes.windows(32).rposition(|v| v == key).unwrap();
                        assert_eq!(bytes[position + 32], 0);
                        bytes[position + 33..position + 37]
                            .copy_from_slice(&u32::MAX.to_le_bytes());
                    }
                    4 => {
                        // Last active frame has an Int32 on top of its stack.
                        assert_eq!(&bytes[length - 6..length - 4], &[1, 1]);
                        bytes.truncate(length - 6);
                        bytes.extend([1, 2]);
                        bytes.extend(3i64.to_le_bytes());
                    }
                    5 => bytes[length - 32..length - 16].fill(0), // Unknown entry code version.
                    6 => bytes[length - 65..length - 49].fill(0), // Unknown completed result object.
                    7 => {
                        bytes.pop();
                    }
                    8 => bytes.push(0),
                    9 => bytes[28..32].copy_from_slice(&u32::MAX.to_le_bytes()),
                    _ => unreachable!(),
                }
            },
            case != 0,
        );
        let error = store.load().unwrap_err();
        let expected = [
            "checksum",
            "format",
            "type ID",
            "offset",
            "evaluation stack",
            "entry code",
            "does not exist",
            "truncated",
            "trailing",
            "collection length",
        ][case];
        assert!(error.0.contains(expected), "case {case}: {error}");
    }
}

#[test]
fn inherited_references_and_control_flow_merges_restore_at_every_instruction() {
    let program = program();
    let database = Database::new();
    let mut store = CheckpointStore::open(&database.path).unwrap();
    let mut vm = Vm::new(&program);
    vm.start(entry(&program, "Inheritance"), vec![]).unwrap();
    for _ in 0..200 {
        let outcome = vm.run(1).unwrap();
        store.save(&vm).unwrap();
        let saved = store.load().unwrap();
        let restored = saved.snapshot.restore().unwrap();
        assert_eq!(trace(&restored), trace(&vm));
        if let RunOutcome::Completed(result) = outcome {
            assert_eq!(
                result,
                Some(Value::I32(fixtures().reference["Inheritance"] as i32))
            );
            return;
        }
    }
    panic!("inheritance fixture did not finish");
}

#[test]
fn an_error_after_commit_does_not_undo_the_committed_state() {
    let program = program();
    let database = Database::new();
    let mut store = CheckpointStore::open(&database.path).unwrap();
    let mut vm = Vm::new(&program);
    vm.start(entry(&program, "Run"), vec![]).unwrap();
    vm.run(30).unwrap();
    let error = store
        .save_with_observer(&vm, |stage| {
            if stage == CommitStage::Committed {
                Err(renraku_vm::error::Error(
                    "observer failed after commit".into(),
                ))
            } else {
                Ok(())
            }
        })
        .unwrap_err();
    assert!(error.0.contains("after commit"));
    assert_eq!(store.generation(), 1);
    let saved = store.load().unwrap();
    assert_eq!(trace(&saved.snapshot.restore().unwrap()), trace(&vm));
}

#[test]
fn failed_first_commits_leave_no_recoverable_execution() {
    let fixture = fixtures();
    let budget = durable_budget().to_string();
    let core = fixture.output.join("Renraku.CoreLib.dll");
    let guest = fixture.output.join("Renraku.Simulation.dll");
    for stage in ["prepared", "written"] {
        let database = Database::new();
        let mut writer = Probe::start(&[
            "prepare".as_ref(),
            database.path.as_os_str(),
            core.as_os_str(),
            guest.as_os_str(),
            budget.as_ref(),
            stage.as_ref(),
        ]);
        writer.kill();
        let mut store = CheckpointStore::open(&database.path).unwrap();
        assert_eq!(store.generation(), 0);
        assert!(
            store
                .load()
                .unwrap_err()
                .0
                .contains("no committed checkpoint")
        );
    }
}

#[test]
fn legacy_format_one_restores_active_frames_and_upgrades_on_commit() {
    let database = Database::new();
    let mut store = CheckpointStore::open(&database.path).unwrap();
    let bytes = include_bytes!("fixtures/checkpoint-v1.bin").as_slice();
    assert_eq!(&bytes[..12], b"RENRAKU\0\x01\0\0\0");
    Connection::open(&database.path).unwrap().execute(
        "INSERT INTO checkpoint(singleton,generation,universe,format,digest,payload) VALUES(1,1,?1,1,?2,?3)",
        params![&bytes[12..28],Sha256::digest(bytes).as_slice(),bytes]).unwrap();
    let saved = store.load().unwrap();
    let mut vm = saved.snapshot.restore().unwrap();
    let universe = vm.universe_id();
    let execution = vm.context().id;
    assert_eq!(vm.context().frames.len(), 3);
    assert_eq!(vm.instructions_executed(), 30);
    assert_eq!(
        vm.run(1000).unwrap(),
        RunOutcome::Completed(Some(Value::I32(48)))
    );
    assert_eq!(vm.instructions_executed(), 155);
    store.save(&vm).unwrap();
    let format: u32 = Connection::open(&database.path)
        .unwrap()
        .query_row("SELECT format FROM checkpoint", [], |r| r.get(0))
        .unwrap();
    assert_eq!(format, 7);
    let current = store.load().unwrap();
    let vm = current.snapshot.restore().unwrap();
    assert_eq!(vm.universe_id(), universe);
    assert_eq!(vm.context().id, execution);
}

fn last_text(vm: &Vm<'_>, program: &renraku_vm::program::Program) -> ObjectId {
    let field = program.fields().find(|f| f.name == "LastText").unwrap().id;
    let Value::Reference(id) = vm.static_fields()[&field] else {
        panic!("missing LastText")
    };
    id
}

#[test]
fn managed_strings_restore_at_every_instruction_with_exact_utf16_units() {
    let program = program();
    let database = Database::new();
    let mut store = CheckpointStore::open(&database.path).unwrap();
    let mut vm = Vm::new(&program);
    vm.start(entry(&program, "Strings"), vec![]).unwrap();
    store.save(&vm).unwrap();
    let mut saved = store.load().unwrap();
    for _ in 0..500 {
        let mut vm = saved.snapshot.restore().unwrap();
        let outcome = vm.run(1).unwrap();
        let expected = trace(&vm);
        store.save(&vm).unwrap();
        drop(vm);
        saved = store.load().unwrap();
        let restored = saved.snapshot.restore().unwrap();
        assert_eq!(trace(&restored), expected);
        for frame in &restored.context().frames {
            let method = saved
                .snapshot
                .program
                .methods()
                .find(|m| m.binding == frame.method)
                .unwrap();
            assert!(
                !method.instructions.is_empty(),
                "intrinsic left behind a native frame"
            );
        }
        if let RunOutcome::Completed(result) = outcome {
            assert_eq!(
                result,
                Some(Value::I32(fixtures().reference["Strings"] as i32))
            );
            let id = last_text(&restored, &saved.snapshot.program);
            assert_eq!(
                restored.heap().object(id).unwrap().string_units().unwrap(),
                [104, 105, 0, 32, 0xd83d, 0xde80, 0xd800]
            );
            return;
        }
    }
    panic!("string program did not complete");
}

#[test]
fn literal_interning_and_string_collection_preserve_identity_across_recovery() {
    let program = program();
    let database = Database::new();
    let mut store = CheckpointStore::open(&database.path).unwrap();
    let mut vm = Vm::new(&program);
    vm.start(entry(&program, "StringInterning"), vec![])
        .unwrap();
    vm.run(1000).unwrap();
    let id = last_text(&vm, &program);
    store.save(&vm).unwrap();
    let saved = store.load().unwrap();
    let mut vm = saved.snapshot.restore().unwrap();
    assert_eq!(last_text(&vm, &saved.snapshot.program), id);
    vm.start(entry(&saved.snapshot.program, "StringInterning"), vec![])
        .unwrap();
    assert_eq!(
        vm.run(1000).unwrap(),
        RunOutcome::Completed(Some(Value::I32(1)))
    );
    assert_eq!(last_text(&vm, &saved.snapshot.program), id);
    assert_eq!(vm.heap().objects().count(), 1);

    let mut vm = Vm::new(&program);
    vm.start(entry(&program, "Strings"), vec![]).unwrap();
    vm.run(1000).unwrap();
    assert_eq!(vm.heap().objects().count(), 6);
    let text = last_text(&vm, &program);
    assert_eq!(vm.collect_garbage().unwrap(), 2);
    assert_eq!(vm.heap().objects().count(), 4);
    assert_eq!(
        vm.heap()
            .object(text)
            .unwrap()
            .string_units()
            .unwrap()
            .len(),
        7
    );
}

#[test]
fn string_faults_are_atomic_and_checkpointable() {
    let program = program();
    for (name, message) in [
        ("StringNullReceiver", "null receiver"),
        ("StringBadIndex", "out of range"),
        ("StringNegativeIndex", "out of range"),
        ("StringTooLarge", "unit limit"),
    ] {
        let database = Database::new();
        let mut store = CheckpointStore::open(&database.path).unwrap();
        let mut vm = Vm::new(&program);
        vm.start(entry(&program, name), vec![]).unwrap();
        let mut faulted = false;
        for _ in 0..1000 {
            let before = trace(&vm);
            let objects = vm.heap().objects().count();
            if let Err(error) = vm.run(1) {
                assert!(error.0.contains(message), "{name}: {error}");
                assert_eq!(vm.context().frames, before.context.frames);
                assert_eq!(
                    vm.heap().objects().count(),
                    objects,
                    "failed intrinsic allocated a string"
                );
                store.save(&vm).unwrap();
                let saved = store.load().unwrap();
                assert_eq!(trace(&saved.snapshot.restore().unwrap()), trace(&vm));
                faulted = true;
                break;
            }
        }
        assert!(faulted, "{name} did not fault within its budget");
    }
}

#[test]
fn strings_survive_process_kills_during_checkpoint_writes() {
    let database = Database::new();
    let fixture = fixtures();
    let core = fixture.output.join("Renraku.CoreLib.dll");
    let guest = fixture.output.join("Renraku.Simulation.dll");
    let mut initial = Probe::start(&[
        "prepare".as_ref(),
        database.path.as_os_str(),
        core.as_os_str(),
        guest.as_os_str(),
        "80".as_ref(),
        "committed".as_ref(),
        "Strings".as_ref(),
    ]);
    initial.kill();
    let mut store = CheckpointStore::open(&database.path).unwrap();
    let first = store.load().unwrap();
    let mut oracle = first.snapshot.restore().unwrap();
    let before = trace(&oracle);
    assert_eq!(
        oracle
            .heap()
            .object(last_text(&oracle, &first.snapshot.program))
            .unwrap()
            .string_units()
            .unwrap(),
        [104, 105, 0, 32, 0xd83d, 0xde80, 0xd800]
    );
    assert_eq!(oracle.run(13).unwrap(), RunOutcome::Paused);
    let expected = trace(&oracle);
    let mut writer = Probe::start(&[
        "resume".as_ref(),
        database.path.as_os_str(),
        "13".as_ref(),
        "written".as_ref(),
    ]);
    writer.kill();
    assert_eq!(
        trace(&store.load().unwrap().snapshot.restore().unwrap()),
        before
    );
    let mut writer = Probe::start(&[
        "resume".as_ref(),
        database.path.as_os_str(),
        "13".as_ref(),
        "committed".as_ref(),
    ]);
    writer.kill();
    let saved = store.load().unwrap();
    let mut vm = saved.snapshot.restore().unwrap();
    assert_eq!(trace(&vm), expected);
    assert_eq!(
        vm.run(1000).unwrap(),
        RunOutcome::Completed(Some(Value::I32(fixture.reference["Strings"] as i32)))
    );
}

#[test]
fn string_intrinsics_do_not_consume_managed_call_frames() {
    let program = program();
    let mut vm = Vm::new(&program);
    vm.set_frame_limit(1).unwrap();
    vm.start(entry(&program, "StringNullConcat"), vec![])
        .unwrap();
    assert_eq!(
        vm.run(1000).unwrap(),
        RunOutcome::Completed(Some(Value::I32(1)))
    );
}

struct Probe(Option<Child>);
impl Probe {
    fn start(args: &[&std::ffi::OsStr]) -> Self {
        let child = Command::new(env!("CARGO_BIN_EXE_renraku-checkpoint-probe"))
            .args(args)
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
                    let _ = sender.send(line);
                    break;
                }
            }
        });
        receiver
            .recv_timeout(Duration::from_secs(20))
            .expect("crash probe did not reach its checkpoint gate");
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
fn durable_budget() -> u64 {
    let program = program();
    let mut vm = Vm::new(&program);
    vm.start(entry(&program, "DurableRun"), vec![]).unwrap();
    let saved_field = program.fields().find(|f| f.name == "Saved").unwrap().id;
    let value_field = program
        .fields()
        .find(|f| f.name == "Value" && !f.is_static)
        .unwrap()
        .id;
    for _ in 0..500 {
        assert_eq!(vm.run(1).unwrap(), RunOutcome::Paused);
        if vm.context().frames.len() >= 3
            && let Some(Value::Reference(id)) = vm.static_fields().get(&saved_field)
            && let Value::I32(value) = vm.heap().object(*id).unwrap().fields()[&value_field]
            && value > 1
        {
            return vm.instructions_executed();
        }
    }
    panic!("did not reach a mutated nested computation");
}

#[test]
fn sigkill_and_repeated_process_recovery_keep_heap_stacks_and_identities_coherent() {
    let database = Database::new();
    let fixture = fixtures();
    let budget = durable_budget().to_string();
    let core = fixture.output.join("Renraku.CoreLib.dll");
    let guest = fixture.output.join("Renraku.Simulation.dll");
    let mut initial = Probe::start(&[
        "prepare".as_ref(),
        database.path.as_os_str(),
        core.as_os_str(),
        guest.as_os_str(),
        budget.as_ref(),
        "committed".as_ref(),
    ]);
    initial.kill();
    let mut store = CheckpointStore::open(&database.path).unwrap();
    let first = store.load().unwrap();
    let vm = first.snapshot.restore().unwrap();
    let initial_trace = trace(&vm);
    assert!(initial_trace.context.frames.len() >= 3);
    assert_eq!(initial_trace.objects.len(), 2);
    let versions: BTreeMap<_, _> = first
        .snapshot
        .program
        .methods()
        .map(|m| (m.binding.method, m.binding.code))
        .collect();
    let original = payload(&database.path);
    for stage in ["prepared", "written"] {
        let mut writer = Probe::start(&[
            "resume".as_ref(),
            database.path.as_os_str(),
            "19".as_ref(),
            stage.as_ref(),
        ]);
        // Readers see the old complete checkpoint even with uncommitted WAL pages.
        assert_eq!(payload(&database.path), original);
        writer.kill();
        let saved = store.load().unwrap();
        assert_eq!(saved.generation, 1);
        assert_eq!(trace(&saved.snapshot.restore().unwrap()), initial_trace);
    }
    for round in 1..=3 {
        let previous = store.load().unwrap();
        let mut oracle = previous.snapshot.restore().unwrap();
        assert_eq!(oracle.run(19).unwrap(), RunOutcome::Paused);
        let expected = trace(&oracle);
        let mut writer = Probe::start(&[
            "resume".as_ref(),
            database.path.as_os_str(),
            "19".as_ref(),
            "committed".as_ref(),
        ]);
        writer.kill();
        let saved = store.load().unwrap();
        let vm = saved.snapshot.restore().unwrap();
        assert_eq!(
            trace(&vm),
            expected,
            "heap and stack diverged at recovery cycle {round}"
        );
        assert_eq!(
            vm.retained_objects().unwrap(),
            initial_trace.objects.iter().map(Object::id).collect()
        );
        assert_eq!(saved.generation, round + 1);
        assert_eq!(vm.universe_id(), initial_trace.universe);
        assert_eq!(vm.context().id, initial_trace.context.id);
        assert_eq!(
            vm.instructions_executed(),
            initial_trace.instructions + round * 19
        );
        assert_eq!(
            saved
                .snapshot
                .program
                .methods()
                .map(|m| (m.binding.method, m.binding.code))
                .collect::<BTreeMap<_, _>>(),
            versions
        );
    }
    let result = Command::new(env!("CARGO_BIN_EXE_renraku-vm"))
        .arg("resume")
        .arg(&database.path)
        .args(["100000", "23"])
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(
        String::from_utf8_lossy(&result.stdout)
            .contains(&format!("result: {}", fixture.reference["DurableRun"]))
    );
    let final_checkpoint = store.load().unwrap();
    let mut vm = final_checkpoint.snapshot.restore().unwrap();
    assert_eq!(
        vm.run(1).unwrap(),
        RunOutcome::Completed(Some(Value::I32(fixture.reference["DurableRun"] as i32)))
    );
    assert_eq!(vm.universe_id(), initial_trace.universe);
    assert_eq!(vm.context().id, initial_trace.context.id);
}
