//! Location independent identities, minted using random UUIDs and retained by
//! checkpoints. IDs never encode memory addresses or physical storage locations.

use std::fmt;

macro_rules! identity {
    ($name:ident) => {
        #[derive(Clone, Copy, Eq, Hash, Ord, PartialEq, PartialOrd)]
        pub struct $name([u8; 16]);

        impl $name {
            pub fn fresh() -> Self {
                Self(*uuid::Uuid::new_v4().as_bytes())
            }

            pub const fn from_bytes(bytes: [u8; 16]) -> Self {
                Self(bytes)
            }

            pub const fn as_bytes(&self) -> &[u8; 16] {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                for byte in self.0 {
                    write!(formatter, "{byte:02x}")?;
                }
                Ok(())
            }
        }

        impl fmt::Debug for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(formatter, "{}({self})", stringify!($name))
            }
        }
    };
}

identity!(ObjectId);
identity!(TypeId);
identity!(FieldId);
identity!(MethodId);
identity!(CodeVersionId);
identity!(ExecutionId);
identity!(ArenaId);
identity!(AssemblyId);
identity!(UniverseId);
identity!(EffectId);
