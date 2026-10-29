#[allow(dead_code)]
mod common;
use common::fixtures;
use renraku_vm::{
    checkpoint::CheckpointStore,
    cil::Op,
    identity::UniverseId,
    program::Program,
    runtime::{RunOutcome, Vm},
    value::Value,
};
use serde::Deserialize;
use std::fs;

fn program() -> Program {
    Program::load(&[
        &fixtures().core,
        &fs::read(fixtures().output.join("Renraku.ConversionTests.dll")).unwrap(),
    ])
    .unwrap()
}
fn bits(width: u32, text: &str) -> Value {
    if width == 32 {
        Value::I32(text.parse().unwrap())
    } else {
        Value::I64(text.parse().unwrap())
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

#[test]
fn actual_emitted_integer_conversions_match_coreclr_for_full_width_bits() {
    let program = program();
    let cases: Vec<Case> = serde_json::from_slice(
        &fs::read(fixtures().output.join("conversion-reference.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(cases.len(), 1764);
    let mut saw_narrow = false;
    let mut saw_sign = false;
    let mut saw_zero = false;
    for case in cases {
        let method = program
            .entry(&format!(
                "Renraku.Guest.IntegerConversions::{}",
                case.method
            ))
            .unwrap();
        for instruction in program.method(method).unwrap().instructions.values() {
            match instruction.op {
                Op::ConvertI32 => saw_narrow = true,
                Op::ConvertI64 => saw_sign = true,
                Op::ConvertU64 => saw_zero = true,
                _ => {}
            }
        }
        let mut vm = Vm::new(&program);
        vm.start(method, vec![bits(case.width, &case.input)])
            .unwrap();
        assert_eq!(
            vm.run(20).unwrap(),
            RunOutcome::Completed(Some(bits(case.result_width, &case.value))),
            "{}({})",
            case.method,
            case.input
        );
    }
    assert!(saw_narrow && saw_sign && saw_zero);
}

#[test]
fn conversion_stacks_roundtrip_each_instruction_with_original_bindings() {
    let program = program();
    for (name, input, expected) in [
        (
            "I64ToI32",
            Value::I64(i64::MIN + 2147483648),
            Value::I32(i32::MIN),
        ),
        ("I64ToU32", Value::I64(-1), Value::I32(-1)),
        ("I32ToI64", Value::I32(-1), Value::I64(-1)),
        ("U32ToU64", Value::I32(-1), Value::I64(4294967295)),
        ("TruncateThenSign", Value::I64(4294967295), Value::I64(-1)),
        ("TruncateThenZero", Value::I64(-1), Value::I64(4294967295)),
    ] {
        let path = fixtures()
            .output
            .join(format!("conversion-{}.db", UniverseId::fresh()));
        let method = program
            .entry(&format!("Renraku.Guest.IntegerConversions::{name}"))
            .unwrap();
        let binding = program.method(method).unwrap().binding;
        let mut vm = Vm::new(&program);
        vm.start(method, vec![input]).unwrap();
        let universe = vm.universe_id();
        let execution = vm.context().id;
        let mut store = CheckpointStore::open(&path).unwrap();
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
            assert_eq!(vm.instructions_executed() as usize, step + 1);
            if step + 1 == steps {
                assert_eq!(outcome, RunOutcome::Completed(Some(expected)));
            } else {
                assert_eq!(outcome, RunOutcome::Paused);
            }
            store.save(&vm).unwrap();
            assert_eq!(
                store.load().unwrap().snapshot.restore().unwrap().context(),
                vm.context()
            );
        }
        drop(store);
        for suffix in ["", "-wal", "-shm"] {
            let _ = fs::remove_file(format!("{}{suffix}", path.display()));
        }
    }
}
