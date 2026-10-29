#[allow(dead_code)]
mod common;
use common::{entry, fixtures};
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
use serde_json::Value as Json;
use std::{collections::BTreeSet, fs, path::PathBuf, process::Command};
fn program() -> Program {
    Program::load(&[
        &fixtures().core,
        &fixtures().guest,
        &fs::read(fixtures().output.join("Renraku.CheckedArithmeticTests.dll")).unwrap(),
    ])
    .unwrap()
}
fn method(p: &Program, name: &str) -> MethodId {
    p.entry(&format!("Renraku.Guest.CheckedIntegerArithmetic::{name}"))
        .unwrap()
}
fn field(p: &Program, owner: &str, name: &str) -> FieldId {
    let owner = p.find_type(owner).unwrap();
    p.fields()
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
                .join(format!("checked-{}.db", UniverseId::fresh())),
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
#[derive(Clone, Deserialize)]
struct Case {
    method: String,
    width: u32,
    left: String,
    right: String,
    value: Option<String>,
    fault: Option<String>,
}
impl Case {
    fn args(&self) -> Vec<Value> {
        vec![bits(self.width, &self.left), bits(self.width, &self.right)]
    }
}
fn cases() -> Vec<Case> {
    serde_json::from_slice(
        &fs::read(fixtures().output.join("checked-arithmetic-reference.json")).unwrap(),
    )
    .unwrap()
}
fn checked(op: &Op) -> bool {
    matches!(
        op,
        Op::AddChecked { .. } | Op::SubtractChecked { .. } | Op::MultiplyChecked { .. }
    )
}
fn representatives() -> Vec<Case> {
    let cases = cases();
    let names: BTreeSet<_> = cases.iter().map(|c| c.method.as_str()).collect();
    let mut result = vec![];
    for name in names {
        for left in [
            if name.ends_with("32") {
                "-2147483648"
            } else {
                "-9223372036854775808"
            },
            "-1",
        ] {
            result.push(
                cases
                    .iter()
                    .find(|c| {
                        c.method == name && c.left == left && c.right == "0" && c.value.is_some()
                    })
                    .unwrap()
                    .clone(),
            );
        }
        result.push(
            cases
                .iter()
                .find(|c| c.method == name && c.fault.is_some())
                .unwrap()
                .clone(),
        );
    }
    result
}
fn counter(vm: &Vm<'_>, p: &Program) -> ObjectId {
    vm.static_fields()[&field(p, "Renraku.Guest.Simulation", "Saved")]
        .referenced_object()
        .unwrap()
}
fn assert_counter(vm: &Vm<'_>, p: &Program, id: ObjectId, value: i32, mutations: i32) {
    assert_eq!(
        vm.heap().object(id).unwrap().fields()[&field(p, "Renraku.Guest.Counter", "Value")],
        Value::I32(value)
    );
    assert_eq!(
        vm.static_fields()[&field(p, "Renraku.Guest.CheckedIntegerArithmetic", "Mutations")],
        Value::I32(mutations)
    );
}
#[test]
fn checked_integer_opcodes_match_coreclr_and_independent_big_integer_ranges() {
    let program = program();
    let cases = cases();
    assert_eq!(cases.len(), 4608);
    let mut faults = 0;
    for case in cases {
        let method = method(&program, &case.method);
        let body = program.method(method).unwrap();
        assert_eq!(body.instructions.len(), 4);
        assert_eq!(body.instructions[&2].next, 3);
        let unsigned = case.method.contains('U');
        assert!(match &body.instructions[&2].op {
            Op::AddChecked { unsigned: u } => case.method.starts_with("Add") && *u == unsigned,
            Op::SubtractChecked { unsigned: u } => case.method.starts_with("Sub") && *u == unsigned,
            Op::MultiplyChecked { unsigned: u } => case.method.starts_with("Mul") && *u == unsigned,
            _ => false,
        });
        let mut vm = Vm::new(&program);
        let args = case.args();
        vm.start(method, args.clone()).unwrap();
        vm.run(2).unwrap();
        let before = vm.context().clone();
        let outcome = vm.run(1);
        if let Some(expected) = case.value {
            assert!(case.fault.is_none());
            outcome.unwrap();
            assert_eq!(
                vm.run(1).unwrap(),
                RunOutcome::Completed(Some(bits(case.width, &expected))),
                "{}({}, {})",
                case.method,
                case.left,
                case.right
            );
        } else {
            faults += 1;
            assert_eq!(case.fault.as_deref(), Some("System.OverflowException"));
            let error = outcome.unwrap_err().0;
            assert!(error.contains("overflow") && error.contains("IL_0002"));
            assert_eq!(vm.context().frames, before.frames);
            assert_eq!(vm.context().id, before.id);
            assert_eq!(vm.context().frames[0].evaluation_stack, args);
            assert_eq!(vm.instructions_executed(), 2);
        }
        assert_eq!(vm.effects().count(), 0);
    }
    assert!(faults > 1000);
}
#[test]
fn checked_success_and_overflow_stacks_restore_each_instruction_with_gc_and_original_ids() {
    let program = program();
    let cases = representatives();
    assert_eq!(cases.len(), 36);
    for case in cases {
        let database = Database::new();
        let mut store = CheckpointStore::open(&database.0).unwrap();
        let method = method(&program, &case.method);
        let binding = program.method(method).unwrap().binding;
        let mut vm = Vm::new(&program);
        vm.start(method, case.args()).unwrap();
        let universe = vm.universe_id();
        let execution = vm.context().id;
        store.save(&vm).unwrap();
        for step in 0..4 {
            let saved = store.load().unwrap();
            let mut vm = saved.snapshot.restore().unwrap();
            assert_eq!(vm.universe_id(), universe);
            assert_eq!(vm.context().id, execution);
            assert_eq!(vm.context().frames[0].method, binding);
            let before = vm.context().clone();
            vm.collect_garbage().unwrap();
            assert_eq!(vm.context(), &before);
            let outcome = vm.run(1);
            if step == 2 && case.fault.is_some() {
                assert!(outcome.unwrap_err().0.contains("overflow"));
                assert_eq!(vm.context().frames, before.frames);
                store.save(&vm).unwrap();
                let saved = store.load().unwrap();
                let vm = saved.snapshot.restore().unwrap();
                assert!(matches!(
                    vm.context().status,
                    ExecutionStatus::Faulted { .. }
                ));
                assert_eq!(vm.context().frames, before.frames);
                break;
            }
            let outcome = outcome.unwrap();
            if step == 3 {
                assert_eq!(
                    outcome,
                    RunOutcome::Completed(Some(bits(case.width, case.value.as_ref().unwrap())))
                );
            } else {
                assert_eq!(outcome, RunOutcome::Paused);
            }
            store.save(&vm).unwrap();
        }
    }
}
#[test]
fn nested_checked_work_resumes_once_and_replays_its_result_after_later_mutations() {
    let program = program();
    let database = Database::new();
    let mut store = CheckpointStore::open(&database.0).unwrap();
    let mut vm = Vm::new(&program);
    vm.start(entry(&program, "SaveObject"), vec![]).unwrap();
    vm.run(1000).unwrap();
    let object = counter(&vm, &program);
    let method = method(&program, "Apply");
    let args = vec![Value::Reference(object), Value::I32(1)];
    let intent = vm
        .invoke_once(&mut store, "checked-first", method, args.clone(), 0, 1)
        .unwrap();
    let mut terminal = None;
    let mut saw_nested = false;
    for _ in 0..200 {
        let saved = store.load().unwrap();
        let mut vm = saved.snapshot.restore().unwrap();
        vm.collect_garbage().unwrap();
        assert_eq!(vm.context().id, intent.execution);
        let frame = vm.context().frames.last().unwrap();
        if let Continuation::Execute { next } = frame.continuation {
            saw_nested |= vm.context().frames.len() > 1
                && checked(
                    &saved
                        .snapshot
                        .program
                        .method(frame.method.method)
                        .unwrap()
                        .instructions[&next]
                        .op,
                );
        }
        let receipt = vm
            .invoke_once(&mut store, "checked-first", method, args.clone(), 1, 1)
            .unwrap();
        if let InvocationState::Completed { result } = receipt.state {
            assert_eq!(result, Some(Value::I32(29)));
            terminal = Some(receipt);
            break;
        }
    }
    assert!(saw_nested);
    let terminal = terminal.unwrap();
    let saved = store.load().unwrap();
    let mut vm = saved.snapshot.restore().unwrap();
    let second = vm
        .invoke_once(
            &mut store,
            "checked-second",
            method,
            vec![Value::Reference(object), Value::I32(2)],
            1000,
            7,
        )
        .unwrap();
    assert_eq!(
        second.state,
        InvocationState::Completed {
            result: Some(Value::I32(42))
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
        vm.invoke_once(&mut store, "checked-first", method, args, 1000, 1)
            .unwrap(),
        terminal
    );
    assert_eq!(vm.instructions_executed(), before);
    assert_counter(&vm, &program, object, 42, 2);
}
#[test]
fn every_checked_overflow_receipt_keeps_prior_mutation_and_roots_after_context_and_static_replacement()
 {
    let program = program();
    for case in representatives().into_iter().filter(|c| c.fault.is_some()) {
        let database = Database::new();
        let mut store = CheckpointStore::open(&database.0).unwrap();
        let mut vm = Vm::new(&program);
        vm.start(entry(&program, "SaveObject"), vec![]).unwrap();
        vm.run(1000).unwrap();
        let object = counter(&vm, &program);
        let method = method(&program, &format!("Mutate{}", case.method));
        let mut args = vec![Value::Reference(object)];
        args.extend(case.args());
        let intent = vm
            .invoke_once(&mut store, "checked-fault", method, args.clone(), 0, 1)
            .unwrap();
        let mut terminal = None;
        for _ in 0..150 {
            let saved = store.load().unwrap();
            let mut vm = saved.snapshot.restore().unwrap();
            vm.collect_garbage().unwrap();
            let before = vm.context().clone();
            let instructions = vm.instructions_executed();
            let receipt = vm
                .invoke_once(&mut store, "checked-fault", method, args.clone(), 1, 1)
                .unwrap();
            assert_eq!(receipt.execution, intent.execution);
            if let InvocationState::Faulted { message } = &receipt.state {
                assert!(message.contains("overflow"));
                assert_eq!(vm.context().frames, before.frames);
                assert_eq!(vm.instructions_executed(), instructions);
                assert_eq!(
                    vm.context().frames.last().unwrap().evaluation_stack,
                    case.args()
                );
                terminal = Some(receipt);
                break;
            }
        }
        let terminal = terminal.unwrap();
        let saved = store.load().unwrap();
        let mut vm = saved.snapshot.restore().unwrap();
        assert_counter(&vm, &program, object, 20, 1);
        vm.start(entry(&program, "SaveObject"), vec![]).unwrap();
        vm.run(1000).unwrap();
        assert_ne!(counter(&vm, &program), object);
        vm.collect_garbage().unwrap();
        store.save(&vm).unwrap();
        let saved = store.load().unwrap();
        let mut vm = saved.snapshot.restore().unwrap();
        let before = vm.instructions_executed();
        assert_eq!(
            vm.invoke_once(&mut store, "checked-fault", method, args, 1000, 1)
                .unwrap(),
            terminal
        );
        assert_eq!(vm.instructions_executed(), before);
        assert_counter(&vm, &program, object, 20, 1);
    }
}
#[test]
fn failed_checked_fault_saves_recover_committed_operands_without_repeating_mutation() {
    let program = program();
    for case in representatives()
        .into_iter()
        .filter(|c| c.width == 64 && c.fault.is_some())
    {
        let database = Database::new();
        let mut store = CheckpointStore::open(&database.0).unwrap();
        let mut vm = Vm::new(&program);
        vm.start(entry(&program, "SaveObject"), vec![]).unwrap();
        vm.run(1000).unwrap();
        let object = counter(&vm, &program);
        let method = method(&program, &format!("Mutate{}", case.method));
        let mut args = vec![Value::Reference(object)];
        args.extend(case.args());
        let intent = vm
            .invoke_once(&mut store, "checked-save-fault", method, args.clone(), 0, 1)
            .unwrap();
        loop {
            let frame = vm.context().frames.last().unwrap();
            if frame.method.method == self::method(&program, &case.method)
                && frame.continuation == (Continuation::Execute { next: 2 })
            {
                break;
            }
            vm.invoke_once(&mut store, "checked-save-fault", method, args.clone(), 1, 1)
                .unwrap();
        }
        assert_counter(&vm, &program, object, 20, 1);
        let before = vm.context().clone();
        let connection = rusqlite::Connection::open(&database.0).unwrap();
        let rejected = store.generation() + 2;
        connection.execute_batch(&format!("CREATE TRIGGER reject_checked BEFORE UPDATE ON checkpoint WHEN NEW.generation={rejected} BEGIN SELECT RAISE(ABORT,'injected checked fault save'); END;")).unwrap();
        assert!(
            vm.invoke_once(&mut store, "checked-save-fault", method, args.clone(), 1, 1)
                .is_err()
        );
        assert_eq!(store.generation(), rejected - 1);
        connection
            .execute_batch("DROP TRIGGER reject_checked")
            .unwrap();
        let saved = store.load().unwrap();
        let mut vm = saved.snapshot.restore().unwrap();
        assert_eq!(vm.context().frames, before.frames);
        assert_eq!(
            vm.invocation("checked-save-fault").unwrap().state,
            InvocationState::Running
        );
        vm.collect_garbage().unwrap();
        let receipt = vm
            .invoke_once(&mut store, "checked-save-fault", method, args.clone(), 1, 1)
            .unwrap();
        assert_eq!(receipt.execution, intent.execution);
        assert!(matches!(receipt.state, InvocationState::Faulted { .. }));
        assert_counter(&vm, &program, object, 20, 1);
        assert_eq!(
            vm.invoke_once(&mut store, "checked-save-fault", method, args, 1000, 1)
                .unwrap(),
            receipt
        );
    }
}
#[test]
fn sdk_free_native_processes_resume_checked_operands_and_replay_success_or_overflow_once() {
    let program = program();
    for case in representatives()
        .into_iter()
        .filter(|c| c.left == "-1" && c.right == "0" && c.value.is_some() || c.fault.is_some())
    {
        let database = Database::new();
        let mut store = CheckpointStore::open(&database.0).unwrap();
        let mut vm = Vm::new(&program);
        let mut args = vec![];
        let mut wire = vec![];
        let mut object = None;
        let name = if case.fault.is_some() {
            vm.start(entry(&program, "SaveObject"), vec![]).unwrap();
            vm.run(1000).unwrap();
            let id = counter(&vm, &program);
            object = Some(id);
            args.push(Value::Reference(id));
            wire.push(format!("object:{id}"));
            format!("Mutate{}", case.method)
        } else {
            case.method.clone()
        };
        args.extend(case.args());
        wire.push(format!("i{}:{}", case.width, case.left));
        wire.push(format!("i{}:{}", case.width, case.right));
        let method = method(&program, &name);
        let intent = vm
            .invoke_once(&mut store, "native-checked", method, args.clone(), 0, 1)
            .unwrap();
        loop {
            let frame = vm.context().frames.last().unwrap();
            if frame.method.method == self::method(&program, &case.method)
                && frame.continuation == (Continuation::Execute { next: 2 })
            {
                break;
            }
            vm.invoke_once(&mut store, "native-checked", method, args.clone(), 1, 1)
                .unwrap();
        }
        let bindings: Vec<_> = program.methods().map(|m| m.binding).collect();
        vm.collect_garbage().unwrap();
        store.save(&vm).unwrap();
        drop(store);
        let invoke = || -> Json {
            let output = Command::new(env!("CARGO_BIN_EXE_renraku-vm"))
                .args([
                    "invoke-once",
                    database.0.to_str().unwrap(),
                    "native-checked",
                    &method.to_string(),
                    "1000",
                ])
                .args(&wire)
                .env("PATH", "/no-checked-sdk-tools")
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
            "diagnostic",
        ] {
            assert_eq!(first[key], replay[key]);
        }
        let mut store = CheckpointStore::open(&database.0).unwrap();
        let saved = store.load().unwrap();
        let vm = saved.snapshot.restore().unwrap();
        if let Some(id) = object {
            assert!(matches!(
                vm.invocation("native-checked").unwrap().state,
                InvocationState::Faulted { .. }
            ));
            assert_counter(&vm, &program, id, 20, 1);
        } else {
            assert_eq!(
                vm.invocation("native-checked").unwrap().state,
                InvocationState::Completed {
                    result: Some(bits(case.width, case.value.as_ref().unwrap()))
                }
            );
        }
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
    }
}
