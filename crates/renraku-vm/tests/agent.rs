#[allow(dead_code)]
mod common;
use common::{entry, fixtures, program};
use renraku_vm::{
    agent::{AgentEventKind, AgentHost, AgentLimits, AgentStage, AgentStop, submit_input},
    checkpoint::CheckpointStore,
    effects::AsyncModelProvider,
    error::{Error, Result},
    identity::UniverseId,
    model::{MessageRole, ModelRequest},
    runtime::{RunOutcome, Vm},
    value::Value,
};
use rusqlite::{Connection, OptionalExtension, params};
use serde_json::{Value as Json, json};
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    sync::OnceLock,
};

fn compiler() -> &'static Path {
    static COMPILER: OnceLock<PathBuf> = OnceLock::new();
    COMPILER.get_or_init(|| {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .canonicalize()
            .unwrap();
        let output = root
            .join("target")
            .join(format!("agent-compiler-tests-{}", std::process::id()));
        let result = Command::new("dotnet")
            .args(["build", "--nologo", "--verbosity", "quiet", "--output"])
            .arg(&output)
            .arg(root.join("tools/Compiler"))
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stdout)
        );
        output.join("Renraku.Compiler.dll")
    })
}
struct Test {
    directory: PathBuf,
    host: AgentHost,
}
impl Test {
    fn new() -> Self {
        let directory = fixtures().output.join(UniverseId::fresh().to_string());
        fs::create_dir(&directory).unwrap();
        let host = AgentHost {
            universe: directory.join("universe.db"),
            compiler_ledger: directory.join("compiler.db"),
            dotnet: PathBuf::from("dotnet"),
            compiler_driver: compiler().to_path_buf(),
            allow_web_publication: false,
        };
        let program = program();
        let mut vm = Vm::new(&program);
        let mut store = CheckpointStore::open(&host.universe).unwrap();
        AgentHost::initialize(&mut vm,&mut store,"test.agent","script-v1","Inspect Counter.Add, compile a new static Generated.Entry.Run caller that adds 7 to the saved Counter, install it, invoke it, then finish.").unwrap();
        Self { directory, host }
    }
    fn provider(&self) -> Script {
        Script::open(&self.directory.join("provider.db"))
    }
}
impl Drop for Test {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.directory);
    }
}
struct Script {
    db: Connection,
    lose_reply: bool,
    no_receipt: bool,
    mode: Mode,
}
#[derive(Clone, Copy)]
enum Mode {
    Workflow,
    ModelDenied,
    Unsafe,
    Repeat,
    Huge,
    Finish,
    FinishAtLimit,
    Followup,
    WebDenied,
    WebPublish,
    WebStrict,
}
impl Script {
    fn open(path: &Path) -> Self {
        let db = Connection::open(path).unwrap();
        db.execute_batch("CREATE TABLE IF NOT EXISTS receipt(id TEXT PRIMARY KEY,body BLOB NOT NULL,response TEXT);").unwrap();
        Self {
            db,
            lose_reply: false,
            no_receipt: false,
            mode: Mode::Workflow,
        }
    }
    fn response(&self, request: &ModelRequest) -> Result<String> {
        let assistants = request
            .messages
            .iter()
            .filter(|message| message.role == MessageRole::Assistant)
            .count();
        let last = String::from_utf16(&request.messages.last().unwrap().content).unwrap();
        match self.mode {
            Mode::WebDenied => return Ok(match assistants {
                0 => json!({"op":"web_status"}),
                1 => { let r:Json=serde_json::from_str(&last).unwrap(); assert_eq!(r["output"]["publicationEnabled"],false); assert!(r["output"]["current"].is_null()); json!({"op":"publish_web","method":"Renraku.Guest.Website::Handle"}) },
                2 => { let r:Json=serde_json::from_str(&last).unwrap(); assert_eq!(r["ok"],false); assert!(r["error"].as_str().unwrap().contains("--allow-web-publication")); json!({"op":"finish","message":"publication denied"}) },
                _ => return Err(Error("unexpected website denial turn".into()))
            }.to_string()),
            Mode::WebPublish => return Ok(if assistants == 0 { json!({"op":"publish_web","method":"Renraku.Guest.Website::Handle"}) } else { json!({"op":"finish","message":"publication result inspected"}) }.to_string()),
            Mode::WebStrict => {
                if assistants > 0 { let r:Json=serde_json::from_str(&last).unwrap(); assert_eq!(r["ok"],false); }
                return Ok(match assistants {
                    0 => json!({"op":"web_status","path":"/private/file"}),
                    1 => json!({"op":"publish_web","method":"Renraku.Guest.Website::Handle","codeVersion":"wrong-code"}),
                    2 => json!({"op":"publish_web","method":"Renraku.Guest.Counter::Add"}),
                    3 => json!({"op":"publish_web","method":"/private/handler.dll"}),
                    4 => json!({"op":"publish_web","method":"Renraku.Guest.Website::Handle","hostPath":"/bin/sh"}),
                    5 => json!({"op":"finish","message":"invalid publication inputs rejected"}),
                    _ => return Err(Error("unexpected strict website turn".into()))
                }.to_string());
            }
            Mode::Finish => return Ok(json!({"op":"finish","message":"done"}).to_string()),
            Mode::FinishAtLimit => return Ok(if assistants == 31 { json!({"op":"finish","message":"done"}) } else { json!({"op":"search","query":"Counter","limit":1}) }.to_string()),
            Mode::Followup => return Ok(match assistants {
                6 => { assert_eq!(last, "Use the installed caller again, then finish."); json!({"op":"read_code","method":"Generated.Entry::Run"}) },
                7 => { let r:Json=serde_json::from_str(&last).unwrap(); assert_eq!(r["output"]["sourceAvailable"],true); json!({"op":"invoke","method":"Generated.Entry::Run","arguments":[{"kind":"static","field":"Renraku.Guest.Simulation::Saved"}]}) },
                8 => { let r:Json=serde_json::from_str(&last).unwrap(); assert_eq!(r["result"]["value"],33); json!({"op":"finish","message":"Counter advanced to 33"}) },
                _ => return Err(Error("unexpected follow-up model turn".into()))
            }.to_string()),
            Mode::Unsafe=>return Ok(if assistants==0 {json!({"op":"compile","assembly":"Evil","source":"public class X { }","compilerPath":"/bin/sh"})}else{let result:Json=serde_json::from_str(&last).unwrap();assert_eq!(result["ok"],false);json!({"op":"finish","message":"unsupported action rejected"})}.to_string()),
            Mode::Repeat=>return Ok(json!({"op":"search","query":"Counter","limit":1}).to_string()),
            Mode::Huge=>return Ok("x".repeat(65537)),
            _=>{},
        }
        let action = match assistants {
            0 => json!({"op":"search","query":"counter","limit":20}),
            1 => {
                let result: Json = serde_json::from_str(&last).unwrap();
                let method = result["output"]["symbols"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|symbol| symbol["name"] == "Renraku.Guest.Counter::Add")
                    .unwrap();
                json!({"op":"read_code","method":method["codeVersion"]})
            }
            2 => {
                let result: Json = serde_json::from_str(&last).unwrap();
                assert_eq!(result["output"]["name"], "Add");
                assert!(
                    !result["output"]["instructions"]
                        .as_array()
                        .unwrap()
                        .is_empty()
                );
                let source = if matches!(self.mode, Mode::ModelDenied) {
                    "namespace Generated { public static class Entry { public static int Run(Renraku.Guest.Counter value) { Renraku.Model.Complete(\"hidden request\"); return value.Add(7); } } }"
                } else {
                    "namespace Generated { public static class Entry { public static int Run(Renraku.Guest.Counter value) { return value.Add(7); } } }"
                };
                json!({"op":"compile","assembly":"Agent.Generated","source":source,"referenceTypes":[result["output"]["owner"].clone()]})
            }
            3 => {
                let result: Json = serde_json::from_str(&last).unwrap();
                assert_eq!(result["output"]["state"], "compiled");
                json!({"op":"install","compilationTurn":3})
            }
            4 => {
                let result: Json = serde_json::from_str(&last).unwrap();
                assert_eq!(result["ok"], true);
                json!({"op":"invoke","method":"Generated.Entry::Run","arguments":[{"kind":"static","field":"Renraku.Guest.Simulation::Saved"}]})
            }
            5 => {
                let result: Json = serde_json::from_str(&last).unwrap();
                if matches!(self.mode, Mode::ModelDenied) {
                    assert_eq!(result["ok"], false);
                    assert!(result["error"].as_str().unwrap().contains("disabled"));
                } else {
                    assert_eq!(result["result"]["value"], 26);
                }
                json!({"op":"finish","message":"Counter advanced to 26"})
            }
            _ => return Err(Error("unexpected extra model turn".into())),
        };
        Ok(action.to_string())
    }
    fn submit_inner(&mut self, request: &ModelRequest) -> Result<Vec<u16>> {
        let body = request.encode()?;
        let text = self.response(request)?;
        self.db.execute(
            "INSERT INTO receipt VALUES(?1,?2,?3)",
            params![
                request.id.to_string(),
                body,
                if self.no_receipt { None } else { Some(&text) }
            ],
        )?;
        if self.lose_reply {
            self.lose_reply = false;
            return Err(Error("lost provider reply".into()));
        }
        Ok(text.encode_utf16().collect())
    }
    fn receipts(&self) -> i64 {
        self.db
            .query_row("SELECT count(*) FROM receipt", [], |row| row.get(0))
            .unwrap()
    }
}

#[tokio::test(flavor = "current_thread")]
async fn website_publication_is_separate_default_denied_authority_with_bounded_inspection() {
    let test = Test::new();
    assert!(!AgentHost::default().allow_web_publication);
    let mut provider = test.provider();
    provider.mode = Mode::WebDenied;
    let report = test.host.drive(&mut provider, limits()).await.unwrap();
    assert!(matches!(report.stop, AgentStop::Finished(_)));
    let mut store = CheckpointStore::open(&test.host.universe).unwrap();
    let status = renraku_vm::web::inspect_publications(&mut store).unwrap();
    assert!(status["current"].is_null());
    let saved = store.load().unwrap();
    let vm = saved.snapshot.restore().unwrap();
    assert_eq!(vm.installations().count(), 0);
    let transcript = vm.agent_status().unwrap()["transcript"].to_string();
    assert!(
        transcript.contains("publicationEnabled") && transcript.contains("--allow-web-publication")
    );
}

#[tokio::test(flavor = "current_thread")]
async fn website_actions_reject_unknown_fields_wrong_code_invalid_abi_and_host_paths() {
    let mut test = Test::new();
    test.host.allow_web_publication = true;
    let mut provider = test.provider();
    provider.mode = Mode::WebStrict;
    assert!(matches!(
        test.host.drive(&mut provider, limits()).await.unwrap().stop,
        AgentStop::Finished(_)
    ));
    let mut store = CheckpointStore::open(&test.host.universe).unwrap();
    assert!(renraku_vm::web::inspect_publications(&mut store).unwrap()["current"].is_null());
}

#[tokio::test(flavor = "current_thread")]
async fn publication_recovery_without_authority_only_reconciles_committed_receipts() {
    use renraku_vm::web::{inspect_publications, publish_handler};
    for committed in [false, true] {
        let mut test = Test::new();
        test.host.allow_web_publication = true;
        let mut provider = test.provider();
        provider.mode = Mode::WebPublish;
        let stop = if committed {
            AgentStage::ToolCompleted(1)
        } else {
            AgentStage::ToolStarted(1)
        };
        assert!(
            test.host
                .drive_with_observer(&mut provider, limits(), |stage| {
                    if stage == stop {
                        Err(Error("lost host controller".into()))
                    } else {
                        Ok(())
                    }
                })
                .await
                .unwrap_err()
                .0
                .contains("lost host controller")
        );
        let mut store = CheckpointStore::open(&test.host.universe).unwrap();
        let before = inspect_publications(&mut store).unwrap();
        let saved = store.load().unwrap();
        let mut vm = saved.snapshot.restore().unwrap();
        let state = vm.agent_status().unwrap();
        let frozen: Json = serde_json::from_str(state["pendingInput"].as_str().unwrap()).unwrap();
        assert_eq!(frozen["op"], "publish_web");
        assert_eq!(frozen["method"].as_str().unwrap().len(), 32);
        assert_eq!(frozen["codeVersion"].as_str().unwrap().len(), 32);
        vm.collect_garbage().unwrap();
        store.save(&vm).unwrap();
        if committed {
            assert_eq!(before["current"]["method"], frozen["method"]);
            publish_handler(
                &mut store,
                "operator-newer",
                "Renraku.Guest.Website::Alternate",
            )
            .unwrap();
        } else {
            assert!(before["current"].is_null());
        }
        test.host.allow_web_publication = false;
        test.host.compiler_driver = test.directory.join("missing-compiler.dll");
        assert!(matches!(
            test.host.drive(&mut provider, limits()).await.unwrap().stop,
            AgentStop::Finished(_)
        ));
        let status = inspect_publications(&mut store).unwrap();
        let saved = store.load().unwrap();
        let vm = saved.snapshot.restore().unwrap();
        let transcript = vm.agent_status().unwrap()["transcript"].clone();
        let result: Json =
            serde_json::from_str(transcript[3]["content"].as_str().unwrap()).unwrap();
        assert_eq!(result["ok"], committed);
        if committed {
            assert_eq!(result["publication"], before["current"]);
            assert_eq!(status["current"]["id"], "operator-newer");
            assert_eq!(status["publications"].as_array().unwrap().len(), 2);
        } else {
            assert!(status["current"].is_null());
        }
    }
}

fn copy_world(source: &Path, target: &Path) {
    let db = Connection::open(source).unwrap();
    let row: (Vec<u8>, u32, Vec<u8>, Vec<u8>) = db
        .query_row(
            "SELECT universe,format,digest,payload FROM checkpoint",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .unwrap();
    let _store = CheckpointStore::open(target).unwrap();
    Connection::open(target)
        .unwrap()
        .execute(
            "INSERT INTO checkpoint VALUES(1,1,?1,?2,?3,?4)",
            params![row.0, row.1, row.2, row.3],
        )
        .unwrap();
}

#[tokio::test(flavor = "current_thread")]
async fn followup_goals_reuse_objects_code_and_monotonic_receipts_without_duplicate_input() {
    let mut test = Test::new();
    let mut provider = test.provider();
    assert!(matches!(
        test.host.drive(&mut provider, limits()).await.unwrap().stop,
        AgentStop::Finished(_)
    ));
    let mut store = CheckpointStore::open(&test.host.universe).unwrap();
    let saved = store.load().unwrap();
    let vm = saved.snapshot.restore().unwrap();
    let (original_id, original) = vm.invocations().next().unwrap();
    let original_id = original_id.to_owned();
    let original = original.clone();
    let goal = "Use the installed caller again, then finish.";
    let accepted = submit_input(&mut store, "goal-2", goal, 1000).unwrap();
    assert_eq!(accepted["state"], "accepted");
    assert_eq!(accepted["afterTurn"], 6);
    let replay = submit_input(&mut store, "goal-2", goal, 0).unwrap();
    assert_eq!(replay["execution"], accepted["execution"]);
    assert_eq!(replay["instructions"], 0);
    let saved = store.load().unwrap();
    let mut vm = saved.snapshot.restore().unwrap();
    assert_eq!(
        vm.agent_status().unwrap()["inputs"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(vm.agent_status().unwrap()["turn"], 6);
    vm.collect_garbage().unwrap();
    store.save(&vm).unwrap();
    drop(vm);
    drop(store);
    test.host.compiler_driver = test.directory.join("missing-compiler.dll");
    provider.mode = Mode::Followup;
    let report = test.host.drive(&mut provider, limits()).await.unwrap();
    assert!(matches!(report.stop, AgentStop::Finished(_)));
    assert_eq!(report.provider_operations, 3);
    let mut store = CheckpointStore::open(&test.host.universe).unwrap();
    let saved = store.load().unwrap();
    let vm = saved.snapshot.restore().unwrap();
    let status = vm.agent_status().unwrap();
    assert_eq!(status["turn"], 9);
    assert_eq!(status["transcript"].as_array().unwrap().len(), 21);
    assert_eq!(
        status["inputs"][0],
        json!({"id":"goal-2","content":goal,"afterTurn":6})
    );
    assert_eq!(vm.invocation(&original_id), Some(&original));
    assert_eq!(vm.installations().count(), 1);
    assert_eq!(vm.invocations().count(), 3);
    assert_eq!(provider.receipts(), 9);
    let before = store.generation();
    assert!(
        submit_input(&mut store, "goal-2", "different content", 1000)
            .unwrap_err()
            .0
            .contains("different content")
    );
    assert_eq!(store.load().unwrap().generation, before);
    let replay = submit_input(&mut store, "goal-2", goal, 1000).unwrap();
    assert_eq!(replay["afterTurn"], 6);
    assert_eq!(replay["instructions"], 0);
    let saved = store.load().unwrap();
    let vm = saved.snapshot.restore().unwrap();
    assert_eq!(vm.agent_status().unwrap()["phase"], 3);
    assert_eq!(
        vm.agent_status().unwrap()["inputs"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
}

#[tokio::test(flavor = "current_thread")]
async fn followup_append_recovers_every_cil_instruction_and_retains_pending_argument_roots() {
    let test = Test::new();
    let mut provider = test.provider();
    provider.mode = Mode::Finish;
    test.host.drive(&mut provider, limits()).await.unwrap();
    let prototype = test.directory.join("prototype.db");
    copy_world(&test.host.universe, &prototype);
    let mut store = CheckpointStore::open(&prototype).unwrap();
    let complete = submit_input(&mut store, "input", "a new goal\0🦀", 1000).unwrap();
    let instructions = complete["instructions"].as_u64().unwrap();
    assert!(instructions > 20);
    for cut in 0..=instructions {
        let database = test.directory.join(format!("cut-{cut}.db"));
        copy_world(&test.host.universe, &database);
        let mut store = CheckpointStore::open(&database).unwrap();
        let first = submit_input(&mut store, "input", "a new goal\0🦀", cut).unwrap();
        assert!(first["instructions"].as_u64().unwrap() <= cut);
        let saved = store.load().unwrap();
        let mut vm = saved.snapshot.restore().unwrap();
        vm.collect_garbage().unwrap();
        store.save(&vm).unwrap();
        drop(vm);
        drop(store);
        let mut store = CheckpointStore::open(&database).unwrap();
        let replay = submit_input(&mut store, "input", "a new goal\0🦀", 1000).unwrap();
        assert_eq!(replay["state"], "accepted");
        assert_eq!(replay["execution"], first["execution"]);
        assert_eq!(
            first["instructions"].as_u64().unwrap() + replay["instructions"].as_u64().unwrap(),
            instructions
        );
        let saved = store.load().unwrap();
        let vm = saved.snapshot.restore().unwrap();
        let status = vm.agent_status().unwrap();
        assert_eq!(status["inputs"].as_array().unwrap().len(), 1);
        assert_eq!(status["transcript"].as_array().unwrap().len(), 5);
        assert_eq!(status["turn"], 1);
    }
    let mut store = CheckpointStore::open(&test.host.universe).unwrap();
    let pending = submit_input(&mut store, "driver-owned", "Finish a second goal.", 1).unwrap();
    assert_eq!(pending["state"], "pending");
    drop(store);
    let report = test.host.drive(&mut provider, limits()).await.unwrap();
    assert!(matches!(report.stop, AgentStop::Finished(_)));
    let mut store = CheckpointStore::open(&test.host.universe).unwrap();
    let saved = store.load().unwrap();
    let vm = saved.snapshot.restore().unwrap();
    assert_eq!(vm.agent_status().unwrap()["turn"], 2);
    assert_eq!(
        vm.agent_status().unwrap()["inputs"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    let replay = submit_input(&mut store, "driver-owned", "Finish a second goal.", 0).unwrap();
    assert_eq!(replay["execution"], pending["execution"]);
    assert_eq!(replay["state"], "accepted");
}

#[tokio::test(flavor = "current_thread")]
async fn input_cli_retries_are_safe_and_new_inputs_require_finished_goals_within_lifetime_limits() {
    let test = Test::new();
    let mut provider = test.provider();
    provider.mode = Mode::Finish;
    let mut store = CheckpointStore::open(&test.host.universe).unwrap();
    let before = store.generation();
    assert!(submit_input(&mut store, "early", "goal", 1000).is_err());
    assert_eq!(store.load().unwrap().generation, before);
    drop(store);
    test.host.drive(&mut provider, limits()).await.unwrap();
    let cli = |budget: &str| {
        Command::new(env!("CARGO_BIN_EXE_renraku-vm"))
            .arg("agent-input")
            .arg(&test.host.universe)
            .args(["request-1", "Follow up.", budget])
            .output()
            .unwrap()
    };
    let first = cli("0");
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    let pending: Json = serde_json::from_slice(&first.stdout).unwrap();
    assert_eq!(pending["state"], "pending");
    let completed = cli("1000");
    assert!(completed.status.success());
    let accepted: Json = serde_json::from_slice(&completed.stdout).unwrap();
    assert_eq!(accepted["execution"], pending["execution"]);
    let mut store = CheckpointStore::open(&test.host.universe).unwrap();
    let before = store.generation();
    for (id, goal, budget) in [
        ("request-1", "changed", 1000),
        ("new-id", "too soon", 1000),
        ("/bad", "goal", 1000),
        ("long", "goal", 1_000_001),
        ("empty", " ", 1000),
    ] {
        assert!(submit_input(&mut store, id, goal, budget).is_err());
        assert_eq!(store.load().unwrap().generation, before);
    }
    assert!(submit_input(&mut store, "long-goal", &"x".repeat(4097), 1000).is_err());
    let capped = Test::new();
    let mut provider = capped.provider();
    provider.mode = Mode::FinishAtLimit;
    let report = capped
        .host
        .drive(
            &mut provider,
            AgentLimits {
                instructions: 100_000,
                provider_operations: 32,
                tool_actions: 32,
            },
        )
        .await
        .unwrap();
    assert!(matches!(report.stop, AgentStop::Finished(_)));
    let mut store = CheckpointStore::open(&capped.host.universe).unwrap();
    let before = store.generation();
    assert!(
        submit_input(&mut store, "over-cap", "more", 1000)
            .unwrap_err()
            .0
            .contains("turn limit")
    );
    assert_eq!(store.load().unwrap().generation, before);
}

#[tokio::test(flavor = "current_thread")]
async fn original_agent_schema_remains_readable_and_resumable_but_rejects_followup_inputs() {
    use sha2::{Digest, Sha256};
    struct Legacy;
    impl AsyncModelProvider for Legacy {
        fn capability(&self) -> &str {
            "openai.chatgpt.11111111-1111-4111-8111-111111111111"
        }
        fn supports_model(&self, model: &str) -> bool {
            model == "fixture-model"
        }
        async fn submit(&mut self, _: &ModelRequest) -> Result<Vec<u16>> {
            Ok(json!({"op":"finish","message":"legacy done"})
                .to_string()
                .encode_utf16()
                .collect())
        }
        async fn reconcile(&mut self, _: &ModelRequest) -> Result<Option<Vec<u16>>> {
            unreachable!()
        }
    }
    let test = Test::new();
    let database = test.directory.join("legacy.db");
    let mut store = CheckpointStore::open(&database).unwrap();
    let payload = include_bytes!("fixtures/checkpoint-v7-agent.bin");
    Connection::open(&database)
        .unwrap()
        .execute(
            "INSERT INTO checkpoint VALUES(1,1,?1,7,?2,?3)",
            params![
                &payload[12..28],
                Sha256::digest(payload).as_slice(),
                payload.as_slice()
            ],
        )
        .unwrap();
    let before = store.generation();
    assert!(
        submit_input(&mut store, "new", "goal", 1000)
            .unwrap_err()
            .0
            .contains("does not support follow-up")
    );
    assert_eq!(store.load().unwrap().generation, 1);
    drop(store);
    let mut host = test.host.clone();
    host.universe = database;
    assert!(matches!(
        host.drive(&mut Legacy, limits()).await.unwrap().stop,
        AgentStop::Finished(_)
    ));
    let mut store = CheckpointStore::open(&host.universe).unwrap();
    let saved = store.load().unwrap();
    let vm = saved.snapshot.restore().unwrap();
    assert_eq!(vm.agent_status().unwrap()["followupSupported"], false);
    assert_eq!(vm.agent_status().unwrap()["phase"], 3);
    assert!(store.generation() > before);
}
impl AsyncModelProvider for Script {
    fn capability(&self) -> &str {
        "test.agent"
    }
    fn supports_model(&self, model: &str) -> bool {
        model == "script-v1"
    }
    async fn submit(&mut self, request: &ModelRequest) -> Result<Vec<u16>> {
        self.submit_inner(request)
    }
    async fn reconcile(&mut self, request: &ModelRequest) -> Result<Option<Vec<u16>>> {
        let record: Option<(Vec<u8>, Option<String>)> = self
            .db
            .query_row(
                "SELECT body,response FROM receipt WHERE id=?1",
                [request.id.to_string()],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        if let Some((body, text)) = record {
            assert_eq!(body, request.encode()?);
            Ok(text.map(|text| text.encode_utf16().collect()))
        } else {
            Ok(None)
        }
    }
}
fn limits() -> AgentLimits {
    AgentLimits {
        instructions: 10000,
        provider_operations: 10,
        tool_actions: 20,
    }
}

#[tokio::test(flavor = "current_thread")]
async fn model_selected_search_inspect_compile_install_invoke_and_finish_are_persistent_cil_turns()
{
    let test = Test::new();
    let mut provider = test.provider();
    let report = test.host.drive(&mut provider, limits()).await.unwrap();
    assert!(
        matches!(report.stop, AgentStop::Finished(_)),
        "{:?}",
        report.stop
    );
    assert_eq!(provider.receipts(), 6);
    let mut store = CheckpointStore::open(&test.host.universe).unwrap();
    let saved = store.load().unwrap();
    let mut vm = saved.snapshot.restore().unwrap();
    let status = vm.agent_status().unwrap();
    assert_eq!(status["turn"], 6);
    assert_eq!(status["phase"], 3);
    assert_eq!(status["transcript"].as_array().unwrap().len(), 14);
    let field = saved
        .snapshot
        .program
        .fields()
        .find(|field| {
            field.name == "Saved"
                && saved
                    .snapshot
                    .program
                    .types()
                    .find(|ty| ty.id == field.owner)
                    .unwrap()
                    .name
                    == "Renraku.Guest.Simulation"
        })
        .unwrap()
        .id;
    let Value::Reference(counter) = vm.static_fields()[&field] else {
        panic!()
    };
    let value = saved
        .snapshot
        .program
        .fields()
        .find(|f| f.name == "Value" && f.owner == vm.heap().object(counter).unwrap().type_id())
        .unwrap()
        .id;
    assert_eq!(
        vm.heap().object(counter).unwrap().fields()[&value],
        Value::I32(26)
    );
    assert_eq!(vm.installations().count(), 1);
    assert_eq!(vm.invocations().count(), 1);
    vm.collect_garbage().unwrap();
    store.save(&vm).unwrap();
    drop(vm);
    let result = test.host.drive(&mut provider, limits()).await.unwrap();
    assert!(matches!(result.stop, AgentStop::Finished(_)));
    assert_eq!(result.provider_operations, 0);
    assert_eq!(provider.receipts(), 6);
}

#[tokio::test(flavor = "current_thread")]
async fn lost_tool_results_replay_receipts_across_restarts_without_new_model_requests_or_mutations()
{
    let mut test = Test::new();
    for turn in 1..=5 {
        let mut provider = test.provider();
        let error = test
            .host
            .drive_with_observer(&mut provider, limits(), |stage| {
                if stage == AgentStage::ToolCompleted(turn) {
                    Err(Error("lost tool result before guest apply".into()))
                } else {
                    Ok(())
                }
            })
            .await
            .unwrap_err();
        assert!(error.0.contains("lost tool result"));
        let mut store = CheckpointStore::open(&test.host.universe).unwrap();
        let saved = store.load().unwrap();
        let mut vm = saved.snapshot.restore().unwrap();
        assert_eq!(vm.agent_status().unwrap()["turn"], turn);
        assert_eq!(vm.agent_status().unwrap()["phase"], 1);
        vm.collect_garbage().unwrap();
        store.save(&vm).unwrap();
        if turn == 3 {
            test.host.compiler_driver = PathBuf::from("/missing/Renraku.Compiler.dll");
        }
    }
    let mut provider = test.provider();
    let result = test.host.drive(&mut provider, limits()).await.unwrap();
    assert!(matches!(result.stop, AgentStop::Finished(_)));
    assert_eq!(provider.receipts(), 6);
    let mut store = CheckpointStore::open(&test.host.universe).unwrap();
    let saved = store.load().unwrap();
    let vm = saved.snapshot.restore().unwrap();
    assert_eq!(vm.invocations().count(), 1);
    assert_eq!(vm.installations().count(), 1);
}

#[tokio::test(flavor = "current_thread")]
async fn invocation_arguments_freeze_before_execution_and_do_not_retarget_changed_statics_on_replay()
 {
    let test = Test::new();
    let mut provider = test.provider();
    test.host
        .drive_with_observer(&mut provider, limits(), |stage| {
            if stage == AgentStage::ToolCompleted(5) {
                Err(Error("lost invocation reply".into()))
            } else {
                Ok(())
            }
        })
        .await
        .unwrap_err();
    let mut store = CheckpointStore::open(&test.host.universe).unwrap();
    let saved = store.load().unwrap();
    let mut vm = saved.snapshot.restore().unwrap();
    let Value::Reference(original) = vm.invocations().next().unwrap().1.arguments[0] else {
        panic!()
    };
    vm.start(entry(&saved.snapshot.program, "SaveObject"), vec![])
        .unwrap();
    assert_eq!(
        vm.run_checkpointed(&mut store, 1000, 10).unwrap(),
        RunOutcome::Completed(Some(Value::I32(19)))
    );
    drop(vm);
    drop(saved);
    drop(store);
    assert!(matches!(
        test.host.drive(&mut provider, limits()).await.unwrap().stop,
        AgentStop::Finished(_)
    ));
    let mut store = CheckpointStore::open(&test.host.universe).unwrap();
    let saved = store.load().unwrap();
    let vm = saved.snapshot.restore().unwrap();
    let value = saved
        .snapshot
        .program
        .fields()
        .find(|f| f.name == "Value" && f.owner == vm.heap().object(original).unwrap().type_id())
        .unwrap()
        .id;
    assert_eq!(
        vm.heap().object(original).unwrap().fields()[&value],
        Value::I32(26)
    );
    let counter = saved
        .snapshot
        .program
        .fields()
        .find(|f| {
            f.name == "Saved"
                && f.value_type
                    == renraku_vm::program::SlotType::Reference(
                        vm.heap().object(original).unwrap().type_id(),
                    )
        })
        .unwrap()
        .id;
    let Value::Reference(current) = vm.static_fields()[&counter] else {
        panic!()
    };
    assert_ne!(current, original);
    assert_eq!(
        vm.heap().object(current).unwrap().fields()[&value],
        Value::I32(19)
    );
}

#[tokio::test(flavor = "current_thread")]
async fn uncertain_model_outcomes_reconcile_receipts_and_never_resubmit_when_receipts_are_missing()
{
    for missing in [false, true] {
        let test = Test::new();
        let mut provider = test.provider();
        provider.lose_reply = true;
        provider.no_receipt = missing;
        let error = test.host.drive(&mut provider, limits()).await.unwrap_err();
        assert!(error.0.contains("uncertain"));
        assert_eq!(provider.receipts(), 1);
        if missing {
            for _ in 0..2 {
                assert!(matches!(
                    test.host.drive(&mut provider, limits()).await.unwrap().stop,
                    AgentStop::OutcomeUnknown(_)
                ));
                assert_eq!(provider.receipts(), 1);
            }
            assert!(!test.host.compiler_ledger.exists());
        } else {
            assert!(matches!(
                test.host.drive(&mut provider, limits()).await.unwrap().stop,
                AgentStop::Finished(_)
            ));
            assert_eq!(provider.receipts(), 6);
        }
    }
}

#[tokio::test(flavor = "current_thread")]
async fn budgets_and_strict_action_schema_bound_progress_and_prevent_model_supplied_host_paths() {
    let test = Test::new();
    let mut provider = test.provider();
    assert_eq!(
        test.host
            .drive(
                &mut provider,
                AgentLimits {
                    instructions: 0,
                    provider_operations: 10,
                    tool_actions: 20
                }
            )
            .await
            .unwrap()
            .stop,
        AgentStop::InstructionBudget
    );
    assert_eq!(provider.receipts(), 0);
    assert_eq!(
        test.host
            .drive(
                &mut provider,
                AgentLimits {
                    instructions: 10000,
                    provider_operations: 0,
                    tool_actions: 20
                }
            )
            .await
            .unwrap()
            .stop,
        AgentStop::ProviderBudget
    );
    assert_eq!(provider.receipts(), 0);
    assert_eq!(
        test.host
            .drive(
                &mut provider,
                AgentLimits {
                    instructions: 10000,
                    provider_operations: 1,
                    tool_actions: 0
                }
            )
            .await
            .unwrap()
            .stop,
        AgentStop::ToolBudget
    );
    assert_eq!(provider.receipts(), 1);
    let mut provider = test.provider();
    assert!(matches!(
        test.host.drive(&mut provider, limits()).await.unwrap().stop,
        AgentStop::Finished(_)
    ));
    let mut test = Test::new();
    test.host.compiler_driver = PathBuf::from("/missing/compiler.dll");
    let mut provider = test.provider();
    provider.mode = Mode::Unsafe;
    assert!(matches!(
        test.host.drive(&mut provider, limits()).await.unwrap().stop,
        AgentStop::Finished(_)
    ));
    assert_eq!(provider.receipts(), 2);
    assert!(!test.host.compiler_ledger.exists());
}

#[tokio::test(flavor = "current_thread")]
async fn generated_code_does_not_inherit_the_agent_model_capability() {
    let test = Test::new();
    let mut provider = test.provider();
    provider.mode = Mode::ModelDenied;
    assert!(matches!(
        test.host.drive(&mut provider, limits()).await.unwrap().stop,
        AgentStop::Finished(_)
    ));
    assert_eq!(provider.receipts(), 6);
    let mut store = CheckpointStore::open(&test.host.universe).unwrap();
    let saved = store.load().unwrap();
    let vm = saved.snapshot.restore().unwrap();
    assert_eq!(vm.effects().count(), 6);
    assert!(matches!(
        vm.invocations().next().unwrap().1.state,
        renraku_vm::invocation::InvocationState::Faulted { .. }
    ));
}

#[tokio::test(flavor = "current_thread")]
async fn compiler_workers_yield_to_other_universes_on_the_same_executor_thread() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let test = Test::new();
    let mut provider = test.provider();
    let ticks = AtomicUsize::new(0);
    let mut before = 0;
    let loop_run = test
        .host
        .drive_with_observer(&mut provider, limits(), |stage| {
            if stage == AgentStage::ToolStarted(3) {
                before = ticks.load(Ordering::SeqCst);
            }
            if stage == AgentStage::ToolCompleted(3) {
                assert!(
                    ticks.load(Ordering::SeqCst) > before,
                    "compiler blocked the executor"
                );
            }
            Ok(())
        });
    let progress = async {
        for _ in 0..200 {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            let program = program();
            let mut vm = Vm::new(&program);
            vm.start(entry(&program, "Run"), vec![]).unwrap();
            assert_eq!(
                vm.run(1000).unwrap(),
                RunOutcome::Completed(Some(Value::I32(48)))
            );
            ticks.fetch_add(1, Ordering::SeqCst);
        }
    };
    let (result, ()) = tokio::join!(loop_run, progress);
    assert!(matches!(result.unwrap().stop, AgentStop::Finished(_)));
}

#[tokio::test(flavor = "current_thread")]
async fn lifetime_turn_and_transcript_limits_stop_repeated_or_oversized_actions() {
    for (mode, stop) in [
        (Mode::Repeat, AgentStop::TurnLimit),
        (Mode::Huge, AgentStop::TranscriptLimit),
    ] {
        let mut test = Test::new();
        test.host.compiler_driver = PathBuf::from("/missing/compiler.dll");
        let mut provider = test.provider();
        provider.mode = mode;
        let result = test
            .host
            .drive(
                &mut provider,
                AgentLimits {
                    instructions: 100000,
                    provider_operations: 64,
                    tool_actions: 32,
                },
            )
            .await
            .unwrap();
        assert_eq!(result.stop, stop);
        assert!(provider.receipts() <= 32);
        assert!(!test.host.compiler_ledger.exists());
    }
}

#[tokio::test(flavor = "current_thread")]
async fn guest_turn_updates_restore_from_small_instruction_budgets() {
    let test = Test::new();
    let mut provider = test.provider();
    let mut finished = false;
    let mut delivered = std::collections::BTreeSet::new();
    for _ in 0..1000 {
        let report = test
            .host
            .drive_with_progress(
                &mut provider,
                AgentLimits {
                    instructions: 7,
                    provider_operations: 1,
                    tool_actions: 1,
                },
                |event| {
                    let mut store = CheckpointStore::open(&test.host.universe)?;
                    let saved = store.load()?;
                    let vm = saved.snapshot.restore()?;
                    let status = vm.agent_status()?;
                    assert_eq!(status["turn"], event.turn);
                    if event.kind == AgentEventKind::ResultSaved {
                        assert!(status["result"].as_str().is_some());
                        assert!(matches!(
                            vm.context().status,
                            renraku_vm::execution::ExecutionStatus::Completed { .. }
                                | renraku_vm::execution::ExecutionStatus::Waiting { .. }
                        ));
                    }
                    delivered.insert(event.id);
                    Ok(())
                },
            )
            .await
            .unwrap();
        assert!(
            report.instructions <= 7 && report.provider_operations <= 1 && report.tool_actions <= 1
        );
        if matches!(report.stop, AgentStop::Finished(_)) {
            finished = true;
            break;
        }
    }
    assert!(
        finished,
        "agent did not resume its partially updated guest turns"
    );
    assert_eq!(provider.receipts(), 6);
    assert_eq!(delivered.len(), 12);
}

#[tokio::test(flavor = "current_thread")]
async fn losing_committed_progress_output_replays_its_id_without_repeating_the_invocation() {
    let test = Test::new();
    let mut provider = test.provider();
    let mut lost = None;
    let error = test
        .host
        .drive_with_progress(&mut provider, limits(), |event| {
            if event.turn == 5 && event.kind == AgentEventKind::ResultSaved {
                lost = Some(event.id);
                return Err(Error("lost console output".into()));
            }
            Ok(())
        })
        .await
        .unwrap_err();
    assert!(error.0.contains("lost console output"));
    let mut store = CheckpointStore::open(&test.host.universe).unwrap();
    let saved = store.load().unwrap();
    let vm = saved.snapshot.restore().unwrap();
    let (id, original) = vm.invocations().next().unwrap();
    let id = id.to_owned();
    let original = original.clone();
    assert_eq!(vm.agent_status().unwrap()["phase"], 2);
    let mut replayed = false;
    let report = test
        .host
        .drive_with_progress(&mut provider, limits(), |event| {
            if Some(&event.id) == lost.as_ref() {
                replayed = true;
            }
            Ok(())
        })
        .await
        .unwrap();
    assert!(matches!(report.stop, AgentStop::Finished(_)) && replayed);
    let mut store = CheckpointStore::open(&test.host.universe).unwrap();
    let saved = store.load().unwrap();
    let vm = saved.snapshot.restore().unwrap();
    assert_eq!(vm.invocation(&id), Some(&original));
    assert_eq!(provider.receipts(), 6);
}

#[tokio::test(flavor = "current_thread")]
async fn frozen_object_arguments_remain_gc_roots_before_the_first_invocation_instruction() {
    let test = Test::new();
    let mut provider = test.provider();
    test.host
        .drive_with_observer(&mut provider, limits(), |stage| {
            if stage == AgentStage::ToolStarted(5) {
                Err(Error("pause before invocation".into()))
            } else {
                Ok(())
            }
        })
        .await
        .unwrap_err();
    let mut store = CheckpointStore::open(&test.host.universe).unwrap();
    let saved = store.load().unwrap();
    let mut vm = saved.snapshot.restore().unwrap();
    assert_eq!(vm.invocations().count(), 0);
    let input: Json =
        serde_json::from_str(vm.agent_status().unwrap()["pendingInput"].as_str().unwrap()).unwrap();
    let original = input["arguments"][0]["id"].as_str().unwrap().to_owned();
    vm.start(entry(&saved.snapshot.program, "SaveObject"), vec![])
        .unwrap();
    vm.run_checkpointed(&mut store, 1000, 10).unwrap();
    vm.collect_garbage().unwrap();
    store.save(&vm).unwrap();
    assert!(
        vm.heap()
            .objects()
            .any(|object| object.id().to_string() == original)
    );
    drop(vm);
    drop(saved);
    drop(store);
    assert!(matches!(
        test.host.drive(&mut provider, limits()).await.unwrap().stop,
        AgentStop::Finished(_)
    ));
    assert_eq!(provider.receipts(), 6);
}
