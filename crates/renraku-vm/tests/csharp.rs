mod common;
use common::{entry, fixtures, program};
use renraku_vm::{
    execution::{CallResult, Continuation, ExecutionStatus},
    program::Program,
    runtime::{RunOutcome, Vm},
    value::Value,
};

#[test]
fn real_csharp_matches_coreclr_for_objects_branches_arithmetic_and_calls() {
    let program = program();
    for (name, expected) in &fixtures().reference {
        // The effects suite drives and compares the model harness through its outbox.
        if name == "ModelHarness" || name == "ChatHarnessScore" {
            continue;
        }
        let mut vm = Vm::new(&program);
        vm.start(entry(&program, name), Vec::new()).unwrap();
        let result = vm.run(100_000).unwrap();
        let actual = match result {
            RunOutcome::Completed(Some(Value::I32(value))) => value as i64,
            RunOutcome::Completed(Some(Value::I64(value))) => value,
            _ => panic!("unexpected result for {name}: {result:?}"),
        };
        assert_eq!(actual, *expected, "CoreCLR mismatch in {name}");
        assert!(vm.context().frames.is_empty());
    }
}

#[test]
fn each_instruction_can_pause_and_resume_with_nested_frames_and_constructor_state() {
    let program = program();
    let mut vm = Vm::new(&program);
    vm.start(entry(&program, "Run"), Vec::new()).unwrap();
    let mut saw_constructor = false;
    let mut saw_nested_calls = false;
    let id = vm.context().id;
    for _ in 0..1_000 {
        match vm.run(1).unwrap() {
            RunOutcome::Completed(result) => {
                assert_eq!(result, Some(Value::I32(48)));
                assert!(saw_constructor && saw_nested_calls);
                assert_eq!(vm.context().id, id);
                return;
            }
            RunOutcome::Paused => {}
            RunOutcome::Waiting(_) => panic!("pure simulation unexpectedly waited for an effect"),
        }
        assert_eq!(vm.context().status, ExecutionStatus::Paused);
        saw_nested_calls |= vm.context().frames.len() >= 3;
        for pair in vm.context().frames.windows(2) {
            assert!(matches!(
                pair[0].continuation,
                Continuation::AwaitReturn { .. }
            ));
        }
        let retained = vm.retained_objects().unwrap();
        for frame in &vm.context().frames {
            if let Continuation::AwaitReturn {
                result: CallResult::ConstructedObject(id),
                ..
            } = frame.continuation
            {
                saw_constructor = true;
                assert!(retained.contains(&id));
                assert_eq!(vm.heap().object(id).unwrap().id(), id);
            }
        }
    }
    panic!("single stepping did not terminate");
}

#[test]
fn budgets_bound_infinite_loops_and_preserve_execution_identity() {
    let program = program();
    let mut vm = Vm::new(&program);
    vm.start(entry(&program, "Forever"), Vec::new()).unwrap();
    let id = vm.context().id;
    assert_eq!(vm.run(100).unwrap(), RunOutcome::Paused);
    assert_eq!(vm.run(100).unwrap(), RunOutcome::Paused);
    assert_eq!(vm.context().id, id);
    assert_eq!(vm.instructions_executed(), 200);
    assert!(!vm.context().frames.is_empty());
}

#[test]
fn null_receiver_fault_retains_the_instruction_and_its_operands() {
    let program = program();
    let mut vm = Vm::new(&program);
    vm.start(entry(&program, "NullReceiver"), Vec::new())
        .unwrap();
    for _ in 0..100 {
        let before = vm.context().frames.clone();
        if let Err(error) = vm.run(1) {
            assert!(error.to_string().contains("null receiver"));
            assert_eq!(vm.context().frames, before);
            assert!(matches!(
                vm.context().status,
                ExecutionStatus::Faulted { .. }
            ));
            return;
        }
    }
    panic!("null receiver did not fault within the instruction budget");
}

#[test]
fn static_storage_survives_successive_executions_in_one_universe() {
    let program = program();
    let mut vm = Vm::new(&program);
    for expected in [4, 8, 12] {
        vm.start(entry(&program, "StaticRoundTrip"), Vec::new())
            .unwrap();
        assert_eq!(
            vm.run(100).unwrap(),
            RunOutcome::Completed(Some(Value::I32(expected)))
        );
    }
    vm.start(entry(&program, "Reset"), Vec::new()).unwrap();
    assert_eq!(vm.run(100).unwrap(), RunOutcome::Completed(None));
    vm.start(entry(&program, "StaticRoundTrip"), Vec::new())
        .unwrap();
    assert_eq!(
        vm.run(100).unwrap(),
        RunOutcome::Completed(Some(Value::I32(4)))
    );
}

#[test]
fn static_fields_and_completed_results_retain_objects_after_frames_return() {
    let program = program();
    let mut vm = Vm::new(&program);
    vm.start(entry(&program, "SaveObject"), Vec::new()).unwrap();
    assert_eq!(
        vm.run(100).unwrap(),
        RunOutcome::Completed(Some(Value::I32(19)))
    );
    assert_eq!(vm.retained_objects().unwrap().len(), 1);
    vm.start(entry(&program, "ReturnObject"), Vec::new())
        .unwrap();
    let RunOutcome::Completed(Some(Value::Reference(id))) = vm.run(100).unwrap() else {
        panic!("missing returned object")
    };
    let retained = vm.retained_objects().unwrap();
    assert!(retained.contains(&id));
    assert_eq!(retained.len(), 2);
}

#[test]
fn recursion_uses_managed_frames_and_obeys_a_frame_limit() {
    let program = program();
    let mut vm = Vm::new(&program);
    vm.set_frame_limit(5).unwrap();
    vm.start(entry(&program, "Recursion"), Vec::new()).unwrap();
    let error = vm.run(1_000).unwrap_err();
    assert!(error.to_string().contains("frame limit exceeded"));
    assert_eq!(vm.context().frames.len(), 5);
}

#[test]
fn unsupported_csharp_capabilities_are_rejected_during_loading() {
    let fixture = fixtures();
    for (name, expected) in [
        ("Bitwise", "unsupported CIL opcode 61"),
        ("FloatArithmetic", "unsupported signature element 0d"),
        // Roslyn also embeds native-int annotation constructors with array
        // slots; either unsupported signature can be encountered first.
        ("NativeArithmetic", "unsupported signature element"),
        ("TypeInitializer", "type initializers"),
        ("ByRef", "unsupported signature element 10"),
        ("Generic", "unsupported metadata table 42"),
        ("UnknownInternalCall", "unrecognized InternalCall method"),
    ] {
        let bytes = std::fs::read(fixture.output.join(format!("{name}.dll"))).unwrap();
        let error = Program::load(&[&fixture.core, &bytes]).unwrap_err();
        assert!(error.to_string().contains(expected), "{name}: {error}");
    }
}

#[test]
fn malformed_and_truncated_assemblies_fail_without_panicking() {
    let fixture = fixtures();
    for length in 0..128 {
        assert!(Program::load(&[&fixture.core[..length]]).is_err());
    }
    for index in (0..fixture.guest.len()).step_by(31) {
        let mut bytes = fixture.guest.clone();
        bytes[index] ^= 0xff;
        let result = std::panic::catch_unwind(|| Program::load(&[&fixture.core, &bytes]));
        assert!(
            result.is_ok(),
            "parser panicked after corrupting byte {index}"
        );
    }
}
