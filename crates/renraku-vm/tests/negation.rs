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
        &fs::read(fixtures().output.join("Renraku.NegationTests.dll")).unwrap(),
    ])
    .unwrap()
}
fn method(program: &Program, name: &str) -> MethodId {
    program
        .entry(&format!("Renraku.Guest.IntegerNegation::{name}"))
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
fn bits(width: u32, text: &str) -> Value {
    if width == 32 {
        Value::I32(text.parse().unwrap())
    } else {
        Value::I64(text.parse().unwrap())
    }
}
struct Database(PathBuf);
impl Database {
    fn new() -> Self {
        Self(
            fixtures()
                .output
                .join(format!("negation-{}.db", UniverseId::fresh())),
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
#[serde(rename_all = "camelCase")]
struct Case {
    method: String,
    width: u32,
    result_width: u32,
    input: String,
    value: String,
}
fn cases() -> Vec<Case> {
    serde_json::from_slice(&fs::read(fixtures().output.join("negation-reference.json")).unwrap())
        .unwrap()
}

#[test]
fn emitted_negation_matches_coreclr_including_minima_and_unsigned_source_conversions() {
    let program = program();
    let cases = cases();
    assert_eq!(cases.len(), 584);
    for case in cases {
        let method = method(&program, &case.method);
        let body = program.method(method).unwrap();
        let neg = body
            .instructions
            .values()
            .find(|i| matches!(i.op, Op::Negate))
            .unwrap();
        assert_eq!(neg.next, neg.offset + 1);
        assert_eq!(
            body.instructions.len(),
            if case.method == "NegUInt32" { 4 } else { 3 }
        );
        let mut vm = Vm::new(&program);
        vm.start(method, vec![bits(case.width, &case.input)])
            .unwrap();
        assert_eq!(
            vm.run(10).unwrap(),
            RunOutcome::Completed(Some(bits(case.result_width, &case.value))),
            "{}({})",
            case.method,
            case.input
        );
        assert_eq!(vm.effects().count(), 0);
    }
}

#[test]
fn negation_operand_and_result_stacks_restore_each_instruction_with_gc_and_original_ids() {
    let program = program();
    let cases = cases();
    for name in ["Neg32", "Neg64", "NegUInt32", "NegAsUInt64"] {
        for input in [
            "0",
            "-1",
            if name.ends_with("32") {
                "-2147483648"
            } else {
                "-9223372036854775808"
            },
        ] {
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
            let steps = program.method(method).unwrap().instructions.len();
            for step in 0..steps {
                let saved = store.load().unwrap();
                let mut vm = saved.snapshot.restore().unwrap();
                assert_eq!(vm.universe_id(), universe);
                assert_eq!(vm.context().id, execution);
                assert_eq!(vm.context().frames[0].method, binding);
                let before = vm.context().clone();
                vm.collect_garbage().unwrap();
                assert_eq!(vm.context(), &before);
                let outcome = vm.run(1).unwrap();
                if step + 1 == steps {
                    assert_eq!(
                        outcome,
                        RunOutcome::Completed(Some(bits(case.result_width, &case.value)))
                    );
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
fn durable_negation_mutations_keep_original_receipts_after_later_state_and_gc() {
    let program = program();
    let database = Database::new();
    let mut store = CheckpointStore::open(&database.0).unwrap();
    let mut vm = Vm::new(&program);
    vm.start(entry(&program, "SaveObject"), vec![]).unwrap();
    vm.run(1000).unwrap();
    let root = field(&program, "Renraku.Guest.Simulation", "Saved");
    let counter_value = field(&program, "Renraku.Guest.Counter", "Value");
    let mutations = field(&program, "Renraku.Guest.IntegerNegation", "Mutations");
    let counter = vm.static_fields()[&root];
    let method = method(&program, "NegateCounter");
    let binding = program.method(method).unwrap().binding;
    let args = vec![counter];
    let intent = vm
        .invoke_once(&mut store, "negate-first", method, args.clone(), 0, 1)
        .unwrap();
    let mut terminal = None;
    let mut saw_negate = false;
    let mut saw_nested = false;
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
            saw_negate |= matches!(
                saved
                    .snapshot
                    .program
                    .method(frame.method.method)
                    .unwrap()
                    .instructions[&next]
                    .op,
                Op::Negate
            );
        }
        let receipt = vm
            .invoke_once(&mut store, "negate-first", method, args.clone(), 1, 1)
            .unwrap();
        if let InvocationState::Completed { result } = receipt.state {
            assert_eq!(result, Some(Value::I32(-19)));
            terminal = Some(receipt);
            break;
        }
    }
    assert!(saw_negate && saw_nested);
    let terminal = terminal.expect("negation mutation finished");
    let saved = store.load().unwrap();
    let mut vm = saved.snapshot.restore().unwrap();
    assert_eq!(vm.static_fields()[&mutations], Value::I32(1));
    let second = vm
        .invoke_once(&mut store, "negate-second", method, args.clone(), 1000, 7)
        .unwrap();
    assert_eq!(
        second.state,
        InvocationState::Completed {
            result: Some(Value::I32(19))
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
        vm.invoke_once(&mut store, "negate-first", method, args, 1000, 1)
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
        Value::I32(19)
    );
}

#[test]
fn sdk_free_native_processes_finish_negation_and_replay_pinned_minimum_results() {
    let program = program();
    for (name, arg, budget, expected) in [
        ("Neg32", Value::I32(i32::MIN), 1, Value::I32(i32::MIN)),
        ("Neg64", Value::I64(i64::MIN), 1, Value::I64(i64::MIN)),
        ("NegUInt32", Value::I32(-1), 2, Value::I64(-4294967295)),
        ("NegAsUInt64", Value::I64(-1), 1, Value::I64(1)),
    ] {
        let database = Database::new();
        let mut store = CheckpointStore::open(&database.0).unwrap();
        let method = method(&program, name);
        let mut vm = Vm::new(&program);
        let pending = vm
            .invoke_once(&mut store, "native-negation", method, vec![arg], budget, 1)
            .unwrap();
        assert_eq!(pending.state, InvocationState::Running);
        vm.collect_garbage().unwrap();
        store.save(&vm).unwrap();
        drop(store);
        let argument = match arg {
            Value::I32(n) => format!("i32:{n}"),
            Value::I64(n) => format!("i64:{n}"),
            _ => unreachable!(),
        };
        let invoke = || -> Json {
            let output = Command::new(env!("CARGO_BIN_EXE_renraku-vm"))
                .args([
                    "invoke-once",
                    database.0.to_str().unwrap(),
                    "native-negation",
                    &method.to_string(),
                    "1000",
                    &argument,
                ])
                .env("PATH", "/no-negation-sdk-tools")
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
            vm.invocation("native-negation").unwrap().state,
            InvocationState::Completed {
                result: Some(expected)
            }
        );
    }
}
