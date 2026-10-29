//! Immutable, provider-independent request values captured at the guest boundary.
use crate::{
    error::{Error, Result, require},
    heap::MAX_STRING_UNITS,
    identity::{EffectId, ObjectId},
};

pub const DEFAULT_MODEL: &str = "echo-v1";
pub const MAX_MESSAGES: usize = 128;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MessageRole {
    System,
    User,
    Assistant,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn request_encoding_has_fixed_golden_bytes_and_exact_utf16() {
        let request = ModelRequest::echo(EffectId::from_bytes([7; 16]), vec![0, 0xd800]);
        assert_eq!(
            request.encode().unwrap(),
            b"\x01\x0a\0\0\0local.echo\x07\0\0\0echo-v1\x01\0\0\0\x01\x02\0\0\0\0\0\0\xd8"
        );
    }
}
impl MessageRole {
    pub(crate) fn tag(self) -> u8 {
        match self {
            Self::System => 0,
            Self::User => 1,
            Self::Assistant => 2,
        }
    }
    pub(crate) fn from_tag(tag: u8) -> Result<Self> {
        match tag {
            0 => Ok(Self::System),
            1 => Ok(Self::User),
            2 => Ok(Self::Assistant),
            _ => Err(Error("unsupported model message role".into())),
        }
    }
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MessageReference {
    pub role: MessageRole,
    pub content: ObjectId,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ModelMessage {
    pub role: MessageRole,
    pub content: Vec<u16>,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ModelRequest {
    pub id: EffectId,
    pub capability: String,
    pub model: String,
    pub messages: Vec<ModelMessage>,
}
pub(crate) fn validate_identifier(value: &str) -> Result<()> {
    require(
        !value.is_empty()
            && value.len() <= 128
            && value
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"._-/:".contains(&b)),
        "invalid model capability or model identifier",
    )
}
impl ModelRequest {
    pub fn echo(id: EffectId, prompt: Vec<u16>) -> Self {
        Self {
            id,
            capability: "local.echo".into(),
            model: DEFAULT_MODEL.into(),
            messages: vec![ModelMessage {
                role: MessageRole::User,
                content: prompt,
            }],
        }
    }
    pub(crate) fn validate(&self) -> Result<()> {
        validate_identifier(&self.capability)?;
        validate_identifier(&self.model)?;
        require(
            !self.messages.is_empty() && self.messages.len() <= MAX_MESSAGES,
            "invalid model message count",
        )?;
        require(
            self.messages.last().unwrap().role == MessageRole::User,
            "model conversation must end with a user message",
        )?;
        let mut total = 0usize;
        for message in &self.messages {
            total = total
                .checked_add(message.content.len())
                .ok_or_else(|| Error("model request size overflow".into()))?;
            require(
                total <= MAX_STRING_UNITS,
                "model request exceeds the UTF-16 unit limit",
            )?;
        }
        Ok(())
    }
    /// Versioned canonical bytes bind receipts to the full immutable request.
    pub fn encode(&self) -> Result<Vec<u8>> {
        self.validate()?;
        fn bytes(output: &mut Vec<u8>, data: &[u8]) {
            output.extend((data.len() as u32).to_le_bytes());
            output.extend(data);
        }
        let mut output = vec![1];
        bytes(&mut output, self.capability.as_bytes());
        bytes(&mut output, self.model.as_bytes());
        output.extend((self.messages.len() as u32).to_le_bytes());
        for message in &self.messages {
            output.push(message.role.tag());
            output.extend((message.content.len() as u32).to_le_bytes());
            output.extend(message.content.iter().flat_map(|unit| unit.to_le_bytes()));
        }
        Ok(output)
    }
    pub(crate) fn legacy_prompt(&self) -> Option<&[u16]> {
        (self.capability == "local.echo"
            && self.model == DEFAULT_MODEL
            && self.messages.len() == 1
            && self.messages[0].role == MessageRole::User)
            .then(|| self.messages[0].content.as_slice())
    }
}
