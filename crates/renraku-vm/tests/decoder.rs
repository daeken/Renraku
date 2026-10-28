use renraku_vm::cil::{Op, decode};

#[test]
fn rejects_unknown_truncated_and_nonboundary_instructions() {
    for bytes in [
        &[0x74, 0, 0, 0, 0, 0x2a][..],
        &[0xfe, 0xff, 0x2a],
        &[0x20, 0],
        &[0xfe],
        &[0x2b, 0xff, 0x2a],
        &[0x2b, 0x7f, 0x2a],
        &[0x00],
        &[],
    ] {
        assert!(
            decode(bytes).is_err(),
            "accepted malformed method {bytes:x?}"
        );
    }
}

#[test]
fn branches_keep_original_byte_offsets_including_long_operands() {
    let instructions = decode(&[0x38, 1, 0, 0, 0, 0x00, 0x2a]).unwrap();
    assert!(matches!(instructions[&0].op, Op::Branch(6)));
    assert_eq!(instructions[&0].next, 5);
    assert_eq!(instructions[&6].offset, 6);
}
