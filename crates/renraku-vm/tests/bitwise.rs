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
        &fs::read(fixtures().output.join("Renraku.BitwiseTests.dll")).unwrap(),
    ])
    .unwrap()
}
fn method(program: &Program, name: &str) -> MethodId {
    program
        .entry(&format!("Renraku.Guest.IntegerBitwise::{name}"))
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
                .join(format!("bitwise-{}.db", UniverseId::fresh())),
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
    left: String,
    right: Option<String>,
    value: String,
}

#[test]
fn emitted_signed_and_unsigned_bitwise_operations_match_coreclr_boundary_and_seeded_matrix() {
    let program = program();
    let cases: Vec<Case> = serde_json::from_slice(
        &fs::read(fixtures().output.join("bitwise-reference.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(cases.len(), 4128);
    for case in cases {
        let method = method(&program, &case.method);
        let instructions = &program.method(method).unwrap().instructions;
        let unary = case.method.starts_with("Not");
        assert_eq!(instructions.len(), if unary { 3 } else { 4 });
        let offset = if unary { 1 } else { 2 };
        assert!(match instructions[&offset].op {
            Op::BitAnd => case.method.starts_with("And"),
            Op::BitOr => case.method.starts_with("Or"),
            Op::BitXor => case.method.starts_with("Xor"),
            Op::BitNot => unary,
            _ => false,
        });
        assert_eq!(instructions[&offset].next, offset + 1);
        let mut args = vec![bits(case.width, &case.left)];
        if let Some(right) = &case.right {
            args.push(bits(case.width, right));
        }
        let mut vm = Vm::new(&program);
        vm.start(method, args).unwrap();
        assert_eq!(
            vm.run(8).unwrap(),
            RunOutcome::Completed(Some(bits(case.width, &case.value))),
            "{}({}, {:?})",
            case.method,
            case.left,
            case.right
        );
        assert_eq!(vm.effects().count(), 0);
    }
}

#[test]
fn every_bitwise_instruction_roundtrips_with_original_ids_and_exact_stacks() {
    let program = program();
    for width in [32, 64] {
        let a = if width == 32 {
            i64::from(i32::MIN)
        } else {
            i64::MIN
        };
        let b = if width == 32 {
            i64::from(0x55555555)
        } else {
            0x5555555555555555
        };
        for unsigned in ["", "U"] {
            for (operation, expected) in
                [("And", a & b), ("Or", a | b), ("Xor", a ^ b), ("Not", !a)]
            {
                let method = method(&program, &format!("{operation}{unsigned}{width}"));
                let binding = program.method(method).unwrap().binding;
                let mut args = vec![bits(width, &a.to_string())];
                if operation != "Not" {
                    args.push(bits(width, &b.to_string()));
                }
                let database = Database::new();
                let mut store = CheckpointStore::open(&database.0).unwrap();
                let mut vm = Vm::new(&program);
                vm.start(method, args.clone()).unwrap();
                let universe = vm.universe_id();
                let execution = vm.context().id;
                store.save(&vm).unwrap();
                let steps = program.method(method).unwrap().instructions.len();
                for step in 0..steps {
                    let saved = store.load().unwrap();
                    let mut vm = saved.snapshot.restore().unwrap();
                    assert_eq!(vm.universe_id(), universe);
                    assert_eq!(vm.context().id, execution);
                    assert_eq!(vm.context().frames[0].method, binding);
                    if step == args.len() {
                        assert_eq!(vm.context().frames[0].evaluation_stack, args);
                    }
                    let before = vm.context().clone();
                    vm.collect_garbage().unwrap();
                    assert_eq!(vm.context(), &before);
                    let outcome = vm.run(1).unwrap();
                    if step + 1 == steps {
                        assert_eq!(
                            outcome,
                            RunOutcome::Completed(Some(bits(width, &expected.to_string())))
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
fn durable_flags_mutation_recovers_every_instruction_and_replays_after_later_work() {
    let program = program();
    let database = Database::new();
    let mut store = CheckpointStore::open(&database.0).unwrap();
    let mut vm = Vm::new(&program);
    vm.start(entry(&program, "SaveObject"), vec![]).unwrap();
    vm.run(1000).unwrap();
    let root = field(&program, "Renraku.Guest.Simulation", "Saved");
    let value_field = field(&program, "Renraku.Guest.Counter", "Value");
    let mutations = field(&program, "Renraku.Guest.IntegerBitwise", "Mutations");
    let counter = vm.static_fields()[&root];
    let method = method(&program, "MutateFlags");
    let binding = program.method(method).unwrap().binding;
    let args = vec![counter, Value::I32(8), Value::I32(2), Value::I32(4)];
    let intent = vm
        .invoke_once(&mut store, "flags-first", method, args.clone(), 0, 1)
        .unwrap();
    let mut saw_bitwise = false;
    let mut saw_nested = false;
    let mut terminal = None;
    for _ in 0..200 {
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
            let op = &saved
                .snapshot
                .program
                .method(frame.method.method)
                .unwrap()
                .instructions[&next]
                .op;
            saw_bitwise |= matches!(op, Op::BitAnd | Op::BitOr | Op::BitXor | Op::BitNot);
        }
        let receipt = vm
            .invoke_once(&mut store, "flags-first", method, args.clone(), 1, 1)
            .unwrap();
        assert_eq!(receipt.execution, intent.execution);
        if let InvocationState::Completed { result } = receipt.state {
            assert_eq!(result, Some(Value::I32(29)));
            terminal = Some(receipt);
            break;
        }
    }
    assert!(saw_bitwise && saw_nested);
    let terminal = terminal.expect("flags mutation finished");
    let saved = store.load().unwrap();
    let mut vm = saved.snapshot.restore().unwrap();
    assert_eq!(vm.static_fields()[&mutations], Value::I32(1));
    assert_eq!(
        vm.heap()
            .object(counter.referenced_object().unwrap())
            .unwrap()
            .fields()[&value_field],
        Value::I32(29)
    );
    let second = vm
        .invoke_once(
            &mut store,
            "flags-second",
            method,
            vec![counter, Value::I32(64), Value::I32(1), Value::I32(8)],
            1000,
            7,
        )
        .unwrap();
    assert_eq!(
        second.state,
        InvocationState::Completed {
            result: Some(Value::I32(84))
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
        vm.invoke_once(&mut store, "flags-first", method, args, 1000, 1)
            .unwrap(),
        terminal
    );
    assert_eq!(vm.instructions_executed(), instructions);
    assert_eq!(vm.static_fields()[&mutations], Value::I32(2));
    assert_eq!(
        vm.heap()
            .object(counter.referenced_object().unwrap())
            .unwrap()
            .fields()[&value_field],
        Value::I32(84)
    );
}

#[test]
fn native_process_finishes_paused_bitwise_operands_and_replays_receipts_without_sdk() {
    let program = program();
    for (name, args, budget, expected) in [
        (
            "AndU32",
            vec![Value::I32(-1), Value::I32(i32::MIN)],
            2,
            Value::I32(i32::MIN),
        ),
        (
            "Or64",
            vec![Value::I64(i64::MIN), Value::I64(1)],
            2,
            Value::I64(i64::MIN + 1),
        ),
        (
            "XorU64",
            vec![Value::I64(-1), Value::I64(i64::MIN)],
            2,
            Value::I64(i64::MAX),
        ),
        (
            "NotU64",
            vec![Value::I64(i64::MIN)],
            1,
            Value::I64(i64::MAX),
        ),
        (
            "Next32",
            vec![Value::I32(19), Value::I32(8), Value::I32(2), Value::I32(4)],
            2,
            Value::I32(29),
        ),
    ] {
        let database = Database::new();
        let mut store = CheckpointStore::open(&database.0).unwrap();
        let method = method(&program, name);
        let mut vm = Vm::new(&program);
        let pending = vm
            .invoke_once(&mut store, "native-flags", method, args.clone(), budget, 1)
            .unwrap();
        assert_eq!(pending.state, InvocationState::Running);
        vm.collect_garbage().unwrap();
        store.save(&vm).unwrap();
        drop(store);
        let args = args
            .into_iter()
            .map(|a| match a {
                Value::I32(n) => format!("i32:{n}"),
                Value::I64(n) => format!("i64:{n}"),
                _ => unreachable!(),
            })
            .collect::<Vec<_>>();
        let invoke = || -> Json {
            let output = Command::new(env!("CARGO_BIN_EXE_renraku-vm"))
                .args([
                    "invoke-once",
                    database.0.to_str().unwrap(),
                    "native-flags",
                    &method.to_string(),
                    "1000",
                ])
                .args(&args)
                .env("PATH", "/no-bitwise-sdk-tools")
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
        assert_eq!(first["state"], "completed");
        assert_eq!(first["execution"], pending.execution.to_string());
        let replay = invoke();
        // The CLI fences each call at the current generation; the receipt's
        // pinned execution/result are independent of that observed generation.
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
            vm.invocation("native-flags").unwrap().state,
            InvocationState::Completed {
                result: Some(expected)
            }
        );
    }
}
