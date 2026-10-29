#[allow(dead_code)]
mod common;
use common::fixtures;
use renraku_vm::{
    checkpoint::CheckpointStore,
    execution::ExecutionStatus,
    identity::{ObjectId, UniverseId},
    invocation::InvocationState,
    program::Program,
    runtime::{RunOutcome, Vm},
    value::Value,
};
use serde::Deserialize;
use std::{fs, path::PathBuf, process::Command};

fn program() -> Program {
    Program::load(&[
        &fixtures().core,
        &fs::read(fixtures().output.join("Renraku.Collections.dll")).unwrap(),
        &fs::read(fixtures().output.join("Renraku.CollectionTests.dll")).unwrap(),
    ])
    .unwrap()
}
fn entry(program: &Program, name: &str) -> renraku_vm::identity::MethodId {
    program
        .entry(&format!("Renraku.Tests.MapHarness::{name}"))
        .unwrap()
}
fn call(vm: &mut Vm<'_>, program: &Program, name: &str, arguments: Vec<Value>) -> Option<Value> {
    vm.start(entry(program, name), arguments).unwrap();
    let RunOutcome::Completed(value) = vm.run(15000).unwrap() else {
        panic!("map operation exceeded budget")
    };
    value
}
fn text(vm: &mut Vm<'_>, units: Option<Vec<u16>>) -> Value {
    units.map_or(Value::Null, |units| {
        Value::Reference(vm.new_string(units).unwrap())
    })
}
fn string(vm: &mut Vm<'_>, value: &str) -> Value {
    text(vm, Some(value.encode_utf16().collect()))
}
fn units(vm: &Vm<'_>, value: Value) -> Option<Vec<u16>> {
    value.referenced_object().map(|id| {
        vm.heap()
            .object(id)
            .unwrap()
            .string_units()
            .unwrap()
            .to_vec()
    })
}
fn field(vm: &Vm<'_>, program: &Program, id: ObjectId, name: &str) -> Value {
    let object = vm.heap().object(id).unwrap();
    let field = program
        .fields()
        .find(|f| f.owner == object.type_id() && f.name == name)
        .unwrap();
    object.fields()[&field.id]
}
fn saved(vm: &Vm<'_>, program: &Program) -> ObjectId {
    let owner = program.find_type("Renraku.Tests.MapHarness").unwrap();
    let field = program
        .fields()
        .find(|f| f.owner == owner && f.name == "Saved")
        .unwrap();
    vm.static_fields()[&field.id].referenced_object().unwrap()
}
#[derive(Debug, Eq, PartialEq)]
struct View {
    success: bool,
    found: bool,
    changed: bool,
    observed: Option<Vec<u16>>,
    error: String,
    count: i32,
}
fn view(vm: &Vm<'_>, program: &Program, value: Value) -> View {
    let id = value.referenced_object().unwrap();
    let boolean = |name| field(vm, program, id, name) == Value::I32(1);
    let Value::I32(count) = field(vm, program, id, "Count") else {
        panic!()
    };
    View {
        success: boolean("Success"),
        found: boolean("Found"),
        changed: boolean("Changed"),
        observed: units(vm, field(vm, program, id, "Value")),
        error: String::from_utf16(&units(vm, field(vm, program, id, "Error")).unwrap()).unwrap(),
        count,
    }
}
#[derive(Deserialize)]
struct Reference {
    operation: String,
    key: Option<Vec<u16>>,
    value: Option<Vec<u16>>,
    success: bool,
    found: bool,
    changed: bool,
    observed: Option<Vec<u16>>,
    error: String,
    count: i32,
}

#[test]
fn emitted_cil_map_matches_coreclr_ordinal_dictionary_for_bounded_stateful_trace() {
    let program = program();
    let data: Vec<Reference> = serde_json::from_slice(
        &fs::read(fixtures().output.join("string-map-reference.json")).unwrap(),
    )
    .unwrap();
    assert!(data.len() > 400);
    let mut vm = Vm::new(&program);
    call(&mut vm, &program, "Initialize", vec![]);
    let map = saved(&vm, &program);
    for case in data {
        vm.collect_garbage().unwrap();
        let mut args = vec![text(&mut vm, case.key.clone())];
        if case.operation == "Set" {
            args.push(text(&mut vm, case.value.clone()));
        }
        let value = call(&mut vm, &program, &case.operation, args).unwrap();
        assert_eq!(
            view(&vm, &program, value),
            View {
                success: case.success,
                found: case.found,
                changed: case.changed,
                observed: case.observed,
                error: case.error,
                count: case.count
            },
            "{} {:?}",
            case.operation,
            case.key
        );
        assert_eq!(saved(&vm, &program), map);
        assert_eq!(vm.effects().count(), 0);
    }
}

struct Database(PathBuf);
impl Database {
    fn new() -> Self {
        Self(
            fixtures()
                .output
                .join(format!("string-map-{}.db", UniverseId::fresh())),
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
fn graph(vm: &Vm<'_>, program: &Program, map: ObjectId) -> Vec<(ObjectId, Value, Value)> {
    let mut next = field(vm, program, map, "head");
    let mut nodes = Vec::new();
    while let Some(node) = next.referenced_object() {
        nodes.push((
            node,
            field(vm, program, node, "Key"),
            field(vm, program, node, "Value"),
        ));
        next = field(vm, program, node, "Next");
        assert!(nodes.len() <= 64);
    }
    nodes
}

#[test]
fn shared_strings_equal_value_noop_and_removed_receipt_roots_survive_gc() {
    let program = program();
    let mut vm = Vm::new(&program);
    call(&mut vm, &program, "Initialize", vec![]);
    let a = string(&mut vm, "a");
    let b = string(&mut vm, "b");
    let shared = text(&mut vm, Some(vec![0xd800, 0]));
    call(&mut vm, &program, "Set", vec![a, shared]);
    call(&mut vm, &program, "Set", vec![b, shared]);
    let map = saved(&vm, &program);
    let before = graph(&vm, &program, map);
    assert_eq!(before.len(), 2);
    assert!(before.iter().all(|n| n.2 == shared));
    let duplicate = text(&mut vm, Some(vec![0xd800, 0]));
    let equal_key = string(&mut vm, "a");
    let result = call(&mut vm, &program, "Set", vec![equal_key, duplicate]).unwrap();
    assert!(!view(&vm, &program, result).changed);
    assert_eq!(graph(&vm, &program, map), before);
    vm.collect_garbage().unwrap();
    assert!(
        vm.heap()
            .object(duplicate.referenced_object().unwrap())
            .is_err()
    );
    let database = Database::new();
    let mut store = CheckpointStore::open(&database.0).unwrap();
    let result = vm
        .invoke_once(
            &mut store,
            "remove-a",
            entry(&program, "Remove"),
            vec![a],
            15000,
            100,
        )
        .unwrap();
    let InvocationState::Completed {
        result: Some(returned),
    } = result.state
    else {
        panic!()
    };
    assert_eq!(
        field(
            &vm,
            &program,
            returned.referenced_object().unwrap(),
            "Value"
        ),
        shared
    );
    call(&mut vm, &program, "Remove", vec![b]);
    call(&mut vm, &program, "Initialize", vec![]);
    vm.collect_garbage().unwrap();
    assert!(before.iter().all(|n| vm.heap().object(n.0).is_err()));
    assert!(
        vm.heap()
            .object(shared.referenced_object().unwrap())
            .is_ok()
    );
    store.save(&vm).unwrap();
    let checkpoint = store.load().unwrap();
    let mut vm = checkpoint.snapshot.restore().unwrap();
    assert_eq!(
        vm.invoke_once(
            &mut store,
            "remove-a",
            entry(&program, "Remove"),
            vec![a],
            15000,
            1
        )
        .unwrap(),
        result
    );
    assert_eq!(units(&vm, shared), Some(vec![0xd800, 0]));
}

#[test]
fn every_mutation_instruction_recovers_with_gc_pinned_receipts_and_typed_map_arguments() {
    let program = program();
    let database = Database::new();
    let mut store = CheckpointStore::open(&database.0).unwrap();
    let mut vm = Vm::new(&program);
    call(&mut vm, &program, "Initialize", vec![]);
    for (key, value) in [("tail", "one"), ("middle", "two"), ("head", "three")] {
        let key = string(&mut vm, key);
        let value = string(&mut vm, value);
        call(&mut vm, &program, "Set", vec![key, value]);
    }
    let map = saved(&vm, &program);
    let initial = graph(&vm, &program, map);
    store.save(&vm).unwrap();
    for (index, (operation, key, value, expected_count)) in [
        ("Set", "new", Some("insert"), 4),
        ("Set", "middle", Some("replace"), 4),
        ("Set", "middle", Some("replace"), 4),
        ("Remove", "new", None, 3),
        ("Remove", "middle", None, 2),
        ("Remove", "tail", None, 1),
        ("Remove", "absent", None, 1),
        ("ReplaceRootAndSet", "old-map", Some("retained"), 2),
    ]
    .into_iter()
    .enumerate()
    {
        let checkpoint = store.load().unwrap();
        let mut vm = checkpoint.snapshot.restore().unwrap();
        let mut args = if operation == "ReplaceRootAndSet" {
            vec![Value::Reference(map)]
        } else {
            vec![]
        };
        args.push(string(&mut vm, key));
        if let Some(value) = value {
            args.push(string(&mut vm, value));
        }
        let method = entry(&program, operation);
        let binding = program.method(method).unwrap().binding;
        let id = format!("operation-{index}");
        let intent = vm
            .invoke_once(&mut store, &id, method, args.clone(), 0, 1)
            .unwrap();
        let mut terminal = None;
        for _ in 0..2000 {
            let checkpoint = store.load().unwrap();
            let mut vm = checkpoint.snapshot.restore().unwrap();
            vm.collect_garbage().unwrap();
            let instructions = vm.instructions_executed();
            let receipt = vm
                .invoke_once(&mut store, &id, method, args.clone(), 1, 1)
                .unwrap();
            assert_eq!(receipt.execution, intent.execution);
            assert_eq!(receipt.method, binding);
            assert_eq!(vm.instructions_executed(), instructions + 1);
            if let InvocationState::Completed {
                result: Some(value),
            } = receipt.state
            {
                let result = view(&vm, &program, value);
                assert_eq!(result.count, expected_count);
                assert_eq!(result.success, key != "absent");
                assert_eq!(result.changed, index != 2 && key != "absent");
                let instructions = vm.instructions_executed();
                assert_eq!(
                    vm.invoke_once(&mut store, &id, method, args.clone(), 15000, 1)
                        .unwrap(),
                    receipt
                );
                assert_eq!(vm.instructions_executed(), instructions);
                terminal = Some(receipt);
                break;
            }
        }
        assert!(terminal.is_some());
    }
    let checkpoint = store.load().unwrap();
    let mut vm = checkpoint.snapshot.restore().unwrap();
    vm.collect_garbage().unwrap();
    assert_ne!(saved(&vm, &program), map);
    let remaining = graph(&vm, &program, map);
    assert_eq!(remaining.len(), 2);
    assert!(remaining.iter().any(|n| n.0 == initial[0].0));
    let head = string(&mut vm, "head");
    let value = call(
        &mut vm,
        &program,
        "GetOn",
        vec![Value::Reference(map), head],
    )
    .unwrap();
    assert_eq!(
        view(&vm, &program, value).observed,
        Some("three".encode_utf16().collect())
    );
    let owner = program.find_type("Renraku.Tests.MapHarness").unwrap();
    let operations = program
        .fields()
        .find(|f| f.owner == owner && f.name == "Operations")
        .unwrap();
    assert_eq!(vm.static_fields()[&operations.id], Value::I32(11));
}

#[test]
fn native_process_resumes_linked_mutation_without_sdk() {
    let program = program();
    let database = Database::new();
    let mut store = CheckpointStore::open(&database.0).unwrap();
    let mut vm = Vm::new(&program);
    call(&mut vm, &program, "Initialize", vec![]);
    let key = string(&mut vm, "key");
    let value = string(&mut vm, "value");
    let method = entry(&program, "Set");
    let first = vm
        .invoke_once(&mut store, "native-set", method, vec![key, value], 40, 1)
        .unwrap();
    assert_eq!(first.state, InvocationState::Running);
    drop(store);
    let output = Command::new(env!("CARGO_BIN_EXE_renraku-vm"))
        .arg("resume")
        .arg(&database.0)
        .args(["15000", "100"])
        .env("PATH", "/no-collection-tools")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let mut store = CheckpointStore::open(&database.0).unwrap();
    let checkpoint = store.load().unwrap();
    let mut vm = checkpoint.snapshot.restore().unwrap();
    assert!(matches!(
        vm.context().status,
        ExecutionStatus::Completed { .. }
    ));
    let result = vm
        .invoke_once(&mut store, "native-set", method, vec![key, value], 15000, 1)
        .unwrap();
    assert_eq!(result.execution, first.execution);
    let InvocationState::Completed {
        result: Some(value),
    } = result.state
    else {
        panic!()
    };
    assert_eq!(view(&vm, &program, value).count, 1);
}
