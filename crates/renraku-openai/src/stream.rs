//! Bounded SSE framing. Deltas are provisional; only the full terminal result commits.
use crate::{Result, fail, require};
use renraku_vm::heap::MAX_STRING_UNITS;
use serde_json::Value;
use std::io::{BufRead, BufReader, Read};

const MAX_EVENT: usize = 8 * 1024 * 1024;
const MAX_STREAM: usize = 32 * 1024 * 1024;
pub(crate) struct Completion {
    pub id: String,
    pub text: Vec<u16>,
}
pub(crate) fn completed(reader: impl Read) -> Result<Completion> {
    let mut reader = BufReader::new(reader);
    let mut data = Vec::new();
    let mut name = String::new();
    let mut total = 0usize;
    let mut event_size = 0usize;
    let mut created_id: Option<String> = None;
    loop {
        let mut line = Vec::new();
        let count = reader
            .by_ref()
            .take((MAX_EVENT + 1) as u64)
            .read_until(b'\n', &mut line)
            .map_err(|_| fail("Responses stream interrupted; outcome is unknown"))?;
        require(
            count > 0,
            "Responses stream ended without response.completed; outcome is unknown",
        )?;
        total += count;
        event_size += count;
        require(
            event_size <= MAX_EVENT && total <= MAX_STREAM,
            "Responses stream exceeds its size limit",
        )?;
        require(
            line.last() == Some(&b'\n'),
            "Responses stream has an unfinished event",
        )?;
        line.pop();
        if line.last() == Some(&b'\r') {
            line.pop();
        }
        let line = std::str::from_utf8(&line).map_err(|_| fail("Responses stream is not UTF-8"))?;
        let line = if total == count {
            line.trim_start_matches('\u{feff}')
        } else {
            line
        };
        if line.is_empty() {
            event_size = 0;
            if !data.is_empty() {
                let value: Value = serde_json::from_slice(&data)
                    .map_err(|_| fail("invalid Responses event JSON"))?;
                let kind = value["type"]
                    .as_str()
                    .ok_or_else(|| fail("Responses event has no type"))?;
                require(
                    name.is_empty() || name == kind,
                    "Responses event name and type disagree",
                )?;
                match kind {
                    "response.created" => {
                        let id = response_id(&value["response"])?;
                        require(
                            created_id.as_ref().is_none_or(|old| old == id),
                            "Responses stream changed response identity",
                        )?;
                        created_id = Some(id.to_owned());
                    }
                    "response.completed" => {
                        let response = &value["response"];
                        let id = response_id(response)?;
                        require(
                            created_id.as_ref().is_none_or(|old| old == id),
                            "Responses stream changed response identity",
                        )?;
                        require(
                            response["status"] == "completed"
                                && response["error"].is_null()
                                && response["incomplete_details"].is_null(),
                            "invalid completed Responses status",
                        )?;
                        return Ok(Completion {
                            id: id.to_owned(),
                            text: text(response)?,
                        });
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
                    _ => {} // Text deltas and reasoning are never completion evidence.
                }
            }
            data.clear();
            name.clear();
        } else if !line.starts_with(':') {
            let (field, value) = line.split_once(':').unwrap_or((line, ""));
            let value = value.strip_prefix(' ').unwrap_or(value);
            match field {
                "event" => name = value.to_owned(),
                "data" => {
                    data.extend_from_slice(value.as_bytes());
                    data.push(b'\n');
                }
                _ => {}
            }
        }
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
