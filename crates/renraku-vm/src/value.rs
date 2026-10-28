//! The initial integer and reference value profile.

use std::{error::Error, fmt};

use crate::identity::ObjectId;

/// Values shared by the first heap and frame model. An opcode determines signed
/// or unsigned interpretation of integer bits. Additional CIL types are deferred.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Value {
    Null,
    I32(i32),
    I64(i64),
    Reference(ObjectId),
}

impl Value {
    pub const fn referenced_object(&self) -> Option<ObjectId> {
        match self {
            Self::Reference(id) => Some(*id),
            _ => None,
        }
    }

    /// Primitive value format v1: version, tag, then a fixed width payload.
    /// Integers are little endian; object IDs are their opaque 16 bytes.
    pub fn to_bytes(self) -> Vec<u8> {
        let mut bytes = vec![1];
        match self {
            Self::Null => bytes.push(0),
            Self::I32(value) => {
                bytes.push(1);
                bytes.extend_from_slice(&value.to_le_bytes());
            }
            Self::I64(value) => {
                bytes.push(2);
                bytes.extend_from_slice(&value.to_le_bytes());
            }
            Self::Reference(id) => {
                bytes.push(3);
                bytes.extend_from_slice(id.as_bytes());
            }
        }
        bytes
    }

    /// Decode exactly one value. Reject unknown versions, tags, and extra data.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, ValueEncodingError> {
        if bytes.len() < 2 {
            return Err(ValueEncodingError::InvalidLength {
                expected: 2,
                actual: bytes.len(),
            });
        }
        if bytes[0] != 1 {
            return Err(ValueEncodingError::UnsupportedVersion(bytes[0]));
        }
        let payload_len = match bytes[1] {
            0 => 0,
            1 => 4,
            2 => 8,
            3 => 16,
            tag => return Err(ValueEncodingError::UnknownTag(tag)),
        };
        if bytes.len() != 2 + payload_len {
            return Err(ValueEncodingError::InvalidLength {
                expected: 2 + payload_len,
                actual: bytes.len(),
            });
        }
        // Exact payload lengths were checked above.
        Ok(match bytes[1] {
            0 => Self::Null,
            1 => Self::I32(i32::from_le_bytes(bytes[2..].try_into().unwrap())),
            2 => Self::I64(i64::from_le_bytes(bytes[2..].try_into().unwrap())),
            3 => Self::Reference(ObjectId::from_bytes(bytes[2..].try_into().unwrap())),
            _ => unreachable!(),
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ValueEncodingError {
    UnsupportedVersion(u8),
    UnknownTag(u8),
    InvalidLength { expected: usize, actual: usize },
}

impl fmt::Display for ValueEncodingError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedVersion(version) => {
                write!(
                    formatter,
                    "unsupported primitive value format version {version}"
                )
            }
            Self::UnknownTag(tag) => write!(formatter, "unsupported primitive value tag {tag}"),
            Self::InvalidLength { expected, actual } => {
                write!(
                    formatter,
                    "expected {expected} value bytes, received {actual}"
                )
            }
        }
    }
}

impl Error for ValueEncodingError {}
