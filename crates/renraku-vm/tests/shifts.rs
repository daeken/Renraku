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

fn program(raw: bool) -> Program {
    Program::load(&[
        &fixtures().core,
        &fixtures().guest,
        &fs::read(fixtures().output.join(if raw {
            "Renraku.RawShiftTests.dll"
        } else {
            "Renraku.ShiftTests.dll"
        }))
        .unwrap(),
    ])
    .unwrap()
}
fn method(program: &Program, name: &str) -> MethodId {
    program
        .entry(&format!("Renraku.Guest.IntegerShifts::{name}"))
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
                .join(format!("shift-{}.db", UniverseId::fresh())),
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
    input: String,
    count: i32,
    value: String,
    raw_value: String,
}
#[derive(Deserialize)]
struct Mix {
    input: String,
    count: i32,
    value: String,
}
#[derive(Deserialize)]
struct Reference {
    cases: Vec<Case>,
    mixes: Vec<Mix>,
}
fn reference() -> Reference {
    serde_json::from_slice(&fs::read(fixtures().output.join("shift-reference.json")).unwrap())
        .unwrap()
}

#[test]
fn actual_csharp_masks_and_raw_opcodes_match_the_declared_portable_count_policy() {
    let data = reference();
    assert_eq!(data.cases.len(), 5570);
    assert_eq!(data.mixes.len(), 231);
    for raw in [false, true] {
        let program = program(raw);
        for case in &data.cases {
            let method = method(&program, &case.method);
            let instructions = &program.method(method).unwrap().instructions;
            let shift = &instructions[&5];
            assert_eq!(shift.next, 6);
            assert!(match shift.op {
                Op::ShiftLeft => case.method.starts_with("Shl"),
                Op::ShiftRight { unsigned } =>
                    case.method.starts_with("Shr")
                        && unsigned == (case.method.contains('U') || case.method.contains("Zero")),
                _ => false,
            });
            if raw {
                for offset in 2..=4 {
                    assert!(matches!(instructions[&offset].op, Op::Nop));
                }
            } else {
                assert!(matches!(instructions[&4].op, Op::BitAnd));
            }
            if case.count >= 0 && case.count < case.width as i32 {
                assert_eq!(case.raw_value, case.value);
            }
            // For unspecified raw counts the expected value is the documented
            // mask policy, obtained from emitted C# rather than a host raw JIT.
            let mut vm = Vm::new(&program);
            vm.start(
                method,
                vec![bits(case.width, &case.input), Value::I32(case.count)],
            )
            .unwrap();
            assert_eq!(
                vm.run(12).unwrap(),
                RunOutcome::Completed(Some(bits(case.width, &case.value))),
                "raw={raw} {}({}, {})",
                case.method,
                case.input,
                case.count
            );
        }
        for case in &data.mixes {
            let mut vm = Vm::new(&program);
            vm.start(
                method(&program, "Mix32"),
                vec![bits(32, &case.input), Value::I32(case.count)],
            )
            .unwrap();
            assert_eq!(
                vm.run(100).unwrap(),
                RunOutcome::Completed(Some(bits(32, &case.value)))
            );
            assert_eq!(vm.effects().count(), 0);
        }
    }
}

#[test]
fn raw_counts_and_shift_stacks_recover_every_instruction_with_gc_and_stable_ids() {
    let program = program(true);
    let cases = reference().cases;
    for width in [32, 64] {
        for operation in ["Shl", "ShlU", "Shr", "ShrU", "ShrZero"] {
            for count in [0, 31, 32, 63, 64, -1, i32::MIN, i32::MAX] {
                let name = format!("{operation}{width}");
                let case = cases
                    .iter()
                    .find(|c| c.method == name && c.count == count && c.input == "-1")
                    .unwrap();
                let method = method(&program, &name);
                let binding = program.method(method).unwrap().binding;
                let args = vec![bits(width, "-1"), Value::I32(count)];
                let mut vm = Vm::new(&program);
                vm.start(method, args.clone()).unwrap();
                let universe = vm.universe_id();
                let execution = vm.context().id;
                let database = Database::new();
                let mut store = CheckpointStore::open(&database.0).unwrap();
                store.save(&vm).unwrap();
                for step in 0..7 {
                    let saved = store.load().unwrap();
                    let mut vm = saved.snapshot.restore().unwrap();
                    assert_eq!(vm.universe_id(), universe);
                    assert_eq!(vm.context().id, execution);
                    assert_eq!(vm.context().frames[0].method, binding);
                    if (2..=5).contains(&step) {
                        assert_eq!(vm.context().frames[0].evaluation_stack, args);
                    }
                    let before = vm.context().clone();
                    vm.collect_garbage().unwrap();
                    assert_eq!(vm.context(), &before);
                    let outcome = vm.run(1).unwrap();
                    if step == 6 {
                        assert_eq!(
                            outcome,
                            RunOutcome::Completed(Some(bits(width, &case.value)))
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
}

#[test]
fn managed_mix_mutation_keeps_receipts_and_original_objects_through_instruction_recovery() {
    let program = program(false);
    let database = Database::new();
    let mut store = CheckpointStore::open(&database.0).unwrap();
    let mut vm = Vm::new(&program);
    vm.start(entry(&program, "SaveObject"), vec![]).unwrap();
    vm.run(1000).unwrap();
    let root = field(&program, "Renraku.Guest.Simulation", "Saved");
    let value_field = field(&program, "Renraku.Guest.Counter", "Value");
    let mutations = field(&program, "Renraku.Guest.IntegerShifts", "Mutations");
    let counter = vm.static_fields()[&root];
    let method = method(&program, "MutateMix");
    let binding = program.method(method).unwrap().binding;
    let args = vec![counter, Value::I32(5)];
    let intent = vm
        .invoke_once(&mut store, "mix-first", method, args.clone(), 0, 1)
        .unwrap();
    let mut terminal = None;
    let mut saw_nested = false;
    let mut saw_shift = false;
    for _ in 0..250 {
        let saved = store.load().unwrap();
        let mut vm = saved.snapshot.restore().unwrap();
        vm.collect_garbage().unwrap();
        assert_eq!(vm.context().id, intent.execution);
        assert_eq!(
            saved.snapshot.program.method(method).unwrap().binding,
            binding
        );
        assert_eq!(vm.static_fields()[&root], counter);
        saw_nested |= vm.context().frames.len() > 1;
        let frame = vm.context().frames.last().unwrap();
        if let renraku_vm::execution::Continuation::Execute { next } = frame.continuation {
            saw_shift |= matches!(
                saved
                    .snapshot
                    .program
                    .method(frame.method.method)
                    .unwrap()
                    .instructions[&next]
                    .op,
                Op::ShiftLeft | Op::ShiftRight { .. }
            );
        }
        let receipt = vm
            .invoke_once(&mut store, "mix-first", method, args.clone(), 1, 1)
            .unwrap();
        if let InvocationState::Completed { result } = receipt.state {
            assert_eq!(result, Some(Value::I32(612)));
            terminal = Some(receipt);
            break;
        }
    }
    assert!(saw_nested && saw_shift);
    let terminal = terminal.expect("mix finished");
    let saved = store.load().unwrap();
    let mut vm = saved.snapshot.restore().unwrap();
    assert_eq!(vm.static_fields()[&mutations], Value::I32(1));
    let second = vm
        .invoke_once(
            &mut store,
            "mix-second",
            method,
            vec![counter, Value::I32(37)],
            1000,
            7,
        )
        .unwrap();
    assert_eq!(
        second.state,
        InvocationState::Completed {
            result: Some(Value::I32(19481))
        }
    );
    vm.start(entry(&program, "Arithmetic"), vec![]).unwrap();
    vm.run(1000).unwrap();
    vm.collect_garbage().unwrap();
    store.save(&vm).unwrap();
    let saved = store.load().unwrap();
    let mut vm = saved.snapshot.restore().unwrap();
    let before = vm.instructions_executed();
    assert_eq!(
        vm.invoke_once(&mut store, "mix-first", method, args, 1000, 1)
            .unwrap(),
        terminal
    );
    assert_eq!(vm.instructions_executed(), before);
    assert_eq!(vm.static_fields()[&mutations], Value::I32(2));
    assert_eq!(
        vm.heap()
            .object(counter.referenced_object().unwrap())
            .unwrap()
            .fields()[&value_field],
        Value::I32(19481)
    );
}

#[test]
fn sdk_free_native_processes_finish_raw_counts_and_replay_original_execution_results() {
    let program = program(true);
    for (name, width, input, count, expected) in [
        ("Shl32", 32, "1", -1, Value::I32(i32::MIN)),
        ("Shr32", 32, "-2147483648", 32, Value::I32(i32::MIN)),
        ("ShrZero32", 32, "-2147483648", -1, Value::I32(1)),
        ("Shl64", 64, "1", 64, Value::I64(1)),
        ("Shr64", 64, "-9223372036854775808", -1, Value::I64(-1)),
        ("ShrZero64", 64, "-9223372036854775808", 63, Value::I64(1)),
    ] {
        let database = Database::new();
        let mut store = CheckpointStore::open(&database.0).unwrap();
        let method = method(&program, name);
        let mut vm = Vm::new(&program);
        let pending = vm
            .invoke_once(
                &mut store,
                "raw-shift",
                method,
                vec![bits(width, input), Value::I32(count)],
                5,
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
                    "raw-shift",
                    &method.to_string(),
                    "1000",
                    &format!("i{width}:{input}"),
                    &format!("i32:{count}"),
                ])
                .env("PATH", "/no-shift-sdk-tools")
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
            vm.invocation("raw-shift").unwrap().state,
            InvocationState::Completed {
                result: Some(expected)
            }
        );
    }
}
