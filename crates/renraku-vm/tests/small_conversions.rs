#[allow(dead_code)]
mod common;
use common::{entry, fixtures};
use renraku_vm::{
    checkpoint::CheckpointStore,
    cil::Op,
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
        &fs::read(fixtures().output.join("Renraku.SmallConversionTests.dll")).unwrap(),
    ])
    .unwrap()
}
fn method(program: &Program, name: &str) -> MethodId {
    program
        .entry(&format!("Renraku.Guest.SmallIntegerConversions::{name}"))
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
fn bits(width: u32, value: &str) -> Value {
    if width == 32 {
        Value::I32(value.parse().unwrap())
    } else {
        Value::I64(value.parse().unwrap())
    }
}
struct Database(PathBuf);
impl Database {
    fn new() -> Self {
        Self(
            fixtures()
                .output
                .join(format!("small-conversion-{}.db", UniverseId::fresh())),
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
    width: u32,
    input: String,
    value: i32,
}
fn cases() -> Vec<Case> {
    serde_json::from_slice(
        &fs::read(fixtures().output.join("small-conversion-reference.json")).unwrap(),
    )
    .unwrap()
}

#[test]
fn emitted_small_width_casts_and_char_returns_match_coreclr_exact_storage_results() {
    let program = program();
    let cases = cases();
    assert_eq!(cases.len(), 1680);
    for case in cases {
        let method = method(&program, &case.method);
        let body = program.method(method).unwrap();
        assert_eq!(body.instructions.len(), 3);
        assert!(match body.instructions[&1].op {
            Op::ConvertI8 => case.method.ends_with("I8"),
            Op::ConvertU8 => case.method.ends_with("U8"),
            Op::ConvertI16 => case.method.ends_with("I16"),
            Op::ConvertU16 => case.method.ends_with("U16") || case.method.ends_with("Char"),
            _ => false,
        });
        assert_eq!(body.instructions[&1].next, 2);
        let mut vm = Vm::new(&program);
        vm.start(method, vec![bits(case.width, &case.input)])
            .unwrap();
        assert_eq!(
            vm.run(4).unwrap(),
            RunOutcome::Completed(Some(Value::I32(case.value))),
            "{}({})",
            case.method,
            case.input
        );
    }
}

#[test]
fn every_small_cast_stack_and_surrogate_char_result_restores_per_instruction_with_gc() {
    let program = program();
    let cases = cases();
    for name in [
        "I32ToI8",
        "I32ToU8",
        "I32ToI16",
        "I32ToU16",
        "I64ToI8",
        "I64ToU8",
        "I64ToI16",
        "I64ToU16",
        "I32ToChar",
        "I64ToChar",
    ] {
        for input in ["-1", "128", "55296", "65536"] {
            let case = cases
                .iter()
                .find(|c| c.method == name && c.input == input)
                .unwrap();
            let method = method(&program, name);
            let binding = program.method(method).unwrap().binding;
            let mut vm = Vm::new(&program);
            vm.start(method, vec![bits(case.width, input)]).unwrap();
            let universe = vm.universe_id();
            let execution = vm.context().id;
            let database = Database::new();
            let mut store = CheckpointStore::open(&database.0).unwrap();
            store.save(&vm).unwrap();
            for step in 0..3 {
                let saved = store.load().unwrap();
                let mut vm = saved.snapshot.restore().unwrap();
                assert_eq!(vm.universe_id(), universe);
                assert_eq!(vm.context().id, execution);
                assert_eq!(vm.context().frames[0].method, binding);
                if step == 1 {
                    assert_eq!(
                        vm.context().frames[0].evaluation_stack,
                        vec![bits(case.width, input)]
                    );
                }
                if step == 2 {
                    assert_eq!(
                        vm.context().frames[0].evaluation_stack,
                        vec![Value::I32(case.value)]
                    );
                }
                let before = vm.context().clone();
                vm.collect_garbage().unwrap();
                assert_eq!(vm.context(), &before);
                let outcome = vm.run(1).unwrap();
                if step == 2 {
                    assert_eq!(outcome, RunOutcome::Completed(Some(Value::I32(case.value))));
                } else {
                    assert_eq!(outcome, RunOutcome::Paused);
                }
                store.save(&vm).unwrap();
                assert_eq!(
                    store.load().unwrap().snapshot.restore().unwrap().context(),
                    vm.context()
                );
            }
        }
    }
}

#[test]
fn durable_char_store_recovers_partial_calls_and_replays_its_original_surrogate() {
    let program = program();
    let database = Database::new();
    let mut store = CheckpointStore::open(&database.0).unwrap();
    let mut vm = Vm::new(&program);
    vm.start(entry(&program, "SaveObject"), vec![]).unwrap();
    vm.run(1000).unwrap();
    let root = field(&program, "Renraku.Guest.Simulation", "Saved");
    let counter_value = field(&program, "Renraku.Guest.Counter", "Value");
    let unit = field(
        &program,
        "Renraku.Guest.SmallIntegerConversions",
        "SavedUnit",
    );
    let mutations = field(
        &program,
        "Renraku.Guest.SmallIntegerConversions",
        "Mutations",
    );
    let counter = vm.static_fields()[&root];
    let method = method(&program, "StoreChar");
    let binding = program.method(method).unwrap().binding;
    let args = vec![counter, Value::I64(0x1d800)];
    let intent = vm
        .invoke_once(&mut store, "char-first", method, args.clone(), 0, 1)
        .unwrap();
    let mut terminal = None;
    let mut saw_nested = false;
    let mut saw_conversion = false;
    for _ in 0..150 {
        let saved = store.load().unwrap();
        let mut vm = saved.snapshot.restore().unwrap();
        vm.collect_garbage().unwrap();
        assert_eq!(vm.context().id, intent.execution);
        assert_eq!(vm.static_fields()[&root], counter);
        assert_eq!(
            saved.snapshot.program.method(method).unwrap().binding,
            binding
        );
        saw_nested |= vm.context().frames.len() > 1;
        let frame = vm.context().frames.last().unwrap();
        if let renraku_vm::execution::Continuation::Execute { next } = frame.continuation {
            saw_conversion |= matches!(
                saved
                    .snapshot
                    .program
                    .method(frame.method.method)
                    .unwrap()
                    .instructions[&next]
                    .op,
                Op::ConvertU16
            );
        }
        let receipt = vm
            .invoke_once(&mut store, "char-first", method, args.clone(), 1, 1)
            .unwrap();
        if let InvocationState::Completed { result } = receipt.state {
            assert_eq!(result, Some(Value::I32(0xd800)));
            terminal = Some(receipt);
            break;
        }
    }
    assert!(saw_nested && saw_conversion);
    let terminal = terminal.expect("char store finished");
    let saved = store.load().unwrap();
    let mut vm = saved.snapshot.restore().unwrap();
    assert_eq!(vm.static_fields()[&unit], Value::I32(0xd800));
    assert_eq!(vm.static_fields()[&mutations], Value::I32(1));
    let second = vm
        .invoke_once(
            &mut store,
            "char-second",
            method,
            vec![counter, Value::I64(0x1dfff)],
            1000,
            7,
        )
        .unwrap();
    assert_eq!(
        second.state,
        InvocationState::Completed {
            result: Some(Value::I32(0xdfff))
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
        vm.invoke_once(&mut store, "char-first", method, args, 1000, 1)
            .unwrap(),
        terminal
    );
    assert_eq!(vm.instructions_executed(), instructions);
    assert_eq!(vm.static_fields()[&unit], Value::I32(0xdfff));
    assert_eq!(vm.static_fields()[&mutations], Value::I32(2));
    assert_eq!(
        vm.heap()
            .object(counter.referenced_object().unwrap())
            .unwrap()
            .fields()[&counter_value],
        Value::I32(21)
    );
}

#[test]
fn sdk_free_native_processes_finish_all_small_conversions_and_preserve_char_units() {
    let program = program();
    for (name, input, expected) in [
        ("I64ToI8", "255", -1),
        ("I64ToU8", "-1", 255),
        ("I64ToI16", "65535", -1),
        ("I64ToU16", "-1", 65535),
        ("I64ToChar", "55296", 0xd800),
        ("I64ToChar", "57343", 0xdfff),
    ] {
        let database = Database::new();
        let mut store = CheckpointStore::open(&database.0).unwrap();
        let method = method(&program, name);
        let mut vm = Vm::new(&program);
        let pending = vm
            .invoke_once(
                &mut store,
                "native-small",
                method,
                vec![Value::I64(input.parse().unwrap())],
                1,
                1,
            )
            .unwrap();
        assert_eq!(pending.state, InvocationState::Running);
        vm.collect_garbage().unwrap();
        store.save(&vm).unwrap();
        drop(store);
        let invoke = || -> Json {
            let output = Command::new(env!("CARGO_BIN_EXE_renraku-vm"))
                .args([
                    "invoke-once",
                    database.0.to_str().unwrap(),
                    "native-small",
                    &method.to_string(),
                    "1000",
                    &format!("i64:{input}"),
                ])
                .env("PATH", "/no-small-conversion-sdk-tools")
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
        assert_eq!(first["execution"], pending.execution.to_string());
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
            vm.invocation("native-small").unwrap().state,
            InvocationState::Completed {
                result: Some(Value::I32(expected))
            }
        );
    }
}
