#[allow(dead_code)]
mod common;
use common::fixtures;
use renraku_vm::{
    checkpoint::CheckpointStore,
    identity::UniverseId,
    invocation::InvocationState,
    program::Program,
    runtime::{RunOutcome, Vm},
    value::Value,
};
use serde::{Deserialize, Serialize};
use std::{fs, path::PathBuf, process::Command};
fn program() -> Program {
    Program::load(&[
        &fixtures().core,
        &fixtures().guest,
        &fs::read(fixtures().output.join("Renraku.OrdinalText.dll")).unwrap(),
    ])
    .unwrap()
}
fn method(program: &Program, name: &str) -> renraku_vm::identity::MethodId {
    program
        .entry(&format!("Renraku.Text.OrdinalText::{name}"))
        .unwrap()
}
fn argument(vm: &mut Vm<'_>, units: &Option<Vec<u16>>) -> Value {
    units.as_ref().map_or(Value::Null, |u| {
        Value::Reference(vm.new_string(u.clone()).unwrap())
    })
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
struct Match {
    success: bool,
    found: bool,
    index: i32,
    error: String,
}
fn matched(vm: &Vm<'_>, program: &Program, value: Value) -> Match {
    let object = vm
        .heap()
        .object(value.referenced_object().unwrap())
        .unwrap();
    assert_eq!(
        object.type_id(),
        program.find_type("Renraku.Text.StringMatchResult").unwrap()
    );
    let field = |name| {
        let f = program
            .fields()
            .find(|f| f.owner == object.type_id() && f.name == name)
            .unwrap();
        object.fields()[&f.id]
    };
    let int = |name| {
        let Value::I32(n) = field(name) else { panic!() };
        n
    };
    Match {
        success: int("Success") == 1,
        found: int("Found") == 1,
        index: int("Index"),
        error: String::from_utf16(
            vm.heap()
                .object(field("Error").referenced_object().unwrap())
                .unwrap()
                .string_units()
                .unwrap(),
        )
        .unwrap(),
    }
}
#[derive(Deserialize)]
struct Case {
    operation: String,
    text: Option<Vec<u16>>,
    needle: Option<Vec<u16>>,
    start: i32,
    success: bool,
    found: bool,
    index: i32,
    error: String,
}
struct Database(PathBuf);
impl Database {
    fn new() -> Self {
        Self(
            fixtures()
                .output
                .join(format!("ordinal-{}.db", UniverseId::fresh())),
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

#[test]
fn managed_ordinal_search_matches_coreclr_over_exact_units_and_has_bounded_worst_case_work() {
    let program = program();
    let cases: Vec<Case> = serde_json::from_slice(
        &fs::read(fixtures().output.join("ordinal-text-reference.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(cases.len(), 2913);
    let mut maximum = 0;
    for case in cases {
        let mut vm = Vm::new(&program);
        let text = argument(&mut vm, &case.text);
        let needle = argument(&mut vm, &case.needle);
        let mut args = vec![text, needle];
        if case.operation == "IndexOf" {
            args.push(Value::I32(case.start));
        }
        vm.start(method(&program, &case.operation), args).unwrap();
        let RunOutcome::Completed(Some(value)) = vm.run(150000).unwrap() else {
            panic!("ordinal work exceeded its bound")
        };
        maximum = maximum.max(vm.instructions_executed());
        assert_eq!(
            matched(&vm, &program, value),
            Match {
                success: case.success,
                found: case.found,
                index: case.index,
                error: case.error
            },
            "{} {:?} {:?} {}",
            case.operation,
            case.text,
            case.needle,
            case.start
        );
        for (value, units) in [(text, &case.text), (needle, &case.needle)] {
            if let Some(id) = value.referenced_object() {
                assert_eq!(
                    vm.heap().object(id).unwrap().string_units(),
                    units.as_deref()
                );
            }
        }
        assert_eq!(vm.effects().count(), 0);
    }
    assert!(maximum > 100000 && maximum < 150000);
    fs::write(
        fixtures().output.join("ordinal-work-bound.json"),
        serde_json::to_vec(&serde_json::json!({"maximumInstructions":maximum})).unwrap(),
    )
    .unwrap();
}

#[test]
fn ordinal_loops_and_result_constructors_restore_every_instruction_with_gc_and_shared_roots() {
    let program = program();
    for (operation, text, needle, start, expected) in [
        (
            "IndexOf",
            "ababababac".encode_utf16().collect::<Vec<_>>(),
            "ababac".encode_utf16().collect(),
            0,
            Match {
                success: true,
                found: true,
                index: 4,
                error: "none".into(),
            },
        ),
        (
            "StartsWith",
            vec![0xd800, 0, 120],
            vec![0xd800, 0],
            0,
            Match {
                success: true,
                found: true,
                index: 0,
                error: "none".into(),
            },
        ),
        (
            "EndsWith",
            vec![120, 0xdc00],
            vec![0xdc00],
            0,
            Match {
                success: true,
                found: true,
                index: 1,
                error: "none".into(),
            },
        ),
        (
            "IndexOf",
            vec![97, 98, 99],
            vec![],
            3,
            Match {
                success: true,
                found: true,
                index: 3,
                error: "none".into(),
            },
        ),
        (
            "IndexOf",
            vec![97, 98, 99],
            vec![122],
            0,
            Match {
                success: true,
                found: false,
                index: -1,
                error: "none".into(),
            },
        ),
        (
            "IndexOf",
            vec![97, 98, 99],
            vec![97],
            -1,
            Match {
                success: false,
                found: false,
                index: -1,
                error: "start_out_of_range".into(),
            },
        ),
    ] {
        let database = Database::new();
        let mut store = CheckpointStore::open(&database.0).unwrap();
        let mut vm = Vm::new(&program);
        let text_id = vm.new_string(text.clone()).unwrap();
        let needle_id = vm.new_string(needle.clone()).unwrap();
        let mut args = vec![Value::Reference(text_id), Value::Reference(needle_id)];
        if operation == "IndexOf" {
            args.push(Value::I32(start));
        }
        let method = method(&program, operation);
        let binding = program.method(method).unwrap().binding;
        vm.start(method, args).unwrap();
        let universe = vm.universe_id();
        let execution = vm.context().id;
        store.save(&vm).unwrap();
        let mut finished = false;
        let mut saw_nested = false;
        let mut saw_ctor = false;
        for _ in 0..5000 {
            let saved = store.load().unwrap();
            let mut vm = saved.snapshot.restore().unwrap();
            let before = vm.context().clone();
            vm.collect_garbage().unwrap();
            assert_eq!(vm.context(), &before);
            assert_eq!(vm.universe_id(), universe);
            assert_eq!(vm.context().id, execution);
            assert_eq!(
                saved.snapshot.program.method(method).unwrap().binding,
                binding
            );
            assert_eq!(
                vm.heap().object(text_id).unwrap().string_units(),
                Some(text.as_slice())
            );
            assert_eq!(
                vm.heap().object(needle_id).unwrap().string_units(),
                Some(needle.as_slice())
            );
            saw_nested |= vm.context().frames.len() > 1;
            saw_ctor |=
                vm.context().frames.iter().any(|f| {
                    saved.snapshot.program.method(f.method.method).unwrap().name == ".ctor"
                });
            let outcome = vm.run(1).unwrap();
            store.save(&vm).unwrap();
            assert_eq!(
                store.load().unwrap().snapshot.restore().unwrap().context(),
                vm.context()
            );
            if let RunOutcome::Completed(Some(value)) = outcome {
                assert_eq!(matched(&vm, &saved.snapshot.program, value), expected);
                finished = true;
                break;
            }
        }
        assert!(finished && saw_nested && saw_ctor);
    }
    // Equal text/needle references are ordinary shared arguments, not copied data.
    let mut vm = Vm::new(&program);
    let id = vm.new_string(vec![0xd800, 0, 0xdc00]).unwrap();
    vm.start(
        method(&program, "IndexOf"),
        vec![Value::Reference(id), Value::Reference(id), Value::I32(0)],
    )
    .unwrap();
    vm.collect_garbage().unwrap();
    let RunOutcome::Completed(Some(value)) = vm.run(1000).unwrap() else {
        panic!()
    };
    assert_eq!(matched(&vm, &program, value).index, 0);
}

#[test]
fn sdk_free_native_ordinal_resumes_keep_exact_inputs_and_original_receipts() {
    let program = program();
    for (operation, needle, index) in [
        ("IndexOf", vec![0, 0xd800], 5),
        ("StartsWith", vec![97, 98, 97], 0),
        ("EndsWith", vec![0xd800, 120], 6),
    ] {
        let database = Database::new();
        let mut store = CheckpointStore::open(&database.0).unwrap();
        let mut vm = Vm::new(&program);
        let text = vm
            .new_string(vec![97, 98, 97, 98, 97, 0, 0xd800, 120])
            .unwrap();
        let needle = vm.new_string(needle).unwrap();
        let method = method(&program, operation);
        let mut args = vec![Value::Reference(text), Value::Reference(needle)];
        if operation == "IndexOf" {
            args.push(Value::I32(0));
        }
        let pending = vm
            .invoke_once(&mut store, "ordinal-native", method, args, 17, 1)
            .unwrap();
        assert_eq!(pending.state, InvocationState::Running);
        vm.collect_garbage().unwrap();
        store.save(&vm).unwrap();
        drop(store);
        let invoke = || -> JsonResult {
            let mut command = Command::new(env!("CARGO_BIN_EXE_renraku-vm"));
            command.args([
                "invoke-once",
                database.0.to_str().unwrap(),
                "ordinal-native",
                &method.to_string(),
                "10000",
                &format!("object:{text}"),
                &format!("object:{needle}"),
            ]);
            if operation == "IndexOf" {
                command.arg("i32:0");
            }
            let output = command
                .env("PATH", "/no-ordinal-sdk-tools")
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
        let InvocationState::Completed {
            result: Some(value),
        } = vm.invocation("ordinal-native").unwrap().state
        else {
            panic!()
        };
        assert_eq!(
            matched(&vm, &saved.snapshot.program, value),
            Match {
                success: true,
                found: true,
                index,
                error: "none".into()
            }
        );
        assert_eq!(
            vm.heap().object(text).unwrap().string_units(),
            Some([97, 98, 97, 98, 97, 0, 0xd800, 120].as_slice())
        );
    }
}
type JsonResult = serde_json::Value;
