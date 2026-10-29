#[allow(dead_code)]
mod common;
use common::{entry, fixtures};
use renraku_vm::{
    checkpoint::CheckpointStore,
    cil::Op,
    execution::Continuation,
    identity::{FieldId, MethodId, UniverseId},
    invocation::InvocationState,
    program::Program,
    runtime::{RunOutcome, Vm},
    value::Value,
};
use serde::Deserialize;
use serde_json::Value as Json;
use std::{fs, path::PathBuf, process::Command};

fn program(raw: bool) -> Program {
    Program::load(&[
        &fixtures().core,
        &fixtures().guest,
        &fs::read(fixtures().output.join(if raw {
            "Renraku.RawSwitchTests.dll"
        } else {
            "Renraku.SwitchTests.dll"
        }))
        .unwrap(),
    ])
    .unwrap()
}
fn method(program: &Program, name: &str) -> MethodId {
    program
        .entry(&format!("Renraku.Guest.IntegerSwitch::{name}"))
        .unwrap()
}
fn field(program: &Program, owner: &str, name: &str) -> FieldId {
    let owner = program.find_type(owner).unwrap();
    program
        .fields()
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
                .join(format!("switch-{}.db", UniverseId::fresh())),
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
    method: String,
    raw: bool,
    input: i32,
    wide: bool,
    value: String,
}
fn cases() -> Vec<Case> {
    serde_json::from_slice(&fs::read(fixtures().output.join("switch-reference.json")).unwrap())
        .unwrap()
}
fn result(case: &Case) -> Value {
    if case.wide {
        Value::I64(case.value.parse().unwrap())
    } else {
        Value::I32(case.value.parse().unwrap())
    }
}

#[test]
fn emitted_dense_signed_unsigned_enum_and_raw_switches_match_coreclr_with_exact_offsets() {
    let normal = program(false);
    let raw = program(true);
    let cases = cases();
    assert_eq!(cases.len(), 1432);
    for case in &cases {
        let program = if case.raw { &raw } else { &normal };
        let method = method(program, &case.method);
        let body = program.method(method).unwrap();
        let table = body
            .instructions
            .values()
            .find(|i| matches!(i.op, Op::Switch(_)))
            .unwrap();
        let Op::Switch(targets) = &table.op else {
            unreachable!()
        };
        assert_eq!(table.next, table.offset + 5 + 4 * targets.len() as u32);
        for target in targets {
            assert!(body.instructions.contains_key(target));
        }
        if case.method == "RawZero" {
            assert!(targets.is_empty());
        }
        if case.method == "RawDuplicate" {
            assert_eq!(targets[0], targets[1]);
        }
        if case.method == "RawBackward" {
            assert!(targets[0] < table.offset);
        }
        let mut vm = Vm::new(program);
        vm.start(method, vec![Value::I32(case.input)]).unwrap();
        assert_eq!(
            vm.run(100).unwrap(),
            RunOutcome::Completed(Some(result(case))),
            "{}({})",
            case.method,
            case.input
        );
        assert_eq!(vm.effects().count(), 0);
    }
}

#[test]
fn switch_index_targets_and_fallthrough_restore_every_instruction_with_gc_and_pinned_ids() {
    for raw in [false, true] {
        let program = program(raw);
        for case in cases()
            .into_iter()
            .filter(|c| c.raw == raw && [-1, 0, 2, 7].contains(&c.input))
        {
            let database = Database::new();
            let mut store = CheckpointStore::open(&database.0).unwrap();
            let method = method(&program, &case.method);
            let binding = program.method(method).unwrap().binding;
            let mut vm = Vm::new(&program);
            vm.start(method, vec![Value::I32(case.input)]).unwrap();
            let universe = vm.universe_id();
            let execution = vm.context().id;
            store.save(&vm).unwrap();
            let mut finished = false;
            let mut saw_table = false;
            for _ in 0..100 {
                let saved = store.load().unwrap();
                let mut vm = saved.snapshot.restore().unwrap();
                assert_eq!(vm.universe_id(), universe);
                assert_eq!(vm.context().id, execution);
                assert_eq!(
                    saved.snapshot.program.method(method).unwrap().binding,
                    binding
                );
                let before = vm.context().clone();
                vm.collect_garbage().unwrap();
                assert_eq!(vm.context(), &before);
                let frame = vm.context().frames.last().unwrap();
                if let Continuation::Execute { next } = frame.continuation
                    && matches!(
                        saved
                            .snapshot
                            .program
                            .method(frame.method.method)
                            .unwrap()
                            .instructions[&next]
                            .op,
                        Op::Switch(_)
                    )
                {
                    assert!(matches!(frame.evaluation_stack.last(), Some(Value::I32(_))));
                    saw_table = true;
                }
                let outcome = vm.run(1).unwrap();
                store.save(&vm).unwrap();
                if let RunOutcome::Completed(actual) = outcome {
                    assert_eq!(actual, Some(result(&case)));
                    finished = true;
                    break;
                }
            }
            assert!(finished && saw_table, "{}({})", case.method, case.input);
        }
    }
}

#[test]
fn switch_selected_mutations_resume_once_and_old_receipts_survive_later_changes_and_gc() {
    let program = program(false);
    let database = Database::new();
    let mut store = CheckpointStore::open(&database.0).unwrap();
    let mut vm = Vm::new(&program);
    vm.start(entry(&program, "SaveObject"), vec![]).unwrap();
    vm.run(1000).unwrap();
    let root = field(&program, "Renraku.Guest.Simulation", "Saved");
    let counter_value = field(&program, "Renraku.Guest.Counter", "Value");
    let mutations = field(&program, "Renraku.Guest.IntegerSwitch", "Mutations");
    let counter = vm.static_fields()[&root];
    let method = method(&program, "MutateCounter");
    let args = vec![counter, Value::I32(2)];
    let intent = vm
        .invoke_once(&mut store, "switch-first", method, args.clone(), 0, 1)
        .unwrap();
    let mut terminal = None;
    let mut saw_nested_table = false;
    for _ in 0..200 {
        let saved = store.load().unwrap();
        let mut vm = saved.snapshot.restore().unwrap();
        vm.collect_garbage().unwrap();
        assert_eq!(vm.context().id, intent.execution);
        assert_eq!(vm.static_fields()[&root], counter);
        let frame = vm.context().frames.last().unwrap();
        if let Continuation::Execute { next } = frame.continuation {
            saw_nested_table |= vm.context().frames.len() > 1
                && matches!(
                    saved
                        .snapshot
                        .program
                        .method(frame.method.method)
                        .unwrap()
                        .instructions[&next]
                        .op,
                    Op::Switch(_)
                );
        }
        let receipt = vm
            .invoke_once(&mut store, "switch-first", method, args.clone(), 1, 1)
            .unwrap();
        if let InvocationState::Completed { result } = receipt.state {
            assert_eq!(result, Some(Value::I32(30)));
            terminal = Some(receipt);
            break;
        }
    }
    assert!(saw_nested_table);
    let terminal = terminal.unwrap();
    let saved = store.load().unwrap();
    let mut vm = saved.snapshot.restore().unwrap();
    let second = vm
        .invoke_once(
            &mut store,
            "switch-second",
            method,
            vec![counter, Value::I32(4)],
            1000,
            7,
        )
        .unwrap();
    assert_eq!(
        second.state,
        InvocationState::Completed {
            result: Some(Value::I32(7))
        }
    );
    vm.start(entry(&program, "Arithmetic"), vec![]).unwrap();
    vm.run(1000).unwrap();
    vm.collect_garbage().unwrap();
    store.save(&vm).unwrap();
    let saved = store.load().unwrap();
    let mut vm = saved.snapshot.restore().unwrap();
    let instructions = vm.instructions_executed();
    assert_eq!(
        vm.invoke_once(&mut store, "switch-first", method, args, 1000, 1)
            .unwrap(),
        terminal
    );
    assert_eq!(vm.instructions_executed(), instructions);
    assert_eq!(vm.static_fields()[&mutations], Value::I32(2));
    assert_eq!(
        vm.heap()
            .object(counter.referenced_object().unwrap())
            .unwrap()
            .fields()[&counter_value],
        Value::I32(7)
    );
}

#[test]
fn native_sdk_free_resumes_saved_switch_operands_and_replays_without_another_mutation() {
    for (name, index, raw, expected) in [
        ("SelectEnum", 7, false, Value::I32(80)),
        ("SelectUnsigned", -1, false, Value::I32(-2)),
        ("SelectWide", 1, false, Value::I64(i64::MIN)),
        ("RawZero", -1, true, Value::I32(42)),
        ("RawDuplicate", 2, true, Value::I32(22)),
        ("RawBackward", 0, true, Value::I32(77)),
        ("MutateCounter", 6, false, Value::I32(62)),
    ] {
        let program = program(raw);
        let database = Database::new();
        let mut store = CheckpointStore::open(&database.0).unwrap();
        let mut vm = Vm::new(&program);
        let method = method(&program, name);
        let mut args = vec![];
        let mut wire = vec![];
        let mut counter = None;
        if name == "MutateCounter" {
            vm.start(entry(&program, "SaveObject"), vec![]).unwrap();
            vm.run(1000).unwrap();
            let object = vm.static_fields()[&field(&program, "Renraku.Guest.Simulation", "Saved")];
            args.push(object);
            wire.push(format!("object:{}", object.referenced_object().unwrap()));
            counter = Some(object);
        }
        args.push(Value::I32(index));
        wire.push(format!("i32:{index}"));
        let intent = vm
            .invoke_once(&mut store, "native-switch", method, args.clone(), 0, 1)
            .unwrap();
        for _ in 0..100 {
            let frame = vm.context().frames.last().unwrap();
            if let Continuation::Execute { next } = frame.continuation
                && matches!(
                    program.method(frame.method.method).unwrap().instructions[&next].op,
                    Op::Switch(_)
                )
            {
                break;
            }
            vm.invoke_once(&mut store, "native-switch", method, args.clone(), 1, 1)
                .unwrap();
        }
        let frame = vm.context().frames.last().unwrap();
        let Continuation::Execute { next } = frame.continuation else {
            panic!("not executable")
        };
        assert!(matches!(
            program.method(frame.method.method).unwrap().instructions[&next].op,
            Op::Switch(_)
        ));
        let bindings: Vec<_> = program.methods().map(|m| m.binding).collect();
        vm.collect_garbage().unwrap();
        store.save(&vm).unwrap();
        drop(store);
        let invoke = || -> Json {
            let output = Command::new(env!("CARGO_BIN_EXE_renraku-vm"))
                .args([
                    "invoke-once",
                    database.0.to_str().unwrap(),
                    "native-switch",
                    &method.to_string(),
                    "1000",
                ])
                .args(&wire)
                .env("PATH", "/no-switch-sdk-tools")
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
            "state",
            "execution",
            "result",
            "methodId",
            "codeVersion",
            "requestId",
        ] {
            assert_eq!(first[key], replay[key]);
        }
        let mut store = CheckpointStore::open(&database.0).unwrap();
        let saved = store.load().unwrap();
        let vm = saved.snapshot.restore().unwrap();
        assert_eq!(
            vm.invocation("native-switch").unwrap().state,
            InvocationState::Completed {
                result: Some(expected)
            }
        );
        for binding in bindings {
            assert_eq!(
                saved
                    .snapshot
                    .program
                    .method(binding.method)
                    .unwrap()
                    .binding,
                binding
            );
        }
        if let Some(counter) = counter {
            assert_eq!(
                vm.static_fields()[&field(&program, "Renraku.Guest.IntegerSwitch", "Mutations")],
                Value::I32(1)
            );
            assert_eq!(
                vm.heap()
                    .object(counter.referenced_object().unwrap())
                    .unwrap()
                    .fields()[&field(&program, "Renraku.Guest.Counter", "Value")],
                Value::I32(62)
            );
        }
    }
}
