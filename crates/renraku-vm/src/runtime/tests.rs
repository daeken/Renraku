use super::*;
use crate::{
    cil::decode,
    execution::MethodVersion,
    identity::{AssemblyId, CodeVersionId},
    program::{Signature, TypeDefinition},
};

fn program(
    bytes: &[u8],
    parameters: Vec<SlotType>,
    locals: Vec<SlotType>,
    result: Option<SlotType>,
) -> (Program, MethodId) {
    let mut program = Program::default();
    let owner = TypeId::fresh();
    program.types.insert(
        owner,
        TypeDefinition {
            id: owner,
            name: "Test".into(),
            base: None,
            fields: Vec::new(),
            flags: 0,
        },
    );
    let binding = MethodVersion {
        method: MethodId::fresh(),
        code: CodeVersionId::fresh(),
    };
    program.dispatch.insert(binding.method, binding.code);
    program.methods.insert(
        binding.code,
        Method {
            intrinsic: None,
            binding,
            owner,
            name: "Run".into(),
            signature: Signature {
                instance: false,
                parameters,
                result,
            },
            locals,
            initialize_locals: false,
            max_stack: 16,
            instructions: decode(bytes).unwrap(),
            assembly: AssemblyId::fresh(),
        },
    );
    (program, binding.method)
}
fn eval(
    bytes: &[u8],
    arguments: Vec<Value>,
    locals: Vec<SlotType>,
    result: SlotType,
) -> Result<RunOutcome> {
    let parameters = arguments
        .iter()
        .map(|v| match v {
            Value::I32(_) => SlotType::I32,
            Value::I64(_) => SlotType::I64,
            _ => unreachable!(),
        })
        .collect();
    let (program, entry) = program(bytes, parameters, locals, Some(result));
    let mut vm = Vm::new(&program);
    vm.start(entry, arguments)?;
    vm.run(100)
}
fn completed(value: i32) -> RunOutcome {
    RunOutcome::Completed(Some(Value::I32(value)))
}

#[test]
fn division_and_remainder_reject_noninteger_or_mixed_stacks_without_consuming_operands() {
    for opcode in 0x5b..=0x5e {
        for (bytes, parameters, arguments) in [
            (
                vec![0x02, 0x03, opcode, 0x2a],
                vec![SlotType::I32, SlotType::I64],
                vec![Value::I32(10), Value::I64(2)],
            ),
            (
                vec![0x02, 0x03, opcode, 0x2a],
                vec![SlotType::I64, SlotType::I32],
                vec![Value::I64(10), Value::I32(0)],
            ),
            (vec![0x14, 0x14, opcode, 0x2a], vec![], vec![]),
        ] {
            let (program, entry) = program(&bytes, parameters, vec![], Some(SlotType::I32));
            let mut vm = Vm::new(&program);
            vm.start(entry, arguments).unwrap();
            vm.run(2).unwrap();
            let before = vm.context().clone();
            let error = vm.run(1).unwrap_err();
            assert!(error.0.contains("matching integer stack types"));
            assert!(error.0.contains("IL_0002"));
            assert_eq!(vm.context().frames, before.frames);
            assert_eq!(vm.context().id, before.id);
            assert_eq!(vm.instructions_executed(), 2);
        }
    }
}

#[test]
fn bitwise_operands_reject_references_and_mixed_widths_without_consuming_the_stack() {
    for opcode in [0x5f, 0x60, 0x61] {
        for (bytes, parameters, arguments) in [
            (
                vec![2, 3, opcode, 0x2a],
                vec![SlotType::I32, SlotType::I64],
                vec![Value::I32(1), Value::I64(-1)],
            ),
            (
                vec![2, 3, opcode, 0x2a],
                vec![SlotType::I64, SlotType::I32],
                vec![Value::I64(-1), Value::I32(0)],
            ),
            (vec![0x14, 0x14, opcode, 0x2a], vec![], vec![]),
        ] {
            let (program, method) = program(&bytes, parameters, vec![], Some(SlotType::I32));
            let mut vm = Vm::new(&program);
            vm.start(method, arguments).unwrap();
            assert_eq!(vm.run(2).unwrap(), RunOutcome::Paused);
            let before = vm.context().clone();
            let error = vm.run(1).unwrap_err();
            assert!(error.0.contains("matching integer stack types"));
            assert!(error.0.contains("IL_0002"));
            assert_eq!(vm.context().frames, before.frames);
            assert_eq!(vm.instructions_executed(), 2);
        }
    }
    let (program, method) = program(&[0x14, 0x66, 0x2a], vec![], vec![], Some(SlotType::I32));
    let mut vm = Vm::new(&program);
    vm.start(method, vec![]).unwrap();
    vm.run(1).unwrap();
    let before = vm.context().clone();
    assert!(vm.run(1).unwrap_err().0.contains("not requires an integer"));
    assert_eq!(vm.context().frames, before.frames);
    assert_eq!(vm.instructions_executed(), 1);
    for opcode in [0x5f, 0x60, 0x61, 0x66] {
        let (program, method) =
            self::program(&[0, 0, opcode, 0x2a], vec![], vec![], Some(SlotType::I32));
        let mut vm = Vm::new(&program);
        vm.start(method, vec![]).unwrap();
        vm.run(2).unwrap();
        let id = ObjectId::fresh();
        vm.heap
            .allocate(
                id,
                program.method(method).unwrap().owner,
                1,
                crate::heap::AllocationDomain::Managed,
            )
            .unwrap();
        vm.context.frames[0].evaluation_stack = if opcode == 0x66 {
            vec![Value::Reference(id)]
        } else {
            vec![Value::Reference(id), Value::Reference(id)]
        };
        let before = vm.context().clone();
        let error = vm.run(1).unwrap_err();
        assert!(error.0.contains("integer"));
        assert_eq!(vm.context().frames, before.frames);
        assert_eq!(vm.instructions_executed(), 2);
        assert!(vm.heap().object(id).is_ok());
    }
    for opcode in [0xd4, 0xd5, 0x8a, 0x8b] {
        assert!(
            decode(&[opcode])
                .unwrap_err()
                .0
                .contains("unsupported CIL opcode")
        );
    }
}

#[test]
fn shifts_use_an_explicit_raw_count_mask_and_reject_invalid_types_without_consuming_operands() {
    for (opcode, input, count, expected, slot) in [
        (0x62, Value::I32(1), -1, Value::I32(i32::MIN), SlotType::I32),
        (0x62, Value::I32(1), 32, Value::I32(1), SlotType::I32),
        (0x62, Value::I64(1), -1, Value::I64(i64::MIN), SlotType::I64),
        (0x62, Value::I64(1), 64, Value::I64(1), SlotType::I64),
        (
            0x63,
            Value::I32(i32::MIN),
            -1,
            Value::I32(-1),
            SlotType::I32,
        ),
        (0x64, Value::I32(i32::MIN), -1, Value::I32(1), SlotType::I32),
        (
            0x63,
            Value::I64(i64::MIN),
            63,
            Value::I64(-1),
            SlotType::I64,
        ),
        (0x64, Value::I64(i64::MIN), 63, Value::I64(1), SlotType::I64),
        (
            0x63,
            Value::I64(i64::MIN),
            i32::MIN,
            Value::I64(i64::MIN),
            SlotType::I64,
        ),
    ] {
        assert_eq!(
            eval(
                &[2, 3, opcode, 0x2a],
                vec![input, Value::I32(count)],
                vec![],
                slot
            )
            .unwrap(),
            RunOutcome::Completed(Some(expected))
        );
    }
    for opcode in [0x62, 0x63, 0x64] {
        for args in [
            vec![Value::I32(1), Value::I64(1)],
            vec![Value::I64(1), Value::I64(1)],
        ] {
            let parameters = args
                .iter()
                .map(|v| {
                    if matches!(v, Value::I32(_)) {
                        SlotType::I32
                    } else {
                        SlotType::I64
                    }
                })
                .collect();
            let (program, method) = program(
                &[2, 3, opcode, 0x2a],
                parameters,
                vec![],
                Some(SlotType::I32),
            );
            let mut vm = Vm::new(&program);
            vm.start(method, args).unwrap();
            vm.run(2).unwrap();
            let before = vm.context().clone();
            assert!(vm.run(1).unwrap_err().0.contains("shift count must be I32"));
            assert_eq!(vm.context().frames, before.frames);
            assert_eq!(vm.instructions_executed(), 2);
        }
        for operands in [
            vec![Value::Null, Value::I32(1)],
            vec![Value::I32(1), Value::Null],
            vec![Value::Null, Value::Null],
        ] {
            let (program, method) =
                program(&[0, 0, opcode, 0x2a], vec![], vec![], Some(SlotType::I32));
            let mut vm = Vm::new(&program);
            vm.start(method, vec![]).unwrap();
            vm.run(2).unwrap();
            vm.context.frames[0].evaluation_stack = operands;
            let before = vm.context().clone();
            assert!(vm.run(1).unwrap_err().0.contains("shift"));
            assert_eq!(vm.context().frames, before.frames);
            assert_eq!(vm.instructions_executed(), 2);
        }
        for reference_count in [false, true] {
            let (program, method) =
                program(&[0, 0, opcode, 0x2a], vec![], vec![], Some(SlotType::I32));
            let mut vm = Vm::new(&program);
            vm.start(method, vec![]).unwrap();
            vm.run(2).unwrap();
            let id = ObjectId::fresh();
            vm.heap
                .allocate(
                    id,
                    program.method(method).unwrap().owner,
                    1,
                    crate::heap::AllocationDomain::Managed,
                )
                .unwrap();
            vm.context.frames[0].evaluation_stack = if reference_count {
                vec![Value::I64(1), Value::Reference(id)]
            } else {
                vec![Value::Reference(id), Value::I32(1)]
            };
            let before = vm.context().clone();
            assert!(vm.run(1).unwrap_err().0.contains("shift"));
            assert_eq!(vm.context().frames, before.frames);
            assert_eq!(vm.instructions_executed(), 2);
        }
    }
}

#[test]
fn integer_negation_wraps_minima_and_preserves_invalid_reference_operands() {
    for (input, expected, slot) in [
        (Value::I32(i32::MIN), Value::I32(i32::MIN), SlotType::I32),
        (Value::I32(i32::MAX), Value::I32(-i32::MAX), SlotType::I32),
        (Value::I32(-1), Value::I32(1), SlotType::I32),
        (Value::I64(i64::MIN), Value::I64(i64::MIN), SlotType::I64),
        (Value::I64(i64::MAX), Value::I64(-i64::MAX), SlotType::I64),
        (Value::I64(0), Value::I64(0), SlotType::I64),
    ] {
        assert_eq!(
            eval(&[2, 0x65, 0x2a], vec![input], vec![], slot).unwrap(),
            RunOutcome::Completed(Some(expected))
        );
    }
    for object in [false, true] {
        let (program, method) = program(&[0x14, 0x65, 0x2a], vec![], vec![], Some(SlotType::I32));
        let mut vm = Vm::new(&program);
        vm.start(method, vec![]).unwrap();
        vm.run(1).unwrap();
        if object {
            let id = ObjectId::fresh();
            vm.heap
                .allocate(
                    id,
                    program.method(method).unwrap().owner,
                    1,
                    crate::heap::AllocationDomain::Managed,
                )
                .unwrap();
            vm.context.frames[0].evaluation_stack[0] = Value::Reference(id);
        }
        let before = vm.context().clone();
        let error = vm.run(1).unwrap_err();
        assert!(error.0.contains("neg requires an integer"));
        assert!(error.0.contains("IL_0001"));
        assert_eq!(vm.context().frames, before.frames);
        assert_eq!(vm.instructions_executed(), 1);
    }
}

#[test]
fn small_width_conversions_truncate_then_extend_and_reject_reference_operands() {
    for (opcode, input, expected) in [
        (0x67, Value::I32(0xff), -1),
        (0xd2, Value::I32(-1), 255),
        (0x68, Value::I64(0x12345678_ffff8000), -32768),
        (0xd1, Value::I64(-1), 65535),
        (0x67, Value::I64(0x12345678_ffffff80), -128),
        (0xd2, Value::I64(-256), 0),
        (0x68, Value::I32(0x10000), 0),
        (0xd1, Value::I32(0x1d800), 0xd800),
    ] {
        assert_eq!(
            eval(&[2, opcode, 0x2a], vec![input], vec![], SlotType::I32).unwrap(),
            completed(expected)
        );
    }
    for opcode in [0x67, 0x68, 0xd1, 0xd2] {
        for object in [false, true] {
            let (program, method) =
                program(&[0x14, opcode, 0x2a], vec![], vec![], Some(SlotType::I32));
            let mut vm = Vm::new(&program);
            vm.start(method, vec![]).unwrap();
            vm.run(1).unwrap();
            if object {
                let id = ObjectId::fresh();
                vm.heap
                    .allocate(
                        id,
                        program.method(method).unwrap().owner,
                        1,
                        crate::heap::AllocationDomain::Managed,
                    )
                    .unwrap();
                vm.context.frames[0].evaluation_stack[0] = Value::Reference(id);
            }
            let before = vm.context().clone();
            let error = vm.run(1).unwrap_err();
            assert!(error.0.contains("requires an integer"));
            assert!(error.0.contains("IL_0001"));
            assert_eq!(vm.context().frames, before.frames);
            assert_eq!(vm.instructions_executed(), 1);
        }
    }
}

#[test]
fn every_integer_constant_encoding_preserves_its_exact_bits() {
    for (opcode, expected) in (0x15..=0x1e).zip(-1..=8) {
        assert_eq!(
            eval(&[opcode, 0x2a], vec![], vec![], SlotType::I32).unwrap(),
            completed(expected)
        );
    }
    assert_eq!(
        eval(&[0x1f, 0x80, 0x2a], vec![], vec![], SlotType::I32).unwrap(),
        completed(-128)
    );
    assert_eq!(
        eval(&[0x20, 0, 0, 0, 0x80, 0x2a], vec![], vec![], SlotType::I32).unwrap(),
        completed(i32::MIN)
    );
    let mut bytes = vec![0x21];
    bytes.extend_from_slice(&i64::MIN.to_le_bytes());
    bytes.push(0x2a);
    assert_eq!(
        eval(&bytes, vec![], vec![], SlotType::I64).unwrap(),
        RunOutcome::Completed(Some(Value::I64(i64::MIN)))
    );
}

#[test]
fn argument_and_local_short_long_and_specialized_forms_execute() {
    for (index, expected) in [11, 22, 33, 44].into_iter().enumerate() {
        for load in [
            vec![2 + index as u8],
            vec![0x0e, index as u8],
            vec![0xfe, 0x09, index as u8, 0],
        ] {
            let mut bytes = load;
            bytes.push(0x2a);
            assert_eq!(
                eval(
                    &bytes,
                    vec![
                        Value::I32(11),
                        Value::I32(22),
                        Value::I32(33),
                        Value::I32(44)
                    ],
                    vec![],
                    SlotType::I32
                )
                .unwrap(),
                completed(expected)
            );
        }
    }
    for store in [vec![0x10, 0], vec![0xfe, 0x0b, 0, 0]] {
        let mut bytes = vec![0x1e];
        bytes.extend(store);
        bytes.extend([0x02, 0x2a]);
        assert_eq!(
            eval(&bytes, vec![Value::I32(99)], vec![], SlotType::I32).unwrap(),
            completed(8)
        );
    }
    for index in 0..4 {
        assert_eq!(
            eval(
                &[0x1e, 0x0a + index, 0x06 + index, 0x2a],
                vec![],
                vec![SlotType::I32; 4],
                SlotType::I32
            )
            .unwrap(),
            completed(8)
        );
    }
    for (index, store, load) in [
        (7, vec![0x13, 7], vec![0x11, 7]),
        (300, vec![0xfe, 0x0e, 0x2c, 1], vec![0xfe, 0x0c, 0x2c, 1]),
    ] {
        let mut bytes = vec![0x1e];
        bytes.extend(store);
        bytes.extend(load);
        bytes.push(0x2a);
        assert_eq!(
            eval(
                &bytes,
                vec![],
                vec![SlotType::I32; index + 1],
                SlotType::I32
            )
            .unwrap(),
            completed(8)
        );
    }
}

#[test]
fn stack_manipulation_and_wrapping_integer_arithmetic_execute() {
    assert_eq!(
        eval(
            &[0, 0x19, 0x25, 0x58, 0x17, 0x26, 0x2a],
            vec![],
            vec![],
            SlotType::I32
        )
        .unwrap(),
        completed(6)
    );
    for (opcode, a, b, expected) in [
        (0x58, i32::MAX, 1, i32::MIN),
        (0x59, i32::MIN, 1, i32::MAX),
        (0x5a, i32::MAX, 2, -2),
    ] {
        assert_eq!(
            eval(
                &[2, 3, opcode, 0x2a],
                vec![Value::I32(a), Value::I32(b)],
                vec![],
                SlotType::I32
            )
            .unwrap(),
            completed(expected)
        );
    }
    for (opcode, a, b, expected) in [
        (0x58, i64::MAX, 1, i64::MIN),
        (0x59, i64::MIN, 1, i64::MAX),
        (0x5a, i64::MAX, 2, -2),
    ] {
        assert_eq!(
            eval(
                &[2, 3, opcode, 0x2a],
                vec![Value::I64(a), Value::I64(b)],
                vec![],
                SlotType::I64
            )
            .unwrap(),
            RunOutcome::Completed(Some(Value::I64(expected)))
        );
    }
    assert_eq!(
        eval(&[0x15, 0x6a, 0x2a], vec![], vec![], SlotType::I64).unwrap(),
        RunOutcome::Completed(Some(Value::I64(-1)))
    );
    assert_eq!(
        eval(
            &[2, 0x6a, 0x2a],
            vec![Value::I64(i64::MIN)],
            vec![],
            SlotType::I64
        )
        .unwrap(),
        RunOutcome::Completed(Some(Value::I64(i64::MIN)))
    );
}

#[test]
fn fixed_width_conversions_use_only_storage_width_and_opcode_signedness() {
    for (opcode, input, expected, slot) in [
        (
            0x69,
            Value::I64(0x12345678_ffffffff),
            Value::I32(-1),
            SlotType::I32,
        ),
        (
            0x6d,
            Value::I64(0x12345678_80000000),
            Value::I32(i32::MIN),
            SlotType::I32,
        ),
        (
            0x69,
            Value::I32(i32::MIN),
            Value::I32(i32::MIN),
            SlotType::I32,
        ),
        (0x6d, Value::I32(-1), Value::I32(-1), SlotType::I32),
        (0x6a, Value::I32(-1), Value::I64(-1), SlotType::I64),
        (0x6e, Value::I32(-1), Value::I64(4294967295), SlotType::I64),
        (
            0x6e,
            Value::I32(i32::MIN),
            Value::I64(2147483648),
            SlotType::I64,
        ),
        (
            0x6a,
            Value::I64(i64::MIN),
            Value::I64(i64::MIN),
            SlotType::I64,
        ),
        (
            0x6e,
            Value::I64(i64::MIN),
            Value::I64(i64::MIN),
            SlotType::I64,
        ),
    ] {
        assert_eq!(
            eval(&[2, opcode, 0x2a], vec![input], vec![], slot).unwrap(),
            RunOutcome::Completed(Some(expected))
        );
    }
    for opcode in [0x69, 0x6d, 0x6a, 0x6e] {
        let (program, method) = program(&[0x14, opcode, 0x2a], vec![], vec![], Some(SlotType::I64));
        let mut vm = Vm::new(&program);
        vm.start(method, vec![]).unwrap();
        assert_eq!(vm.run(1).unwrap(), RunOutcome::Paused);
        let before = vm.context().frames.clone();
        assert!(vm.run(1).unwrap_err().0.contains("requires an integer"));
        assert_eq!(vm.context().frames, before);
        assert_eq!(vm.instructions_executed(), 1);
    }
}

#[test]
fn every_short_and_long_integer_comparison_branch_executes() {
    let negative = [0, 0, 0, 1, 1, 1, 1, 1, 0, 0];
    let equal = [1, 1, 0, 1, 0, 0, 1, 0, 1, 0];
    for (a, b, expected) in [(-1, 2, negative), (2, 2, equal)] {
        for (base, long) in [(0x2e, false), (0x3b, true)] {
            for (index, expected) in expected.into_iter().enumerate() {
                let mut bytes = vec![2, 3, base + index as u8, 2];
                if long {
                    bytes.extend([0, 0, 0]);
                }
                bytes.extend([0x16, 0x2a, 0x17, 0x2a]);
                for arguments in [
                    vec![Value::I32(a), Value::I32(b)],
                    vec![Value::I64(a as i64), Value::I64(b as i64)],
                ] {
                    assert_eq!(
                        eval(&bytes, arguments, vec![], SlotType::I32).unwrap(),
                        completed(expected),
                        "branch {:02x}",
                        base + index as u8
                    );
                }
            }
        }
    }
}

#[test]
fn comparison_instructions_and_truth_branches_execute() {
    for (opcode, expected) in [(1, 0), (2, 0), (3, 1), (4, 1), (5, 0)] {
        assert_eq!(
            eval(
                &[2, 3, 0xfe, opcode, 0x2a],
                vec![Value::I32(-1), Value::I32(2)],
                vec![],
                SlotType::I32
            )
            .unwrap(),
            completed(expected)
        );
        assert_eq!(
            eval(
                &[2, 3, 0xfe, opcode, 0x2a],
                vec![Value::I64(-1), Value::I64(2)],
                vec![],
                SlotType::I32
            )
            .unwrap(),
            completed(expected)
        );
    }
    for (opcode, long, when_true) in [
        (0x2c, false, false),
        (0x2d, false, true),
        (0x39, true, false),
        (0x3a, true, true),
    ] {
        let mut bytes = vec![2, opcode, 2];
        if long {
            bytes.extend([0, 0, 0]);
        }
        bytes.extend([0x16, 0x2a, 0x17, 0x2a]);
        for value in [0, 1, -1] {
            let expected = i32::from((value != 0) == when_true);
            for arguments in [vec![Value::I32(value)], vec![Value::I64(value as i64)]] {
                assert_eq!(
                    eval(&bytes, arguments, vec![], SlotType::I32).unwrap(),
                    completed(expected)
                );
            }
        }
    }
    for bytes in [
        vec![0x2b, 1, 0, 0x17, 0x2a],
        vec![0x38, 1, 0, 0, 0, 0, 0x17, 0x2a],
    ] {
        assert_eq!(
            eval(&bytes, vec![], vec![], SlotType::I32).unwrap(),
            completed(1)
        );
    }
}

#[test]
fn null_and_reference_comparisons_never_depend_on_host_addresses() {
    let (mut program, entry) = program(&[2, 3, 0xfe, 3, 0x2a], vec![], vec![], Some(SlotType::I32));
    let owner = program.method(entry).unwrap().owner;
    program
        .methods
        .values_mut()
        .next()
        .unwrap()
        .signature
        .parameters = vec![SlotType::Reference(owner); 2];
    let mut vm = Vm::new(&program);
    let id = ObjectId::fresh();
    vm.heap
        .allocate_initialized(id, owner, BTreeMap::new())
        .unwrap();
    for (args, expected) in [
        (vec![Value::Reference(id), Value::Null], 1),
        (vec![Value::Null, Value::Reference(id)], 0),
        (vec![Value::Null, Value::Null], 0),
    ] {
        vm.start(entry, args).unwrap();
        assert_eq!(vm.run(10).unwrap(), completed(expected));
    }
    // Arbitrary ordering of two non-null objects is outside this profile.
    vm.start(entry, vec![Value::Reference(id), Value::Reference(id)])
        .unwrap();
    assert!(vm.run(10).is_err());
}

#[test]
fn stack_faults_leave_the_failing_instruction_unmodified() {
    for (bytes, locals, result, message) in [
        (vec![0x26, 0x2a], vec![], None, "underflow"),
        (
            vec![0x06, 0x2a],
            vec![SlotType::I32],
            Some(SlotType::I32),
            "uninitialized",
        ),
        (
            vec![0x17, 0x18, 0x2a],
            vec![],
            Some(SlotType::I32),
            "extra evaluation",
        ),
        (
            vec![0x17, 0x2a],
            vec![],
            Some(SlotType::I64),
            "declared slot type",
        ),
    ] {
        let (program, entry) = program(&bytes, vec![], locals, result);
        let mut vm = Vm::new(&program);
        vm.start(entry, vec![]).unwrap();
        let mut faulted = false;
        for _ in 0..10 {
            let before = vm.context.frames.clone();
            if let Err(error) = vm.run(1) {
                assert!(error.0.contains(message), "{error}");
                assert_eq!(vm.context.frames, before);
                faulted = true;
                break;
            }
        }
        assert!(faulted);
    }
    let (mut program, entry) = program(
        &[0x17, 0x18, 0x58, 0x2a],
        vec![],
        vec![],
        Some(SlotType::I32),
    );
    program.methods.values_mut().next().unwrap().max_stack = 1;
    let mut vm = Vm::new(&program);
    vm.start(entry, vec![]).unwrap();
    assert_eq!(vm.run(1).unwrap(), RunOutcome::Paused);
    let before = vm.context.frames.clone();
    assert!(vm.run(1).unwrap_err().0.contains("maxstack"));
    assert_eq!(vm.context.frames, before);
}

#[test]
fn switch_consumes_only_i32_and_preserves_invalid_index_frames() {
    // A zero-entry table still consumes its index, leaving the lower stack alone.
    for index in [i32::MIN, -1, 0, 1, i32::MAX] {
        assert_eq!(
            eval(
                &[0x1f, 42, 2, 0x45, 0, 0, 0, 0, 0x2a],
                vec![Value::I32(index)],
                vec![],
                SlotType::I32
            )
            .unwrap(),
            completed(42)
        );
    }
    for invalid in [Value::I64(0), Value::Null] {
        let (program, method) = program(
            &[0, 0x45, 0, 0, 0, 0, 0x16, 0x2a],
            vec![],
            vec![],
            Some(SlotType::I32),
        );
        let mut vm = Vm::new(&program);
        vm.start(method, vec![]).unwrap();
        vm.run(1).unwrap();
        vm.context.frames[0].evaluation_stack = vec![Value::I32(42), invalid];
        let before = vm.context().clone();
        let error = vm.run(1).unwrap_err();
        assert!(error.0.contains("switch requires an I32 index"));
        assert!(error.0.contains("IL_0001"));
        assert_eq!(vm.context().frames, before.frames);
        assert_eq!(vm.instructions_executed(), 1);
    }
    let (program, method) = program(
        &[0, 0x45, 0, 0, 0, 0, 0x16, 0x2a],
        vec![],
        vec![],
        Some(SlotType::I32),
    );
    let mut vm = Vm::new(&program);
    vm.start(method, vec![]).unwrap();
    vm.run(1).unwrap();
    let owner = program.method(method).unwrap().owner;
    let object = ObjectId::fresh();
    vm.heap
        .allocate(object, owner, 1, crate::heap::AllocationDomain::Managed)
        .unwrap();
    vm.context.frames[0].evaluation_stack = vec![Value::Reference(object)];
    let before = vm.context().clone();
    assert!(
        vm.run(1)
            .unwrap_err()
            .0
            .contains("switch requires an I32 index")
    );
    assert_eq!(vm.context().frames, before.frames);
}

#[test]
fn checked_arithmetic_rejects_mixed_widths_null_and_objects_without_consuming_operands() {
    for opcode in 0xd6..=0xdb {
        for (parameters, arguments) in [
            (
                vec![SlotType::I32, SlotType::I64],
                vec![Value::I32(1), Value::I64(0)],
            ),
            (
                vec![SlotType::I64, SlotType::I32],
                vec![Value::I64(-1), Value::I32(1)],
            ),
        ] {
            let (program, method) = program(
                &[2, 3, opcode, 0x2a],
                parameters,
                vec![],
                Some(SlotType::I32),
            );
            let mut vm = Vm::new(&program);
            vm.start(method, arguments).unwrap();
            vm.run(2).unwrap();
            let before = vm.context().clone();
            let error = vm.run(1).unwrap_err();
            assert!(error.0.contains("matching integer stack types"));
            assert!(error.0.contains("IL_0002"));
            assert_eq!(vm.context().frames, before.frames);
            assert_eq!(vm.instructions_executed(), 2);
        }
        for object in [false, true] {
            let (program, method) = program(
                &[0x14, 0x14, opcode, 0x2a],
                vec![],
                vec![],
                Some(SlotType::I32),
            );
            let mut vm = Vm::new(&program);
            vm.start(method, vec![]).unwrap();
            vm.run(2).unwrap();
            if object {
                let id = ObjectId::fresh();
                vm.heap
                    .allocate(
                        id,
                        program.method(method).unwrap().owner,
                        1,
                        crate::heap::AllocationDomain::Managed,
                    )
                    .unwrap();
                vm.context.frames[0].evaluation_stack =
                    vec![Value::Reference(id), Value::Reference(id)];
            }
            let before = vm.context().clone();
            assert!(
                vm.run(1)
                    .unwrap_err()
                    .0
                    .contains("matching integer stack types")
            );
            assert_eq!(vm.context().frames, before.frames);
            assert_eq!(vm.instructions_executed(), 2);
        }
    }
}

#[test]
fn checked_conversion_source_interpretation_is_separate_from_target_and_reference_failures_keep_frames()
 {
    for (opcode, input, expected, result) in [
        (0xb9, Value::I32(-1), Value::I64(-1), SlotType::I64),
        (0x85, Value::I32(-1), Value::I64(4294967295), SlotType::I64),
        (0x89, Value::I32(-1), Value::I64(4294967295), SlotType::I64),
        (0x89, Value::I64(-1), Value::I64(-1), SlotType::I64),
        (0xb7, Value::I32(-1), Value::I32(-1), SlotType::I32),
        (0x88, Value::I32(-1), Value::I32(-1), SlotType::I32),
    ] {
        assert_eq!(
            eval(&[2, opcode, 0x2a], vec![input], vec![], result).unwrap(),
            RunOutcome::Completed(Some(expected))
        );
    }
    for opcode in [0x84, 0x85, 0x88, 0x89, 0xb7, 0xb8, 0xb9, 0xba] {
        for object in [false, true] {
            let (program, method) =
                program(&[0x14, opcode, 0x2a], vec![], vec![], Some(SlotType::I32));
            let mut vm = Vm::new(&program);
            vm.start(method, vec![]).unwrap();
            vm.run(1).unwrap();
            if object {
                let id = ObjectId::fresh();
                vm.heap
                    .allocate(
                        id,
                        program.method(method).unwrap().owner,
                        1,
                        crate::heap::AllocationDomain::Managed,
                    )
                    .unwrap();
                vm.context.frames[0].evaluation_stack[0] = Value::Reference(id);
            }
            let before = vm.context().clone();
            let error = vm.run(1).unwrap_err();
            assert!(
                error.0.contains("checked conversion requires an integer")
                    && error.0.contains("IL_0001")
            );
            assert_eq!(vm.context().frames, before.frames);
            assert_eq!(vm.instructions_executed(), 1);
        }
    }
}

#[test]
fn small_checked_conversions_enforce_ranges_extend_into_i32_and_keep_invalid_frames() {
    for (opcode, input, expected) in [
        (0xb3, Value::I32(-128), -128),
        (0xb4, Value::I32(255), 255),
        (0xb5, Value::I64(-32768), -32768),
        (0xb6, Value::I64(65535), 65535),
        (0x82, Value::I32(127), 127),
        (0x86, Value::I64(255), 255),
        (0x83, Value::I64(32767), 32767),
        (0x87, Value::I32(0xd800), 0xd800),
    ] {
        assert_eq!(
            eval(&[2, opcode, 0x2a], vec![input], vec![], SlotType::I32).unwrap(),
            completed(expected)
        );
    }
    for opcode in [0xb3, 0xb4, 0xb5, 0xb6, 0x82, 0x83, 0x86, 0x87] {
        for object in [false, true] {
            let (program, method) =
                program(&[0x14, opcode, 0x2a], vec![], vec![], Some(SlotType::I32));
            let mut vm = Vm::new(&program);
            vm.start(method, vec![]).unwrap();
            vm.run(1).unwrap();
            if object {
                let id = ObjectId::fresh();
                vm.heap
                    .allocate(
                        id,
                        program.method(method).unwrap().owner,
                        1,
                        crate::heap::AllocationDomain::Managed,
                    )
                    .unwrap();
                vm.context.frames[0].evaluation_stack[0] = Value::Reference(id);
            }
            let before = vm.context().clone();
            let error = vm.run(1).unwrap_err();
            assert!(
                error.0.contains("checked conversion requires an integer")
                    && error.0.contains("IL_0001")
            );
            assert_eq!(vm.context().frames, before.frames);
            assert_eq!(vm.instructions_executed(), 1);
        }
    }
}

#[test]
fn reference_casts_reject_scalars_preserve_identity_and_never_promote_transient_objects() {
    for test in [false, true] {
        for value in [Value::I32(0), Value::I32(-1), Value::I64(0)] {
            let (mut program, method) = program(
                &[0, if test { 0x75 } else { 0x74 }, 1, 0, 0, 2, 0x2a],
                vec![],
                vec![],
                Some(SlotType::I32),
            );
            let owner = program.method(method).unwrap().owner;
            let assembly = program.method(method).unwrap().assembly;
            program.type_tokens.insert((assembly, 0x02000001), owner);
            let mut vm = Vm::new(&program);
            vm.start(method, vec![]).unwrap();
            vm.run(1).unwrap();
            vm.context.frames[0].evaluation_stack = vec![value];
            let before = vm.context().clone();
            assert!(
                vm.run(1)
                    .unwrap_err()
                    .0
                    .contains("reference cast requires an object reference or null")
            );
            assert_eq!(vm.context().frames, before.frames);
            assert_eq!(vm.instructions_executed(), 1);
        }
        for transient in [false, true] {
            let (mut program, method) = program(
                &[0, if test { 0x75 } else { 0x74 }, 1, 0, 0, 2, 0x2a],
                vec![],
                vec![],
                Some(SlotType::I32),
            );
            let owner = program.method(method).unwrap().owner;
            let assembly = program.method(method).unwrap().assembly;
            program.type_tokens.insert((assembly, 0x02000001), owner);
            let mut vm = Vm::new(&program);
            vm.start(method, vec![]).unwrap();
            vm.run(1).unwrap();
            let id = ObjectId::fresh();
            let domain = if transient {
                crate::heap::AllocationDomain::Transient(crate::identity::ArenaId::fresh())
            } else {
                crate::heap::AllocationDomain::Managed
            };
            vm.heap.allocate(id, owner, 1, domain).unwrap();
            vm.context.frames[0].evaluation_stack = vec![Value::Reference(id)];
            let heap = vm.heap().clone();
            assert_eq!(vm.run(1).unwrap(), RunOutcome::Paused);
            assert_eq!(
                vm.context().frames[0].evaluation_stack,
                vec![Value::Reference(id)]
            );
            assert_eq!(vm.heap(), &heap);
            assert_eq!(vm.heap().object(id).unwrap().domain(), domain);
        }
    }
}

#[test]
fn instance_entry_receivers_reject_scalars_null_and_transient_domains_without_changing_context() {
    let (mut p, id) = program(&[0x17, 0x2a], vec![], vec![], Some(SlotType::I32));
    let binding = p.method(id).unwrap().binding;
    p.methods.get_mut(&binding.code).unwrap().signature.instance = true;
    let owner = p.method(id).unwrap().owner;
    let mut vm = Vm::new(&p);
    for bad in [Value::Null, Value::I32(1), Value::I64(1)] {
        assert!(
            vm.check_entry_arguments(p.method(id).unwrap(), &[bad])
                .is_err()
        );
    }
    let receiver = ObjectId::fresh();
    vm.heap
        .allocate(
            receiver,
            owner,
            1,
            crate::heap::AllocationDomain::Transient(crate::identity::ArenaId::fresh()),
        )
        .unwrap();
    let context = vm.context().clone();
    assert!(
        vm.start_instance(crate::invocation::InstanceCall {
            method: binding,
            receiver,
            arguments: vec![]
        })
        .is_err()
    );
    assert_eq!(vm.context(), &context);
    assert_eq!(vm.instructions_executed(), 0);
}
