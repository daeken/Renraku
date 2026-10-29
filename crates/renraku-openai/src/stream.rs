//! Bounded SSE framing. Deltas are provisional; only the full terminal result commits.
use crate::{Result, fail, require};
use renraku_vm::heap::MAX_STRING_UNITS;
use serde_json::Value;
use std::io::Read;

const MAX_EVENT: usize = 8 * 1024 * 1024;
const MAX_STREAM: usize = 32 * 1024 * 1024;
pub(crate) struct Completion {
    pub id: String,
    pub text: Vec<u16>,
}
pub(crate) fn completed(mut reader: impl Read) -> Result<Completion> {
    let mut decoder = Decoder::default();
    let mut buffer = [0u8; 16 * 1024];
    loop {
        let count = match reader.read(&mut buffer) {
            Ok(0) => return decoder.end(),
            Ok(count) => count,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => return Err(fail("Responses stream interrupted; outcome is unknown")),
        };
        if let Some(completion) = decoder.push(&buffer[..count])? {
            return Ok(completion);
        }
    }
}
/// The same bounded parser serves blocking reads and asynchronous HTTP chunks.
/// Its provisional bytes are native transport state, never guest checkpoint data.
#[derive(Default)]
pub(crate) struct Decoder {
    line: Vec<u8>,
    data: Vec<u8>,
    name: String,
    total: usize,
    event_size: usize,
    lines: usize,
    created_id: Option<String>,
}
impl Decoder {
    pub fn push(&mut self, bytes: &[u8]) -> Result<Option<Completion>> {
        for part in bytes.split_inclusive(|byte| *byte == b'\n') {
            self.total += part.len();
            self.event_size += part.len();
            require(
                self.event_size <= MAX_EVENT && self.total <= MAX_STREAM,
                "Responses stream exceeds its size limit",
            )?;
            self.line.extend_from_slice(part);
            if part.last() != Some(&b'\n') {
                continue;
            }
            let mut raw = std::mem::take(&mut self.line);
            raw.pop();
            if raw.last() == Some(&b'\r') {
                raw.pop();
            }
            let line =
                std::str::from_utf8(&raw).map_err(|_| fail("Responses stream is not UTF-8"))?;
            let line = if self.lines == 0 {
                line.trim_start_matches('\u{feff}')
            } else {
                line
            };
            self.lines += 1;
            if let Some(completion) = self.on_line(line)? {
                return Ok(Some(completion));
            }
            raw.clear();
            self.line = raw;
        }
        Ok(None)
    }
    pub fn end(self) -> Result<Completion> {
        if !self.line.is_empty() {
            return Err(fail("Responses stream has an unfinished event"));
        }
        Err(fail(
            "Responses stream ended without response.completed; outcome is unknown",
        ))
    }
    fn on_line(&mut self, line: &str) -> Result<Option<Completion>> {
        if line.is_empty() {
            self.event_size = 0;
            if !self.data.is_empty() {
                let value: Value = serde_json::from_slice(&self.data)
                    .map_err(|_| fail("invalid Responses event JSON"))?;
                let kind = value["type"]
                    .as_str()
                    .ok_or_else(|| fail("Responses event has no type"))?;
                require(
                    self.name.is_empty() || self.name == kind,
                    "Responses event name and type disagree",
                )?;
                match kind {
                    "response.created" => {
                        let id = response_id(&value["response"])?;
                        require(
                            self.created_id.as_ref().is_none_or(|old| old == id),
                            "Responses stream changed response identity",
                        )?;
                        self.created_id = Some(id.to_owned());
                    }
                    "response.completed" => {
                        let response = &value["response"];
                        let id = response_id(response)?;
                        require(
                            self.created_id.as_ref().is_none_or(|old| old == id),
                            "Responses stream changed response identity",
                        )?;
                        require(
                            response["status"] == "completed"
                                && response["error"].is_null()
                                && response["incomplete_details"].is_null(),
                            "invalid completed Responses status",
                        )?;
                        return Ok(Some(Completion {
                            id: id.to_owned(),
                            text: text(response)?,
                        }));
                    }
                    "response.failed" | "response.incomplete" | "error" => {
                        let code = value["response"]["error"]["code"]
                            .as_str()
                            .or_else(|| value["code"].as_str())
                            .unwrap_or("unknown_error");
                        return Err(fail(&format!(
                            "Responses stream failed ({})",
                            diagnostic(code)
                        )));
                    }
                    _ => {} // Deltas and reasoning never prove completion.
                }
            }
            self.data.clear();
            self.name.clear();
        } else if !line.starts_with(':') {
            let (field, value) = line.split_once(':').unwrap_or((line, ""));
            let value = value.strip_prefix(' ').unwrap_or(value);
            match field {
                "event" => self.name = value.to_owned(),
                "data" => {
                    self.data.extend_from_slice(value.as_bytes());
                    self.data.push(b'\n');
                }
                _ => {}
            }
        }
        Ok(None)
    }
}
fn response_id(response: &Value) -> Result<&str> {
    response["id"]
        .as_str()
        .filter(|id| {
            !id.is_empty()
                && id.len() <= 256
                && id
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
        })
        .ok_or_else(|| fail("Responses event has an invalid response identity"))
}
fn text(response: &Value) -> Result<Vec<u16>> {
    let output = response["output"]
        .as_array()
        .ok_or_else(|| fail("completed response has no output array"))?;
    let mut result = Vec::new();
    let mut found = false;
    for item in output {
        if item["type"] == "reasoning" {
            continue;
        }
        require(
            item["type"] == "message"
                && item["role"] == "assistant"
                && item["status"] == "completed",
            "unsupported non-text or unfinished Responses output",
        )?;
        match item["phase"].as_str() {
            Some("commentary") => continue,
            None if item["phase"].is_null() => {}
            Some("final_answer") => {}
            _ => return Err(fail("unsupported Responses message phase")),
        }
        let content = item["content"]
            .as_array()
            .ok_or_else(|| fail("assistant message has no content array"))?;
        for part in content {
            require(
                part["type"] == "output_text",
                "refusal or non-text Responses output is unsupported",
            )?;
            let text = part["text"]
                .as_str()
                .ok_or_else(|| fail("assistant text is missing"))?;
            for unit in text.encode_utf16() {
                require(
                    result.len() < MAX_STRING_UNITS,
                    "response exceeds the managed string limit",
                )?;
                result.push(unit);
            }
            found = true;
        }
    }
    require(found, "completed response contains no final assistant text")?;
    Ok(result)
}
pub(crate) fn diagnostic(value: &str) -> String {
    if !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
    {
        value.to_owned()
    } else {
        "unavailable".into()
    }
}
