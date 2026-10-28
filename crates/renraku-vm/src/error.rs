use std::{error, fmt};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Error(pub String);

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl error::Error for Error {}
impl From<std::io::Error> for Error {
    fn from(error: std::io::Error) -> Self {
        Self(error.to_string())
    }
}
impl From<crate::heap::HeapError> for Error {
    fn from(error: crate::heap::HeapError) -> Self {
        Self(error.to_string())
    }
}
impl From<rusqlite::Error> for Error {
    fn from(error: rusqlite::Error) -> Self {
        Self(format!("checkpoint storage: {error}"))
    }
}

pub type Result<T> = std::result::Result<T, Error>;

pub(crate) fn require(condition: bool, message: &str) -> Result<()> {
    if condition {
        Ok(())
    } else {
        Err(Error(message.into()))
    }
}
