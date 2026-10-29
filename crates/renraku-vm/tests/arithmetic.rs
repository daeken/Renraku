#[allow(dead_code)]
mod common;
use common::{entry, fixtures, program};
use renraku_vm::{
    checkpoint::CheckpointStore,
    cil::Op,
    execution::{Continuation, ExecutionStatus},
    identity::{FieldId, MethodId, ObjectId, UniverseId},
    invocation::InvocationState,
    program::Program,
    runtime::{RunOutcome, Vm},
    value::Value,
};
use serde::Deserialize;
use std::{fs, path::PathBuf, process::Command};

struct Database(PathBuf);
impl Database {
    fn new() -> Self {
        Self(
            fixtures()
                .output
                .join(format!("arithmetic-{}.db", UniverseId::fresh())),
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
fn method(program: &Program, name: &str) -> MethodId {
    program
        .entry(&format!("Renraku.Guest.IntegerArithmetic::{name}"))
        .unwrap()
}
fn field(program: &Program, owner: &str, name: &str) -> FieldId {
    let owner = program.types().find(|t| t.name == owner).unwrap().id;
    program
        .fields()
        .find(|f| f.name == name && f.owner == owner)
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
#[derive(Deserialize)]
struct Reference {
    method: String,
    width: u32,
    left: String,
    right: String,
    value: Option<String>,
    fault: Option<String>,
}

#[test]
fn emitted_integer_operators_match_coreclr_boundary_matrix() {
    let cases: Vec<Reference> = serde_json::from_slice(
        &fs::read(fixtures().output.join("arithmetic-reference.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(cases.len(), 2312);
    let program = program();
    let mut faults = 0;
    for mut case in cases {
        if (case.method == "Rem32" && case.left == i32::MIN.to_string()
            || case.method == "Rem64" && case.left == i64::MIN.to_string())
            && case.right == "-1"
        {
            // ECMA permits either result here. Pin Renraku's portable fault
            // policy even on a CoreCLR implementation that returns zero.
            assert!(
                case.value.as_deref() == Some("0") && case.fault.is_none()
                    || case.value.is_none()
                        && case.fault.as_deref() == Some("System.OverflowException")
            );
            case.value = None;
            case.fault = Some("System.OverflowException".into());
        }
        let entry = method(&program, &case.method);
        let body = program.method(entry).unwrap();
        assert_eq!(body.instructions.len(), 4);
        let op = &body.instructions[&2].op;
        let unsigned = case.method.contains('U');
        assert!(
            match op {
                Op::Divide { unsigned: actual } =>
                    case.method.starts_with("Div") && unsigned == *actual,
                Op::Remainder { unsigned: actual } =>
                    case.method.starts_with("Rem") && unsigned == *actual,
                _ => false,
            },
            "{op:?}"
        );
        let args = vec![bits(case.width, &case.left), bits(case.width, &case.right)];
        let mut vm = Vm::new(&program);
        vm.start(entry, args.clone()).unwrap();
        assert_eq!(vm.run(2).unwrap(), RunOutcome::Paused);
        let before = vm.context().clone();
        let outcome = vm.run(1);
        let description = format!("{}({}, {})", case.method, case.left, case.right);
        if let Some(expected) = case.value {
            assert!(case.fault.is_none());
            outcome.unwrap();
            assert_eq!(
                vm.run(1).unwrap(),
                RunOutcome::Completed(Some(bits(case.width, &expected))),
                "{description}"
            );
        } else {
            faults += 1;
            let message = outcome.unwrap_err().0;
            let diagnostic = match case.fault.as_deref() {
                Some("System.DivideByZeroException") => "by zero",
                Some("System.OverflowException") => "overflow",
                other => panic!("unexpected CoreCLR fault: {other:?}"),
            };
            assert!(message.contains(diagnostic), "{description}: {message}");
            assert!(message.contains("IL_0002"));
            assert_eq!(vm.context().frames, before.frames);
            assert_eq!(vm.context().id, before.id);
            assert_eq!(vm.context().frames[0].evaluation_stack, args);
            assert_eq!(vm.instructions_executed(), 2);
        }
    }
    assert_eq!(faults, 140);
}

#[test]
fn all_operator_stacks_survive_each_instruction_with_gc_and_original_ids() {
    let program = program();
    for (name, width, a, b, expected) in [
        ("Div32", 32, "-14", "3", "-4"),
        ("Rem32", 32, "-14", "3", "-2"),
        ("DivU32", 32, "-1", "3", "1431655765"),
        ("RemU32", 32, "-2", "3", "2"),
        (
            "Div64",
            64,
            "-9223372036854775808",
            "3",
            "-3074457345618258602",
        ),
        ("Rem64", 64, "-9223372036854775808", "3", "-2"),
        ("DivU64", 64, "-1", "3", "6148914691236517205"),
        ("RemU64", 64, "-2", "3", "2"),
    ] {
        let database = Database::new();
        let mut store = CheckpointStore::open(&database.0).unwrap();
        let entry = method(&program, name);
        let binding = program.method(entry).unwrap().binding;
        let mut vm = Vm::new(&program);
        vm.start(entry, vec![bits(width, a), bits(width, b)])
            .unwrap();
        let universe = vm.universe_id();
        let execution = vm.context().id;
        store.save(&vm).unwrap();
        for step in 0..4 {
            let saved = store.load().unwrap();
            let mut vm = saved.snapshot.restore().unwrap();
            assert_eq!(vm.universe_id(), universe);
            assert_eq!(vm.context().id, execution);
            assert_eq!(vm.context().frames[0].method, binding);
            assert_eq!(
                saved.snapshot.program.method(entry).unwrap().binding,
                binding
            );
            if step == 2 {
                assert_eq!(
                    vm.context().frames[0].evaluation_stack,
                    vec![bits(width, a), bits(width, b)]
                );
            }
            vm.collect_garbage().unwrap();
            let outcome = vm.run(1).unwrap();
            if step == 3 {
                assert_eq!(outcome, RunOutcome::Completed(Some(bits(width, expected))));
            } else {
                assert_eq!(outcome, RunOutcome::Paused);
            }
            store.save(&vm).unwrap();
        }
    }
}

#[test]
fn nested_remainder_and_digit_loops_recover_every_instruction_with_a_cyclic_object() {
    let program = program();
    let expected: i32 =
        fs::read_to_string(fixtures().output.join("arithmetic-workload-reference.txt"))
            .unwrap()
            .parse()
            .unwrap();
    assert_eq!(expected, 59);
    let database = Database::new();
    let mut store = CheckpointStore::open(&database.0).unwrap();
    let entry = method(&program, "PersistentArithmetic");
    let root = field(
        &program,
        "Renraku.Guest.IntegerArithmetic",
        "ArithmeticState",
    );
    let value = field(&program, "Renraku.Guest.Counter", "Value");
    let next = field(&program, "Renraku.Guest.Counter", "Next");
    let mut vm = Vm::new(&program);
    vm.start(entry, vec![]).unwrap();
    let execution = vm.context().id;
    store.save(&vm).unwrap();
    let mut object = None;
    let mut saw_nested = false;
    for _ in 0..1500 {
        let saved = store.load().unwrap();
        let mut vm = saved.snapshot.restore().unwrap();
        assert_eq!(vm.context().id, execution);
        saw_nested |= vm.context().frames.len() >= 3;
        vm.collect_garbage().unwrap();
        if let Some(Value::Reference(id)) = vm.static_fields().get(&root) {
            assert_eq!(*object.get_or_insert(*id), *id);
            assert_eq!(
                vm.heap().object(*id).unwrap().fields()[&next],
                Value::Reference(*id)
            );
        }
        let outcome = vm.run(1).unwrap();
        store.save(&vm).unwrap();
        if outcome == RunOutcome::Completed(Some(Value::I32(expected))) {
            assert!(saw_nested);
            assert_eq!(
                vm.heap().object(object.unwrap()).unwrap().fields()[&value],
                Value::I32(59)
            );
            return;
        }
    }
    panic!("arithmetic workload did not finish");
}

fn saved_counter(vm: &Vm<'_>, program: &Program) -> ObjectId {
    vm.static_fields()[&field(program, "Renraku.Guest.Simulation", "Saved")]
        .referenced_object()
        .unwrap()
}

#[test]
fn arithmetic_fault_receipts_retain_operands_and_never_repeat_prior_mutations() {
    let program = program();
    for width in [32, 64] {
        for operation in ["Div", "Rem"] {
            for divisor in ["0", "-1"] {
                let database = Database::new();
                let mut store = CheckpointStore::open(&database.0).unwrap();
                let mut vm = Vm::new(&program);
                vm.start(entry(&program, "SaveObject"), vec![]).unwrap();
                vm.run(100).unwrap();
                let counter = saved_counter(&vm, &program);
                let entry = method(&program, &format!("Mutate{operation}{width}"));
                let min = if width == 32 {
                    "-2147483648"
                } else {
                    "-9223372036854775808"
                };
                let args = vec![
                    Value::Reference(counter),
                    bits(width, min),
                    bits(width, divisor),
                ];
                let first = vm
                    .invoke_once(&mut store, "arithmetic-fault", entry, args.clone(), 0, 1)
                    .unwrap();
                let mut terminal = None;
                for _ in 0..100 {
                    let saved = store.load().unwrap();
                    let mut vm = saved.snapshot.restore().unwrap();
                    vm.collect_garbage().unwrap();
                    let before = vm.context().clone();
                    let instructions = vm.instructions_executed();
                    let receipt = vm
                        .invoke_once(&mut store, "arithmetic-fault", entry, args.clone(), 1, 1)
                        .unwrap();
                    assert_eq!(receipt.execution, first.execution);
                    if let InvocationState::Faulted { message } = &receipt.state {
                        assert!(message.contains(if divisor == "0" {
                            "by zero"
                        } else {
                            "overflow"
                        }));
                        assert_eq!(vm.context().frames, before.frames);
                        assert_eq!(vm.instructions_executed(), instructions);
                        assert_eq!(
                            vm.context().frames.last().unwrap().evaluation_stack,
                            args[1..]
                        );
                        assert!(matches!(
                            vm.context().frames.last().unwrap().continuation,
                            Continuation::Execute { next: 2 }
                        ));
                        terminal = Some(receipt);
                        break;
                    }
                }
                let receipt = terminal.unwrap();
                let saved = store.load().unwrap();
                let mut vm = saved.snapshot.restore().unwrap();
                assert_eq!(
                    vm.heap().object(counter).unwrap().fields()
                        [&field(&program, "Renraku.Guest.Counter", "Value")],
                    Value::I32(20)
                );
                assert!(matches!(
                    vm.context().status,
                    ExecutionStatus::Faulted { .. }
                ));
                // Replace the static root and context; the receipt still roots its argument.
                vm.start(common::entry(&program, "SaveObject"), vec![])
                    .unwrap();
                vm.run(100).unwrap();
                assert_ne!(saved_counter(&vm, &program), counter);
                vm.collect_garbage().unwrap();
                let instructions = vm.instructions_executed();
                assert_eq!(
                    vm.invoke_once(&mut store, "arithmetic-fault", entry, args.clone(), 100, 1)
                        .unwrap(),
                    receipt
                );
                assert_eq!(vm.instructions_executed(), instructions);
                assert_eq!(
                    vm.heap().object(counter).unwrap().fields()
                        [&field(&program, "Renraku.Guest.Counter", "Value")],
                    Value::I32(20)
                );
            }
        }
    }
}

#[test]
fn failed_fault_checkpoint_replays_from_committed_operands_with_no_second_mutation() {
    let program = program();
    let database = Database::new();
    let mut store = CheckpointStore::open(&database.0).unwrap();
    let mut vm = Vm::new(&program);
    vm.start(entry(&program, "SaveObject"), vec![]).unwrap();
    vm.run(100).unwrap();
    let counter = saved_counter(&vm, &program);
    let entry = method(&program, "MutateRem64");
    let args = vec![
        Value::Reference(counter),
        Value::I64(i64::MIN),
        Value::I64(-1),
    ];
    vm.invoke_once(&mut store, "fault-save", entry, args.clone(), 0, 1)
        .unwrap();
    loop {
        let frame = vm.context().frames.last().unwrap();
        if frame.method.method == method(&program, "Rem64")
            && frame.continuation == (Continuation::Execute { next: 2 })
        {
            assert_eq!(frame.evaluation_stack, args[1..]);
            break;
        }
        vm.invoke_once(&mut store, "fault-save", entry, args.clone(), 1, 1)
            .unwrap();
    }
    let connection = rusqlite::Connection::open(&database.0).unwrap();
    // The replay fence commits, but the subsequent terminal fault save fails.
    let rejected = store.generation() + 2;
    connection.execute_batch(&format!("CREATE TRIGGER reject_fault BEFORE UPDATE ON checkpoint WHEN NEW.generation={rejected} BEGIN SELECT RAISE(ABORT,'injected arithmetic fault save'); END;")).unwrap();
    assert!(
        vm.invoke_once(&mut store, "fault-save", entry, args.clone(), 1, 1)
            .is_err()
    );
    assert_eq!(store.generation(), rejected - 1);
    connection
        .execute_batch("DROP TRIGGER reject_fault")
        .unwrap();
    let saved = store.load().unwrap();
    let mut vm = saved.snapshot.restore().unwrap();
    assert_eq!(
        vm.invocation("fault-save").unwrap().state,
        InvocationState::Running
    );
    let receipt = vm
        .invoke_once(&mut store, "fault-save", entry, args.clone(), 1, 1)
        .unwrap();
    assert!(matches!(receipt.state, InvocationState::Faulted { .. }));
    assert_eq!(
        vm.heap().object(counter).unwrap().fields()
            [&field(&program, "Renraku.Guest.Counter", "Value")],
        Value::I32(20)
    );
    assert_eq!(
        vm.invoke_once(&mut store, "fault-save", entry, args, 100, 1)
            .unwrap(),
        receipt
    );
}

#[test]
fn native_process_resumes_division_without_sdk_or_original_assemblies() {
    let program = program();
    let database = Database::new();
    let mut store = CheckpointStore::open(&database.0).unwrap();
    let mut vm = Vm::new(&program);
    vm.start(
        method(&program, "DivU64"),
        vec![Value::I64(-1), Value::I64(3)],
    )
    .unwrap();
    vm.run(2).unwrap();
    store.save(&vm).unwrap();
    drop(store);
    let output = Command::new(env!("CARGO_BIN_EXE_renraku-vm"))
        .arg("resume")
        .arg(&database.0)
        .args(["2", "1"])
        .env("PATH", "/no-sdk-or-assemblies")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let mut store = CheckpointStore::open(&database.0).unwrap();
    let saved = store.load().unwrap();
    let vm = saved.snapshot.restore().unwrap();
    assert_eq!(
        vm.context().status,
        ExecutionStatus::Completed {
            result: Some(Value::I64(6148914691236517205))
        }
    );
}
