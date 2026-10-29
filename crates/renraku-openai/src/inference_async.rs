//! Async HTTP delivery, sharing wire policy and durable receipts with the legacy host.
use crate::{
    Result, fail,
    inference::{Binding, OpenAIProvider, vm_error},
    protocol,
    receipts::Receipts,
    require, stream,
};
use renraku_vm::{effects::AsyncModelProvider, model::ModelRequest};
use reqwest::{Client, Response, redirect::Policy};
use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};
use uuid::Uuid;

/// Requires a Tokio runtime with I/O and time enabled. Profile and receipt I/O
/// use its blocking pool; model HTTP/stream waits use asynchronous sockets.
/// Native futures and worker jobs never become guest checkpoint state.
pub struct AsyncOpenAIProvider {
    auth_directory: PathBuf,
    binding: Binding,
    capability: String,
    model: String,
    http: Client,
    models: String,
    responses: String,
    receipts: Arc<Mutex<Receipts>>,
    validated: Option<Vec<u8>>,
}
async fn worker<T: Send + 'static>(
    operation: impl FnOnce() -> Result<T> + Send + 'static,
) -> Result<T> {
    require(
        tokio::runtime::Handle::try_current().is_ok(),
        "async OpenAI delivery requires a Tokio runtime",
    )?;
    tokio::task::spawn_blocking(operation)
        .await
        .map_err(|_| fail("OpenAI host worker failed"))?
}
impl AsyncOpenAIProvider {
    /// Opens host metadata/ledger only; no authentication or inference HTTP call.
    pub async fn open(
        auth_directory: impl AsRef<Path>,
        receipt_directory: impl AsRef<Path>,
        profile: Uuid,
        model: &str,
    ) -> Result<Self> {
        require(protocol::valid_model(model), "invalid OpenAI model slug")?;
        let auth_directory = auth_directory.as_ref().to_path_buf();
        let receipt_directory = receipt_directory.as_ref().to_path_buf();
        let model = model.to_owned();
        worker(move || {
            let binding = Binding::load(&auth_directory, profile)?;
            let receipts = Arc::new(Mutex::new(Receipts::open(&receipt_directory)?));
            let http = Client::builder()
                .redirect(Policy::none())
                .retry(reqwest::retry::never())
                .timeout(Duration::from_secs(120))
                .connect_timeout(Duration::from_secs(10))
                .user_agent("Renraku/0.1.0")
                .build()
                .map_err(|_| fail("could not initialize inference HTTPS client"))?;
            Ok(Self {
                auth_directory,
                binding,
                capability: OpenAIProvider::capability_for(profile),
                model,
                http,
                models: "https://api.openai.com/v1/models".into(),
                responses: "https://api.openai.com/v1/responses".into(),
                receipts,
                validated: None,
            })
        })
        .await
    }
    async fn bearer(&self) -> Result<String> {
        let binding = self.binding.clone();
        let directory = self.auth_directory.clone();
        // Token refresh retains the existing pending marker and exact-profile
        // semantics. It may finish after the awaiting host future is abandoned.
        worker(move || binding.bearer(&directory)).await
    }
    async fn ledger<T: Send + 'static>(
        &self,
        operation: impl FnOnce(&mut Receipts) -> Result<T> + Send + 'static,
    ) -> Result<T> {
        let receipts = self.receipts.clone();
        worker(move || {
            let mut ledger = receipts
                .lock()
                .map_err(|_| fail("completion receipt ledger is unavailable"))?;
            operation(&mut ledger)
        })
        .await
    }
    async fn validate(&mut self, request: &ModelRequest) -> Result<()> {
        let (bytes, _) = protocol::request_body(&self.capability, &self.model, request)?;
        let token = self.bearer().await?;
        let response = self
            .http
            .get(&self.models)
            .bearer_auth(token)
            .send()
            .await
            .map_err(|_| fail("OpenAI model discovery failed"))?;
        let models = protocol::parse_models(&body(checked(response).await?).await?)?;
        require(
            models.iter().any(|m| m.slug == self.model),
            "saved model is not available in the bound account's model catalog",
        )?;
        self.validated = Some(bytes);
        Ok(())
    }
    async fn lookup(&self, request: &ModelRequest, bytes: Vec<u8>) -> Result<Option<Vec<u16>>> {
        let binding = self.binding.bytes()?;
        let id = request.id;
        self.ledger(move |ledger| ledger.reconcile(id, &binding, &bytes))
            .await
    }
    async fn submit_once(&mut self, request: &ModelRequest) -> Result<Vec<u16>> {
        let (bytes, payload) = protocol::request_body(&self.capability, &self.model, request)?;
        if let Some(response) = self.lookup(request, bytes.clone()).await? {
            return Ok(response);
        }
        if self.validated.as_ref() != Some(&bytes) {
            self.validate(request).await?;
        }
        let token = self.bearer().await?;
        let binding = self.binding.bytes()?;
        let id = request.id;
        if let Some(response) = self
            .ledger(move |ledger| ledger.reserve(id, &binding, &bytes))
            .await?
        {
            return Ok(response);
        }
        // Abandonment after reservation never authorizes another POST, even if
        // no socket write had happened yet. Reconciliation is always local.
        let response = self
            .http
            .post(&self.responses)
            .bearer_auth(token)
            .header("Accept", "text/event-stream")
            .json(&payload)
            .send()
            .await
            .map_err(|_| fail("Responses request failed; outcome is unknown"))?;
        let mut response = checked(response).await?;
        if !protocol::accepts_sse(response.headers()) {
            let status = response.status().as_u16();
            let headers = response.headers().clone();
            let bytes = body(response).await;
            return Err(protocol::unexpected_transport(
                status,
                &headers,
                bytes.as_deref().ok(),
            ));
        }
        let mut decoder = stream::Decoder::default();
        let completion = loop {
            let chunk = response
                .chunk()
                .await
                .map_err(|_| fail("Responses stream interrupted; outcome is unknown"))?;
            let Some(chunk) = chunk else {
                break decoder.end()?;
            };
            if let Some(completion) = decoder.push(&chunk)? {
                break completion;
            }
        };
        let text = completion.text.clone();
        self.ledger(move |ledger| ledger.complete(id, &completion.id, &text))
            .await?;
        Ok(completion.text)
    }
    #[cfg(test)]
    pub(crate) fn test_endpoints(mut self, base: &str) -> Self {
        self.models = format!("{base}/models");
        self.responses = format!("{base}/responses");
        self.http = Client::builder()
            .redirect(Policy::none())
            .retry(reqwest::retry::never())
            .timeout(Duration::from_secs(3))
            .build()
            .unwrap();
        self
    }
}
async fn body(mut response: Response) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| fail("could not read OpenAI response"))?
    {
        let count = chunk.len().min(crate::MAX_BODY as usize + 1 - bytes.len());
        bytes.extend_from_slice(&chunk[..count]);
        if bytes.len() as u64 > crate::MAX_BODY {
            break;
        }
    }
    Ok(bytes)
}
async fn checked(response: Response) -> Result<Response> {
    if response.status().is_success() {
        return Ok(response);
    }
    let status = response.status().as_u16();
    let headers = response.headers().clone();
    let bytes = body(response).await;
    Err(protocol::http_error(
        status,
        &headers,
        bytes.as_deref().ok(),
    ))
}
impl AsyncModelProvider for AsyncOpenAIProvider {
    fn capability(&self) -> &str {
        &self.capability
    }
    fn supports_model(&self, model: &str) -> bool {
        model == self.model
    }
    async fn validate_request(&mut self, request: &ModelRequest) -> renraku_vm::error::Result<()> {
        self.validate(request).await.map_err(vm_error)
    }
    async fn submit(&mut self, request: &ModelRequest) -> renraku_vm::error::Result<Vec<u16>> {
        self.submit_once(request).await.map_err(vm_error)
    }
    async fn reconcile(
        &mut self,
        request: &ModelRequest,
    ) -> renraku_vm::error::Result<Option<Vec<u16>>> {
        let (bytes, _) =
            protocol::request_body(&self.capability, &self.model, request).map_err(vm_error)?;
        self.lookup(request, bytes).await.map_err(vm_error)
    }
}
