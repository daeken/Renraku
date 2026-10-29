//! Exercise the real subscription adapter against loopback HTTP, never a live account.
use super::*;
use renraku_vm::{
    agent::{AgentHost, AgentLimits, AgentStage, AgentStop},
    compiler::CompilationStore,
    error::{Error as VmError, Result as VmResult},
    invocation::{InvocationRecord, InvocationState},
};
use std::{collections::BTreeSet, process::Command, sync::OnceLock};

const GOAL: &str = "Inspect Counter.Add; compile a caller that adds 7 to the saved counter, install and invoke it, then inspect its source. Preserve this text: \0🦀";
const SOURCE: &str = "namespace Generated { public static class Entry { public static int Run(Renraku.Guest.Counter counter) { return counter.Add(7); } } }";
const BAD_SOURCE: &str = "namespace Generated { public static class Entry { public static int Run(Renraku.Guest.Counter counter) { return counter.Add(\"wrong\"); } } }";

fn compiler() -> &'static Path {
    static DRIVER: OnceLock<PathBuf> = OnceLock::new();
    DRIVER.get_or_init(|| {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let output = root
            .join("target")
            .join(format!("agent-http-compiler-{}", std::process::id()));
        let result = Command::new("dotnet")
            .args(["build", "--nologo", "--verbosity", "quiet", "--output"])
            .arg(&output)
            .arg(root.join("tools/Compiler"))
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
        output.join("Renraku.Compiler.dll")
    })
}
fn host(directory: &Path, compiler: &Path) -> AgentHost {
    AgentHost {
        universe: directory.join("agent.db"),
        compiler_ledger: directory.join("compiler.db"),
        dotnet: "dotnet".into(),
        compiler_driver: compiler.into(),
    }
}
fn initialize(host: &AgentHost, profile: Uuid) {
    let program = common::program();
    let mut vm = Vm::new(&program);
    let mut store = CheckpointStore::open(&host.universe).unwrap();
    AgentHost::initialize(
        &mut vm,
        &mut store,
        &OpenAIProvider::capability_for(profile),
        "chosen-model",
        GOAL,
    )
    .unwrap();
}
fn budgets(operations: u64) -> AgentLimits {
    AgentLimits {
        instructions: 10_000,
        provider_operations: operations,
        tool_actions: 20,
    }
}
fn last_result(body: &Value) -> Value {
    serde_json::from_str(
        body["input"].as_array().unwrap().last().unwrap()["content"]
            .as_str()
            .unwrap(),
    )
    .unwrap()
}
fn assistant_count(body: &Value) -> usize {
    body["input"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|m| m["role"] == "assistant")
        .count()
}
fn action(request: &Observed) -> Value {
    assert_eq!(request.path, "/responses");
    assert_eq!(request.bearer, "Bearer coding-token");
    let body = &request.body;
    assert_eq!(body.as_object().unwrap().len(), 4);
    assert_eq!(body["model"], "chosen-model");
    assert_eq!(body["store"], false);
    assert_eq!(body["stream"], true);
    assert_eq!(body["input"][0]["role"], "developer");
    assert_eq!(
        body["input"][0]["content"],
        renraku_vm::agent::SYSTEM_PROMPT
    );
    assert_eq!(body["input"][1], object!({"role":"user","content":GOAL}));
    let count = assistant_count(body);
    assert_eq!(body["input"].as_array().unwrap().len(), 2 + 2 * count);
    match count {
        0 => object!({"op":"search","query":"Counter","limit":20}),
        1 => {
            let result = last_result(body);
            let symbol = result["output"]["symbols"]
                .as_array()
                .unwrap()
                .iter()
                .find(|s| s["name"] == "Renraku.Guest.Counter::Add")
                .unwrap();
            object!({"op":"read_code","method":symbol["codeVersion"]})
        }
        2 => {
            let result = last_result(body);
            assert_eq!(result["output"]["name"], "Add");
            assert!(
                !result["output"]["instructions"]
                    .as_array()
                    .unwrap()
                    .is_empty()
            );
            object!({"op":"describe","type":result["output"]["owner"]})
        }
        3 => {
            let result = last_result(body);
            assert!(
                result["output"]["declaration"]
                    .as_str()
                    .unwrap()
                    .contains("Add(")
            );
            object!({"op":"compile","assembly":"Http.Generated","source":BAD_SOURCE,"referenceTypes":["Renraku.Guest.Counter"]})
        }
        4 => {
            let result = last_result(body);
            assert_eq!(result["ok"], false);
            assert_eq!(result["output"]["state"], "rejected");
            let diagnostic = result["output"]["report"]["diagnostics"]
                .as_array()
                .unwrap()
                .iter()
                .find(|d| d["code"] == "CS1503")
                .unwrap();
            assert_eq!(diagnostic["file"], "Generated.cs");
            assert_eq!(diagnostic["line"], 1);
            assert!(diagnostic["message"].as_str().unwrap().contains("string"));
            object!({"op":"compile","assembly":"Http.Generated","source":SOURCE,"referenceTypes":["Renraku.Guest.Counter"]})
        }
        5 => {
            let result = last_result(body);
            assert_eq!(result["ok"], true);
            assert_eq!(result["output"]["state"], "compiled");
            object!({"op":"install","compilationTurn":5})
        }
        6 => {
            assert_eq!(last_result(body)["ok"], true);
            object!({"op":"invoke","method":"Generated.Entry::Run","arguments":[{"kind":"static","field":"Renraku.Guest.Simulation::Saved"}]})
        }
        7 => {
            assert_eq!(last_result(body)["result"]["value"], 26);
            object!({"op":"read_code","method":"Generated.Entry::Run"})
        }
        8 => {
            let result = last_result(body);
            assert_eq!(result["output"]["sourceAvailable"], true);
            assert_eq!(result["output"]["sourceScope"], "assembly");
            assert_eq!(result["output"]["sources"][0]["text"], SOURCE);
            object!({"op":"finish","message":"Counter is 26; compiled source inspected. 🦀"})
        }
        _ => panic!("unexpected extra HTTP model request"),
    }
}
fn server() -> Server {
    Server::with_responder(
        |request| FixtureReply::completed(&action(request).to_string()),
        false,
    )
}
fn inspect(host: &AgentHost, expected: i32) -> Value {
    let mut store = CheckpointStore::open(&host.universe).unwrap();
    let saved = store.load().unwrap();
    let mut vm = saved.snapshot.restore().unwrap();
    let program = &saved.snapshot.program;
    let simulation = program.find_type("Renraku.Guest.Simulation").unwrap();
    let counter_type = program.find_type("Renraku.Guest.Counter").unwrap();
    let field = program
        .fields()
        .find(|f| f.owner == simulation && f.name == "Saved")
        .unwrap()
        .id;
    let GuestValue::Reference(counter) = vm.static_fields()[&field] else {
        panic!()
    };
    let value = program
        .fields()
        .find(|f| f.owner == counter_type && f.name == "Value")
        .unwrap()
        .id;
    assert_eq!(
        vm.heap().object(counter).unwrap().fields()[&value],
        GuestValue::I32(expected)
    );
    let status = vm.agent_status().unwrap();
    if expected == 26 {
        assert_eq!(vm.installations().count(), 1);
        assert_eq!(vm.invocations().count(), 1);
    }
    vm.collect_garbage().unwrap();
    store.save(&vm).unwrap();
    status
}
fn receipt_counts(directory: &Path) -> (u32, u32) {
    rusqlite::Connection::open(directory.join("receipts/receipts.db"))
        .unwrap()
        .query_row("SELECT count(*), count(response) FROM receipts", [], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
        .unwrap()
}
fn signed_out(directory: &Path, id: Uuid) {
    let store = Store::open(&directory.join("auth")).unwrap();
    let mut guard = store.lock().unwrap();
    guard
        .vault
        .profiles
        .iter_mut()
        .find(|p| p.id == id)
        .unwrap()
        .session = None;
    guard.save().unwrap();
}
fn invocation(host: &AgentHost) -> (String, InvocationRecord) {
    let mut store = CheckpointStore::open(&host.universe).unwrap();
    let saved = store.load().unwrap();
    let vm = saved.snapshot.restore().unwrap();
    let (id, record) = vm.invocations().next().unwrap();
    assert_eq!(
        record.state,
        InvocationState::Completed {
            result: Some(GuestValue::I32(26))
        }
    );
    (id.to_owned(), record.clone())
}
fn assert_http_history(server: &Server, status: &Value) {
    let requests = server.requests.lock().unwrap();
    let posts = requests.iter().filter(|r| r.path == "/responses");
    for (index, request) in posts.enumerate() {
        let expected: Vec<_> = status["transcript"].as_array().unwrap()[..2 + 2 * index]
            .iter()
            .map(|message| {
                let role = match message["role"].as_i64().unwrap() {
                    0 => "developer",
                    1 => "user",
                    2 => "assistant",
                    _ => panic!(),
                };
                object!({"role":role,"content":message["content"]})
            })
            .collect();
        assert_eq!(request.body["input"], object!(expected));
    }
}

#[tokio::test(flavor = "current_thread")]
async fn http_coding_loop_corrects_compiler_errors_and_replays_lost_results_from_managed_history() {
    let directory = Directory::new();
    let id = profile(&directory.0.join("auth"), "coding", "coding-token");
    let mut host = host(&directory.0, compiler());
    initialize(&host, id);
    let server = server();
    let mut lost = BTreeSet::new();
    for _ in 0..20 {
        // Reopening both the controller and adapter on each turn simulates lost
        // host projections. Active account UI selection must not retarget calls.
        let mut provider = async_adapter(&directory.0, id, &server.base).await;
        let report = host
            .drive_with_observer(&mut provider, budgets(1), |stage| {
                if let AgentStage::ToolCompleted(turn @ (5..=7)) = stage
                    && lost.insert(turn)
                {
                    return Err(VmError("lost tool reply".into()));
                }
                Ok(())
            })
            .await;
        match report {
            Err(error) => {
                assert!(error.0.contains("lost tool reply"), "{error}");
                // Turn 5's artifact is published. Replays and all later actions
                // must work without the compiler; do not delete a shared fixture.
                host.compiler_driver = directory.0.join("missing-compiler.dll");
            }
            Ok(report) if matches!(report.stop, AgentStop::Finished(_)) => break,
            Ok(report) => assert!(
                matches!(report.stop, AgentStop::ProviderBudget),
                "{report:?}"
            ),
        }
        profile(&directory.0.join("auth"), "other-account", "other-token");
    }
    assert_eq!(lost, [5, 6, 7].into());
    let status = inspect(&host, 26);
    assert_eq!(status["phase"], 3);
    assert_eq!(status["turn"], 9);
    assert_eq!(receipt_counts(&directory.0), (9, 9));
    assert_eq!(server.posts(), 9);
    assert_http_history(&server, &status);
    let mut store = CheckpointStore::open(&host.universe).unwrap();
    let saved = store.load().unwrap();
    let vm = saved.snapshot.restore().unwrap();
    let compiler = CompilationStore::open(&host.compiler_ledger).unwrap();
    for (turn, state) in [(4, "rejected"), (5, "compiled")] {
        let request = format!("agent/{}/{turn}/compile", vm.universe_id());
        assert_eq!(
            compiler
                .get(vm.universe_id(), &request)
                .unwrap()
                .summary()
                .unwrap()["state"],
            state
        );
    }
    signed_out(&directory.0, id);
    let mut offline = async_adapter(&directory.0, id, "http://127.0.0.1:1").await;
    let report = host.drive(&mut offline, budgets(0)).await.unwrap();
    assert!(matches!(report.stop, AgentStop::Finished(_)));
    assert_eq!(
        (
            report.instructions,
            report.provider_operations,
            report.tool_actions
        ),
        (0, 0, 0)
    );
    assert_eq!(server.posts(), 9);
    assert!(
        !fs::read(&host.universe)
            .unwrap()
            .windows(b"coding-token".len())
            .any(|b| b == b"coding-token")
    );
}

#[tokio::test(flavor = "current_thread")]
async fn interrupted_or_native_call_stream_cannot_become_an_agent_action_or_be_resent() {
    for native_call in [false, true] {
        let directory = Directory::new();
        let id = profile(&directory.0.join("auth"), "coding", "coding-token");
        let host = host(&directory.0, compiler());
        initialize(&host, id);
        let server = Server::with_responder(
            move |request| {
                let selected = action(request);
                if assistant_count(&request.body) != 6 {
                    return FixtureReply::completed(&selected.to_string());
                }
                let event = if native_call {
                    object!({"type":"response.completed","response":{"id":"resp_test","status":"completed","error":null,
                    "output":[{"type":"function_call","name":"invoke","call_id":"call_test","arguments":selected.to_string()}]}})
                } else {
                    // Even a complete valid action in provisional delta text is
                    // unusable without a terminal response.completed event.
                    object!({"type":"response.output_text.delta","delta":selected.to_string()})
                };
                FixtureReply {
                    status: 200,
                    content_type: Some("text/event-stream".into()),
                    payload: format!("data: {event}\n\n"),
                    tail: String::new(),
                }
            },
            false,
        );
        let mut provider = async_adapter(&directory.0, id, &server.base).await;
        let error = host.drive(&mut provider, budgets(9)).await.unwrap_err();
        assert!(error.0.contains("uncertain"), "{error}");
        assert!(
            error.0.contains(if native_call {
                "unsupported non-text"
            } else {
                "without response.completed"
            }),
            "{error}"
        );
        assert_eq!(server.posts(), 7);
        assert_eq!(receipt_counts(&directory.0), (7, 6));
        let status = inspect(&host, 19);
        assert_eq!(status["turn"], 6);
        assert_eq!(status["phase"], 0);
        let mut store = CheckpointStore::open(&host.universe).unwrap();
        let saved = store.load().unwrap();
        let vm = saved.snapshot.restore().unwrap();
        assert_eq!(vm.installations().count(), 1);
        assert_eq!(vm.invocations().count(), 0);
        let effect = vm
            .effects()
            .find(|e| matches!(e.state, EffectState::OutcomeUnknown { .. }))
            .unwrap()
            .id;
        let execution = vm.context().id;
        signed_out(&directory.0, id);
        for _ in 0..2 {
            let mut offline = async_adapter(&directory.0, id, "http://127.0.0.1:1").await;
            let report = host.drive(&mut offline, budgets(1)).await.unwrap();
            assert_eq!(report.stop, AgentStop::OutcomeUnknown(effect.to_string()));
            assert_eq!(
                (
                    report.instructions,
                    report.provider_operations,
                    report.tool_actions
                ),
                (0, 1, 0)
            );
            let mut store = CheckpointStore::open(&host.universe).unwrap();
            let saved = store.load().unwrap();
            let vm = saved.snapshot.restore().unwrap();
            assert_eq!(vm.context().id, execution);
            assert_eq!(vm.invocations().count(), 0);
        }
        assert_eq!(server.posts(), 7);
    }
}

struct LostReceiptReply(AsyncOpenAIProvider);
impl AsyncModelProvider for LostReceiptReply {
    fn capability(&self) -> &str {
        self.0.capability()
    }
    fn supports_model(&self, model: &str) -> bool {
        self.0.supports_model(model)
    }
    async fn validate_request(&mut self, request: &ModelRequest) -> VmResult<()> {
        self.0.validate_request(request).await
    }
    async fn submit(&mut self, request: &ModelRequest) -> VmResult<Vec<u16>> {
        let text = self.0.submit(request).await?;
        if request
            .messages
            .iter()
            .filter(|m| m.role == MessageRole::Assistant)
            .count()
            == 6
        {
            Err(VmError(
                "lost reply after durable HTTP completion receipt".into(),
            ))
        } else {
            Ok(text)
        }
    }
    async fn reconcile(&mut self, request: &ModelRequest) -> VmResult<Option<Vec<u16>>> {
        self.0.reconcile(request).await
    }
}

#[tokio::test(flavor = "current_thread")]
async fn committed_http_receipt_recovers_offline_into_one_managed_action_and_mutation() {
    let directory = Directory::new();
    let id = profile(&directory.0.join("auth"), "coding", "coding-token");
    let host = host(&directory.0, compiler());
    initialize(&host, id);
    let server = server();
    let mut provider = LostReceiptReply(async_adapter(&directory.0, id, &server.base).await);
    let error = host.drive(&mut provider, budgets(9)).await.unwrap_err();
    assert!(
        error
            .0
            .contains("lost reply after durable HTTP completion receipt")
    );
    assert_eq!(server.posts(), 7);
    assert_eq!(receipt_counts(&directory.0), (7, 7));
    let status = inspect(&host, 19);
    assert_eq!(status["turn"], 6);
    signed_out(&directory.0, id);
    let mut offline = async_adapter(&directory.0, id, "http://127.0.0.1:1").await;
    let mut limits = budgets(1);
    limits.tool_actions = 0;
    let report = host.drive(&mut offline, limits).await.unwrap();
    assert_eq!(report.stop, AgentStop::ToolBudget);
    assert_eq!(report.provider_operations, 1);
    let status = inspect(&host, 19);
    assert_eq!(status["turn"], 7);
    assert_eq!(status["phase"], 1);
    let action: Value =
        serde_json::from_str(status["transcript"][14]["content"].as_str().unwrap()).unwrap();
    assert_eq!(action["op"], "invoke");
    let report = host.drive(&mut offline, budgets(0)).await.unwrap();
    assert_eq!(report.stop, AgentStop::ProviderBudget);
    assert_eq!(report.provider_operations, 0);
    let status = inspect(&host, 26);
    assert_eq!(status["turn"], 7);
    let repeated = host.drive(&mut offline, budgets(0)).await.unwrap();
    assert_eq!(repeated.stop, AgentStop::ProviderBudget);
    inspect(&host, 26);
    assert_eq!(server.posts(), 7);
    assert_eq!(receipt_counts(&directory.0), (7, 7));
}

#[tokio::test(flavor = "current_thread")]
#[ignore = "launched only by the coding-agent SIGKILL recovery test"]
async fn agent_kill_probe() {
    let directory = PathBuf::from(std::env::var_os("RENRAKU_AGENT_PROBE_DIR").unwrap());
    let id = std::env::var("RENRAKU_AGENT_PROBE_PROFILE")
        .unwrap()
        .parse()
        .unwrap();
    let base = std::env::var("RENRAKU_AGENT_PROBE_URL").unwrap();
    let driver = PathBuf::from(std::env::var_os("RENRAKU_AGENT_PROBE_COMPILER").unwrap());
    let host = host(&directory, &driver);
    let mut provider = async_adapter(&directory, id, &base).await;
    host.drive_with_observer(&mut provider, budgets(9), |stage| {
        if stage == AgentStage::ToolCompleted(7) {
            fs::write(directory.join("invoke-committed"), b"ready").unwrap();
            loop {
                thread::sleep(Duration::from_millis(10));
            }
        }
        Ok(())
    })
    .await
    .unwrap();
    panic!("probe did not stop at the invocation boundary");
}

#[tokio::test(flavor = "current_thread")]
async fn sigkill_after_http_selected_invocation_restores_its_result_without_repeating_the_call() {
    use std::process::Stdio;
    let directory = Directory::new();
    let id = profile(&directory.0.join("auth"), "coding", "coding-token");
    let host = host(&directory.0, compiler());
    initialize(&host, id);
    let server = server();
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "inference::tests::agent::agent_kill_probe",
            "--ignored",
        ])
        .env("RENRAKU_AGENT_PROBE_DIR", &directory.0)
        .env("RENRAKU_AGENT_PROBE_PROFILE", id.to_string())
        .env("RENRAKU_AGENT_PROBE_URL", &server.base)
        .env("RENRAKU_AGENT_PROBE_COMPILER", &host.compiler_driver)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    while !directory.0.join("invoke-committed").exists() {
        if std::time::Instant::now() >= deadline || child.try_wait().unwrap().is_some() {
            let _ = child.kill();
            let output = child.wait_with_output().unwrap();
            panic!(
                "coding-agent probe did not reach commit: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    child.kill().unwrap();
    let status = child.wait().unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        assert_eq!(status.signal(), Some(9));
    }
    assert_eq!(server.posts(), 7);
    let status = inspect(&host, 26);
    assert_eq!(status["turn"], 7);
    assert_eq!(status["phase"], 1);
    assert!(status["result"].is_null());
    let original = invocation(&host);
    let mut provider = async_adapter(&directory.0, id, &server.base).await;
    let result = host.drive(&mut provider, budgets(0)).await.unwrap();
    assert_eq!(result.stop, AgentStop::ProviderBudget);
    assert_eq!(result.provider_operations, 0);
    inspect(&host, 26);
    assert_eq!(invocation(&host), original);
    let result = host.drive(&mut provider, budgets(2)).await.unwrap();
    assert!(matches!(result.stop, AgentStop::Finished(_)), "{result:?}");
    assert_eq!(result.provider_operations, 2);
    inspect(&host, 26);
    assert_eq!(invocation(&host), original);
    assert_eq!(server.posts(), 9);
    assert_eq!(receipt_counts(&directory.0), (9, 9));
}
