use super::*;
use crate::{ISSUER, storage::Session};
use renraku_vm::{
    checkpoint::CheckpointStore,
    driver::{DriverLimits, DriverStop},
    effects::EffectState,
    execution::ExecutionStatus,
    identity::EffectId,
    model::ModelMessage,
    runtime::{RunOutcome, Vm},
    value::Value as GuestValue,
};
use std::{
    fs,
    io::{Read, Write},
    net::TcpListener,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
};
#[allow(dead_code)]
#[path = "../../../renraku-vm/tests/common/mod.rs"]
mod common;

struct Directory(PathBuf);
impl Directory {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("renraku-responses-{}", Uuid::new_v4()));
        Store::open(&path).unwrap();
        Self(path)
    }
}
impl Drop for Directory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn profile(directory: &Path, subject: &str, token: &str) -> Uuid {
    let store = Store::open(directory).unwrap();
    let mut guard = store.lock().unwrap();
    let id = Uuid::new_v4();
    guard.vault.profiles.push(Profile {
        id,
        client_id: Some(format!("client_{subject}")),
        identity: Some(Identity {
            issuer: ISSUER.into(),
            subject: subject.into(),
            email: None,
        }),
        session: Some(Session {
            access_token: Some(token.into()),
            refresh_token: None,
            id_token: Some("test-id-token".into()),
            scopes: ["resource.invoke".into(), "chatgpt.tokens.use.direct".into()].into(),
            expires_at: now().unwrap() + 3600,
        }),
        refresh_pending: false,
    });
    guard.vault.active = Some(id);
    guard.save().unwrap();
    id
}
fn completion(text: &str) -> String {
    let response = object!({"type":"response.completed","response":{"id":"resp_test","status":"completed","error":null,
        "output":[{"type":"reasoning"},{"type":"message","role":"assistant","status":"completed","phase":"commentary","content":[{"type":"output_text","text":"provisional"}]},
        {"type":"message","role":"assistant","status":"completed","phase":"final_answer","content":[{"type":"output_text","text":text}]}]}});
    let created = object!({"type":"response.created","response":{"id":"resp_test"}});
    format!(
        ": keepalive\r\nevent: response.created\r\ndata: {created}\r\n\r\nevent: response.completed\r\ndata: {response}\r\n\r\n"
    )
}
#[derive(Clone)]
struct Observed {
    path: String,
    bearer: String,
    body: Value,
}
struct Server {
    base: String,
    requests: Arc<Mutex<Vec<Observed>>>,
    stop: Arc<AtomicBool>,
    arrived: Arc<AtomicBool>,
    release: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
}
impl Server {
    fn new(payload: String, status: u16, pause: bool) -> Self {
        Self::with_content_type(payload, status, pause, Some("text/event-stream"))
    }
    fn with_content_type(
        payload: String,
        status: u16,
        pause: bool,
        content_type: Option<&str>,
    ) -> Self {
        let content_type = content_type
            .map(|v| format!("Content-Type: {v}\r\n"))
            .unwrap_or_default();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let requests = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let arrived = Arc::new(AtomicBool::new(false));
        let release = Arc::new(AtomicBool::new(!pause));
        let (observed, stopped, received, released) = (
            requests.clone(),
            stop.clone(),
            arrived.clone(),
            release.clone(),
        );
        let worker = thread::spawn(move || {
            while !stopped.load(Ordering::SeqCst) {
                let (mut socket, _) = match listener.accept() {
                    Ok(v) => v,
                    Err(_) => {
                        thread::sleep(Duration::from_millis(2));
                        continue;
                    }
                };
                socket.set_nonblocking(false).unwrap();
                socket
                    .set_read_timeout(Some(Duration::from_secs(10)))
                    .unwrap();
                let mut bytes = Vec::new();
                let mut buffer = [0u8; 2048];
                let end = loop {
                    let count = socket.read(&mut buffer).unwrap_or(0);
                    if count == 0 {
                        break None;
                    }
                    bytes.extend_from_slice(&buffer[..count]);
                    if let Some(end) = bytes.windows(4).position(|b| b == b"\r\n\r\n") {
                        break Some(end + 4);
                    }
                };
                let Some(end) = end else {
                    continue;
                };
                let header = String::from_utf8(bytes[..end].to_vec()).unwrap();
                let field = |name: &str| {
                    header
                        .lines()
                        .filter_map(|l| l.split_once(':'))
                        .find(|(key, _)| key.eq_ignore_ascii_case(name))
                        .map(|(_, value)| value.trim().to_owned())
                        .unwrap_or_default()
                };
                let length: usize = field("content-length").parse().unwrap_or(0);
                while bytes.len() < end + length {
                    let count = socket.read(&mut buffer).unwrap_or(0);
                    if count == 0 {
                        break;
                    }
                    bytes.extend_from_slice(&buffer[..count]);
                }
                let path = header
                    .lines()
                    .next()
                    .unwrap()
                    .split_whitespace()
                    .nth(1)
                    .unwrap()
                    .to_owned();
                observed.lock().unwrap().push(Observed {
                    path: path.clone(),
                    bearer: field("authorization"),
                    body: serde_json::from_slice(&bytes[end..]).unwrap_or(Value::Null),
                });
                if path == "/models" {
                    let body = object!({"models":[{"slug":"hidden","display_name":"Hidden","visibility":"hide"},
                        {"slug":"chosen-model","display_name":"Chosen","visibility":"list"},
                        {"slug":"other-model","display_name":"Other","visibility":"list"}]}).to_string();
                    let _ = write!(
                        socket,
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                } else {
                    let _ = write!(
                        socket,
                        "HTTP/1.1 {status} Test\r\n{content_type}Connection: close\r\n\r\n"
                    );
                    let _ = socket.write_all(payload.as_bytes());
                    let _ = socket.flush();
                    received.store(true, Ordering::SeqCst);
                    while !released.load(Ordering::SeqCst) && !stopped.load(Ordering::SeqCst) {
                        thread::sleep(Duration::from_millis(2));
                    }
                }
            }
        });
        Self {
            base,
            requests,
            stop,
            arrived,
            release,
            worker: Some(worker),
        }
    }
    fn posts(&self) -> usize {
        self.requests
            .lock()
            .unwrap()
            .iter()
            .filter(|r| r.path == "/responses")
            .count()
    }
}

#[test]
fn missing_content_type_still_requires_terminal_sse_and_rejects_declared_other_transports() {
    let json_response =
        object!({"object":"response","status":"completed","output_text":"secret-output"})
            .to_string();
    let cases = [
        (None, completion("reply"), true, ""),
        (
            Some("Text/Event-Stream; charset=utf-8"),
            completion("reply"),
            true,
            "",
        ),
        (
            None,
            "data: {\"type\":\"response.output_text.delta\",\"delta\":\"secret-partial\"}\n\n"
                .into(),
            false,
            "without response.completed",
        ),
        (
            Some("application/json"),
            json_response,
            false,
            "content type application/json; completed response JSON",
        ),
        (
            Some("text/html"),
            completion("secret-output"),
            false,
            "SSE with an unexpected content type",
        ),
    ];
    for (content_type, payload, succeeds, diagnostic) in cases {
        let directory = Directory::new();
        let id = profile(&directory.0.join("auth"), "a", "secret-bearer");
        let server = Server::with_content_type(payload, 200, false, content_type);
        let mut provider = adapter(&directory.0, id, &server.base);
        let request = request(id);
        let result = provider.submit(&request);
        if succeeds {
            assert_eq!(result.unwrap(), "reply".encode_utf16().collect::<Vec<_>>());
            assert!(provider.reconcile(&request).unwrap().is_some());
        } else {
            let error = result.unwrap_err().0;
            assert!(error.contains(diagnostic), "{error}");
            assert!(!error.contains("secret-"));
            assert!(provider.reconcile(&request).unwrap().is_none());
            assert!(provider.submit(&request).is_err());
        }
        assert_eq!(server.posts(), 1);
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        self.release.store(true, Ordering::SeqCst);
        self.worker.take().unwrap().join().unwrap();
    }
}
fn adapter(directory: &Path, id: Uuid, base: &str) -> OpenAIProvider {
    let mut result = OpenAIProvider::open(
        directory.join("auth"),
        directory.join("receipts"),
        id,
        "chosen-model",
    )
    .unwrap();
    result.transport.models = format!("{base}/models");
    result.transport.responses = format!("{base}/responses");
    result
}
fn request(id: Uuid) -> ModelRequest {
    ModelRequest {
        id: EffectId::fresh(),
        capability: OpenAIProvider::capability_for(id),
        model: "chosen-model".into(),
        messages: vec![
            ModelMessage {
                role: MessageRole::System,
                content: "Be concise.".encode_utf16().collect(),
            },
            ModelMessage {
                role: MessageRole::Assistant,
                content: "earlier".encode_utf16().collect(),
            },
            ModelMessage {
                role: MessageRole::User,
                content: "hello\0🦀".encode_utf16().collect(),
            },
        ],
    }
}
fn limits() -> DriverLimits {
    DriverLimits {
        instructions: 1000,
        provider_operations: 1,
        checkpoint_interval: 7,
    }
}
fn start<'a>(program: &'a renraku_vm::program::Program, profile: Uuid) -> Vm<'a> {
    let mut vm = Vm::new(program);
    let args = [
        OpenAIProvider::capability_for(profile),
        "chosen-model".into(),
        "hello\0🦀".into(),
    ]
    .into_iter()
    .map(|text| GuestValue::Reference(vm.new_string(text.encode_utf16().collect()).unwrap()))
    .collect();
    vm.start(common::entry(program, "OpenAIHarness"), args)
        .unwrap();
    vm.set_model_calls_enabled(true);
    vm
}
fn assert_result(vm: &Vm<'_>, stop: DriverStop) {
    let DriverStop::Completed(Some(GuestValue::Reference(result))) = stop else {
        panic!("{stop:?}");
    };
    assert_eq!(
        String::from_utf16(vm.heap().object(result).unwrap().string_units().unwrap()).unwrap(),
        "hello from model\0🦀"
    );
}

#[test]
fn sse_handles_fragmented_utf8_multiline_crlf_and_only_final_text() {
    struct Fragmented<'a>(&'a [u8]);
    impl Read for Fragmented<'_> {
        fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
            let size = self.0.len().min(out.len()).min(1);
            out[..size].copy_from_slice(&self.0[..size]);
            self.0 = &self.0[size..];
            Ok(size)
        }
    }
    let payload =
        completion("hello\0🦀").replace("data: {\"response\":", "data: {\n data: \"response\":");
    // JSON field order is deterministic, and explicit multi-line data remains valid JSON.
    let payload = payload.replace("\n data:", "\ndata:");
    let result = stream::completed(Fragmented(payload.as_bytes())).unwrap();
    assert_eq!(result.id, "resp_test");
    assert_eq!(String::from_utf16(&result.text).unwrap(), "hello\0🦀");
}

#[test]
fn sse_rejects_partial_failed_incomplete_mismatched_and_unsupported_results() {
    let good = completion("ok");
    let cases = ["data: {\"type\":\"response.output_text.delta\",\"delta\":\"partial\"}\n\n".into(),
        "data: [DONE]\n\n".into(), good.trim_end().into(),
        good.replace("event: response.completed", "event: response.failed"),
        good.replace("\"status\":\"completed\"", "\"status\":\"incomplete\""),
        good.replace("\"type\":\"output_text\"", "\"type\":\"refusal\""),
        "data: {\"type\":\"response.failed\",\"response\":{\"error\":{\"code\":\"subscription_sharing_usage_limit_exceeded\"}}}\n\n".into(),
        "data: {\"type\":\"response.incomplete\"}\n\n".into(),
        good.replacen("resp_test", "resp_other", 1)];
    for payload in cases {
        assert!(stream::completed(payload.as_bytes()).is_err(), "{payload}");
    }
    let huge = format!("data: {}\n\n", "x".repeat(8 * 1024 * 1024));
    assert!(stream::completed(huge.as_bytes()).is_err());
    assert!(stream::completed(completion(&"x".repeat(1_000_001)).as_bytes()).is_err());
}

#[test]
fn profile_is_pinned_complete_body_is_sent_and_receipt_reconciles_offline_after_logout() {
    let directory = Directory::new();
    let a = profile(&directory.0.join("auth"), "a", "token-a");
    profile(&directory.0.join("auth"), "b", "token-b"); // active selection differs
    let server = Server::new(completion("reply"), 200, false);
    let mut provider = adapter(&directory.0, a, &server.base);
    let catalog = provider
        .transport
        .models(&provider.binding.bearer(&provider.auth_directory).unwrap())
        .unwrap();
    assert_eq!(
        catalog.iter().map(|m| m.slug.as_str()).collect::<Vec<_>>(),
        ["chosen-model", "other-model"]
    );
    let request = request(a);
    provider.validate_request(&request).unwrap();
    assert_eq!(
        provider.submit(&request).unwrap(),
        "reply".encode_utf16().collect::<Vec<_>>()
    );
    let requests = server.requests.lock().unwrap().clone();
    assert!(requests.iter().all(|r| r.bearer == "Bearer token-a"));
    let sent = &requests
        .iter()
        .find(|r| r.path == "/responses")
        .unwrap()
        .body;
    assert_eq!(
        sent,
        &object!({"model":"chosen-model","store":false,"stream":true,
        "input":[{"role":"developer","content":"Be concise."},{"role":"assistant","content":"earlier"},{"role":"user","content":"hello\0🦀"}]})
    );
    drop(provider);
    let store = Store::open(&directory.0.join("auth")).unwrap();
    let mut guard = store.lock().unwrap();
    guard
        .vault
        .profiles
        .iter_mut()
        .find(|p| p.id == a)
        .unwrap()
        .session = None;
    guard.save().unwrap();
    drop(guard);
    let mut provider = adapter(&directory.0, a, "http://127.0.0.1:1");
    assert!(provider.reconcile(&request).unwrap().is_some());
    assert_eq!(
        provider.submit(&request).unwrap(),
        "reply".encode_utf16().collect::<Vec<_>>()
    );
    let mut changed = request.clone();
    changed.messages[0].content.push(33);
    assert!(
        provider
            .reconcile(&changed)
            .unwrap_err()
            .0
            .contains("match")
    );
    let other_binding = Binding {
        subject: "other".into(),
        ..provider.binding.clone()
    }
    .bytes()
    .unwrap();
    assert!(
        provider
            .receipts
            .reconcile(request.id, &other_binding, &request.encode().unwrap())
            .is_err()
    );
    assert_eq!(server.posts(), 1);
}

#[test]
fn invalid_text_auth_and_model_rejections_leave_vm_prepared_without_posting() {
    let directory = Directory::new();
    let id = profile(&directory.0.join("auth"), "a", "token-a");
    let server = Server::new(completion("reply"), 200, false);
    let mut provider = adapter(&directory.0, id, &server.base);
    let program = common::program();
    let mut vm = start(&program, id);
    let mut store = CheckpointStore::open(directory.0.join("vm.db")).unwrap();
    let prompt = vm.new_string(vec![0xd800]).unwrap();
    let effect = vm
        .queue_chat_request(
            provider.capability().into(),
            "chosen-model".into(),
            vec![renraku_vm::model::MessageReference {
                role: MessageRole::User,
                content: prompt,
            }],
        )
        .unwrap();
    assert!(
        vm.dispatch_model(&mut store, effect, &mut provider)
            .unwrap_err()
            .0
            .contains("UTF-16")
    );
    assert_eq!(vm.effect(effect).unwrap().state, EffectState::Prepared);
    assert_eq!(store.generation(), 0);
    assert!(server.requests.lock().unwrap().is_empty());
    let mut request = request(id);
    request.model = "unavailable".into();
    provider.model = request.model.clone();
    assert!(
        provider
            .validate_request(&request)
            .unwrap_err()
            .0
            .contains("catalog")
    );
    provider.model = "chosen-model".into();
    let auth = Store::open(&directory.0.join("auth")).unwrap();
    let mut guard = auth.lock().unwrap();
    guard.vault.profiles[0]
        .session
        .as_mut()
        .unwrap()
        .scopes
        .clear();
    guard.save().unwrap();
    drop(guard);
    assert!(
        provider
            .validate_request(&self::request(id))
            .unwrap_err()
            .0
            .contains("plan usage")
    );
    assert_eq!(server.posts(), 0);
}

#[test]
fn interrupted_failed_and_http_error_attempts_are_never_resubmitted() {
    for (payload, status) in [
        (
            "data: {\"type\":\"response.output_text.delta\",\"delta\":\"partial\"}\n\n",
            200,
        ),
        (
            "data: {\"type\":\"response.failed\",\"response\":{\"error\":{\"code\":\"subscription_sharing_usage_unavailable\"}}}\n\n",
            200,
        ),
        ("secret-server-error", 401),
    ] {
        let directory = Directory::new();
        let id = profile(&directory.0.join("auth"), "a", "secret-bearer");
        let server = Server::new(payload.into(), status, false);
        let request = request(id);
        let mut provider = adapter(&directory.0, id, &server.base);
        let error = provider.submit(&request).unwrap_err();
        assert!(!error.0.contains("secret-"));
        drop(provider);
        let mut provider = adapter(&directory.0, id, &server.base);
        assert!(provider.reconcile(&request).unwrap().is_none());
        assert!(
            provider
                .submit(&request)
                .unwrap_err()
                .0
                .contains("already attempted")
        );
        assert_eq!(server.posts(), 1);
    }
}

#[test]
fn cil_harness_receives_a_durable_reply_and_restores_without_network() {
    let directory = Directory::new();
    let id = profile(&directory.0.join("auth"), "a", "token-a");
    let server = Server::new(completion("hello from model\0🦀"), 200, false);
    let mut provider = adapter(&directory.0, id, &server.base);
    let program = common::program();
    let mut vm = start(&program, id);
    let mut store = CheckpointStore::open(directory.0.join("vm.db")).unwrap();
    let report = vm.drive_model(&mut store, &mut provider, limits()).unwrap();
    assert_result(&vm, report.stop);
    assert_eq!(report.provider_operations, 1);
    let snapshot = store.load().unwrap();
    let vm = snapshot.snapshot.restore().unwrap();
    let ExecutionStatus::Completed { result } = vm.context().status else {
        panic!();
    };
    assert_result(&vm, DriverStop::Completed(result));
    assert_eq!(server.posts(), 1);
}

#[test]
#[ignore = "launched only by the Responses crash-recovery test"]
fn responses_kill_probe() {
    let directory = std::env::var("RENRAKU_RESPONSES_PROBE_DIR").unwrap();
    let directory = PathBuf::from(directory);
    let id = std::env::var("RENRAKU_RESPONSES_PROBE_PROFILE")
        .unwrap()
        .parse()
        .unwrap();
    let mut provider = adapter(
        &directory,
        id,
        &std::env::var("RENRAKU_RESPONSES_PROBE_URL").unwrap(),
    );
    let mut store = CheckpointStore::open(directory.join("vm.db")).unwrap();
    let saved = store.load().unwrap();
    let mut vm = saved.snapshot.restore().unwrap();
    vm.set_model_calls_enabled(true);
    if std::env::var("RENRAKU_RESPONSES_PROBE_PHASE").unwrap() == "receipt" {
        struct PauseAfterReceipt(OpenAIProvider, PathBuf);
        impl ModelProvider for PauseAfterReceipt {
            fn capability(&self) -> &str {
                self.0.capability()
            }
            fn supports_model(&self, model: &str) -> bool {
                self.0.supports_model(model)
            }
            fn validate_request(&mut self, r: &ModelRequest) -> renraku_vm::error::Result<()> {
                self.0.validate_request(r)
            }
            fn submit(&mut self, r: &ModelRequest) -> renraku_vm::error::Result<Vec<u16>> {
                let response = self.0.submit(r)?;
                fs::write(self.1.join("receipt-committed"), b"ready").unwrap();
                loop {
                    thread::sleep(Duration::from_millis(20));
                    if self.1.join("release").exists() {
                        return Ok(response);
                    }
                }
            }
            fn reconcile(
                &mut self,
                r: &ModelRequest,
            ) -> renraku_vm::error::Result<Option<Vec<u16>>> {
                self.0.reconcile(r)
            }
        }
        vm.drive_model(
            &mut store,
            &mut PauseAfterReceipt(provider, directory),
            limits(),
        )
        .unwrap();
    } else {
        vm.drive_model(&mut store, &mut provider, limits()).unwrap();
    }
}

#[test]
fn sigkill_preserves_uncertain_streams_and_recovers_committed_receipts_into_cil() {
    use std::process::{Command, Stdio};
    for phase in ["stream", "receipt"] {
        let directory = Directory::new();
        let id = profile(&directory.0.join("auth"), "a", "token-a");
        let server = Server::new(
            if phase == "receipt" {
                completion("hello from model\0🦀")
            } else {
                "data: {\"type\":\"response.output_text.delta\",\"delta\":\"partial\"}\n\n".into()
            },
            200,
            phase == "stream",
        );
        let program = common::program();
        let mut vm = start(&program, id);
        let mut store = CheckpointStore::open(directory.0.join("vm.db")).unwrap();
        assert!(matches!(
            vm.run_checkpointed(&mut store, 1000, 7).unwrap(),
            RunOutcome::Waiting(_)
        ));
        drop(store);
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "inference::tests::responses_kill_probe",
                "--ignored",
            ])
            .env("RENRAKU_RESPONSES_PROBE_DIR", &directory.0)
            .env("RENRAKU_RESPONSES_PROBE_PROFILE", id.to_string())
            .env("RENRAKU_RESPONSES_PROBE_URL", &server.base)
            .env("RENRAKU_RESPONSES_PROBE_PHASE", phase)
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(15);
        loop {
            let ready = if phase == "receipt" {
                directory.0.join("receipt-committed").exists()
            } else {
                server.arrived.load(Ordering::SeqCst)
            };
            if ready {
                break;
            }
            if std::time::Instant::now() >= deadline || child.try_wait().unwrap().is_some() {
                let _ = child.kill();
                let output = child.wait_with_output().unwrap();
                panic!(
                    "probe did not reach crash boundary: {}",
                    String::from_utf8_lossy(&output.stderr)
                );
            }
            thread::sleep(Duration::from_millis(5));
        }
        child.kill().unwrap();
        let status = child.wait().unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::process::ExitStatusExt;
            assert_eq!(status.signal(), Some(9));
        }
        server.release.store(true, Ordering::SeqCst);
        let mut store = CheckpointStore::open(directory.0.join("vm.db")).unwrap();
        let saved = store.load().unwrap();
        let mut vm = saved.snapshot.restore().unwrap();
        assert!(matches!(
            vm.effects().next().unwrap().state,
            EffectState::OutcomeUnknown { .. }
        ));
        vm.set_model_calls_enabled(true);
        let mut provider = adapter(&directory.0, id, "http://127.0.0.1:1"); // recovery must be offline
        let report = vm.drive_model(&mut store, &mut provider, limits()).unwrap();
        if phase == "receipt" {
            assert_result(&vm, report.stop);
        } else {
            assert!(matches!(report.stop, DriverStop::OutcomeUnknown { .. }));
        }
        assert_eq!(server.posts(), 1);
    }
}

#[test]
fn receipt_commit_failure_keeps_the_attempt_uncertain_without_another_post() {
    let directory = Directory::new();
    let id = profile(&directory.0.join("auth"), "a", "token-a");
    let server = Server::new(completion("reply"), 200, false);
    let mut provider = adapter(&directory.0, id, &server.base);
    let db = rusqlite::Connection::open(directory.0.join("receipts/receipts.db")).unwrap();
    db.execute_batch("CREATE TRIGGER fail_receipt BEFORE UPDATE ON receipts BEGIN SELECT RAISE(ABORT, 'injected'); END;").unwrap();
    let request = request(id);
    assert!(provider.submit(&request).unwrap_err().0.contains("storage"));
    assert!(provider.reconcile(&request).unwrap().is_none());
    assert!(
        provider
            .submit(&request)
            .unwrap_err()
            .0
            .contains("already attempted")
    );
    assert_eq!(server.posts(), 1);
}

#[test]
fn concurrent_hosts_reserve_only_one_attempt_and_foreign_databases_are_rejected() {
    let directory = Directory::new();
    let id = EffectId::fresh();
    let first = Receipts::open(&directory.0.join("receipts")).unwrap();
    let second = Receipts::open(&directory.0.join("receipts")).unwrap();
    let barrier = Arc::new(std::sync::Barrier::new(2));
    let threads: Vec<_> = [first, second]
        .into_iter()
        .map(|mut ledger| {
            let barrier = barrier.clone();
            thread::spawn(move || {
                barrier.wait();
                ledger.reserve(id, b"profile", b"request")
            })
        })
        .collect();
    let accepted = threads
        .into_iter()
        .map(|t| t.join().unwrap())
        .filter(|r| matches!(r, Ok(None)))
        .count();
    assert_eq!(accepted, 1);
    let ledger = rusqlite::Connection::open(directory.0.join("receipts/receipts.db")).unwrap();
    assert_eq!(
        ledger
            .query_row("SELECT count(*) FROM receipts", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        1
    );
    let foreign = directory.0.join("foreign");
    Store::open(&foreign).unwrap();
    let db = rusqlite::Connection::open(foreign.join("receipts.db")).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(
            foreign.join("receipts.db"),
            fs::Permissions::from_mode(0o600),
        )
        .unwrap();
    }
    db.execute_batch("CREATE TABLE unrelated (value INTEGER);")
        .unwrap();
    assert!(
        Receipts::open(&foreign)
            .err()
            .unwrap()
            .to_string()
            .contains("foreign")
    );
    assert_eq!(
        db.query_row("PRAGMA application_id", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        0
    );
}
