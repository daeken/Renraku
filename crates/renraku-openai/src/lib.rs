//! Host-only ChatGPT subscription authentication. No tokens enter guest state.
mod callback;
mod storage;

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode, decode_header, jwk::JwkSet};
use reqwest::{
    blocking::{Client, Response},
    redirect::Policy,
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeSet,
    io::Read,
    path::Path,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use storage::{Guard, Profile, Session, Store};
use url::Url;
use uuid::Uuid;

const ISSUER: &str = "https://auth.openai.com";
const DISCOVERY: &str = "https://auth.openai.com/.well-known/openid-configuration";
const RESOURCE: &str = "https://api.openai.com/v1";
const SCOPES: &str =
    "openid profile email offline_access resource.invoke chatgpt.tokens.use.direct";
const DYNAMIC_CLIENT: &str = "dynamic_agent_client";
const MAX_BODY: u64 = 1024 * 1024;

#[derive(Debug)]
pub struct Error(String);
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for Error {}
pub type Result<T> = std::result::Result<T, Error>;
fn fail(message: &str) -> Error {
    Error(message.into())
}
fn require(condition: bool, message: &str) -> Result<()> {
    if condition {
        Ok(())
    } else {
        Err(fail(message))
    }
}
fn now() -> Result<u64> {
    Ok(SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| fail("system clock is before the Unix epoch"))?
        .as_secs())
}
fn random() -> Result<String> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).map_err(|_| fail("system random source unavailable"))?;
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}
fn valid_client(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 512
        && value != DYNAMIC_CLIENT
        && value.trim() == value
        && value.chars().all(|c| !c.is_control())
}

#[derive(Clone, Debug, Serialize, Deserialize, Eq, PartialEq)]
pub struct Identity {
    pub issuer: String,
    pub subject: String,
    pub email: Option<String>,
}
#[derive(Clone, Debug)]
pub struct ProfileSummary {
    pub id: Uuid,
    pub active: bool,
    pub registered: bool,
    pub identity: Option<Identity>,
    pub signed_in: bool,
    pub plan_usage: bool,
    pub expires_at: Option<u64>,
    pub needs_sign_in: bool,
}
impl Profile {
    fn summary(&self, active: Option<Uuid>) -> ProfileSummary {
        ProfileSummary {
            id: self.id,
            active: active == Some(self.id),
            registered: self.client_id.is_some(),
            identity: self.identity.clone(),
            signed_in: self.session.is_some() && !self.refresh_pending,
            plan_usage: self.session.as_ref().is_some_and(|s| {
                s.access_token.is_some()
                    && s.scopes.contains("chatgpt.tokens.use.direct")
                    && s.scopes.contains("resource.invoke")
            }) && !self.refresh_pending,
            expires_at: self.session.as_ref().map(|s| s.expires_at),
            needs_sign_in: self.refresh_pending,
        }
    }
}

/// Read metadata only. Does not contact OpenAI or expose credentials.
pub fn profiles(directory: impl AsRef<Path>) -> Result<Vec<ProfileSummary>> {
    let store = Store::open(directory.as_ref())?;
    let guard = store.lock()?;
    Ok(guard
        .vault
        .profiles
        .iter()
        .map(|p| p.summary(guard.vault.active))
        .collect())
}
pub fn select_profile(directory: impl AsRef<Path>, id: Uuid) -> Result<ProfileSummary> {
    let store = Store::open(directory.as_ref())?;
    let mut guard = store.lock()?;
    let index = guard
        .vault
        .profiles
        .iter()
        .position(|p| p.id == id && p.identity.is_some())
        .ok_or_else(|| fail("verified profile does not exist"))?;
    guard.vault.active = Some(id);
    guard.save()?;
    Ok(guard.vault.profiles[index].summary(guard.vault.active))
}
pub fn logout(directory: impl AsRef<Path>) -> Result<bool> {
    match AuthClient::open(directory.as_ref()) {
        Ok(client) => client.logout(),
        Err(_) => {
            let store = Store::open(directory.as_ref())?;
            let mut guard = store.lock()?;
            let index = active(&guard)?;
            guard.vault.profiles[index].session = None;
            guard.vault.profiles[index].refresh_pending = false;
            guard.save()?;
            Ok(false)
        }
    }
}

#[derive(Deserialize)]
struct Discovery {
    issuer: String,
    authorization_endpoint: Url,
    token_endpoint: Url,
    jwks_uri: Url,
    revocation_endpoint: Option<Url>,
}
#[derive(Deserialize)]
struct TokenResponse {
    access_token: Option<String>,
    refresh_token: Option<String>,
    id_token: Option<String>,
    token_type: Option<String>,
    expires_in: Option<u64>,
    scope: Option<String>,
}
#[derive(Deserialize)]
struct Claims {
    sub: String,
    iss: String,
    exp: u64,
    iat: u64,
    aud: serde_json::Value,
    azp: Option<String>,
    nonce: Option<String>,
    email: Option<String>,
}

pub struct AuthClient {
    http: Client,
    discovery: Discovery,
    store: Store,
}
impl AuthClient {
    pub fn open(directory: impl AsRef<Path>) -> Result<Self> {
        let http = Self::http()?;
        let discovery: Discovery = json(
            http.get(DISCOVERY)
                .send()
                .map_err(|_| fail("OpenAI discovery request failed"))?,
        )?;
        require(discovery.issuer == ISSUER, "unexpected OpenAI issuer")?;
        for endpoint in [
            &discovery.authorization_endpoint,
            &discovery.token_endpoint,
            &discovery.jwks_uri,
        ]
        .into_iter()
        .chain(discovery.revocation_endpoint.iter())
        {
            require(
                endpoint.scheme() == "https"
                    && endpoint.host_str() == Some("auth.openai.com")
                    && endpoint.port_or_known_default() == Some(443)
                    && endpoint.username().is_empty()
                    && endpoint.password().is_none()
                    && endpoint.query().is_none()
                    && endpoint.fragment().is_none(),
                "untrusted OpenAI authentication endpoint",
            )?;
        }
        Ok(Self {
            http,
            discovery,
            store: Store::open(directory.as_ref())?,
        })
    }
    fn http() -> Result<Client> {
        Client::builder()
            .redirect(Policy::none())
            .retry(reqwest::retry::never())
            .timeout(Duration::from_secs(30))
            .connect_timeout(Duration::from_secs(10))
            .user_agent("Renraku/0.1.0")
            .build()
            .map_err(|_| fail("could not initialize HTTPS client"))
    }
    /// The callback receives a public authorization URL; it must open that URL
    /// in the system browser. Credentials and ID-token hints are never included.
    pub fn login(
        &self,
        new_profile: bool,
        timeout: Duration,
        open_browser: impl FnOnce(&Url) -> Result<()>,
    ) -> Result<ProfileSummary> {
        require(
            !timeout.is_zero() && timeout <= Duration::from_secs(600),
            "sign-in timeout must be between 1 and 600 seconds",
        )?;
        let mut guard = self.store.lock()?;
        let selected = if new_profile {
            None
        } else {
            guard
                .vault
                .active
                .or_else(|| guard.vault.profiles.last().map(|p| p.id))
        };
        let index = if let Some(selected) = selected {
            guard
                .vault
                .profiles
                .iter()
                .position(|p| p.id == selected)
                .ok_or_else(|| fail("selected profile is missing"))?
        } else {
            guard.vault.profiles.push(Profile {
                id: Uuid::new_v4(),
                client_id: None,
                identity: None,
                session: None,
                refresh_pending: false,
            });
            guard.vault.profiles.len() - 1
        };
        guard.save()?; // Keep the host and registration IDs stable across attempts.
        let state = random()?;
        let nonce = random()?;
        let verifier = random()?;
        let listener = callback::Listener::bind()?;
        let redirect = listener.redirect_uri();
        let saved_client = guard.vault.profiles[index].client_id.clone();
        let mut authorization = self.discovery.authorization_endpoint.clone();
        {
            let mut query = authorization.query_pairs_mut();
            query.append_pair(
                "client_id",
                saved_client.as_deref().unwrap_or(DYNAMIC_CLIENT),
            );
            if saved_client.is_none() {
                query.append_pair("agent_name_hint", "Renraku");
            }
            query.append_pair("ext_agent_host_id", &guard.vault.host_id);
            query
                .append_pair("response_type", "code")
                .append_pair("redirect_uri", &redirect)
                .append_pair("scope", SCOPES)
                .append_pair("resource", RESOURCE)
                .append_pair("state", &state)
                .append_pair("nonce", &nonce)
                .append_pair("code_challenge_method", "S256")
                .append_pair(
                    "code_challenge",
                    &URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes())),
                );
        }
        open_browser(&authorization)?;
        let returned = listener.wait(&state, timeout)?;
        let client_id = match (saved_client, returned.client_id) {
            (Some(saved), Some(returned)) => {
                require(
                    saved == returned,
                    "sign-in returned a different client registration",
                )?;
                saved
            }
            (Some(saved), None) => saved,
            (None, Some(issued)) => {
                require(
                    valid_client(&issued),
                    "sign-in returned an invalid client registration",
                )?;
                issued
            }
            (None, None) => {
                return Err(fail("sign-in did not return an issued client registration"));
            }
        };
        guard.vault.profiles[index].client_id = Some(client_id.clone());
        guard.save()?; // An expired code must not erase a newly issued client ID.
        let response = self.token(&[
            ("grant_type", "authorization_code"),
            ("client_id", &client_id),
            ("code", &returned.code),
            ("code_verifier", &verifier),
            ("redirect_uri", &redirect),
            ("resource", RESOURCE),
        ])?;
        let id_token = response
            .id_token
            .as_ref()
            .ok_or_else(|| fail("sign-in did not return an ID token"))?;
        let identity = self.verify(id_token, &client_id, Some(&nonce))?;
        if let Some(expected) = &guard.vault.profiles[index].identity {
            require(
                expected.issuer == identity.issuer && expected.subject == identity.subject,
                "sign-in returned a different account for this registration",
            )?;
        }
        let session = session(response, None)?;
        let profile_id = guard.vault.profiles[index].id;
        guard.vault.profiles[index].identity = Some(identity);
        guard.vault.profiles[index].session = Some(session);
        guard.vault.profiles[index].refresh_pending = false;
        guard.vault.active = Some(profile_id);
        guard.save()?;
        Ok(guard.vault.profiles[index].summary(guard.vault.active))
    }
    fn token(&self, form: &[(&str, &str)]) -> Result<TokenResponse> {
        let response = self
            .http
            .post(self.discovery.token_endpoint.clone())
            .form(form)
            .send()
            .map_err(|_| fail("token request failed; sign in again if refresh was interrupted"))?;
        if !response.status().is_success() {
            // Do not reflect token responses or error_description into diagnostics.
            return Err(fail("token request was rejected; sign in again"));
        }
        json(response)
    }
    fn verify(&self, token: &str, client_id: &str, nonce: Option<&str>) -> Result<Identity> {
        require(token.len() <= 65536, "ID token exceeds its size limit")?;
        let header = decode_header(token).map_err(|_| fail("invalid ID token header"))?;
        require(
            matches!(header.alg, Algorithm::RS256 | Algorithm::ES256),
            "unsupported ID token algorithm",
        )?;
        let kid = header
            .kid
            .ok_or_else(|| fail("ID token has no signing key ID"))?;
        let keys: JwkSet = json(
            self.http
                .get(self.discovery.jwks_uri.clone())
                .send()
                .map_err(|_| fail("signing key request failed"))?,
        )?;
        let matches: Vec<_> = keys
            .keys
            .iter()
            .filter(|key| key.common.key_id.as_ref() == Some(&kid))
            .collect();
        require(
            matches.len() == 1,
            "ID token signing key is missing or ambiguous",
        )?;
        let common = &matches[0].common;
        require(
            common
                .public_key_use
                .as_ref()
                .is_none_or(|usage| *usage == jsonwebtoken::jwk::PublicKeyUse::Signature)
                && common.key_operations.as_ref().is_none_or(|operations| {
                    operations.contains(&jsonwebtoken::jwk::KeyOperations::Verify)
                })
                && common
                    .key_algorithm
                    .is_none_or(|algorithm| algorithm == header.alg.into()),
            "ID token signing key does not permit this algorithm or use",
        )?;
        let key =
            DecodingKey::from_jwk(matches[0]).map_err(|_| fail("invalid ID token signing key"))?;
        let mut validation = Validation::new(header.alg);
        validation.set_issuer(&[&self.discovery.issuer]);
        validation.set_audience(&[client_id]);
        validation.set_required_spec_claims(&["iss", "sub", "aud", "exp", "iat"]);
        validation.leeway = 5;
        validation.validate_nbf = true;
        let claims = decode::<Claims>(token, &key, &validation)
            .map_err(|_| fail("ID token signature or claims were rejected"))?
            .claims;
        require(
            !claims.sub.is_empty()
                && claims.sub.len() <= 1024
                && claims.iat <= now()?.saturating_add(5)
                && claims.exp >= claims.iat,
            "invalid ID token subject or issued time",
        )?;
        if let Some(nonce) = nonce {
            require(
                claims
                    .nonce
                    .as_deref()
                    .is_some_and(|value| callback::equal(value, nonce)),
                "ID token nonce did not match",
            )?;
        }
        if let Some(azp) = &claims.azp {
            require(azp == client_id, "ID token authorized party did not match")?;
        }
        if claims.aud.as_array().is_some_and(|values| values.len() > 1) {
            require(
                claims.azp.as_deref() == Some(client_id),
                "multi-audience ID token has no matching authorized party",
            )?;
        }
        Ok(Identity {
            issuer: claims.iss,
            subject: claims.sub,
            email: claims.email,
        })
    }
    /// Refreshes are serialized. A durable pending marker prevents reuse of a
    /// rotating refresh token after a crash or uncertain network outcome.
    pub fn refresh(&self) -> Result<ProfileSummary> {
        let mut guard = self.store.lock()?;
        let index = active(&guard)?;
        require(
            !guard.vault.profiles[index].refresh_pending,
            "previous refresh outcome is unknown; sign in again",
        )?;
        let profile = &guard.vault.profiles[index];
        let client_id = profile
            .client_id
            .clone()
            .ok_or_else(|| fail("profile has no issued client registration"))?;
        let old = profile
            .session
            .clone()
            .ok_or_else(|| fail("profile is signed out"))?;
        let refresh = old
            .refresh_token
            .as_deref()
            .ok_or_else(|| fail("profile has no refresh token; sign in again"))?;
        let expected = profile
            .identity
            .clone()
            .ok_or_else(|| fail("profile has no verified identity"))?;
        require(
            expected.issuer == self.discovery.issuer,
            "profile belongs to a different issuer",
        )?;
        guard.vault.profiles[index].refresh_pending = true;
        guard.save()?;
        let response = self.token(&[
            ("grant_type", "refresh_token"),
            ("client_id", &client_id),
            ("refresh_token", refresh),
            ("resource", RESOURCE),
        ])?;
        require(
            response.access_token.is_some(),
            "refresh did not return an access token; sign in again",
        )?;
        if let Some(token) = &response.id_token {
            let identity = self.verify(token, &client_id, None)?;
            require(
                identity.issuer == expected.issuer && identity.subject == expected.subject,
                "refresh returned a different account",
            )?;
        }
        guard.vault.profiles[index].session = Some(session(response, Some(&old))?);
        guard.vault.profiles[index].refresh_pending = false;
        guard.save()?;
        Ok(guard.vault.profiles[index].summary(guard.vault.active))
    }
    /// Clears local credentials even when remote revocation cannot be confirmed.
    /// Returns true only when the renewable session was confirmed revoked.
    pub fn logout(&self) -> Result<bool> {
        let mut guard = self.store.lock()?;
        let index = active(&guard)?;
        let profile = &guard.vault.profiles[index];
        let client_id = profile.client_id.clone();
        let refresh_token = profile
            .session
            .as_ref()
            .and_then(|s| s.refresh_token.clone());
        // Publish local sign-out before contacting the revocation service.
        guard.vault.profiles[index].session = None;
        guard.vault.profiles[index].refresh_pending = false;
        guard.save()?;
        let confirmed = match (
            &self.discovery.revocation_endpoint,
            &client_id,
            &refresh_token,
        ) {
            (Some(endpoint), Some(client), Some(refresh)) => self
                .http
                .post(endpoint.clone())
                .form(&[
                    ("token", refresh.as_str()),
                    ("token_type_hint", "refresh_token"),
                    ("client_id", client.as_str()),
                ])
                .send()
                .is_ok_and(|r| r.status().as_u16() == 200),
            _ => false,
        };
        Ok(confirmed)
    }
}
fn active(guard: &Guard<'_>) -> Result<usize> {
    let selected = guard
        .vault
        .active
        .ok_or_else(|| fail("no active sign-in profile"))?;
    guard
        .vault
        .profiles
        .iter()
        .position(|p| p.id == selected)
        .ok_or_else(|| fail("active profile is missing"))
}
fn session(response: TokenResponse, old: Option<&Session>) -> Result<Session> {
    for token in [
        &response.access_token,
        &response.refresh_token,
        &response.id_token,
    ]
    .into_iter()
    .flatten()
    {
        require(
            !token.is_empty() && token.len() <= 65536,
            "invalid token length",
        )?;
    }
    if response.access_token.is_some() {
        require(
            response
                .token_type
                .as_deref()
                .is_some_and(|t| t.eq_ignore_ascii_case("Bearer")),
            "token response is not a Bearer token",
        )?;
    }
    let scope = response
        .scope
        .map(|s| {
            s.split_whitespace()
                .map(String::from)
                .collect::<BTreeSet<_>>()
        })
        .or_else(|| old.map(|s| s.scopes.clone()))
        .ok_or_else(|| fail("sign-in did not report granted scopes"))?;
    require(
        scope.len() <= 64 && scope.iter().all(|s| s.len() <= 256),
        "invalid granted scopes",
    )?;
    let lifetime = response
        .expires_in
        .ok_or_else(|| fail("token response has no expiry"))?;
    require(lifetime > 0, "token response is already expired")?;
    Ok(Session {
        access_token: response.access_token,
        refresh_token: response
            .refresh_token
            .or_else(|| old.and_then(|s| s.refresh_token.clone())),
        id_token: response
            .id_token
            .or_else(|| old.and_then(|s| s.id_token.clone())),
        scopes: scope,
        expires_at: now()?
            .checked_add(lifetime)
            .ok_or_else(|| fail("token expiry overflow"))?,
    })
}
fn json<T: DeserializeOwned>(response: Response) -> Result<T> {
    require(
        response.status().is_success(),
        "authentication service returned an error",
    )?;
    let mut bytes = Vec::new();
    response
        .take(MAX_BODY + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| fail("could not read authentication response"))?;
    require(
        bytes.len() as u64 <= MAX_BODY,
        "authentication response exceeds its size limit",
    )?;
    serde_json::from_slice(&bytes).map_err(|_| fail("invalid authentication response"))
}

#[cfg(test)]
mod tests;
