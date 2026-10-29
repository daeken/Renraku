#[allow(dead_code)]
mod common;
use common::fixtures;
use renraku_vm::{
    checkpoint::CheckpointStore,
    execution::ExecutionStatus,
    identity::UniverseId,
    program::Program,
    runtime::{RunOutcome, Vm},
    value::Value,
};
use serde::Deserialize;
use std::{fs, path::PathBuf, process::Command};

fn program() -> Program {
    Program::load(&[
        &fixtures().core,
        &fs::read(fixtures().output.join("Renraku.Library.dll")).unwrap(),
        &fixtures().guest,
    ])
    .unwrap()
}
fn entry(program: &Program, name: &str) -> renraku_vm::identity::MethodId {
    program
        .entry(&format!("Renraku.Text.IntegerText::{name}"))
        .unwrap()
}
fn string(vm: &Vm<'_>, value: Value) -> String {
    String::from_utf16(
        vm.heap()
            .object(value.referenced_object().unwrap())
            .unwrap()
            .string_units()
            .unwrap(),
    )
    .unwrap()
}
fn result(outcome: RunOutcome) -> Value {
    let RunOutcome::Completed(Some(value)) = outcome else {
        panic!("expected completed managed result: {outcome:?}")
    };
    value
}
#[derive(Debug, Eq, PartialEq)]
struct Parsed {
    success: bool,
    value: i64,
    error: String,
    position: i32,
}
fn parsed(vm: &Vm<'_>, program: &Program, value: Value) -> Parsed {
    let object = vm
        .heap()
        .object(value.referenced_object().unwrap())
        .unwrap();
    let field = |name| {
        let field = program
            .fields()
            .find(|f| f.owner == object.type_id() && f.name == name)
            .unwrap();
        object.fields()[&field.id]
    };
    let Value::I32(success) = field("Success") else {
        panic!()
    };
    let value = match field("Value") {
        Value::I32(n) => i64::from(n),
        Value::I64(n) => n,
        _ => panic!(),
    };
    let Value::I32(position) = field("Position") else {
        panic!()
    };
    Parsed {
        success: success == 1,
        value,
        error: string(vm, field("Error")),
        position,
    }
}
#[derive(Deserialize)]
struct Format {
    width: u32,
    value: String,
    text: String,
}
#[derive(Deserialize)]
struct Parse {
    width: u32,
    units: Option<Vec<u16>>,
    success: bool,
    value: String,
    error: String,
    position: i32,
}
#[derive(Deserialize)]
struct Reference {
    formats: Vec<Format>,
    parses: Vec<Parse>,
}

#[test]
fn managed_decimal_library_matches_coreclr_and_independent_big_integer_prefix_oracle() {
    let program = program();
    let data: Reference = serde_json::from_slice(
        &fs::read(fixtures().output.join("integer-text-reference.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(data.formats.len(), 286);
    assert_eq!(data.parses.len(), 1188);
    for case in data.formats {
        let argument = if case.width == 32 {
            Value::I32(case.value.parse().unwrap())
        } else {
            Value::I64(case.value.parse().unwrap())
        };
        let mut vm = Vm::new(&program);
        vm.start(
            entry(&program, &format!("FormatInt{}", case.width)),
            vec![argument],
        )
        .unwrap();
        let returned = result(vm.run(5000).unwrap());
        assert_eq!(string(&vm, returned), case.text);
        assert!(vm.instructions_executed() < 5000);
        assert_eq!(vm.effects().count(), 0);
    }
    for case in data.parses {
        let mut vm = Vm::new(&program);
        let argument = case.units.clone().map_or(Value::Null, |units| {
            Value::Reference(vm.new_string(units).unwrap())
        });
        vm.start(
            entry(&program, &format!("ParseInt{}", case.width)),
            vec![argument],
        )
        .unwrap();
        let returned = result(vm.run(5000).unwrap());
        let actual = parsed(&vm, &program, returned);
        assert_eq!(
            actual,
            Parsed {
                success: case.success,
                value: case.value.parse().unwrap(),
                error: case.error,
                position: case.position
            },
            "width={} units={:?}",
            case.width,
            case.units
        );
        if let Some(id) = argument.referenced_object() {
            assert_eq!(
                vm.heap().object(id).unwrap().string_units(),
                case.units.as_deref()
            );
        }
        assert_eq!(vm.effects().count(), 0);
        if case.units.as_ref().is_some_and(|units| units.len() > 64) {
            assert!(vm.instructions_executed() < 100);
        }
    }
}

struct Database(PathBuf);
impl Database {
    fn new() -> Self {
        Self(
            fixtures()
                .output
                .join(format!("integer-text-{}.db", UniverseId::fresh())),
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
fn format_and_parse_loops_resume_every_instruction_with_gc_and_exact_portable_state() {
    let program = program();
    for (method, argument, expected_text, expected_parse) in [
        ("FormatInt64", None, Some("-9223372036854775808"), None),
        (
            "ParseInt32",
            Some("-2147483648".encode_utf16().collect()),
            None,
            Some(Parsed {
                success: true,
                value: -2147483648,
                error: "none".into(),
                position: 11,
            }),
        ),
        (
            "ParseInt64",
            Some("+0009223372036854775807".encode_utf16().collect()),
            None,
            Some(Parsed {
                success: true,
                value: i64::MAX,
                error: "none".into(),
                position: 23,
            }),
        ),
        (
            "ParseInt64",
            Some("9223372036854775808".encode_utf16().collect()),
            None,
            Some(Parsed {
                success: false,
                value: 0,
                error: "overflow".into(),
                position: 18,
            }),
        ),
        (
            "ParseInt64",
            Some(vec![48; 64]),
            None,
            Some(Parsed {
                success: true,
                value: 0,
                error: "none".into(),
                position: 64,
            }),
        ),
        (
            "ParseInt32",
            Some(vec![49, 0xd800]),
            None,
            Some(Parsed {
                success: false,
                value: 0,
                error: "invalid_digit".into(),
                position: 1,
            }),
        ),
    ] {
        let database = Database::new();
        let mut store = CheckpointStore::open(&database.0).unwrap();
        let mut vm = Vm::new(&program);
        let arg = argument.clone().map_or(Value::I64(i64::MIN), |units| {
            Value::Reference(vm.new_string(units).unwrap())
        });
        let method = entry(&program, method);
        let binding = program.method(method).unwrap().binding;
        vm.start(method, vec![arg]).unwrap();
        let universe = vm.universe_id();
        let execution = vm.context().id;
        store.save(&vm).unwrap();
        let mut terminal = false;
        let mut saw_nested = false;
        for instruction in 1..=5000 {
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
            if let Some(id) = arg.referenced_object() {
                assert_eq!(
                    vm.heap().object(id).unwrap().string_units(),
                    argument.as_deref()
                );
            }
            saw_nested |= vm.context().frames.len() > 1;
            let outcome = vm.run(1).unwrap();
            assert_eq!(vm.instructions_executed(), instruction);
            store.save(&vm).unwrap();
            let roundtrip = store.load().unwrap();
            assert_eq!(
                roundtrip.snapshot.restore().unwrap().context(),
                vm.context()
            );
            if let RunOutcome::Completed(Some(value)) = outcome {
                if let Some(text) = expected_text {
                    assert_eq!(string(&vm, value), text);
                }
                if let Some(expected) = &expected_parse {
                    assert_eq!(&parsed(&vm, &program, value), expected);
                }
                let id = value.referenced_object().unwrap();
                let mut restored = roundtrip.snapshot.restore().unwrap();
                restored.collect_garbage().unwrap();
                assert!(restored.heap().object(id).is_ok());
                assert!(saw_nested);
                terminal = true;
                break;
            }
        }
        assert!(terminal);
    }
}

#[test]
fn native_process_finishes_saved_managed_formatter_and_parser_without_sdk() {
    let program = program();
    for name in ["FormatInt32", "ParseInt64"] {
        let database = Database::new();
        let mut store = CheckpointStore::open(&database.0).unwrap();
        let mut vm = Vm::new(&program);
        let argument = if name == "FormatInt32" {
            Value::I32(i32::MIN)
        } else {
            Value::Reference(
                vm.new_string("-9223372036854775808".encode_utf16().collect())
                    .unwrap(),
            )
        };
        vm.start(entry(&program, name), vec![argument]).unwrap();
        assert_eq!(vm.run(60).unwrap(), RunOutcome::Paused);
        let execution = vm.context().id;
        store.save(&vm).unwrap();
        drop(store);
        let output = Command::new(env!("CARGO_BIN_EXE_renraku-vm"))
            .arg("resume")
            .arg(&database.0)
            .args(["5000", "100"])
            .env("PATH", "/no-integer-text-tools")
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
        assert_eq!(vm.context().id, execution);
        let ExecutionStatus::Completed {
            result: Some(value),
        } = vm.context().status
        else {
            panic!()
        };
        if name == "FormatInt32" {
            assert_eq!(string(&vm, value), "-2147483648");
        } else {
            assert_eq!(
                parsed(&vm, &saved.snapshot.program, value),
                Parsed {
                    success: true,
                    value: i64::MIN,
                    error: "none".into(),
                    position: 20
                }
            );
        }
    }
}
