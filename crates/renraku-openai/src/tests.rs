use super::*;
use jsonwebtoken::{EncodingKey, Header, encode};
use rsa::{pkcs8::EncodePrivateKey, traits::PublicKeyParts};
use serde_json::{Value, json};
use std::{
    fs,
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
};

struct Directory(PathBuf);
impl Directory {
    fn new() -> Self {
        let directory = std::env::temp_dir().join(format!("renraku-auth-test-{}", Uuid::new_v4()));
        fs::create_dir(&directory).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&directory, fs::Permissions::from_mode(0o700)).unwrap();
        }
        Self(directory)
    }
}
impl Drop for Directory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
#[derive(Default)]
struct State {
    nonce: String,
    challenge: String,
    redirect: String,
    subject: String,
    client: String,
    mode: String,
    scope: Option<String>,
    token_requests: Vec<BTreeMap<String, String>>,
    revoked: usize,
    revoke_fails: bool,
    vault_path: Option<PathBuf>,
}
use std::collections::BTreeMap;
fn test_keys() -> &'static (String, Value) {
    static KEYS: std::sync::OnceLock<(String, Value)> = std::sync::OnceLock::new();
    KEYS.get_or_init(|| {
        let key = rsa::RsaPrivateKey::new(&mut rand::rngs::OsRng, 2048).unwrap();
        let private = key.to_pkcs8_pem(rsa::pkcs8::LineEnding::LF).unwrap().to_string();
        let public = json!({"keys":[{"kty":"RSA","use":"sig","alg":"RS256","kid":"test-key","n":URL_SAFE_NO_PAD.encode(key.n().to_bytes_be()),"e":URL_SAFE_NO_PAD.encode(key.e().to_bytes_be())}]});
        (private, public)
    })
}
struct Server {
    base: Url,
    state: Arc<Mutex<State>>,
    stop: Arc<AtomicBool>,
    pause: Arc<AtomicBool>,
    arrived: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
}
impl Server {
    fn new() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let base: Url = format!("http://{}/", listener.local_addr().unwrap())
            .parse()
            .unwrap();
        let state = Arc::new(Mutex::new(State {
            subject: "test-user".into(),
            client: "oaiapp_test".into(),
            ..State::default()
        }));
        let stop = Arc::new(AtomicBool::new(false));
        let worker_state = Arc::clone(&state);
        let worker_stop = Arc::clone(&stop);
        let pause = Arc::new(AtomicBool::new(false));
        let arrived = Arc::new(AtomicBool::new(false));
        let worker_pause = Arc::clone(&pause);
        let worker_arrived = Arc::clone(&arrived);
        let worker = thread::spawn(move || {
            while !worker_stop.load(Ordering::Relaxed) {
                let (mut socket, _) = match listener.accept() {
                    Ok(value) => value,
                    Err(_) => {
                        thread::sleep(Duration::from_millis(2));
                        continue;
                    }
                };
                socket.set_nonblocking(false).unwrap();
                socket
                    .set_read_timeout(Some(Duration::from_secs(2)))
                    .unwrap();
                let mut bytes = Vec::new();
                let mut buffer = [0u8; 2048];
                let header = loop {
                    let count = socket.read(&mut buffer).unwrap();
                    if count == 0 {
                        break String::new();
                    }
                    bytes.extend_from_slice(&buffer[..count]);
                    if let Some(end) = bytes.windows(4).position(|v| v == b"\r\n\r\n") {
                        break String::from_utf8(bytes[..end + 4].to_vec()).unwrap();
                    }
                };
                if header.is_empty() {
                    continue;
                }
                let end = header.len();
                let length = header
                    .lines()
                    .filter_map(|l| l.split_once(':'))
                    .find(|(key, _)| key.eq_ignore_ascii_case("content-length"))
                    .map(|(_, v)| v.trim().parse::<usize>().unwrap())
                    .unwrap_or(0);
                while bytes.len() < end + length {
                    let count = socket.read(&mut buffer).unwrap();
                    if count == 0 {
                        break;
                    }
                    bytes.extend_from_slice(&buffer[..count]);
                }
                let mut state = worker_state.lock().unwrap();
                let path = header
                    .lines()
                    .next()
                    .unwrap()
                    .split_whitespace()
                    .nth(1)
                    .unwrap();
                let mut status = "200 OK";
                let body = match path {
                    "/jwks" => {
                        let mut keys = test_keys().1.clone();
                        if state.mode == "bad-key-use" {
                            keys["keys"][0]["use"] = "enc".into();
                        }
                        keys.to_string()
                    }
                    "/token" => {
                        let form: BTreeMap<_, _> =
                            url::form_urlencoded::parse(&bytes[end..end + length])
                                .map(|(k, v)| (k.into_owned(), v.into_owned()))
                                .collect();
                        state.token_requests.push(form.clone());
                        let refreshing = form["grant_type"] == "refresh_token";
                        if state.mode == "reject-code" {
                            status = "400 Bad Request";
                            json!({"error":"invalid_grant", "error_description":"DO_NOT_LOG_TOKEN"})
                                .to_string()
                        } else if refreshing && state.mode == "park-refresh" {
                            worker_arrived.store(true, Ordering::SeqCst);
                            drop(state);
                            while worker_pause.load(Ordering::SeqCst)
                                && !worker_stop.load(Ordering::Relaxed)
                            {
                                thread::sleep(Duration::from_millis(2));
                            }
                            continue;
                        } else if refreshing && state.mode == "drop-refresh" {
                            continue;
                        } else {
                            assert_eq!(form["client_id"], state.client);
                            assert_eq!(form["resource"], RESOURCE);
                            if !refreshing {
                                assert_eq!(form["redirect_uri"], state.redirect);
                                assert_eq!(
                                    URL_SAFE_NO_PAD
                                        .encode(Sha256::digest(form["code_verifier"].as_bytes())),
                                    state.challenge
                                );
                            } else {
                                assert!(!form.contains_key("scope"));
                            }
                            let mut claims = json!({"iss": ISSUER, "sub": state.subject, "aud": state.client, "iat": now().unwrap(), "exp": now().unwrap()+3600, "nonce": state.nonce, "email":"test@example.invalid"});
                            let mut header = Header::new(Algorithm::RS256);
                            header.kid = Some("test-key".into());
                            match state.mode.as_str() {
                                "bad-nonce" => claims["nonce"] = "wrong".into(),
                                "bad-issuer" => claims["iss"] = "https://wrong.invalid".into(),
                                "bad-audience" => claims["aud"] = "different-client".into(),
                                "expired" => {
                                    claims["iat"] = (now().unwrap() - 8000).into();
                                    claims["exp"] = (now().unwrap() - 7000).into();
                                }
                                "future-iat" => claims["iat"] = (now().unwrap() + 500).into(),
                                "empty-sub" => claims["sub"] = "".into(),
                                "bad-kid" => header.kid = Some("unknown".into()),
                                "multiple-audiences" => {
                                    claims["aud"] = json!([state.client, "another-client"])
                                }
                                _ => {}
                            }
                            let mut token = encode(
                                &header,
                                &claims,
                                &EncodingKey::from_rsa_pem(test_keys().0.as_bytes()).unwrap(),
                            )
                            .unwrap();
                            if state.mode == "bad-signature" {
                                token.push('x');
                            }
                            if state.mode == "symmetric-algorithm" {
                                token = encode(
                                    &Header::default(),
                                    &claims,
                                    &EncodingKey::from_secret(b"test-only"),
                                )
                                .unwrap();
                            }
                            json!({"access_token":if refreshing { "access-after-refresh" } else { "access-at-login" }, "refresh_token":if refreshing { "rotated-refresh" } else { "refresh-at-login" }, "id_token":token, "token_type":"Bearer", "expires_in":3600, "scope":state.scope.as_deref().unwrap_or(SCOPES)}).to_string()
                        }
                    }
                    "/revoke" => {
                        let vault: Value = serde_json::from_slice(
                            &fs::read(state.vault_path.as_ref().unwrap()).unwrap(),
                        )
                        .unwrap();
                        let active = &vault["active"];
                        let profile = vault["profiles"]
                            .as_array()
                            .unwrap()
                            .iter()
                            .find(|p| &p["id"] == active)
                            .unwrap();
                        assert!(
                            profile["session"].is_null(),
                            "local sign-out must precede remote revocation"
                        );
                        state.revoked += 1;
                        if state.revoke_fails {
                            status = "500 Internal Server Error";
                        }
                        String::new()
                    }
                    _ => {
                        status = "404 Not Found";
                        String::new()
                    }
                };
                let response = format!(
                    "HTTP/1.1 {status}\r\nContent-Length: {}\r\nContent-Type: application/json\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = socket.write_all(response.as_bytes());
            }
        });
        Self {
            base,
            state,
            stop,
            pause,
            arrived,
            worker: Some(worker),
        }
    }
    fn client(&self, directory: &Directory) -> AuthClient {
        self.state.lock().unwrap().vault_path = Some(directory.0.join("profiles.json"));
        test_client(&directory.0, &self.base)
    }
    fn browser(&self, url: &Url) -> Result<()> {
        let query: BTreeMap<_, _> = url
            .query_pairs()
            .map(|(k, v)| (k.into_owned(), v.into_owned()))
            .collect();
        assert_eq!(query["agent_name_hint"], "Renraku");
        assert_eq!(query["code_challenge_method"], "S256");
        let mut state = self.state.lock().unwrap();
        state.nonce = query["nonce"].clone();
        state.challenge = query["code_challenge"].clone();
        state.redirect = query["redirect_uri"].clone();
        let mut callback: Url = query["redirect_uri"].parse().unwrap();
        callback
            .query_pairs_mut()
            .append_pair("code", "test-code")
            .append_pair("state", &query["state"])
            .append_pair("client_id", &state.client);
        drop(state);
        thread::spawn(move || {
            let _ = callback_get(&callback);
        });
        Ok(())
    }
    fn returning_browser(&self, url: &Url) -> Result<()> {
        assert!(
            !url.query_pairs()
                .any(|(k, _)| k == "agent_name_hint" || k == "id_token_hint")
        );
        let query: BTreeMap<_, _> = url
            .query_pairs()
            .map(|(k, v)| (k.into_owned(), v.into_owned()))
            .collect();
        let mut state = self.state.lock().unwrap();
        assert_eq!(query["client_id"], state.client);
        state.nonce = query["nonce"].clone();
        state.challenge = query["code_challenge"].clone();
        state.redirect = query["redirect_uri"].clone();
        let mut callback: Url = query["redirect_uri"].parse().unwrap();
        callback
            .query_pairs_mut()
            .append_pair("code", "test-code")
            .append_pair("state", &query["state"]);
        drop(state);
        thread::spawn(move || {
            let _ = callback_get(&callback);
        });
        Ok(())
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        let result = self.worker.take().unwrap().join();
        if !thread::panicking() {
            result.unwrap();
        }
    }
}
fn callback_get(url: &Url) -> String {
    let mut stream = TcpStream::connect(("127.0.0.1", url.port().unwrap())).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    write!(
        stream,
        "GET {}?{} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nConnection: close\r\n\r\n",
        url.path(),
        url.query().unwrap(),
        url.port().unwrap()
    )
    .unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).unwrap();
    response
}
fn test_client(directory: &std::path::Path, base: &Url) -> AuthClient {
    AuthClient {
        http: AuthClient::http().unwrap(),
        discovery: Discovery {
            issuer: ISSUER.into(),
            authorization_endpoint: base.join("authorize").unwrap(),
            token_endpoint: base.join("token").unwrap(),
            jwks_uri: base.join("jwks").unwrap(),
            revocation_endpoint: Some(base.join("revoke").unwrap()),
        },
        store: Store::open(directory).unwrap(),
    }
}
fn sign_in(server: &Server, client: &AuthClient) -> ProfileSummary {
    client
        .login(false, Duration::from_secs(5), |url| server.browser(url))
        .unwrap()
}

#[test]
fn browser_flow_verifies_identity_grant_pkce_and_private_atomic_profiles() {
    let directory = Directory::new();
    let server = Server::new();
    let client = server.client(&directory);
    let signed = sign_in(&server, &client);
    assert!(signed.signed_in && signed.plan_usage && signed.active);
    assert_eq!(signed.identity.unwrap().subject, "test-user");
    let status = profiles(&directory.0).unwrap();
    assert_eq!(status[0].id, signed.id);
    let printed = format!("{status:?}");
    assert!(
        !printed.contains("access-at-login")
            && !printed.contains("refresh-at-login")
            && !printed.contains("eyJ")
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        assert_eq!(
            fs::metadata(directory.0.join("profiles.json"))
                .unwrap()
                .mode()
                & 0o777,
            0o600
        );
    }
}

#[test]
fn returning_login_reuses_registration_and_host_but_has_fresh_pkce_and_nonce() {
    let directory = Directory::new();
    let server = Server::new();
    let client = server.client(&directory);
    let first = sign_in(&server, &client);
    let previous = server.state.lock().unwrap().nonce.clone();
    let before: Value =
        serde_json::from_slice(&fs::read(directory.0.join("profiles.json")).unwrap()).unwrap();
    let second = client
        .login(false, Duration::from_secs(5), |url| {
            server.returning_browser(url)
        })
        .unwrap();
    let after: Value =
        serde_json::from_slice(&fs::read(directory.0.join("profiles.json")).unwrap()).unwrap();
    assert_eq!(first.id, second.id);
    assert_eq!(before["host_id"], after["host_id"]);
    assert_ne!(previous, server.state.lock().unwrap().nonce);
}

#[test]
fn invalid_signed_identity_claims_algorithms_and_signatures_never_activate_a_profile() {
    for mode in [
        "bad-nonce",
        "bad-issuer",
        "bad-audience",
        "expired",
        "future-iat",
        "empty-sub",
        "bad-kid",
        "multiple-audiences",
        "bad-signature",
        "symmetric-algorithm",
        "bad-key-use",
    ] {
        let directory = Directory::new();
        let server = Server::new();
        let client = server.client(&directory);
        server.state.lock().unwrap().mode = mode.into();
        let error = client
            .login(false, Duration::from_secs(5), |url| server.browser(url))
            .unwrap_err();
        assert!(!error.to_string().contains("access-at-login"));
        let profiles = profiles(&directory.0).unwrap();
        assert!(profiles.iter().all(|p| !p.signed_in && !p.active), "{mode}");
        assert!(profiles[0].registered); // Retain issued registration even if verification fails.
    }
}

#[test]
fn identity_sign_in_does_not_imply_plan_permission() {
    let directory = Directory::new();
    let server = Server::new();
    let client = server.client(&directory);
    server.state.lock().unwrap().scope = Some("openid profile email".into());
    let profile = sign_in(&server, &client);
    assert!(profile.signed_in);
    assert!(!profile.plan_usage);
}

#[test]
fn callback_rejects_wrong_state_duplicates_and_host_then_accepts_the_bound_request() {
    let listener = callback::Listener::bind().unwrap();
    let url: Url = format!("{}?state=expected&code=ok", listener.redirect_uri())
        .parse()
        .unwrap();
    let worker = thread::spawn(move || {
        let mut wrong = url.clone();
        wrong.set_query(Some("state=wrong&code=secret"));
        assert!(callback_get(&wrong).starts_with("HTTP/1.1 400"));
        wrong.set_query(Some("state=expected&state=expected&code=secret"));
        assert!(callback_get(&wrong).starts_with("HTTP/1.1 400"));
        let mut stream = TcpStream::connect(("127.0.0.1", url.port().unwrap())).unwrap();
        write!(
            stream,
            "GET /auth/callback?state=expected&code=secret HTTP/1.1\r\nHost: evil.invalid\r\n\r\n"
        )
        .unwrap();
        let mut ignored = String::new();
        stream.read_to_string(&mut ignored).unwrap();
        assert!(ignored.starts_with("HTTP/1.1 400"));
        assert!(callback_get(&url).starts_with("HTTP/1.1 200"));
    });
    assert_eq!(
        listener
            .wait("expected", Duration::from_secs(5))
            .unwrap()
            .code,
        "ok"
    );
    worker.join().unwrap();
}

#[test]
fn callback_denial_and_timeout_do_not_exchange_a_code() {
    let directory = Directory::new();
    let server = Server::new();
    let client = server.client(&directory);
    let error = client
        .login(false, Duration::from_secs(2), |url| {
            let query: BTreeMap<_, _> = url.query_pairs().into_owned().collect();
            let mut returned: Url = query["redirect_uri"].parse().unwrap();
            returned
                .query_pairs_mut()
                .append_pair("state", &query["state"])
                .append_pair("error", "access_denied");
            thread::spawn(move || {
                let _ = callback_get(&returned);
            });
            Ok(())
        })
        .unwrap_err();
    assert!(error.to_string().contains("declined"));
    assert!(
        client
            .login(false, Duration::from_millis(30), |_| Ok(()))
            .unwrap_err()
            .to_string()
            .contains("timed out")
    );
    assert!(server.state.lock().unwrap().token_requests.is_empty());
}

#[test]
fn code_exchange_failure_keeps_issued_client_id_and_redacts_server_error_details() {
    let directory = Directory::new();
    let server = Server::new();
    let client = server.client(&directory);
    server.state.lock().unwrap().mode = "reject-code".into();
    let error = client
        .login(false, Duration::from_secs(5), |url| server.browser(url))
        .unwrap_err();
    assert!(!format!("{error:?}").contains("DO_NOT_LOG_TOKEN"));
    assert!(profiles(&directory.0).unwrap()[0].registered);
    server.state.lock().unwrap().mode.clear();
    assert!(
        client
            .login(false, Duration::from_secs(5), |url| server
                .returning_browser(url))
            .unwrap()
            .plan_usage
    );
}

#[test]
fn refresh_rotates_tokens_atomically_and_logout_revokes_then_clears_only_credentials() {
    let directory = Directory::new();
    let server = Server::new();
    let client = server.client(&directory);
    let profile = sign_in(&server, &client);
    assert!(client.refresh().unwrap().plan_usage);
    let vault: Value =
        serde_json::from_slice(&fs::read(directory.0.join("profiles.json")).unwrap()).unwrap();
    assert_eq!(
        vault["profiles"][0]["session"]["refresh_token"],
        "rotated-refresh"
    );
    assert!(!vault["profiles"][0]["refresh_pending"].as_bool().unwrap());
    assert!(client.logout().unwrap());
    assert_eq!(server.state.lock().unwrap().revoked, 1);
    let summary = profiles(&directory.0).unwrap().remove(0);
    assert_eq!(summary.id, profile.id);
    assert!(summary.registered);
    assert!(!summary.signed_in);
    let bytes = fs::read(directory.0.join("profiles.json")).unwrap();
    let text = String::from_utf8(bytes).unwrap();
    assert!(
        !text.contains("rotated-refresh")
            && !text.contains("access-at-login")
            && !text.contains("eyJ")
    );
}

#[test]
fn uncertain_refresh_is_saved_and_never_automatically_repeated() {
    let directory = Directory::new();
    let server = Server::new();
    let client = server.client(&directory);
    sign_in(&server, &client);
    server.state.lock().unwrap().mode = "drop-refresh".into();
    assert!(client.refresh().is_err());
    let requests = server.state.lock().unwrap().token_requests.len();
    assert_eq!(requests, 2);
    let saved = profiles(&directory.0).unwrap().remove(0);
    assert!(saved.needs_sign_in && !saved.signed_in && !saved.plan_usage);
    assert!(
        client
            .refresh()
            .unwrap_err()
            .to_string()
            .contains("unknown")
    );
    assert_eq!(server.state.lock().unwrap().token_requests.len(), requests);
}

#[test]
fn account_mismatch_does_not_overwrite_existing_credentials_and_profiles_remain_separate() {
    let directory = Directory::new();
    let server = Server::new();
    let client = server.client(&directory);
    let first = sign_in(&server, &client);
    server.state.lock().unwrap().subject = "other-user".into();
    assert!(
        client
            .login(false, Duration::from_secs(5), |url| server
                .returning_browser(url))
            .unwrap_err()
            .to_string()
            .contains("different account")
    );
    assert_eq!(
        profiles(&directory.0).unwrap()[0]
            .identity
            .as_ref()
            .unwrap()
            .subject,
        "test-user"
    );
    server.state.lock().unwrap().client = "oaiapp_other".into();
    let second = client
        .login(true, Duration::from_secs(5), |url| server.browser(url))
        .unwrap();
    assert_ne!(first.id, second.id);
    assert_eq!(profiles(&directory.0).unwrap().len(), 2);
    assert!(select_profile(&directory.0, first.id).unwrap().active);
    assert_eq!(
        profiles(&directory.0)
            .unwrap()
            .iter()
            .filter(|p| p.active)
            .count(),
        1
    );
}

#[test]
fn refresh_cannot_switch_account_and_lost_plan_permission_is_respected() {
    let directory = Directory::new();
    let server = Server::new();
    let client = server.client(&directory);
    sign_in(&server, &client);
    server.state.lock().unwrap().scope = Some("openid profile email".into());
    assert!(!client.refresh().unwrap().plan_usage);
    server.state.lock().unwrap().subject = "other-user".into();
    assert!(
        client
            .refresh()
            .unwrap_err()
            .to_string()
            .contains("different account")
    );
    assert!(profiles(&directory.0).unwrap()[0].needs_sign_in);
}

#[test]
fn store_lock_permissions_and_orphan_recovery_protect_credentials() {
    let directory = Directory::new();
    let store = Store::open(&directory.0).unwrap();
    let guard = store.lock().unwrap();
    guard.save().unwrap();
    assert!(store.lock().is_err());
    drop(guard);
    let orphan = directory.0.join(format!(".pending-{}", Uuid::new_v4()));
    fs::write(&orphan, b"test-only-uncommitted-secret").unwrap();
    assert!(store.lock().is_ok());
    assert!(!orphan.exists());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let file = directory.0.join("profiles.json");
        fs::set_permissions(&file, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(store.lock().is_err());
        fs::set_permissions(&file, fs::Permissions::from_mode(0o600)).unwrap();
        fs::remove_file(&file).unwrap();
        std::os::unix::fs::symlink(directory.0.join("auth.lock"), &file).unwrap();
        assert!(store.lock().is_err());
    }
}

#[test]
fn logout_clears_credentials_even_if_revocation_fails() {
    let directory = Directory::new();
    let server = Server::new();
    let client = server.client(&directory);
    sign_in(&server, &client);
    server.state.lock().unwrap().revoke_fails = true;
    assert!(!client.logout().unwrap());
    assert!(!profiles(&directory.0).unwrap()[0].signed_in);
}

#[test]
#[ignore = "launched only by the refresh crash-recovery test"]
fn refresh_process_probe() {
    let directory = PathBuf::from(std::env::var_os("RENRAKU_TEST_AUTH_DIR").unwrap());
    let base = std::env::var("RENRAKU_TEST_AUTH_BASE")
        .unwrap()
        .parse()
        .unwrap();
    test_client(&directory, &base).refresh().unwrap();
}

#[test]
fn sigkill_during_refresh_preserves_the_pending_marker_and_releases_the_store_lock() {
    use std::process::{Command, Stdio};
    use std::time::Instant;
    let directory = Directory::new();
    let server = Server::new();
    let client = server.client(&directory);
    sign_in(&server, &client);
    server.state.lock().unwrap().mode = "park-refresh".into();
    server.pause.store(true, Ordering::SeqCst);
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "tests::refresh_process_probe", "--ignored"])
        .env("RENRAKU_TEST_AUTH_DIR", &directory.0)
        .env("RENRAKU_TEST_AUTH_BASE", server.base.as_str())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while !server.arrived.load(Ordering::SeqCst) && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(5));
    }
    let arrived = server.arrived.load(Ordering::SeqCst);
    let _ = child.kill();
    let status = child.wait().unwrap();
    server.pause.store(false, Ordering::SeqCst);
    assert!(arrived, "refresh process did not reach the server");
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        assert_eq!(status.signal(), Some(9));
    }
    assert!(profiles(&directory.0).unwrap()[0].needs_sign_in);
    let count = server.state.lock().unwrap().token_requests.len();
    assert!(
        client
            .refresh()
            .unwrap_err()
            .to_string()
            .contains("unknown")
    );
    assert_eq!(server.state.lock().unwrap().token_requests.len(), count);
}

#[test]
fn credential_directories_inside_git_checkouts_are_rejected() {
    let directory = Directory::new();
    fs::create_dir(directory.0.join(".git")).unwrap();
    assert!(
        Store::open(&directory.0.join("auth"))
            .err()
            .unwrap()
            .to_string()
            .contains("outside Git")
    );
}
