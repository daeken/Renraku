use crate::{
    AuthClient, Identity, Result, fail, now, protocol,
    receipts::Receipts,
    require,
    storage::{Profile, Store},
    stream,
};
use renraku_vm::{effects::ModelProvider, model::ModelRequest};
use reqwest::{
    blocking::{Client, Response},
    redirect::Policy,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    io::Read,
    path::{Path, PathBuf},
    time::Duration,
};
use uuid::Uuid;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
pub struct AvailableModel {
    pub slug: String,
    pub display_name: String,
}
#[derive(Clone, Serialize, Eq, PartialEq)]
pub(crate) struct Binding {
    id: Uuid,
    issuer: String,
    subject: String,
    client: String,
}
impl Binding {
    fn from_profile(profile: &Profile) -> Result<Self> {
        let Identity {
            issuer, subject, ..
        } = profile
            .identity
            .as_ref()
            .ok_or_else(|| fail("profile has no verified identity"))?;
        Ok(Self {
            id: profile.id,
            issuer: issuer.clone(),
            subject: subject.clone(),
            client: profile
                .client_id
                .clone()
                .ok_or_else(|| fail("profile has no client registration"))?,
        })
    }
    pub(crate) fn load(directory: &Path, id: Uuid) -> Result<Self> {
        let store = Store::open(directory)?;
        let guard = store.lock()?;
        Self::from_profile(
            guard
                .vault
                .profiles
                .iter()
                .find(|p| p.id == id)
                .ok_or_else(|| fail("authentication profile does not exist"))?,
        )
    }
    pub(crate) fn bytes(&self) -> Result<Vec<u8>> {
        serde_json::to_vec(self).map_err(|_| fail("could not encode profile binding"))
    }
    pub(crate) fn bearer(&self, directory: &Path) -> Result<String> {
        let store = Store::open(directory)?;
        for refresh in [false, true] {
            let guard = store.lock()?;
            let profile = guard
                .vault
                .profiles
                .iter()
                .find(|p| p.id == self.id)
                .ok_or_else(|| fail("bound authentication profile was removed"))?;
            require(
                Self::from_profile(profile)? == *self,
                "bound authentication identity or registration changed",
            )?;
            require(
                !profile.refresh_pending,
                "previous refresh outcome is unknown; sign in again",
            )?;
            let session = profile
                .session
                .as_ref()
                .ok_or_else(|| fail("bound profile is signed out"))?;
            require(
                session.scopes.contains("resource.invoke")
                    && session.scopes.contains("chatgpt.tokens.use.direct"),
                "bound profile has not granted ChatGPT plan usage",
            )?;
            let token = session
                .access_token
                .as_ref()
                .ok_or_else(|| fail("bound profile has no access token"))?;
            if session.expires_at > now()?.saturating_add(30) {
                return Ok(token.clone());
            }
            require(
                !refresh,
                "refreshed access token expires too soon; sign in again",
            )?;
            drop(guard);
            AuthClient::open(directory)?.refresh_profile(self.id)?;
        }
        Err(fail("bound profile requires sign-in"))
    }
}
struct Transport {
    http: Client,
    models: String,
    responses: String,
}
impl Transport {
    fn production() -> Result<Self> {
        let http = Client::builder()
            .redirect(Policy::none())
            .retry(reqwest::retry::never())
            .timeout(Duration::from_secs(120))
            .connect_timeout(Duration::from_secs(10))
            .user_agent("Renraku/0.1.0")
            .build()
            .map_err(|_| fail("could not initialize inference HTTPS client"))?;
        Ok(Self {
            http,
            models: "https://api.openai.com/v1/models".into(),
            responses: "https://api.openai.com/v1/responses".into(),
        })
    }
    fn models(&self, bearer: &str) -> Result<Vec<AvailableModel>> {
        let response = self
            .http
            .get(&self.models)
            .bearer_auth(bearer)
            .send()
            .map_err(|_| fail("OpenAI model discovery failed"))?;
        protocol::parse_models(&read_body(checked(response)?)?)
    }
}
fn read_body(response: Response) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    response
        .take(crate::MAX_BODY + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| fail("could not read OpenAI response"))?;
    Ok(bytes)
}
fn checked(response: Response) -> Result<Response> {
    if response.status().is_success() {
        return Ok(response);
    }
    let status = response.status().as_u16();
    let headers = response.headers().clone();
    let body = read_body(response);
    Err(protocol::http_error(status, &headers, body.as_deref().ok()))
}
/// Discover the explicitly selected account's displayable models, in server order.
pub fn models(directory: impl AsRef<Path>, profile: Uuid) -> Result<Vec<AvailableModel>> {
    let directory = directory.as_ref();
    let binding = Binding::load(directory, profile)?;
    Transport::production()?.models(&binding.bearer(directory)?)
}
/// Host-owned adapter. Profile ID and model are explicit; active selection never
/// retargets pending requests. Construction and receipt reconciliation are offline.
pub struct OpenAIProvider {
    auth_directory: PathBuf,
    binding: Binding,
    capability: String,
    model: String,
    transport: Transport,
    receipts: Receipts,
    validated: Option<Vec<u8>>,
}
impl OpenAIProvider {
    pub fn capability_for(profile: Uuid) -> String {
        format!("openai.chatgpt.{profile}")
    }
    pub fn open(
        auth_directory: impl AsRef<Path>,
        receipt_directory: impl AsRef<Path>,
        profile: Uuid,
        model: &str,
    ) -> Result<Self> {
        require(protocol::valid_model(model), "invalid OpenAI model slug")?;
        Ok(Self {
            auth_directory: auth_directory.as_ref().to_path_buf(),
            binding: Binding::load(auth_directory.as_ref(), profile)?,
            capability: Self::capability_for(profile),
            model: model.into(),
            transport: Transport::production()?,
            receipts: Receipts::open(receipt_directory.as_ref())?,
            validated: None,
        })
    }
    fn request(&self, request: &ModelRequest) -> Result<(Vec<u8>, Value)> {
        protocol::request_body(&self.capability, &self.model, request)
    }
    fn validate(&mut self, request: &ModelRequest) -> Result<()> {
        let (bytes, _) = self.request(request)?;
        let token = self.binding.bearer(&self.auth_directory)?;
        require(
            self.transport
                .models(&token)?
                .iter()
                .any(|m| m.slug == self.model),
            "saved model is not available in the bound account's model catalog",
        )?;
        self.validated = Some(bytes);
        Ok(())
    }
    fn submit_once(&mut self, request: &ModelRequest) -> Result<Vec<u16>> {
        let (bytes, body) = self.request(request)?;
        let binding = self.binding.bytes()?;
        if let Some(response) = self.receipts.reconcile(request.id, &binding, &bytes)? {
            return Ok(response);
        }
        if self.validated.as_ref() != Some(&bytes) {
            self.validate(request)?;
        }
        let token = self.binding.bearer(&self.auth_directory)?;
        if let Some(response) = self.receipts.reserve(request.id, &binding, &bytes)? {
            return Ok(response);
        }
        // No retries, redirects, or remote GET reconciliation. The reservation
        // remains durable even when connection/stream/completion saving fails.
        let response = self
            .transport
            .http
            .post(&self.transport.responses)
            .bearer_auth(token)
            .header("Accept", "text/event-stream")
            .json(&body)
            .send()
            .map_err(|_| fail("Responses request failed; outcome is unknown"))?;
        let response = checked(response)?;
        if !protocol::accepts_sse(response.headers()) {
            let status = response.status().as_u16();
            let headers = response.headers().clone();
            let body = read_body(response);
            return Err(protocol::unexpected_transport(
                status,
                &headers,
                body.as_deref().ok(),
            ));
        }
        let completion = stream::completed(response)?;
        self.receipts
            .complete(request.id, &completion.id, &completion.text)?;
        Ok(completion.text)
    }
}
pub(crate) fn vm_error(error: crate::Error) -> renraku_vm::error::Error {
    renraku_vm::error::Error(error.to_string())
}
impl ModelProvider for OpenAIProvider {
    fn capability(&self) -> &str {
        &self.capability
    }
    fn supports_model(&self, model: &str) -> bool {
        model == self.model
    }
    fn validate_request(&mut self, request: &ModelRequest) -> renraku_vm::error::Result<()> {
        self.validate(request).map_err(vm_error)
    }
    fn submit(&mut self, request: &ModelRequest) -> renraku_vm::error::Result<Vec<u16>> {
        self.submit_once(request).map_err(vm_error)
    }
    fn reconcile(&mut self, request: &ModelRequest) -> renraku_vm::error::Result<Option<Vec<u16>>> {
        let (bytes, _) = self.request(request).map_err(vm_error)?;
        self.receipts
            .reconcile(request.id, &self.binding.bytes().map_err(vm_error)?, &bytes)
            .map_err(vm_error)
    }
}

#[cfg(test)]
mod tests;
