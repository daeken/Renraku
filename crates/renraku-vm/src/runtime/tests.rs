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
