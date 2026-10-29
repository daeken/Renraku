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
    Divide {
        unsigned: bool,
    },
    Remainder {
        unsigned: bool,
    },
    ConvertI64,
    Branch(u32),
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
            0x58 => Op::Add,
            0x59 => Op::Subtract,
            0x5a => Op::Multiply,
            0x5b => Op::Divide { unsigned: false },
            0x5c => Op::Divide { unsigned: true },
            0x5d => Op::Remainder { unsigned: false },
            0x5e => Op::Remainder { unsigned: true },
            0x6a => Op::ConvertI64,
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
        if !matches!(instruction.op, Op::Return | Op::Branch(_)) {
            require(
                instructions.contains_key(&instruction.next),
                "instruction falls off end of method",
            )?;
        }
    }
    Ok(instructions)
}
