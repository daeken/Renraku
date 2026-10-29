//! The supported CIL decoder. Offsets remain byte offsets, never vector indices.

use crate::{
    error::{Error, Result, require},
    value::Value,
};
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug)]
pub enum Comparison {
    Equal,
    NotEqual,
    Greater,
    GreaterEqual,
    Less,
    LessEqual,
}

#[derive(Clone, Debug)]
pub enum Op {
    LoadString(u32),
    Nop,
    Constant(Value),
    LoadArgument(usize),
    StoreArgument(usize),
    LoadLocal(usize),
    StoreLocal(usize),
    Dup,
    Pop,
    Add,
    Subtract,
    Multiply,
    AddChecked {
        unsigned: bool,
    },
    SubtractChecked {
        unsigned: bool,
    },
    MultiplyChecked {
        unsigned: bool,
    },
    BitAnd,
    BitOr,
    BitXor,
    BitNot,
    Negate,
    ShiftLeft,
    ShiftRight {
        unsigned: bool,
    },
    Divide {
        unsigned: bool,
    },
    Remainder {
        unsigned: bool,
    },
    ConvertI8,
    ConvertU8,
    ConvertI16,
    ConvertU16,
    ConvertI32,
    ConvertI64,
    ConvertU64,
    Branch(u32),
    Switch(Vec<u32>),
    BranchIf {
        target: u32,
        when_true: bool,
    },
    Compare {
        comparison: Comparison,
        unsigned: bool,
    },
    BranchCompare {
        target: u32,
        comparison: Comparison,
        unsigned: bool,
    },
    Call {
        token: u32,
        null_check: bool,
    },
    NewObject(u32),
    Return,
    LoadField {
        token: u32,
        is_static: bool,
    },
    StoreField {
        token: u32,
        is_static: bool,
    },
}

#[derive(Clone, Debug)]
pub struct Instruction {
    pub offset: u32,
    pub next: u32,
    pub op: Op,
}

pub(crate) struct Reader<'a> {
    pub data: &'a [u8],
    pub position: usize,
}
impl<'a> Reader<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self { data, position: 0 }
    }
    pub fn take(&mut self, count: usize) -> Result<&'a [u8]> {
        let end = self
            .position
            .checked_add(count)
            .ok_or_else(|| Error("input length overflow".into()))?;
        let bytes = self
            .data
            .get(self.position..end)
            .ok_or_else(|| Error("truncated input".into()))?;
        self.position = end;
        Ok(bytes)
    }
    pub fn u8(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }
    pub fn u16(&mut self) -> Result<u16> {
        Ok(u16::from_le_bytes(self.take(2)?.try_into().unwrap()))
    }
    pub fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    pub fn compressed(&mut self) -> Result<u32> {
        let first = self.u8()?;
        match first {
            0..=0x7f => Ok(first as u32),
            0x80..=0xbf => {
                let value = (((first & 0x3f) as u32) << 8) | self.u8()? as u32;
                require(value >= 0x80, "noncanonical compressed integer")?;
                Ok(value)
            }
            0xc0..=0xdf => {
                let value = (((first & 0x1f) as u32) << 24)
                    | ((self.u8()? as u32) << 16)
                    | ((self.u8()? as u32) << 8)
                    | self.u8()? as u32;
                require(value >= 0x4000, "noncanonical compressed integer")?;
                Ok(value)
            }
            _ => Err(Error("invalid compressed integer".into())),
        }
    }
}

fn branch_target(reader: &mut Reader<'_>, short: bool) -> Result<u32> {
    let delta = if short {
        reader.u8()? as i8 as i64
    } else {
        reader.u32()? as i32 as i64
    };
    u32::try_from(reader.position as i64 + delta)
        .map_err(|_| Error("branch target outside method".into()))
}

// Tables are immutable code metadata, never guest arrays. Check the complete
// encoded size before reserving memory; each delta shares the table-end base.
pub const MAX_SWITCH_TARGETS: usize = 4096;
fn switch_targets(reader: &mut Reader<'_>) -> Result<Vec<u32>> {
    let count = reader.u32()? as usize;
    require(
        count <= (reader.data.len() - reader.position) / 4,
        "truncated switch table",
    )?;
    require(
        count <= MAX_SWITCH_TARGETS,
        "switch table exceeds 4096 targets",
    )?;
    let base = reader
        .position
        .checked_add(
            count
                .checked_mul(4)
                .ok_or_else(|| Error("switch table size overflow".into()))?,
        )
        .ok_or_else(|| Error("switch table size overflow".into()))?;
    let mut targets = Vec::with_capacity(count);
    for _ in 0..count {
        let delta = reader.u32()? as i32;
        let target = (base as i64)
            .checked_add(i64::from(delta))
            .and_then(|target| u32::try_from(target).ok())
            .ok_or_else(|| Error("switch target outside method".into()))?;
        targets.push(target);
    }
    Ok(targets)
}

pub fn decode(bytes: &[u8]) -> Result<BTreeMap<u32, Instruction>> {
    require(bytes.len() <= u32::MAX as usize, "method body too large")?;
    let mut reader = Reader::new(bytes);
    let mut instructions = BTreeMap::new();
    while reader.position < bytes.len() {
        let offset = reader.position as u32;
        let opcode = reader.u8()?;
        let op = match opcode {
            0x00 => Op::Nop,
            0x02..=0x05 => Op::LoadArgument((opcode - 2) as usize),
            0x06..=0x09 => Op::LoadLocal((opcode - 6) as usize),
            0x0a..=0x0d => Op::StoreLocal((opcode - 10) as usize),
            0x0e => Op::LoadArgument(reader.u8()? as usize),
            0x10 => Op::StoreArgument(reader.u8()? as usize),
            0x11 => Op::LoadLocal(reader.u8()? as usize),
            0x13 => Op::StoreLocal(reader.u8()? as usize),
            0x14 => Op::Constant(Value::Null),
            0x15..=0x1e => Op::Constant(Value::I32(opcode as i32 - 0x16)),
            0x1f => Op::Constant(Value::I32(reader.u8()? as i8 as i32)),
            0x20 => Op::Constant(Value::I32(reader.u32()? as i32)),
            0x21 => Op::Constant(Value::I64(i64::from_le_bytes(
                reader.take(8)?.try_into().unwrap(),
            ))),
            0x25 => Op::Dup,
            0x26 => Op::Pop,
            0x28 | 0x6f => Op::Call {
                token: reader.u32()?,
                null_check: opcode == 0x6f,
            },
            0x2a => Op::Return,
            0x2b | 0x38 => Op::Branch(branch_target(&mut reader, opcode == 0x2b)?),
            0x2c | 0x2d | 0x39 | 0x3a => Op::BranchIf {
                target: branch_target(&mut reader, opcode < 0x38)?,
                when_true: opcode == 0x2d || opcode == 0x3a,
            },
            0x2e..=0x37 | 0x3b..=0x44 => {
                let index = if opcode < 0x38 {
                    opcode - 0x2e
                } else {
                    opcode - 0x3b
                };
                let comparison = match index {
                    0 => Comparison::Equal,
                    1 | 6 => Comparison::GreaterEqual,
                    2 | 7 => Comparison::Greater,
                    3 | 8 => Comparison::LessEqual,
                    4 | 9 => Comparison::Less,
                    5 => Comparison::NotEqual,
                    _ => unreachable!(),
                };
                Op::BranchCompare {
                    target: branch_target(&mut reader, opcode < 0x38)?,
                    comparison,
                    unsigned: index >= 5,
                }
            }
            0x45 => Op::Switch(switch_targets(&mut reader)?),
            0x58 => Op::Add,
            0x59 => Op::Subtract,
            0x5a => Op::Multiply,
            0x5b => Op::Divide { unsigned: false },
            0x5c => Op::Divide { unsigned: true },
            0x5d => Op::Remainder { unsigned: false },
            0x5e => Op::Remainder { unsigned: true },
            0x5f => Op::BitAnd,
            0x60 => Op::BitOr,
            0x61 => Op::BitXor,
            0x62 => Op::ShiftLeft,
            0x63 => Op::ShiftRight { unsigned: false },
            0x64 => Op::ShiftRight { unsigned: true },
            0x65 => Op::Negate,
            0x66 => Op::BitNot,
            0x67 => Op::ConvertI8,
            0x68 => Op::ConvertI16,
            0x69 | 0x6d => Op::ConvertI32,
            0x6a => Op::ConvertI64,
            0x6e => Op::ConvertU64,
            0xd1 => Op::ConvertU16,
            0xd2 => Op::ConvertU8,
            0xd6 => Op::AddChecked { unsigned: false },
            0xd7 => Op::AddChecked { unsigned: true },
            0xd8 => Op::MultiplyChecked { unsigned: false },
            0xd9 => Op::MultiplyChecked { unsigned: true },
            0xda => Op::SubtractChecked { unsigned: false },
            0xdb => Op::SubtractChecked { unsigned: true },
            0x72 => Op::LoadString(reader.u32()?),
            0x73 => Op::NewObject(reader.u32()?),
            0x7b | 0x7e => Op::LoadField {
                token: reader.u32()?,
                is_static: opcode == 0x7e,
            },
            0x7d | 0x80 => Op::StoreField {
                token: reader.u32()?,
                is_static: opcode == 0x80,
            },
            0xfe => match reader.u8()? {
                0x01 => Op::Compare {
                    comparison: Comparison::Equal,
                    unsigned: false,
                },
                0x02 | 0x03 => Op::Compare {
                    comparison: Comparison::Greater,
                    unsigned: bytes[reader.position - 1] == 3,
                },
                0x04 | 0x05 => Op::Compare {
                    comparison: Comparison::Less,
                    unsigned: bytes[reader.position - 1] == 5,
                },
                0x09 => Op::LoadArgument(reader.u16()? as usize),
                0x0b => Op::StoreArgument(reader.u16()? as usize),
                0x0c => Op::LoadLocal(reader.u16()? as usize),
                0x0e => Op::StoreLocal(reader.u16()? as usize),
                code => {
                    return Err(Error(format!(
                        "unsupported CIL opcode fe {code:02x} at IL_{offset:04x}"
                    )));
                }
            },
            _ => {
                return Err(Error(format!(
                    "unsupported CIL opcode {opcode:02x} at IL_{offset:04x}"
                )));
            }
        };
        instructions.insert(
            offset,
            Instruction {
                offset,
                next: reader.position as u32,
                op,
            },
        );
    }
    require(!instructions.is_empty(), "empty method body")?;
    for instruction in instructions.values() {
        let target = match instruction.op {
            Op::Branch(target) | Op::BranchIf { target, .. } | Op::BranchCompare { target, .. } => {
                Some(target)
            }
            _ => None,
        };
        if let Some(target) = target {
            require(
                instructions.contains_key(&target),
                "branch target is not an instruction boundary",
            )?;
        }
        if let Op::Switch(targets) = &instruction.op {
            for target in targets {
                require(
                    instructions.contains_key(target),
                    "switch target is not an instruction boundary",
                )?;
            }
        }
        if !matches!(instruction.op, Op::Return | Op::Branch(_)) {
            require(
                instructions.contains_key(&instruction.next),
                "instruction falls off end of method",
            )?;
        }
    }
    Ok(instructions)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table(deltas: &[i32]) -> Vec<u8> {
        let mut body = vec![0x45];
        body.extend_from_slice(&(deltas.len() as u32).to_le_bytes());
        for delta in deltas {
            body.extend_from_slice(&delta.to_le_bytes());
        }
        body
    }
    #[test]
    fn switch_tables_use_one_end_base_and_allow_zero_duplicate_and_backward_targets() {
        let mut body = vec![0];
        body.extend(table(&[-18, 0, 0])); // table ends at 18, targets 0/18/18
        body.extend([0x16, 0x2a]);
        let decoded = decode(&body).unwrap();
        assert_eq!(decoded[&1].next, 18);
        let Op::Switch(targets) = &decoded[&1].op else {
            panic!("not a switch")
        };
        assert_eq!(targets, &[0, 18, 18]);
        let mut empty = table(&[]);
        empty.extend([0x16, 0x2a]);
        assert!(matches!(&decode(&empty).unwrap()[&0].op, Op::Switch(t) if t.is_empty()));
    }
    #[test]
    fn switch_rejects_truncated_counts_tables_and_noninstruction_or_outside_targets() {
        for body in [
            vec![0x45],
            vec![0x45, 0, 0, 0],
            vec![0x45, 255, 255, 255, 255],
            vec![0x45, 1, 0, 0, 0, 0, 0, 0],
            table(&[]), // fallthrough is mandatory even for a table with no entries
            {
                let mut b = table(&[i32::MIN]);
                b.extend([0x16, 0x2a]);
                b
            },
            {
                let mut b = table(&[i32::MAX]);
                b.extend([0x16, 0x2a]);
                b
            },
            {
                let mut b = table(&[-1]);
                b.extend([0x16, 0x2a]);
                b
            }, // inside delta
            {
                let mut b = table(&[2]);
                b.extend([0x16, 0x2a]);
                b
            }, // exactly body end
            {
                let mut b = table(&[1]);
                b.extend([0x20, 0, 0, 0, 0, 0x2a]);
                b
            }, // constant operand
        ] {
            assert!(decode(&body).is_err(), "accepted {body:?}");
        }
    }
    #[test]
    fn switch_table_limit_is_explicit_and_checked_before_reserving_targets() {
        let mut maximum = table(&vec![0; MAX_SWITCH_TARGETS]);
        maximum.extend([0x16, 0x2a]);
        assert!(decode(&maximum).is_ok());
        let mut excessive = table(&vec![0; MAX_SWITCH_TARGETS + 1]);
        excessive.extend([0x16, 0x2a]);
        assert!(decode(&excessive).unwrap_err().0.contains("4096"));
    }
}
