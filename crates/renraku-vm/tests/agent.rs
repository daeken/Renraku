#[allow(dead_code)]
mod common;
use common::{entry, fixtures, program};
use renraku_vm::{
    agent::{AgentEventKind, AgentHost, AgentLimits, AgentStage, AgentStop},
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
