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
fn program(raw: bool) -> Program {
    Program::load(&[
        &fixtures().core,
        &fixtures().guest,
        &fs::read(fixtures().output.join(if raw {
            "Renraku.RawSmallCheckedConversionTests.dll"
        } else {
            "Renraku.SmallCheckedConversionTests.dll"
        }))
        .unwrap(),
    ])
    .unwrap()
}
fn method(p: &Program, name: &str) -> MethodId {
    p.entry(&format!(
        "Renraku.Guest.SmallCheckedIntegerConversions::{name}"
    ))
    .unwrap()
}
fn field(p: &Program, owner: &str, name: &str) -> FieldId {
    let owner = p.find_type(owner).unwrap();
    p.fields()
        .find(|f| f.owner == owner && f.name == name)
        .unwrap()
        .id
}
fn bits(width: u32, n: &str) -> Value {
    if width <= 32 {
        Value::I32(n.parse().unwrap())
    } else {
        Value::I64(n.parse().unwrap())
    }
}
struct Database(PathBuf);
impl Database {
    fn new() -> Self {
        Self(fixtures().output.join(format!(
            "small-checked-conversion-{}.db",
            UniverseId::fresh()
        )))
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
#[serde(rename_all = "camelCase")]
struct Case {
    name: String,
    raw: bool,
    width: u32,
    target_width: u32,
    target_unsigned: bool,
    source_unsigned: bool,
    input: String,
    value: Option<String>,
    fault: Option<String>,
}
impl Case {
    fn args(&self) -> Vec<Value> {
        vec![bits(self.width, &self.input)]
    }
}
fn cases() -> Vec<Case> {
    serde_json::from_slice(
        &fs::read(
            fixtures()
                .output
                .join("small-checked-conversion-reference.json"),
        )
        .unwrap(),
    )
    .unwrap()
}
fn representatives() -> Vec<Case> {
    let cases = cases();
    let names: BTreeSet<_> = cases.iter().map(|c| c.name.as_str()).collect();
    let mut result = vec![];
    for name in names {
        result.push(
            cases
                .iter()
                .find(|c| c.name == name && c.input == "0" && c.value.is_some())
                .unwrap()
                .clone(),
        );
        result.push(
            cases
                .iter()
                .find(|c| c.name == name && c.input == "-1" && c.value.is_some())
                .or_else(|| {
                    cases
                        .iter()
                        .find(|c| c.name == name && c.input == "1" && c.value.is_some())
                })
                .unwrap()
                .clone(),
        );
        if let Some(fault) = cases.iter().find(|c| c.name == name && c.fault.is_some()) {
            result.push(fault.clone());
        }
    }
    for source in ["I32", "U32", "I64", "U64"] {
        for unit in ["55296", "57343"] {
            result.push(
                cases
                    .iter()
                    .find(|c| c.name == format!("{source}ToChar") && c.input == unit)
                    .unwrap()
                    .clone(),
            );
        }
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
        vm.static_fields()[&field(
            p,
            "Renraku.Guest.SmallCheckedIntegerConversions",
            "Mutations"
        )],
        Value::I32(mutations)
    );
}
#[test]
fn checked_csharp_casts_and_every_raw_opcode_match_coreclr_and_big_integer_ranges() {
    let normal = program(false);
    let raw = program(true);
    let cases = cases();
    assert_eq!(cases.len(), 6192);
    let mut opcodes = BTreeSet::new();
    let mut faults = 0;
    for case in cases {
        let p = if case.raw { &raw } else { &normal };
        let method = method(p, &case.name);
        let body = p.method(method).unwrap();
        if case.raw {
            let op = &body.instructions[&1];
            assert_eq!(op.next, 2);
            let Op::ConvertSmallChecked {
                sixteen,
                unsigned,
                source_unsigned,
            } = op.op
            else {
                panic!("wrong opcode")
            };
            assert_eq!(sixteen, case.target_width == 16);
            assert_eq!(unsigned, case.target_unsigned);
            assert_eq!(source_unsigned, case.source_unsigned);
            opcodes.insert((sixteen, unsigned, source_unsigned));
        }
        let mut vm = Vm::new(p);
        let args = case.args();
        vm.start(method, args.clone()).unwrap();
        vm.run(1).unwrap();
        let before = vm.context().clone();
        let outcome = vm.run(10);
        if let Some(expected) = case.value {
            assert!(case.fault.is_none());
            assert_eq!(
                outcome.unwrap(),
                RunOutcome::Completed(Some(bits(case.target_width, &expected))),
                "{}({})",
                case.name,
                case.input
            );
        } else {
            faults += 1;
            assert_eq!(case.fault.as_deref(), Some("System.OverflowException"));
            let message = outcome.unwrap_err().0;
            assert!(message.contains("overflow") && message.contains("IL_0001"));
            assert_eq!(vm.context().frames, before.frames);
            assert_eq!(vm.context().frames[0].evaluation_stack, args);
            assert_eq!(vm.instructions_executed(), 1);
        }
        assert_eq!(vm.effects().count(), 0);
    }
    assert_eq!(opcodes.len(), 8);
    assert!(faults > 1000);
}
#[test]
fn checked_cast_success_and_faults_restore_each_instruction_with_gc_and_original_identities() {
    let cases = representatives();
    assert_eq!(cases.len(), 116);
    for raw in [false, true] {
        let program = program(raw);
        for case in cases.iter().filter(|c| c.raw == raw) {
            let database = Database::new();
            let mut store = CheckpointStore::open(&database.0).unwrap();
            let method = method(&program, &case.name);
            let binding = program.method(method).unwrap().binding;
            let mut vm = Vm::new(&program);
            vm.start(method, case.args()).unwrap();
            let universe = vm.universe_id();
            let execution = vm.context().id;
            store.save(&vm).unwrap();
            let mut finished = false;
            for _ in 0..10 {
                let saved = store.load().unwrap();
                let mut vm = saved.snapshot.restore().unwrap();
                assert_eq!(vm.universe_id(), universe);
                assert_eq!(vm.context().id, execution);
                assert_eq!(vm.context().frames[0].method, binding);
                let before = vm.context().clone();
                vm.collect_garbage().unwrap();
                assert_eq!(vm.context(), &before);
                let outcome = vm.run(1);
                store.save(&vm).unwrap();
                match outcome {
                    Err(e) => {
                        assert!(case.fault.is_some() && e.0.contains("overflow"));
                        assert_eq!(vm.context().frames, before.frames);
                        let saved = store.load().unwrap();
                        let vm = saved.snapshot.restore().unwrap();
                        assert_eq!(vm.context().frames, before.frames);
                        assert!(matches!(
                            vm.context().status,
                            ExecutionStatus::Faulted { .. }
                        ));
                        finished = true;
                        break;
                    }
                    Ok(RunOutcome::Completed(value)) => {
                        assert_eq!(
                            value,
                            Some(bits(case.target_width, case.value.as_ref().unwrap()))
                        );
                        finished = true;
                        break;
                    }
                    Ok(RunOutcome::Paused) => {}
                    other => panic!("unexpected {other:?}"),
                }
            }
            assert!(finished, "{}({})", case.name, case.input);
        }
    }
}
#[test]
fn checked_char_stores_keep_exact_surrogates_and_old_receipts_through_later_faults_and_gc() {
    let program = program(false);
    let database = Database::new();
    let mut store = CheckpointStore::open(&database.0).unwrap();
    let mut vm = Vm::new(&program);
    vm.start(entry(&program, "SaveObject"), vec![]).unwrap();
    vm.run(1000).unwrap();
    let object = counter(&vm, &program);
    let method = method(&program, "StoreChar");
    let unit = field(
        &program,
        "Renraku.Guest.SmallCheckedIntegerConversions",
        "SavedUnit",
    );
    let args = vec![Value::Reference(object), Value::I64(0xd800)];
    let intent = vm
        .invoke_once(&mut store, "char-first", method, args.clone(), 0, 1)
        .unwrap();
    let mut terminal = None;
    let mut saw_checked = false;
    for _ in 0..200 {
        let saved = store.load().unwrap();
        let mut vm = saved.snapshot.restore().unwrap();
        vm.collect_garbage().unwrap();
        assert_eq!(vm.context().id, intent.execution);
        let frame = vm.context().frames.last().unwrap();
        if let Continuation::Execute { next } = frame.continuation {
            saw_checked |= vm.context().frames.len() > 1
                && matches!(
                    saved
                        .snapshot
                        .program
                        .method(frame.method.method)
                        .unwrap()
                        .instructions[&next]
                        .op,
                    Op::ConvertSmallChecked { .. }
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
    assert!(saw_checked);
    let terminal = terminal.unwrap();
    let saved = store.load().unwrap();
    let mut vm = saved.snapshot.restore().unwrap();
    assert_eq!(vm.static_fields()[&unit], Value::I32(0xd800));
    assert_counter(&vm, &program, object, 20, 1);
    assert_eq!(
        vm.invoke_once(
            &mut store,
            "char-second",
            method,
            vec![Value::Reference(object), Value::I64(0xdfff)],
            1000,
            7
        )
        .unwrap()
        .state,
        InvocationState::Completed {
            result: Some(Value::I32(0xdfff))
        }
    );
    let invalid_args = vec![Value::Reference(object), Value::I64(65536)];
    let fault = vm
        .invoke_once(
            &mut store,
            "char-overflow",
            method,
            invalid_args.clone(),
            1000,
            7,
        )
        .unwrap();
    assert!(matches!(fault.state, InvocationState::Faulted { .. }));
    assert_counter(&vm, &program, object, 22, 3);
    assert_eq!(vm.static_fields()[&unit], Value::I32(0xdfff));
    vm.start(entry(&program, "SaveObject"), vec![]).unwrap();
    vm.run(1000).unwrap();
    assert_ne!(counter(&vm, &program), object);
    vm.collect_garbage().unwrap();
    store.save(&vm).unwrap();
    let saved = store.load().unwrap();
    let mut vm = saved.snapshot.restore().unwrap();
    let before = vm.instructions_executed();
    assert_eq!(
        vm.invoke_once(&mut store, "char-first", method, args, 1000, 1)
            .unwrap(),
        terminal
    );
    assert_eq!(
        vm.invoke_once(&mut store, "char-overflow", method, invalid_args, 1000, 1)
            .unwrap(),
        fault
    );
    assert_eq!(vm.instructions_executed(), before);
    assert_counter(&vm, &program, object, 22, 3);
    assert_eq!(vm.static_fields()[&unit], Value::I32(0xdfff));
}
#[test]
fn checked_cast_fault_receipts_retain_prior_mutations_through_static_context_replacement_and_gc() {
    let cases: Vec<_> = representatives()
        .into_iter()
        .filter(|c| c.fault.is_some())
        .collect();
    assert_eq!(cases.len(), 36);
    for raw in [false, true] {
        let program = program(raw);
        for case in cases.iter().filter(|c| c.raw == raw) {
            let database = Database::new();
            let mut store = CheckpointStore::open(&database.0).unwrap();
            let mut vm = Vm::new(&program);
            vm.start(entry(&program, "SaveObject"), vec![]).unwrap();
            vm.run(1000).unwrap();
            let object = counter(&vm, &program);
            let method = method(&program, &format!("Mutate{}", case.name));
            let mut args = vec![Value::Reference(object)];
            args.extend(case.args());
            let intent = vm
                .invoke_once(&mut store, "cast-fault", method, args.clone(), 0, 1)
                .unwrap();
            let mut terminal = None;
            for _ in 0..150 {
                let saved = store.load().unwrap();
                let mut vm = saved.snapshot.restore().unwrap();
                vm.collect_garbage().unwrap();
                let before = vm.context().clone();
                let instructions = vm.instructions_executed();
                let receipt = vm
                    .invoke_once(&mut store, "cast-fault", method, args.clone(), 1, 1)
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
                vm.invoke_once(&mut store, "cast-fault", method, args, 1000, 1)
                    .unwrap(),
                terminal
            );
            assert_eq!(vm.instructions_executed(), before);
            assert_counter(&vm, &program, object, 20, 1);
        }
    }
}
#[test]
fn failed_conversion_fault_saves_reload_committed_operands_without_repeating_mutation() {
    let program = program(true);
    let cases: Vec<_> = representatives()
        .into_iter()
        .filter(|c| c.raw && c.width == 64 && c.fault.is_some())
        .collect();
    assert_eq!(cases.len(), 8);
    for case in cases {
        let database = Database::new();
        let mut store = CheckpointStore::open(&database.0).unwrap();
        let mut vm = Vm::new(&program);
        vm.start(entry(&program, "SaveObject"), vec![]).unwrap();
        vm.run(1000).unwrap();
        let object = counter(&vm, &program);
        let method = method(&program, &format!("Mutate{}", case.name));
        let mut args = vec![Value::Reference(object)];
        args.extend(case.args());
        let intent = vm
            .invoke_once(&mut store, "cast-save-fault", method, args.clone(), 0, 1)
            .unwrap();
        loop {
            let frame = vm.context().frames.last().unwrap();
            if frame.method.method == self::method(&program, &case.name)
                && frame.continuation == (Continuation::Execute { next: 1 })
            {
                break;
            }
            vm.invoke_once(&mut store, "cast-save-fault", method, args.clone(), 1, 1)
                .unwrap();
        }
        assert_counter(&vm, &program, object, 20, 1);
        let before = vm.context().clone();
        let connection = rusqlite::Connection::open(&database.0).unwrap();
        let rejected = store.generation() + 2;
        connection.execute_batch(&format!("CREATE TRIGGER reject_cast BEFORE UPDATE ON checkpoint WHEN NEW.generation={rejected} BEGIN SELECT RAISE(ABORT,'injected checked cast save'); END;")).unwrap();
        assert!(
            vm.invoke_once(&mut store, "cast-save-fault", method, args.clone(), 1, 1)
                .is_err()
        );
        assert_eq!(store.generation(), rejected - 1);
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
        assert!(matches!(receipt.state, InvocationState::Faulted { .. }));
        assert_counter(&vm, &program, object, 20, 1);
        assert_eq!(
            vm.invoke_once(&mut store, "cast-save-fault", method, args, 1000, 1)
                .unwrap(),
            receipt
        );
    }
}
#[test]
fn sdk_free_native_processes_resume_raw_checked_casts_and_replay_success_or_mutating_overflow() {
    let program = program(true);
    let cases: Vec<_> = representatives()
        .into_iter()
        .filter(|c| c.raw && (c.fault.is_some() || c.input != "0"))
        .collect();
    assert_eq!(cases.len(), 32);
    for case in cases {
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
            format!("Mutate{}", case.name)
        } else {
            case.name.clone()
        };
        args.extend(case.args());
        wire.push(format!("i{}:{}", case.width, case.input));
        let method = method(&program, &name);
        let intent = vm
            .invoke_once(&mut store, "native-cast", method, args.clone(), 0, 1)
            .unwrap();
        loop {
            let frame = vm.context().frames.last().unwrap();
            if frame.method.method == self::method(&program, &case.name)
                && frame.continuation == (Continuation::Execute { next: 1 })
            {
                break;
            }
            vm.invoke_once(&mut store, "native-cast", method, args.clone(), 1, 1)
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
                    "native-cast",
                    &method.to_string(),
                    "1000",
                ])
                .args(&wire)
                .env("PATH", "/no-checked-cast-sdk")
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
                vm.invocation("native-cast").unwrap().state,
                InvocationState::Faulted { .. }
            ));
            assert_counter(&vm, &program, id, 20, 1);
        } else {
            assert_eq!(
                vm.invocation("native-cast").unwrap().state,
                InvocationState::Completed {
                    result: Some(bits(case.target_width, case.value.as_ref().unwrap()))
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

fn native_invoke(
    database: &Database,
    request: &str,
    method: MethodId,
    arguments: &[String],
) -> Json {
    let output = Command::new(env!("CARGO_BIN_EXE_renraku-vm"))
        .args([
            "invoke-once",
            database.0.to_str().unwrap(),
            request,
            &method.to_string(),
            "1000",
        ])
        .args(arguments)
        .env("PATH", "/no-small-checked-sdk")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}
#[test]
fn native_char_returns_and_saved_surrogate_state_replay_without_repeated_mutations() {
    let program = program(false);
    for (name, arg, expected) in [
        ("I32ToChar", Value::I32(0xd800), 0xd800),
        ("U32ToChar", Value::I32(0xdbff), 0xdbff),
        ("I64ToChar", Value::I64(0xdc00), 0xdc00),
        ("U64ToChar", Value::I64(0xdfff), 0xdfff),
    ] {
        let database = Database::new();
        let mut store = CheckpointStore::open(&database.0).unwrap();
        let mut vm = Vm::new(&program);
        let method = method(&program, name);
        let intent = vm
            .invoke_once(&mut store, "native-char-return", method, vec![arg], 1, 1)
            .unwrap();
        assert_eq!(intent.state, InvocationState::Running);
        vm.collect_garbage().unwrap();
        store.save(&vm).unwrap();
        drop(store);
        let wire = vec![match arg {
            Value::I32(n) => format!("i32:{n}"),
            Value::I64(n) => format!("i64:{n}"),
            _ => unreachable!(),
        }];
        let first = native_invoke(&database, "native-char-return", method, &wire);
        let replay = native_invoke(&database, "native-char-return", method, &wire);
        assert_eq!(first["execution"], intent.execution.to_string());
        assert_eq!(first["result"]["value"], expected);
        assert_eq!(first["result"], replay["result"]);
        let mut store = CheckpointStore::open(&database.0).unwrap();
        let saved = store.load().unwrap();
        let vm = saved.snapshot.restore().unwrap();
        assert_eq!(
            vm.invocation("native-char-return").unwrap().state,
            InvocationState::Completed {
                result: Some(Value::I32(expected))
            }
        );
    }
    let database = Database::new();
    let mut store = CheckpointStore::open(&database.0).unwrap();
    let mut vm = Vm::new(&program);
    vm.start(entry(&program, "SaveObject"), vec![]).unwrap();
    vm.run(1000).unwrap();
    let object = counter(&vm, &program);
    let method = method(&program, "StoreChar");
    let callee = self::method(&program, "I64ToChar");
    let unit = field(
        &program,
        "Renraku.Guest.SmallCheckedIntegerConversions",
        "SavedUnit",
    );
    let args = vec![Value::Reference(object), Value::I64(0xd800)];
    let intent = vm
        .invoke_once(&mut store, "native-char-store", method, args.clone(), 0, 1)
        .unwrap();
    loop {
        let frame = vm.context().frames.last().unwrap();
        if frame.method.method == callee
            && frame.continuation == (Continuation::Execute { next: 1 })
        {
            break;
        }
        vm.invoke_once(&mut store, "native-char-store", method, args.clone(), 1, 1)
            .unwrap();
    }
    assert_counter(&vm, &program, object, 20, 1);
    assert_eq!(vm.static_fields()[&unit], Value::I32(0));
    vm.collect_garbage().unwrap();
    store.save(&vm).unwrap();
    drop(store);
    let wire = vec![format!("object:{object}"), "i64:55296".into()];
    let first = native_invoke(&database, "native-char-store", method, &wire);
    assert_eq!(first["execution"], intent.execution.to_string());
    assert_eq!(first["result"]["value"], 0xd800);
    let mut store = CheckpointStore::open(&database.0).unwrap();
    let saved = store.load().unwrap();
    let mut vm = saved.snapshot.restore().unwrap();
    assert_counter(&vm, &program, object, 20, 1);
    assert_eq!(vm.static_fields()[&unit], Value::I32(0xd800));
    assert_eq!(
        vm.invoke_once(
            &mut store,
            "native-char-second",
            method,
            vec![Value::Reference(object), Value::I64(0xdfff)],
            1000,
            7
        )
        .unwrap()
        .state,
        InvocationState::Completed {
            result: Some(Value::I32(0xdfff))
        }
    );
    vm.collect_garbage().unwrap();
    store.save(&vm).unwrap();
    drop(store);
    let replay = native_invoke(&database, "native-char-store", method, &wire);
    for key in ["execution", "result", "methodId", "codeVersion", "state"] {
        assert_eq!(first[key], replay[key]);
    }
    let mut store = CheckpointStore::open(&database.0).unwrap();
    let saved = store.load().unwrap();
    let vm = saved.snapshot.restore().unwrap();
    assert_counter(&vm, &program, object, 21, 2);
    assert_eq!(vm.static_fields()[&unit], Value::I32(0xdfff));
}
