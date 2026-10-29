#[allow(dead_code)]
mod common;
use common::fixtures;
use renraku_vm::{
    checkpoint::CheckpointStore,
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
        &fs::read(fixtures().output.join("Renraku.ObjectCollections.dll")).unwrap(),
        &fs::read(fixtures().output.join("Renraku.ObjectCollectionTests.dll")).unwrap(),
    ])
    .unwrap()
}
fn entry(p: &Program, name: &str) -> renraku_vm::identity::MethodId {
    p.entry(&format!("Renraku.Tests.ObjectMapHarness::{name}"))
        .unwrap()
}
fn call(vm: &mut Vm<'_>, p: &Program, name: &str, args: Vec<Value>) -> Option<Value> {
    vm.start(entry(p, name), args).unwrap();
    let RunOutcome::Completed(value) = vm.run(20000).unwrap() else {
        panic!("operation exceeded budget")
    };
    value
}
fn text(vm: &mut Vm<'_>, units: Option<Vec<u16>>) -> Value {
    units.map_or(Value::Null, |s| Value::Reference(vm.new_string(s).unwrap()))
}
fn string(vm: &mut Vm<'_>, s: &str) -> Value {
    text(vm, Some(s.encode_utf16().collect()))
}
fn units(vm: &Vm<'_>, value: Value) -> Vec<u16> {
    vm.heap()
        .object(value.referenced_object().unwrap())
        .unwrap()
        .string_units()
        .unwrap()
        .to_vec()
}
fn field(vm: &Vm<'_>, p: &Program, id: ObjectId, name: &str) -> Value {
    let object = vm.heap().object(id).unwrap();
    let field = p
        .fields()
        .find(|f| f.owner == object.type_id() && f.name == name)
        .unwrap();
    object.fields()[&field.id]
}
fn static_(vm: &Vm<'_>, p: &Program, name: &str) -> Value {
    let owner = p.find_type("Renraku.Tests.ObjectMapHarness").unwrap();
    vm.static_fields()[&p
        .fields()
        .find(|f| f.owner == owner && f.name == name)
        .unwrap()
        .id]
}
fn saved(vm: &Vm<'_>, p: &Program) -> ObjectId {
    static_(vm, p, "Saved").referenced_object().unwrap()
}
fn pool(vm: &Vm<'_>, p: &Program, id: i32) -> Value {
    match id {
        0 => Value::Null,
        1 => static_(vm, p, "A"),
        2 => static_(vm, p, "B"),
        3 => static_(vm, p, "TextA"),
        4 => static_(vm, p, "TextB"),
        5 => static_(vm, p, "Saved"),
        _ => unreachable!(),
    }
}
fn initialize(vm: &mut Vm<'_>, p: &Program) {
    let a = text(vm, Some(vec![115, 97, 109, 101, 0xd800]));
    let b = text(vm, Some(vec![115, 97, 109, 101, 0xd800]));
    assert_ne!(a, b);
    call(vm, p, "Initialize", vec![a, b]);
}
struct Database(PathBuf);
impl Database {
    fn new() -> Self {
        Self(
            fixtures()
                .output
                .join(format!("object-map-{}.db", UniverseId::fresh())),
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
#[derive(Debug, PartialEq, Eq)]
struct View {
    success: bool,
    found: bool,
    changed: bool,
    observed: Value,
    error: String,
    count: i32,
}
fn view(vm: &Vm<'_>, p: &Program, value: Value) -> View {
    let id = value.referenced_object().unwrap();
    let boolean = |n| field(vm, p, id, n) == Value::I32(1);
    let Value::I32(count) = field(vm, p, id, "Count") else {
        panic!()
    };
    View {
        success: boolean("Success"),
        found: boolean("Found"),
        changed: boolean("Changed"),
        observed: field(vm, p, id, "Value"),
        error: String::from_utf16(&units(vm, field(vm, p, id, "Error"))).unwrap(),
        count,
    }
}
fn graph(vm: &Vm<'_>, p: &Program, map: ObjectId) -> Vec<(ObjectId, Value, Value)> {
    let mut next = field(vm, p, map, "head");
    let mut result = vec![];
    while let Some(id) = next.referenced_object() {
        result.push((id, field(vm, p, id, "Key"), field(vm, p, id, "Value")));
        next = field(vm, p, id, "Next");
        assert!(result.len() <= 64);
    }
    result
}
#[derive(Deserialize)]
struct Case {
    operation: String,
    key: Option<Vec<u16>>,
    value: i32,
    success: bool,
    found: bool,
    changed: bool,
    observed: i32,
    error: String,
    count: i32,
}
#[test]
fn object_map_matches_coreclr_ordinal_dictionary_with_reference_identity_values() {
    let p = program();
    let cases: Vec<Case> = serde_json::from_slice(
        &fs::read(fixtures().output.join("object-map-reference.json")).unwrap(),
    )
    .unwrap();
    assert!(cases.len() > 400);
    let mut vm = Vm::new(&p);
    initialize(&mut vm, &p);
    let map = saved(&vm, &p);
    for case in cases {
        vm.collect_garbage().unwrap();
        let mut args = vec![text(&mut vm, case.key.clone())];
        if case.operation == "Set" {
            args.push(pool(&vm, &p, case.value));
        }
        let result = call(&mut vm, &p, &case.operation, args).unwrap();
        assert_eq!(
            view(&vm, &p, result),
            View {
                success: case.success,
                found: case.found,
                changed: case.changed,
                observed: pool(&vm, &p, case.observed),
                error: case.error,
                count: case.count
            },
            "{} {:?}",
            case.operation,
            case.key
        );
        assert_eq!(saved(&vm, &p), map);
        assert_eq!(vm.effects().count(), 0);
    }
    let key = string(&mut vm, "no-implicit-boxing");
    let heap = vm.heap().clone();
    let context = vm.context().clone();
    for scalar in [Value::I32(0), Value::I32(-1), Value::I64(1)] {
        assert!(vm.start(entry(&p, "Set"), vec![key, scalar]).is_err());
        assert_eq!(vm.heap(), &heap);
        assert_eq!(vm.context(), &context);
    }
}
#[test]
fn shared_mutable_values_equal_text_replacement_and_removed_receipt_roots_keep_exact_identities() {
    let p = program();
    let mut vm = Vm::new(&p);
    initialize(&mut vm, &p);
    let map = saved(&vm, &p);
    let a = pool(&vm, &p, 1);
    let b = pool(&vm, &p, 2);
    let text_a = pool(&vm, &p, 3);
    let text_b = pool(&vm, &p, 4);
    assert_ne!(text_a, text_b);
    assert_eq!(units(&vm, text_a), units(&vm, text_b));
    let key = string(&mut vm, "strings");
    call(&mut vm, &p, "Set", vec![key, text_a]);
    let before = graph(&vm, &p, map);
    let equal_key = string(&mut vm, "strings");
    let result = call(&mut vm, &p, "Set", vec![equal_key, text_b]).unwrap();
    assert!(view(&vm, &p, result).changed);
    let after = graph(&vm, &p, map);
    assert_eq!(before[0].0, after[0].0);
    assert_eq!(before[0].1, after[0].1);
    assert_eq!(after[0].2, text_b);
    let result = call(&mut vm, &p, "Set", vec![equal_key, text_b]).unwrap();
    assert!(!view(&vm, &p, result).changed);
    assert_eq!(graph(&vm, &p, map), after);
    let key_a = string(&mut vm, "a");
    let key_b = string(&mut vm, "b");
    call(&mut vm, &p, "Set", vec![key_a, a]);
    call(&mut vm, &p, "Set", vec![key_b, a]);
    let nodes = graph(&vm, &p, map);
    let database = Database::new();
    let mut store = CheckpointStore::open(&database.0).unwrap();
    let observed = vm
        .invoke_once(
            &mut store,
            "observe-a",
            entry(&p, "Get"),
            vec![key_a],
            20000,
            100,
        )
        .unwrap();
    let InvocationState::Completed {
        result: Some(observed_result),
    } = observed.state
    else {
        panic!()
    };
    call(&mut vm, &p, "Link", vec![b, Value::Reference(map)]);
    assert_eq!(
        field(
            &vm,
            &p,
            observed_result.referenced_object().unwrap(),
            "Value"
        ),
        a
    );
    assert_eq!(field(&vm, &p, a.referenced_object().unwrap(), "Peer"), b);
    assert_eq!(
        field(&vm, &p, b.referenced_object().unwrap(), "Peer"),
        Value::Reference(map)
    );
    let removed = vm
        .invoke_once(
            &mut store,
            "remove-a",
            entry(&p, "Remove"),
            vec![key_a],
            20000,
            100,
        )
        .unwrap();
    let InvocationState::Completed {
        result: Some(result),
    } = removed.state
    else {
        panic!()
    };
    assert_eq!(view(&vm, &p, result).observed, a);
    call(&mut vm, &p, "Remove", vec![key_b]);
    call(&mut vm, &p, "Remove", vec![key]);
    call(&mut vm, &p, "ClearPool", vec![]);
    initialize(&mut vm, &p);
    vm.collect_garbage().unwrap();
    assert!(nodes.iter().all(|n| vm.heap().object(n.0).is_err()));
    assert!(vm.heap().object(a.referenced_object().unwrap()).is_ok());
    assert!(vm.heap().object(b.referenced_object().unwrap()).is_ok());
    store.save(&vm).unwrap();
    let saved = store.load().unwrap();
    let mut vm = saved.snapshot.restore().unwrap();
    let instructions = vm.instructions_executed();
    assert_eq!(
        vm.invoke_once(
            &mut store,
            "remove-a",
            entry(&p, "Remove"),
            vec![key_a],
            20000,
            1
        )
        .unwrap(),
        removed
    );
    assert_eq!(vm.instructions_executed(), instructions);
    assert_eq!(
        field(&vm, &p, result.referenced_object().unwrap(), "Value"),
        a
    );
}
#[test]
fn every_object_map_mutation_instruction_recovers_with_gc_and_typed_replaced_root_arguments() {
    let p = program();
    let database = Database::new();
    let mut store = CheckpointStore::open(&database.0).unwrap();
    let mut vm = Vm::new(&p);
    initialize(&mut vm, &p);
    let values = [
        pool(&vm, &p, 1),
        pool(&vm, &p, 2),
        pool(&vm, &p, 3),
        pool(&vm, &p, 4),
        Value::Reference(saved(&vm, &p)),
    ];
    for (i, key) in ["tail", "middle", "head"].into_iter().enumerate() {
        let key = string(&mut vm, key);
        call(&mut vm, &p, "Set", vec![key, values[i]]);
    }
    let map = saved(&vm, &p);
    let initial = graph(&vm, &p, map);
    store.save(&vm).unwrap();
    for (index, (operation, key, value, count)) in [
        ("Set", "new", Some(values[4]), 4),
        ("Set", "middle", Some(values[3]), 4),
        ("Set", "middle", Some(values[3]), 4),
        ("Remove", "new", None, 3),
        ("Remove", "middle", None, 2),
        ("Remove", "tail", None, 1),
        ("Remove", "absent", None, 1),
        ("ReplaceRootAndSet", "old-map", Some(values[0]), 2),
    ]
    .into_iter()
    .enumerate()
    {
        let saved = store.load().unwrap();
        let mut vm = saved.snapshot.restore().unwrap();
        let mut args = if operation == "ReplaceRootAndSet" {
            vec![Value::Reference(map)]
        } else {
            vec![]
        };
        args.push(string(&mut vm, key));
        if let Some(value) = value {
            args.push(value);
        }
        let method = entry(&p, operation);
        let binding = p.method(method).unwrap().binding;
        let id = format!("object-operation-{index}");
        let intent = vm
            .invoke_once(&mut store, &id, method, args.clone(), 0, 1)
            .unwrap();
        let mut complete = false;
        for _ in 0..2000 {
            let saved = store.load().unwrap();
            let mut vm = saved.snapshot.restore().unwrap();
            vm.collect_garbage().unwrap();
            let before = vm.instructions_executed();
            let receipt = vm
                .invoke_once(&mut store, &id, method, args.clone(), 1, 1)
                .unwrap();
            assert_eq!(receipt.execution, intent.execution);
            assert_eq!(receipt.method, binding);
            assert_eq!(vm.instructions_executed(), before + 1);
            if let InvocationState::Completed {
                result: Some(result),
            } = receipt.state
            {
                let v = view(&vm, &p, result);
                assert_eq!(v.count, count);
                assert_eq!(v.success, key != "absent");
                assert_eq!(v.changed, index != 2 && key != "absent");
                let before = vm.instructions_executed();
                assert_eq!(
                    vm.invoke_once(&mut store, &id, method, args.clone(), 20000, 1)
                        .unwrap(),
                    receipt
                );
                assert_eq!(vm.instructions_executed(), before);
                complete = true;
                break;
            }
        }
        assert!(complete);
    }
    let saved = store.load().unwrap();
    let mut vm = saved.snapshot.restore().unwrap();
    vm.collect_garbage().unwrap();
    assert_ne!(self::saved(&vm, &p), map);
    let remaining = graph(&vm, &p, map);
    assert_eq!(remaining.len(), 2);
    assert!(remaining.iter().any(|n| n.0 == initial[0].0));
    let key = string(&mut vm, "head");
    let result = call(&mut vm, &p, "GetOn", vec![Value::Reference(map), key]).unwrap();
    assert_eq!(view(&vm, &p, result).observed, values[2]);
}
#[test]
fn sdk_free_native_process_resumes_partial_object_map_links_and_replays_one_mutation() {
    let p = program();
    let database = Database::new();
    let mut store = CheckpointStore::open(&database.0).unwrap();
    let mut vm = Vm::new(&p);
    initialize(&mut vm, &p);
    let map = saved(&vm, &p);
    let key = string(&mut vm, "cycle");
    let value = pool(&vm, &p, 1);
    let method = entry(&p, "Set");
    let args = vec![key, value];
    let intent = vm
        .invoke_once(&mut store, "native-object-set", method, args.clone(), 0, 1)
        .unwrap();
    let mut linked = false;
    for _ in 0..500 {
        if !graph(&vm, &p, map).is_empty() && field(&vm, &p, map, "count") == Value::I32(0) {
            linked = true;
            break;
        }
        vm.invoke_once(&mut store, "native-object-set", method, args.clone(), 1, 1)
            .unwrap();
    }
    assert!(linked);
    vm.collect_garbage().unwrap();
    store.save(&vm).unwrap();
    drop(store);
    let invoke = || -> serde_json::Value {
        let output = Command::new(env!("CARGO_BIN_EXE_renraku-vm"))
            .args([
                "invoke-once",
                database.0.to_str().unwrap(),
                "native-object-set",
                &method.to_string(),
                "20000",
                &format!("object:{}", key.referenced_object().unwrap()),
                &format!("object:{}", value.referenced_object().unwrap()),
            ])
            .env("PATH", "/no-object-map-sdk")
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
    assert_eq!(first["result"], replay["result"]);
    let mut store = CheckpointStore::open(&database.0).unwrap();
    let saved = store.load().unwrap();
    let vm = saved.snapshot.restore().unwrap();
    let InvocationState::Completed {
        result: Some(result),
    } = vm.invocation("native-object-set").unwrap().state
    else {
        panic!()
    };
    assert_eq!(view(&vm, &p, result).observed, value);
    assert_eq!(field(&vm, &p, map, "count"), Value::I32(1));
    assert_eq!(static_(&vm, &p, "Operations"), Value::I32(1));
}
