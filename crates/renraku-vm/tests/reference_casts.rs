#[allow(dead_code)]
mod common;
use common::{entry, fixtures};
use renraku_vm::{
    checkpoint::CheckpointStore,
    cil::Op,
    execution::{Continuation, ExecutionStatus},
    identity::{FieldId, MethodId, UniverseId},
    invocation::InvocationState,
    program::Program,
    runtime::{RunOutcome, Vm},
    value::Value,
};
use serde::Deserialize;
use serde_json::Value as Json;
use std::{fs, path::PathBuf, process::Command};
fn program() -> Program {
    Program::load(&[
        &fixtures().core,
        &fixtures().guest,
        &fs::read(fixtures().output.join("Renraku.ReferenceCastTests.dll")).unwrap(),
    ])
    .unwrap()
}
fn method(p: &Program, name: &str) -> MethodId {
    p.entry(&format!("Renraku.Guest.ReferenceCasts::{name}"))
        .unwrap()
}
fn field(p: &Program, owner: &str, name: &str) -> FieldId {
    let owner = p.find_type(owner).unwrap();
    p.fields()
        .find(|f| f.owner == owner && f.name == name)
        .unwrap()
        .id
}
struct Database(PathBuf);
impl Database {
    fn new() -> Self {
        Self(
            fixtures()
                .output
                .join(format!("reference-cast-{}.db", UniverseId::fresh())),
        )
    }
}
impl Drop for Database {
    fn drop(&mut self) {
        for suffix in ["", "-wal", "-shm"] {
            let _ = fs::remove_file(format!("{}{suffix}", self.0.display()));
        }
    }
}
#[derive(Deserialize)]
struct Case {
    input: i32,
    method: String,
    kind: String,
    scalar: Option<i32>,
    fault: Option<String>,
}
fn cases() -> Vec<Case> {
    serde_json::from_slice(
        &fs::read(fixtures().output.join("reference-cast-reference.json")).unwrap(),
    )
    .unwrap()
}
fn create(vm: &mut Vm<'_>, p: &Program, kind: i32) -> Value {
    vm.start(method(p, "Create"), vec![Value::I32(kind)])
        .unwrap();
    let RunOutcome::Completed(Some(value)) = vm.run(1000).unwrap() else {
        panic!("factory")
    };
    value
}
fn expected(case: &Case, input: Value) -> Value {
    match case.kind.as_str() {
        "same" => input,
        "null" => Value::Null,
        "i32" => Value::I32(case.scalar.unwrap()),
        _ => unreachable!(),
    }
}
fn assert_counter(vm: &Vm<'_>, p: &Program, object: Value, value: i32, mutations: i32) {
    assert_eq!(
        vm.heap()
            .object(object.referenced_object().unwrap())
            .unwrap()
            .fields()[&field(p, "Renraku.Guest.Counter", "Value")],
        Value::I32(value)
    );
    assert_eq!(
        vm.static_fields()[&field(p, "Renraku.Guest.ReferenceCasts", "Mutations")],
        Value::I32(mutations)
    );
}
#[test]
fn emitted_class_casts_as_and_is_match_coreclr_and_resolve_original_logical_targets() {
    let program = program();
    let cases = cases();
    assert_eq!(cases.len(), 171);
    let mut defs = false;
    let mut refs = false;
    for case in cases {
        let mut vm = Vm::new(&program);
        let input = create(&mut vm, &program, case.input);
        let method = method(&program, &case.method);
        let code = program.read_code(&method.to_string()).unwrap();
        for i in program.method(method).unwrap().instructions.values() {
            if let Op::ReferenceCast { token, .. } = i.op {
                assert_eq!(i.next, i.offset + 5);
                defs |= token >> 24 == 2;
                refs |= token >> 24 == 1;
                let row = code["instructions"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|r| r["offset"] == i.offset)
                    .unwrap();
                assert_eq!(row["target"]["kind"], "type");
                assert_eq!(
                    row["target"]["id"],
                    program
                        .find_type(row["target"]["name"].as_str().unwrap())
                        .unwrap()
                        .to_string()
                );
            }
        }
        let heap = vm.heap().clone();
        let effects = vm.effects().count();
        vm.start(method, vec![input]).unwrap();
        let outcome = vm.run(100);
        if case.fault.is_some() {
            assert_eq!(case.fault.as_deref(), Some("System.InvalidCastException"));
            let error = outcome.unwrap_err().0;
            assert!(error.contains("invalid reference cast"));
            assert_eq!(
                vm.context().frames.last().unwrap().evaluation_stack,
                vec![input]
            );
        } else {
            assert_eq!(
                outcome.unwrap(),
                RunOutcome::Completed(Some(expected(&case, input))),
                "{}({})",
                case.method,
                case.input
            );
        }
        assert_eq!(vm.heap(), &heap);
        assert_eq!(vm.effects().count(), effects);
    }
    assert!(defs && refs);
}
#[test]
fn every_cast_path_restores_each_instruction_with_gc_null_cycles_shared_strings_and_saved_ids() {
    let program = program();
    for case in cases() {
        let database = Database::new();
        let mut store = CheckpointStore::open(&database.0).unwrap();
        let mut vm = Vm::new(&program);
        let input = create(&mut vm, &program, case.input);
        assert_eq!(
            vm.static_fields()[&field(&program, "Renraku.Guest.ReferenceCasts", "Saved")],
            input
        );
        assert_eq!(
            vm.static_fields()[&field(&program, "Renraku.Guest.ReferenceCasts", "Alias")],
            input
        );
        let original = input
            .referenced_object()
            .map(|id| vm.heap().object(id).unwrap().clone());
        let method = method(&program, &case.method);
        let binding = program.method(method).unwrap().binding;
        vm.start(method, vec![input]).unwrap();
        let universe = vm.universe_id();
        let execution = vm.context().id;
        store.save(&vm).unwrap();
        let mut finished = false;
        for _ in 0..30 {
            let saved = store.load().unwrap();
            let mut vm = saved.snapshot.restore().unwrap();
            assert_eq!(vm.universe_id(), universe);
            assert_eq!(vm.context().id, execution);
            assert_eq!(vm.context().frames[0].method, binding);
            vm.collect_garbage().unwrap();
            if let Some(original) = &original {
                assert_eq!(vm.heap().object(original.id()).unwrap(), original);
            }
            let frame = vm.context().frames.last().unwrap();
            let Continuation::Execute { next } = frame.continuation else {
                panic!("not executable")
            };
            let pure = matches!(
                saved
                    .snapshot
                    .program
                    .method(frame.method.method)
                    .unwrap()
                    .instructions[&next]
                    .op,
                Op::ReferenceCast { .. }
            );
            let before = vm.context().clone();
            let heap = vm.heap().clone();
            let statics = vm.static_fields().clone();
            let outcome = vm.run(1);
            if pure {
                assert_eq!(vm.heap(), &heap);
                assert_eq!(vm.static_fields(), &statics);
            }
            store.save(&vm).unwrap();
            match outcome {
                Err(error) => {
                    assert!(case.fault.is_some() && error.0.contains("invalid reference cast"));
                    assert_eq!(vm.context().frames, before.frames);
                    let saved = store.load().unwrap();
                    let vm = saved.snapshot.restore().unwrap();
                    assert_eq!(vm.context().frames, before.frames);
                    finished = true;
                    break;
                }
                Ok(RunOutcome::Completed(result)) => {
                    assert_eq!(result, Some(expected(&case, input)));
                    finished = true;
                    break;
                }
                Ok(RunOutcome::Paused) => {}
                other => panic!("unexpected {other:?}"),
            }
        }
        assert!(finished, "{}({})", case.method, case.input);
    }
}
#[test]
fn unsupported_missing_and_invalid_cast_targets_are_rejected_before_execution() {
    let core = &fixtures().core;
    let guest = &fixtures().guest;
    for (name, message) in [
        ("ValueIsInst", "reference cast target"),
        ("ArrayIsInst", "unsupported metadata table 27"),
        ("InterfaceIsInst", "interfaces are unsupported"),
        ("GenericIsInst", "unsupported metadata table"),
    ] {
        let bytes = fs::read(fixtures().output.join(format!("{name}.dll"))).unwrap();
        assert!(
            Program::load(&[core, guest, &bytes])
                .unwrap_err()
                .0
                .contains(message)
        );
    }
    for token in [0, 0x0200ffff, 0x0100ffff, 0x04000001, 0x1b000001] {
        let bytes = fs::read(fixtures().output.join(format!("BadCast-{token:08x}.dll"))).unwrap();
        assert!(Program::load(&[core, guest, &bytes]).is_err());
    }
    let bytes = fs::read(fixtures().output.join("Renraku.ReferenceCastTests.dll")).unwrap();
    assert!(
        Program::load(&[core, &bytes])
            .unwrap_err()
            .0
            .contains("unresolved type")
    );
}
#[test]
fn cast_failure_receipts_keep_prior_mutations_and_typed_object_roots_after_later_contexts() {
    let program = program();
    let database = Database::new();
    let mut store = CheckpointStore::open(&database.0).unwrap();
    let mut vm = Vm::new(&program);
    let wrong = create(&mut vm, &program, 5);
    vm.start(entry(&program, "SaveObject"), vec![]).unwrap();
    vm.run(1000).unwrap();
    let counter = vm.static_fields()[&field(&program, "Renraku.Guest.Simulation", "Saved")];
    let method = method(&program, "MutateThenCast");
    let args = vec![counter, wrong];
    let intent = vm
        .invoke_once(&mut store, "cast-fault", method, args.clone(), 0, 1)
        .unwrap();
    let mut terminal = None;
    for _ in 0..200 {
        let saved = store.load().unwrap();
        let mut vm = saved.snapshot.restore().unwrap();
        vm.collect_garbage().unwrap();
        let before = vm.context().clone();
        let steps = vm.instructions_executed();
        let receipt = vm
            .invoke_once(&mut store, "cast-fault", method, args.clone(), 1, 1)
            .unwrap();
        assert_eq!(receipt.execution, intent.execution);
        if let InvocationState::Faulted { message } = &receipt.state {
            assert!(message.contains("invalid reference cast"));
            assert_eq!(vm.context().frames, before.frames);
            assert_eq!(vm.instructions_executed(), steps);
            terminal = Some(receipt);
            break;
        }
    }
    let terminal = terminal.unwrap();
    let saved = store.load().unwrap();
    let mut vm = saved.snapshot.restore().unwrap();
    assert_counter(&vm, &program, counter, 20, 1);
    create(&mut vm, &program, 0);
    vm.start(entry(&program, "SaveObject"), vec![]).unwrap();
    vm.run(1000).unwrap();
    vm.collect_garbage().unwrap();
    store.save(&vm).unwrap();
    let saved = store.load().unwrap();
    let mut vm = saved.snapshot.restore().unwrap();
    let steps = vm.instructions_executed();
    assert_eq!(
        vm.invoke_once(&mut store, "cast-fault", method, args, 1000, 1)
            .unwrap(),
        terminal
    );
    assert_eq!(vm.instructions_executed(), steps);
    assert_counter(&vm, &program, counter, 20, 1);
    assert!(vm.heap().object(wrong.referenced_object().unwrap()).is_ok());
}
#[test]
fn failed_cast_fault_checkpoint_resumes_original_operands_without_repeating_mutation() {
    let program = program();
    let database = Database::new();
    let mut store = CheckpointStore::open(&database.0).unwrap();
    let mut vm = Vm::new(&program);
    let wrong = create(&mut vm, &program, 5);
    vm.start(entry(&program, "SaveObject"), vec![]).unwrap();
    vm.run(1000).unwrap();
    let counter = vm.static_fields()[&field(&program, "Renraku.Guest.Simulation", "Saved")];
    let method = method(&program, "MutateThenCast");
    let args = vec![counter, wrong];
    let intent = vm
        .invoke_once(&mut store, "cast-save-fault", method, args.clone(), 0, 1)
        .unwrap();
    loop {
        let frame = vm.context().frames.last().unwrap();
        let Continuation::Execute { next } = frame.continuation else {
            panic!()
        };
        if matches!(
            program.method(frame.method.method).unwrap().instructions[&next].op,
            Op::ReferenceCast { test: false, .. }
        ) {
            break;
        }
        vm.invoke_once(&mut store, "cast-save-fault", method, args.clone(), 1, 1)
            .unwrap();
    }
    assert_counter(&vm, &program, counter, 20, 1);
    let before = vm.context().clone();
    let connection = rusqlite::Connection::open(&database.0).unwrap();
    let rejected = store.generation() + 2;
    connection.execute_batch(&format!("CREATE TRIGGER reject_cast BEFORE UPDATE ON checkpoint WHEN NEW.generation={rejected} BEGIN SELECT RAISE(ABORT,'injected cast save'); END;")).unwrap();
    assert!(
        vm.invoke_once(&mut store, "cast-save-fault", method, args.clone(), 1, 1)
            .is_err()
    );
    connection
        .execute_batch("DROP TRIGGER reject_cast")
        .unwrap();
    let saved = store.load().unwrap();
    let mut vm = saved.snapshot.restore().unwrap();
    assert_eq!(vm.context().frames, before.frames);
    assert_eq!(
        vm.invocation("cast-save-fault").unwrap().state,
        InvocationState::Running
    );
    vm.collect_garbage().unwrap();
    let receipt = vm
        .invoke_once(&mut store, "cast-save-fault", method, args.clone(), 1, 1)
        .unwrap();
    assert_eq!(receipt.execution, intent.execution);
    assert!(matches!(
        vm.context().status,
        ExecutionStatus::Faulted { .. }
    ));
    assert_counter(&vm, &program, counter, 20, 1);
    assert_eq!(
        vm.invoke_once(&mut store, "cast-save-fault", method, args, 1000, 1)
            .unwrap(),
        receipt
    );
}
#[test]
fn sdk_free_native_casts_retain_same_objects_nulls_strings_and_terminal_failures() {
    let program = program();
    for (name, input) in [
        ("AsDerived", 3),
        ("CastBase", 3),
        ("CastCounter", 6),
        ("CastString", 7),
        ("AsString", 5),
        ("CastString", 0),
        ("ClearThenAs", 3),
        ("CastDerived", 5),
        ("IsDerived", 3),
        ("ReadDerived", 3),
    ] {
        let case = cases()
            .into_iter()
            .find(|c| c.method == name && c.input == input)
            .unwrap();
        let database = Database::new();
        let mut store = CheckpointStore::open(&database.0).unwrap();
        let mut vm = Vm::new(&program);
        let value = create(&mut vm, &program, input);
        let method = method(&program, name);
        let args = vec![value];
        let intent = vm
            .invoke_once(&mut store, "native-reference-cast", method, args, 1, 1)
            .unwrap();
        vm.collect_garbage().unwrap();
        store.save(&vm).unwrap();
        drop(store);
        let argument = match value {
            Value::Null => "null".into(),
            Value::Reference(id) => format!("object:{id}"),
            _ => unreachable!(),
        };
        let invoke = || -> Json {
            let output = Command::new(env!("CARGO_BIN_EXE_renraku-vm"))
                .args([
                    "invoke-once",
                    database.0.to_str().unwrap(),
                    "native-reference-cast",
                    &method.to_string(),
                    "1000",
                    &argument,
                ])
                .env("PATH", "/no-reference-cast-sdk")
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            serde_json::from_slice(&output.stdout).unwrap()
        };
        let first = invoke();
        let replay = invoke();
        assert_eq!(first["execution"], intent.execution.to_string());
        for key in [
            "execution",
            "methodId",
            "codeVersion",
            "state",
            "result",
            "diagnostic",
        ] {
            assert_eq!(first[key], replay[key]);
        }
        let mut store = CheckpointStore::open(&database.0).unwrap();
        let saved = store.load().unwrap();
        let vm = saved.snapshot.restore().unwrap();
        let receipt = vm.invocation("native-reference-cast").unwrap();
        if case.fault.is_some() {
            assert!(matches!(receipt.state, InvocationState::Faulted { .. }));
        } else {
            assert_eq!(
                receipt.state,
                InvocationState::Completed {
                    result: Some(expected(&case, value))
                }
            );
        }
        if input == 7 {
            assert_eq!(
                vm.heap()
                    .object(value.referenced_object().unwrap())
                    .unwrap()
                    .string_units()
                    .unwrap(),
                &[99, 97, 115, 116, 0, 0xd800]
            );
        }
    }
}
