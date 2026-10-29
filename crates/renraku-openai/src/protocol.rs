//! Shared request/catalog/diagnostic policy for blocking and async transports.
use crate::{Result, fail, inference::AvailableModel, require, stream};
use renraku_vm::model::{MessageRole, ModelRequest};
use reqwest::header::HeaderMap;
use serde::Deserialize;
use serde_json::{Value, json};
pub(crate) fn valid_model(model: &str) -> bool {
    !model.is_empty()
        && model.len() <= 128
        && model
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._-/:".contains(&b))
}
pub(crate) fn request_body(
    capability: &str,
    model: &str,
    request: &ModelRequest,
) -> Result<(Vec<u8>, Value)> {
    require(
        request.capability == capability && request.model == model,
        "request does not match the bound profile and model",
    )?;
    let bytes = request.encode().map_err(|e| fail(&e.to_string()))?;
    let input: Result<Vec<_>> = request
        .messages
        .iter()
        .map(|message| {
            let role = match message.role {
                MessageRole::System => "developer",
                MessageRole::User => "user",
                MessageRole::Assistant => "assistant",
            };
            let content = String::from_utf16(&message.content).map_err(|_| {
                fail("OpenAI text transport cannot represent unpaired UTF-16 surrogates")
            })?;
            Ok(json!({"role":role,"content":content}))
        })
        .collect();
    Ok((
        bytes,
        json!({"model":model,"input":input?,"store":false,"stream":true}),
    ))
}
pub(crate) fn parse_models(bytes: &[u8]) -> Result<Vec<AvailableModel>> {
    #[derive(Deserialize)]
    struct Entry {
        slug: Option<String>,
        display_name: Option<String>,
        visibility: Option<String>,
    }
    #[derive(Deserialize)]
    struct Catalog {
        models: Vec<Entry>,
    }
    require(
        bytes.len() as u64 <= crate::MAX_BODY,
        "OpenAI model catalog exceeds its size limit",
    )?;
    let catalog: Catalog =
        serde_json::from_slice(bytes).map_err(|_| fail("invalid OpenAI model catalog"))?;
    require(
        catalog.models.len() <= 4096,
        "OpenAI model catalog exceeds its count limit",
    )?;
    let mut slugs = std::collections::BTreeSet::new();
    catalog
        .models
        .into_iter()
        .filter(|m| m.visibility.as_deref() == Some("list"))
        .map(|model| {
            let slug = model
                .slug
                .ok_or_else(|| fail("visible model has no slug"))?;
            let display_name = model
                .display_name
                .ok_or_else(|| fail("visible model has no display name"))?;
            require(
                valid_model(&slug)
                    && slugs.insert(slug.clone())
                    && !display_name.is_empty()
                    && display_name.len() <= 512
                    && !display_name.chars().any(char::is_control),
                "invalid or duplicate OpenAI model entry",
            )?;
            Ok(AvailableModel { slug, display_name })
        })
        .collect()
}
pub(crate) fn accepts_sse(headers: &HeaderMap) -> bool {
    headers.get("content-type").is_none_or(|header| {
        header.to_str().is_ok_and(|value| {
            value
                .split(';')
                .next()
                .is_some_and(|v| v.trim().eq_ignore_ascii_case("text/event-stream"))
        })
    })
}
pub(crate) fn http_error(status: u16, headers: &HeaderMap, body: Option<&[u8]>) -> crate::Error {
    let request_id = headers
        .get("x-request-id")
        .and_then(|v| v.to_str().ok())
        .map(stream::diagnostic)
        .unwrap_or_else(|| "unavailable".into());
    let code = body
        .filter(|bytes| bytes.len() as u64 <= crate::MAX_BODY)
        .and_then(|bytes| serde_json::from_slice::<Value>(bytes).ok())
        .and_then(|body| body["error"]["code"].as_str().map(stream::diagnostic))
        .unwrap_or_else(|| "unavailable".into());
    fail(&format!(
        "OpenAI HTTP {status}; code {code}; request ID {request_id}"
    ))
}
pub(crate) fn unexpected_transport(
    status: u16,
    headers: &HeaderMap,
    body: Option<&[u8]>,
) -> crate::Error {
    let content_type = headers
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(';').next())
        .map(str::trim)
        .filter(|v| {
            v.len() <= 100
                && v.contains('/')
                && v.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"/+.-".contains(&b))
        })
        .unwrap_or("unavailable");
    let shape = if let Some(bytes) = body {
        if bytes.len() as u64 > crate::MAX_BODY {
            "oversized body"
        } else if let Ok(value) = serde_json::from_slice::<Value>(bytes) {
            if value["object"] == "response" && value["status"] == "completed" {
                "completed response JSON"
            } else if value["type"] == "response.completed" {
                "completion event JSON"
            } else if value.get("error").is_some() || value.get("detail").is_some() {
                "error JSON"
            } else {
                "other JSON"
            }
        } else if bytes.starts_with(b"data:")
            || bytes.starts_with(b"event:")
            || bytes.starts_with(b":")
        {
            "SSE with an unexpected content type"
        } else {
            "unrecognized body"
        }
    } else {
        "unreadable body"
    };
    fail(&format!(
        "Responses expected SSE; HTTP {status}; content type {content_type}; {shape}; outcome is unknown"
    ))
}
