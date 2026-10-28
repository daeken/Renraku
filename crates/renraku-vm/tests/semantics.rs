use std::collections::BTreeSet;

use renraku_vm::{
    execution::{
        CallResult, Continuation, ExecutionContext, ExecutionStatus, Frame, MethodVersion,
    },
    heap::{AllocationDomain, Heap, HeapError},
    identity::{ArenaId, CodeVersionId, ExecutionId, FieldId, MethodId, ObjectId, TypeId},
    value::{Value, ValueEncodingError},
};

fn object(number: u128) -> ObjectId {
    ObjectId::from_bytes(number.to_be_bytes())
}

fn field(number: u128) -> FieldId {
    FieldId::from_bytes(number.to_be_bytes())
}

fn allocate(heap: &mut Heap, number: u128, domain: AllocationDomain) -> ObjectId {
    let id = object(number);
    heap.allocate(id, TypeId::from_bytes([1; 16]), 1, domain)
        .unwrap();
    id
}

fn frame(continuation: Continuation) -> Frame {
    Frame {
        method: MethodVersion {
            method: MethodId::from_bytes([2; 16]),
            code: CodeVersionId::from_bytes([3; 16]),
        },
        continuation,
        arguments: Vec::new(),
        locals: Vec::new(),
        evaluation_stack: Vec::new(),
    }
}

#[test]
fn roots_retain_shared_and_cyclic_graphs_but_not_unreachable_objects() {
    let mut heap = Heap::default();
    let a = allocate(&mut heap, 1, AllocationDomain::Managed);
    let b = allocate(&mut heap, 2, AllocationDomain::Managed);
    let shared = allocate(&mut heap, 3, AllocationDomain::Managed);
    allocate(&mut heap, 4, AllocationDomain::Managed);
    heap.set_field(a, field(1), Value::Reference(b)).unwrap();
    heap.set_field(b, field(1), Value::Reference(a)).unwrap();
    heap.set_field(a, field(2), Value::Reference(shared))
        .unwrap();
    heap.set_field(b, field(2), Value::Reference(shared))
        .unwrap();
    heap.set_root("world", a).unwrap();

    assert_eq!(
        heap.retained_objects([]).unwrap(),
        BTreeSet::from([a, b, shared])
    );
    assert_eq!(
        heap.object(b).unwrap().fields()[&field(2)],
        Value::Reference(shared)
    );
    heap.remove_root("world");
    assert!(heap.retained_objects([]).unwrap().is_empty());
}

#[test]
fn duplicate_identity_cannot_replace_an_existing_object() {
    let mut heap = Heap::default();
    let id = allocate(&mut heap, 1, AllocationDomain::Managed);
    heap.set_field(id, field(1), Value::I32(42)).unwrap();
    let original = heap.object(id).unwrap().clone();
    assert_eq!(
        heap.allocate(
            id,
            TypeId::from_bytes([9; 16]),
            2,
            AllocationDomain::Managed
        ),
        Err(HeapError::DuplicateObject(id)),
    );
    assert_eq!(heap.object(id).unwrap(), &original);
}

#[test]
fn rejected_transient_and_dangling_stores_preserve_the_existing_field() {
    let mut heap = Heap::default();
    let owner = allocate(&mut heap, 1, AllocationDomain::Managed);
    let transient = allocate(
        &mut heap,
        2,
        AllocationDomain::Transient(ArenaId::from_bytes([1; 16])),
    );
    heap.set_field(owner, field(1), Value::I32(7)).unwrap();

    assert_eq!(
        heap.set_field(owner, field(1), Value::Reference(transient)),
        Err(HeapError::InvalidLifetime {
            owner,
            target: transient
        })
    );
    assert_eq!(
        heap.set_field(owner, field(1), Value::Reference(object(99))),
        Err(HeapError::UnknownObject(object(99)))
    );
    assert_eq!(
        heap.object(owner).unwrap().fields()[&field(1)],
        Value::I32(7)
    );
}

#[test]
fn transient_objects_can_refer_to_managed_objects_and_their_own_arena() {
    let mut heap = Heap::default();
    let managed = allocate(&mut heap, 1, AllocationDomain::Managed);
    let first_arena = AllocationDomain::Transient(ArenaId::from_bytes([1; 16]));
    let owner = allocate(&mut heap, 2, first_arena);
    let same_arena = allocate(&mut heap, 3, first_arena);
    let other_arena = allocate(
        &mut heap,
        4,
        AllocationDomain::Transient(ArenaId::from_bytes([2; 16])),
    );

    heap.set_field(owner, field(1), Value::Reference(managed))
        .unwrap();
    heap.set_field(owner, field(2), Value::Reference(same_arena))
        .unwrap();
    assert_eq!(
        heap.set_field(owner, field(2), Value::Reference(other_arena)),
        Err(HeapError::InvalidLifetime {
            owner,
            target: other_arena
        })
    );
    assert_eq!(
        heap.object(owner).unwrap().fields()[&field(2)],
        Value::Reference(same_arena)
    );
}

#[test]
fn rejected_root_replacement_preserves_the_existing_root() {
    let mut heap = Heap::default();
    let managed = allocate(&mut heap, 1, AllocationDomain::Managed);
    let transient = allocate(
        &mut heap,
        2,
        AllocationDomain::Transient(ArenaId::from_bytes([1; 16])),
    );
    heap.set_root("world", managed).unwrap();
    assert_eq!(
        heap.set_root("world", transient),
        Err(HeapError::TransientRetention(transient))
    );
    assert_eq!(
        heap.set_root("world", object(99)),
        Err(HeapError::UnknownObject(object(99)))
    );
    assert_eq!(heap.roots()["world"], managed);
}

#[test]
fn execution_roots_include_all_frames_slots_and_pending_constructor_results() {
    let mut heap = Heap::default();
    let ids: Vec<_> = (1..=5)
        .map(|i| allocate(&mut heap, i, AllocationDomain::Managed))
        .collect();
    allocate(&mut heap, 99, AllocationDomain::Managed);
    let mut caller = frame(Continuation::AwaitReturn {
        call_site: 5,
        next: 10,
        result: CallResult::ConstructedObject(ids[3]),
    });
    caller.arguments = vec![Value::Reference(ids[0]), Value::I32(3)];
    caller.locals = vec![None, Some(Value::Null), Some(Value::Reference(ids[1]))];
    caller.evaluation_stack = vec![Value::Reference(ids[2])];
    let mut callee = frame(Continuation::Execute { next: 12 });
    callee.arguments = vec![Value::Reference(ids[4])];
    let execution = ExecutionContext {
        id: ExecutionId::from_bytes([4; 16]),
        status: ExecutionStatus::Paused,
        frames: vec![caller, callee],
    };

    assert_eq!(
        heap.retained_objects(execution.references()).unwrap(),
        BTreeSet::from_iter(ids)
    );
}

#[test]
fn completed_contexts_retain_returned_objects_without_active_frames() {
    let mut heap = Heap::default();
    let result = allocate(&mut heap, 1, AllocationDomain::Managed);
    let context = ExecutionContext {
        id: ExecutionId::from_bytes([4; 16]),
        status: ExecutionStatus::Completed {
            result: Some(Value::Reference(result)),
        },
        frames: Vec::new(),
    };
    assert_eq!(
        heap.retained_objects(context.references()).unwrap(),
        BTreeSet::from([result])
    );
}

#[test]
fn transient_execution_references_make_durable_tracing_fail() {
    let mut heap = Heap::default();
    let managed = allocate(&mut heap, 1, AllocationDomain::Managed);
    let transient = allocate(
        &mut heap,
        2,
        AllocationDomain::Transient(ArenaId::from_bytes([1; 16])),
    );
    heap.set_root("world", managed).unwrap();
    let mut active = frame(Continuation::Execute { next: 0 });
    active.evaluation_stack.push(Value::Reference(transient));
    assert_eq!(
        heap.retained_objects(active.references()),
        Err(HeapError::TransientRetention(transient))
    );
    assert_eq!(
        heap.retained_objects([object(99)]),
        Err(HeapError::UnknownObject(object(99)))
    );
    assert_eq!(
        heap.retained_objects([]).unwrap(),
        BTreeSet::from([managed])
    );
}

#[test]
fn primitive_encoding_has_fixed_golden_bytes() {
    assert_eq!(Value::Null.to_bytes(), [1, 0]);
    assert_eq!(Value::I32(-42).to_bytes(), [1, 1, 0xd6, 0xff, 0xff, 0xff]);
    assert_eq!(
        Value::I64(0x0102030405060708).to_bytes(),
        [1, 2, 8, 7, 6, 5, 4, 3, 2, 1]
    );
    let id = ObjectId::from_bytes([0x7a; 16]);
    let mut encoded_reference = vec![1, 3];
    encoded_reference.extend_from_slice(&[0x7a; 16]);
    assert_eq!(Value::Reference(id).to_bytes(), encoded_reference);
}

#[test]
fn integer_extrema_and_references_round_trip() {
    for value in [
        Value::Null,
        Value::I32(i32::MIN),
        Value::I32(i32::MAX),
        Value::I64(i64::MIN),
        Value::I64(i64::MAX),
        Value::Reference(object(u128::MAX)),
    ] {
        assert_eq!(Value::from_bytes(&value.to_bytes()), Ok(value));
    }
}

#[test]
fn null_is_distinct_from_an_all_zero_object_identity() {
    let reference = Value::Reference(object(0));
    assert_ne!(reference, Value::Null);
    assert_eq!(Value::from_bytes(&reference.to_bytes()), Ok(reference));
    assert_eq!(reference.referenced_object(), Some(object(0)));
}

#[test]
fn decoder_rejects_unknown_versions_tags_truncation_and_trailing_data() {
    assert_eq!(
        Value::from_bytes(&[2, 0]),
        Err(ValueEncodingError::UnsupportedVersion(2))
    );
    assert_eq!(
        Value::from_bytes(&[1, 255]),
        Err(ValueEncodingError::UnknownTag(255))
    );
    for value in [
        Value::Null,
        Value::I32(42),
        Value::I64(-1),
        Value::Reference(object(7)),
    ] {
        let mut bytes = value.to_bytes();
        for length in 0..bytes.len() {
            assert!(Value::from_bytes(&bytes[..length]).is_err());
        }
        bytes.push(0);
        assert!(Value::from_bytes(&bytes).is_err());
    }
}
