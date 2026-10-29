mod common;
use common::{entry, fixtures, program};
use renraku_vm::{
    checkpoint::CheckpointStore,
    effects::{EffectState, LocalModelProvider, ModelProvider, ModelRequest},
    error::{Error, Result},
    execution::{Continuation, ExecutionStatus},
    identity::{EffectId, UniverseId},
    runtime::{RunOutcome, Vm},
    value::Value,
};
use std::{
    io::{BufRead, BufReader},
    path::PathBuf,
    process::{Child, Command, Stdio},
    sync::mpsc,
    time::Duration,
};

struct Databases {
    directory: PathBuf,
    universe: PathBuf,
    provider: PathBuf,
}

fn start_harness<'a>(program: &'a renraku_vm::program::Program) -> Vm<'a> {
    let mut vm = Vm::new(program);
    vm.set_model_calls_enabled(true);
    vm.start(entry(program, "ModelHarness"), Vec::new())
        .unwrap();
    vm
}

fn assert_conversation(vm: &Vm<'_>, program: &renraku_vm::program::Program) {
    let field = |owner: &str, name: &str| {
        let owner = program
            .types()
            .find(|t| t.name == format!("Renraku.Guest.{owner}"))
            .unwrap()
            .id;
        program
            .fields()
            .find(|f| f.owner == owner && f.name == name)
            .unwrap()
            .id
    };
    let Value::Reference(conversation) =
        vm.static_fields()[&field("Simulation", "LastConversation")]
    else {
        panic!("missing conversation");
    };
    let conversation = vm.heap().object(conversation).unwrap();
    assert_eq!(
        conversation.fields()[&field("ModelConversation", "Count")],
        Value::I32(4)
    );
    let mut next = conversation.fields()[&field("ModelConversation", "First")];
    let mut texts = Vec::new();
    let mut ids = std::collections::BTreeSet::new();
    while let Value::Reference(id) = next {
        assert!(ids.insert(id), "conversation unexpectedly cycles");
        let message = vm.heap().object(id).unwrap();
        let Value::Reference(text) = message.fields()[&field("ModelMessage", "Text")] else {
            panic!("missing message text");
        };
        texts.push(
            vm.heap()
                .object(text)
                .unwrap()
                .string_units()
                .unwrap()
                .to_vec(),
        );
        next = message.fields()[&field("ModelMessage", "Next")];
        if next == Value::Null {
            assert_eq!(
                conversation.fields()[&field("ModelConversation", "Last")],
                Value::Reference(id)
            );
        }
    }
    let expected: Vec<Vec<u16>> = ["hello", "echo: hello", "echo: hello", "echo: echo: hello"]
        .into_iter()
        .map(|s| s.encode_utf16().collect())
        .collect();
    assert_eq!(texts, expected);
}

#[test]
fn linked_cil_conversation_restores_at_every_instruction_and_effect_boundary() {
    let program = program();
    let databases = Databases::new();
    let mut store = CheckpointStore::open(&databases.universe).unwrap();
    let mut provider = LocalModelProvider::open(&databases.provider).unwrap();
    let vm = start_harness(&program);
    let universe = vm.universe_id();
    let execution = vm.context().id;
    store.save(&vm).unwrap();
    let mut delivered = Vec::new();
    for _ in 0..1000 {
        let saved = store.load().unwrap();
        let mut vm = saved.snapshot.restore().unwrap();
        vm.set_model_calls_enabled(true);
        assert_eq!(vm.universe_id(), universe);
        assert_eq!(vm.context().id, execution);
        let outcome = vm.run(1).unwrap();
        store.save(&vm).unwrap();
        match outcome {
            RunOutcome::Paused => {}
            RunOutcome::Waiting(id) => {
                assert!(vm.context().frames.len() >= 3);
                let before = vm.context().clone();
                let instructions = vm.instructions_executed();
                assert_eq!(vm.run(1000).unwrap(), outcome);
                assert_eq!(vm.context(), &before);
                assert_eq!(vm.instructions_executed(), instructions);
                assert_eq!(provider.submission_attempts(id).unwrap(), 0);
                let saved = store.load().unwrap();
                let mut vm = saved.snapshot.restore().unwrap();
                assert_eq!(vm.context(), &before);
                let response = vm.dispatch_model(&mut store, id, &mut provider).unwrap();
                assert_eq!(vm.context().status, ExecutionStatus::Runnable);
                assert_eq!(vm.instructions_executed(), instructions);
                assert_eq!(provider.submission_attempts(id).unwrap(), 1);
                let saved = store.load().unwrap();
                let vm = saved.snapshot.restore().unwrap();
                assert_eq!(vm.context().status, ExecutionStatus::Runnable);
                let frame = vm.context().frames.last().unwrap();
                assert!(matches!(frame.continuation, Continuation::Execute { .. }));
                assert_eq!(
                    frame.evaluation_stack.last(),
                    Some(&Value::Reference(response))
                );
                delivered.push(id);
            }
            RunOutcome::Completed(result) => {
                assert_eq!(
                    result,
                    Some(Value::I32(fixtures().reference["ModelHarness"] as i32))
                );
                assert_eq!(result, Some(Value::I32(417)));
                assert_eq!(delivered.len(), 2);
                assert_ne!(delivered[0], delivered[1]);
                assert_conversation(&vm, &saved.snapshot.program);
                let EffectState::Completed { response } = vm.effect(delivered[0]).unwrap().state
                else {
                    panic!()
                };
                assert_eq!(vm.effect(delivered[1]).unwrap().request, response);
                for id in delivered {
                    assert_eq!(provider.submission_attempts(id).unwrap(), 1);
                    vm.forget_completed_effect(id).unwrap();
                }
                vm.collect_garbage().unwrap();
                store.save(&vm).unwrap();
                let saved = store.load().unwrap();
                let vm = saved.snapshot.restore().unwrap();
                assert_eq!(vm.effects().count(), 0);
                assert_conversation(&vm, &saved.snapshot.program);
                return;
            }
        }
    }
    panic!("harness did not complete");
}

#[test]
fn disabled_calls_and_null_prompts_fault_without_publishing_intent_or_changing_operands() {
    let program = program();
    for (name, enabled, error_text) in [
        ("ModelHarness", false, "disabled"),
        ("NullModelPrompt", true, "non-null"),
    ] {
        let mut vm = Vm::new(&program);
        vm.set_model_calls_enabled(enabled);
        vm.start(entry(&program, name), Vec::new()).unwrap();
        loop {
            let before = vm.context().frames.clone();
            match vm.run(1) {
                Ok(RunOutcome::Paused) => {}
                Err(error) => {
                    assert!(error.0.contains(error_text), "{error}");
                    assert_eq!(vm.context().frames, before);
                    assert_eq!(vm.effects().count(), 0);
                    break;
                }
                other => panic!("unexpected result: {other:?}"),
            }
        }
    }
    let databases = Databases::new();
    let mut store = CheckpointStore::open(&databases.universe).unwrap();
    store.save(&start_harness(&program)).unwrap();
    let saved = store.load().unwrap();
    let mut vm = saved.snapshot.restore().unwrap();
    // A saved guest universe cannot authorize future host capabilities.
    assert!(vm.run(1000).unwrap_err().0.contains("disabled"));
}

#[test]
fn model_capability_internal_calls_require_the_exact_corelib_owner_and_name() {
    use renraku_vm::program::Program;
    for (original, replacement, expected) in [
        (
            b"Complete\0".as_slice(),
            b"ComplXte\0".as_slice(),
            "invalid model intrinsic signature",
        ),
        (
            b"Model\0".as_slice(),
            b"Bogus\0".as_slice(),
            "unrecognized InternalCall",
        ),
    ] {
        let mut core = fixtures().core.clone();
        let positions: Vec<_> = core
            .windows(original.len())
            .enumerate()
            .filter(|(_, s)| *s == original)
            .map(|(i, _)| i)
            .collect();
        assert_eq!(positions.len(), 1);
        core[positions[0]..positions[0] + replacement.len()].copy_from_slice(replacement);
        let error = Program::load(&[&core, &fixtures().guest]).unwrap_err();
        assert!(error.0.contains(expected), "{error}");
    }
}

#[test]
fn killing_before_intent_commit_restores_the_previous_guest_state_without_an_effect() {
    let program = program();
    let databases = Databases::new();
    let mut store = CheckpointStore::open(&databases.universe).unwrap();
    let vm = start_harness(&program);
    store.save(&vm).unwrap();
    let mut probe = Probe::start(&databases, "intent");
    probe.kill();
    let saved = store.load().unwrap();
    let mut restored = saved.snapshot.restore().unwrap();
    assert_eq!(restored.context(), vm.context());
    assert_eq!(restored.instructions_executed(), 0);
    assert_eq!(restored.effects().count(), 0);
    restored.set_model_calls_enabled(true);
    assert!(matches!(
        restored.run(1000).unwrap(),
        RunOutcome::Waiting(_)
    ));
    assert_eq!(restored.effects().count(), 1);
}

#[test]
fn repeated_process_kills_preserve_waiting_cil_frames_receipts_and_atomic_wakeup() {
    let program = program();
    for gate in ["before", "after", "committed"] {
        let databases = Databases::new();
        let mut store = CheckpointStore::open(&databases.universe).unwrap();
        let mut vm = start_harness(&program);
        let universe = vm.universe_id();
        let execution = vm.context().id;
        let mut outcome = vm.run_checkpointed(&mut store, 1000, 100).unwrap();
        let mut delivered = 0;
        loop {
            let RunOutcome::Waiting(id) = outcome else {
                assert_eq!(
                    outcome,
                    RunOutcome::Completed(Some(Value::I32(
                        fixtures().reference["ModelHarness"] as i32
                    )))
                );
                assert_eq!(delivered, 2);
                let saved = store.load().unwrap();
                assert_conversation(&saved.snapshot.restore().unwrap(), &saved.snapshot.program);
                break;
            };
            let saved = store.load().unwrap();
            let waiting = saved.snapshot.restore().unwrap().context().clone();
            let mut probe = Probe::start(&databases, gate);
            probe.kill();
            let saved = store.load().unwrap();
            let mut vm = saved.snapshot.restore().unwrap();
            assert_eq!(vm.universe_id(), universe);
            assert_eq!(vm.context().id, execution);
            let mut provider = LocalModelProvider::open(&databases.provider).unwrap();
            if gate == "committed" {
                assert_eq!(vm.context().status, ExecutionStatus::Runnable);
                let EffectState::Completed { response } = vm.effect(id).unwrap().state else {
                    panic!()
                };
                assert_eq!(
                    vm.dispatch_model(&mut store, id, &mut provider).unwrap(),
                    response
                );
            } else {
                assert_eq!(vm.context(), &waiting);
                assert!(matches!(
                    vm.effect(id).unwrap().state,
                    EffectState::OutcomeUnknown { .. }
                ));
                assert_eq!(vm.run(1000).unwrap(), RunOutcome::Waiting(id));
                assert!(vm.dispatch_model(&mut store, id, &mut provider).is_err());
                let response = vm.reconcile_model(&mut store, id, &mut provider).unwrap();
                if gate == "before" {
                    assert_eq!(response, None);
                    assert_eq!(vm.context(), &waiting);
                    assert_eq!(provider.submission_attempts(id).unwrap(), 0);
                    break;
                }
                assert!(response.is_some());
                assert_eq!(vm.context().status, ExecutionStatus::Runnable);
            }
            assert_eq!(provider.submission_attempts(id).unwrap(), 1);
            delivered += 1;
            vm.set_model_calls_enabled(true);
            outcome = vm.run_checkpointed(&mut store, 1000, 100).unwrap();
        }
    }
}

#[test]
fn failed_response_commit_restores_the_waiting_frame_and_reconciliation_wakes_it() {
    let program = program();
    let databases = Databases::new();
    let mut store = CheckpointStore::open(&databases.universe).unwrap();
    let mut vm = start_harness(&program);
    let RunOutcome::Waiting(id) = vm.run_checkpointed(&mut store, 1000, 100).unwrap() else {
        panic!()
    };
    let waiting = vm.context().clone();
    let mut provider = RacingProvider {
        path: databases.universe.clone(),
    };
    assert!(
        vm.dispatch_model(&mut store, id, &mut provider)
            .unwrap_err()
            .0
            .contains("conflict")
    );
    assert_eq!(vm.context(), &waiting);
    assert!(matches!(
        vm.effect(id).unwrap().state,
        EffectState::OutcomeUnknown { .. }
    ));
    let saved = store.load().unwrap();
    let mut vm = saved.snapshot.restore().unwrap();
    assert_eq!(vm.context(), &waiting);
    vm.reconcile_model(&mut store, id, &mut provider)
        .unwrap()
        .unwrap();
    assert_eq!(vm.context().status, ExecutionStatus::Runnable);
    let saved = store.load().unwrap();
    assert_eq!(saved.snapshot.restore().unwrap().context(), vm.context());
}

#[test]
fn completing_an_unrelated_host_request_does_not_wake_the_guest() {
    let program = program();
    let databases = Databases::new();
    let mut store = CheckpointStore::open(&databases.universe).unwrap();
    let mut provider = LocalModelProvider::open(&databases.provider).unwrap();
    let mut vm = start_harness(&program);
    let RunOutcome::Waiting(id) = vm.run_checkpointed(&mut store, 1000, 100).unwrap() else {
        panic!()
    };
    let waiting = vm.context().clone();
    let other = queue(&mut vm, vec![7]);
    vm.dispatch_model(&mut store, other, &mut provider).unwrap();
    assert_eq!(vm.context(), &waiting);
    vm.dispatch_model(&mut store, id, &mut provider).unwrap();
    assert_eq!(vm.context().status, ExecutionStatus::Runnable);
}

#[test]
fn format_three_outbox_and_active_frames_restore_and_upgrade_together() {
    use rusqlite::{Connection, params};
    use sha2::{Digest, Sha256};
    let databases = Databases::new();
    let mut store = CheckpointStore::open(&databases.universe).unwrap();
    let bytes = include_bytes!("fixtures/checkpoint-v3.bin").as_slice();
    assert_eq!(&bytes[..12], b"RENRAKU\0\x03\0\0\0");
    Connection::open(&databases.universe)
        .unwrap()
        .execute(
            "INSERT INTO checkpoint VALUES(1,1,?1,3,?2,?3)",
            params![&bytes[12..28], Sha256::digest(bytes).as_slice(), bytes],
        )
        .unwrap();
    let saved = store.load().unwrap();
    let mut vm = saved.snapshot.restore().unwrap();
    let universe = vm.universe_id();
    let execution = vm.context().id;
    assert_eq!(vm.context().frames.len(), 3);
    assert_eq!(vm.instructions_executed(), 30);
    let record = vm.effects().next().unwrap().clone();
    assert_eq!(vm.effects().count(), 1);
    let EffectState::Completed { response } = record.state else {
        panic!()
    };
    assert_eq!(
        vm.heap().object(response).unwrap().string_units().unwrap(),
        "echo: legacy".encode_utf16().collect::<Vec<_>>()
    );
    assert_eq!(
        vm.run(1000).unwrap(),
        RunOutcome::Completed(Some(Value::I32(48)))
    );
    store.save(&vm).unwrap();
    let saved = store.load().unwrap();
    let vm = saved.snapshot.restore().unwrap();
    assert_eq!(vm.universe_id(), universe);
    assert_eq!(vm.context().id, execution);
    assert_eq!(vm.effect(record.id).unwrap(), &record);
    let format: i64 = Connection::open(&databases.universe)
        .unwrap()
        .query_row("SELECT format FROM checkpoint", [], |r| r.get(0))
        .unwrap();
    assert_eq!(format, 7);
}

#[test]
fn recovery_rejects_inconsistent_waiting_calls_with_repaired_checksums() {
    use rusqlite::{Connection, params};
    use sha2::{Digest, Sha256};
    let program = program();
    for case in 0..9 {
        let databases = Databases::new();
        let mut store = CheckpointStore::open(&databases.universe).unwrap();
        let mut vm = start_harness(&program);
        let RunOutcome::Waiting(id) = vm.run_checkpointed(&mut store, 1000, 100).unwrap() else {
            panic!()
        };
        let connection = Connection::open(&databases.universe).unwrap();
        let mut bytes: Vec<u8> = connection
            .query_row("SELECT payload FROM checkpoint", [], |r| r.get(0))
            .unwrap();
        let positions: Vec<_> = bytes
            .windows(16)
            .enumerate()
            .filter(|(_, v)| *v == id.as_bytes())
            .map(|(i, _)| i)
            .collect();
        assert_eq!(positions.len(), 3);
        let [record, status, frame] = positions[..] else {
            panic!()
        };
        let name_len =
            u32::from_le_bytes(bytes[record + 32..record + 36].try_into().unwrap()) as usize;
        let state = record + 36 + name_len;
        assert_eq!(bytes[status - 1], 4);
        assert_eq!(bytes[frame - 9], 2);
        match case {
            0 => bytes[status..status + 16].fill(0),
            1 => bytes[frame..frame + 16].fill(0),
            2 => bytes[frame + 16..frame + 32].fill(0),
            3 => bytes[frame - 8..frame - 4].copy_from_slice(&0u32.to_le_bytes()),
            4 => bytes[frame - 4..frame].copy_from_slice(&0u32.to_le_bytes()),
            5 => {
                let response = bytes[record + 16..record + 32].to_vec();
                bytes[state] = 2;
                bytes.splice(state + 1..state + 1, response);
            }
            6 => {
                bytes[status - 1] = 0;
                bytes.drain(status..status + 16);
            }
            7 => {
                bytes[8..12].copy_from_slice(&3u32.to_le_bytes());
                connection
                    .execute("UPDATE checkpoint SET format=3", [])
                    .unwrap();
            }
            8 => {
                let end = bytes.len();
                assert_eq!(&bytes[end - 4..], &[0; 4]);
                bytes[end - 4..].copy_from_slice(&1u32.to_le_bytes());
                bytes.extend(Value::I32(3).to_bytes());
            }
            _ => unreachable!(),
        }
        connection
            .execute(
                "UPDATE checkpoint SET payload=?1,digest=?2",
                params![&bytes, Sha256::digest(&bytes).as_slice()],
            )
            .unwrap();
        assert!(
            store.load().is_err(),
            "case {case} accepted an inconsistent waiting call"
        );
    }
}
impl Databases {
    fn new() -> Self {
        let directory = fixtures().output.join(UniverseId::fresh().to_string());
        std::fs::create_dir(&directory).unwrap();
        Self {
            universe: directory.join("universe.db"),
            provider: directory.join("provider.db"),
            directory,
        }
    }
}
impl Drop for Databases {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.directory);
    }
}
fn queue(vm: &mut Vm<'_>, units: Vec<u16>) -> EffectId {
    let request = vm.new_string(units).unwrap();
    vm.queue_model_request(request).unwrap()
}

#[test]
fn outbox_responses_are_persistent_managed_strings_consumable_by_cil() {
    let program = program();
    let databases = Databases::new();
    let mut store = CheckpointStore::open(&databases.universe).unwrap();
    let mut provider = LocalModelProvider::open(&databases.provider).unwrap();
    let mut vm = Vm::new(&program);
    let id = queue(&mut vm, vec![104, 0, 0xd800]);
    store.save(&vm).unwrap();
    assert_eq!(vm.collect_garbage().unwrap(), 0);
    let saved = store.load().unwrap();
    let mut vm = saved.snapshot.restore().unwrap();
    assert_eq!(vm.effect(id).unwrap().state, EffectState::Prepared);
    let response = vm.dispatch_model(&mut store, id, &mut provider).unwrap();
    let mut expected: Vec<u16> = "echo: ".encode_utf16().collect();
    expected.extend([104, 0, 0xd800]);
    assert_eq!(
        vm.heap().object(response).unwrap().string_units().unwrap(),
        expected
    );
    assert_eq!(provider.submission_attempts(id).unwrap(), 1);
    let saved = store.load().unwrap();
    let mut vm = saved.snapshot.restore().unwrap();
    assert_eq!(
        vm.dispatch_model(&mut store, id, &mut provider).unwrap(),
        response
    );
    assert_eq!(provider.submission_attempts(id).unwrap(), 1);
    vm.start(
        entry(&saved.snapshot.program, "ConsumeModelReply"),
        vec![Value::Reference(response)],
    )
    .unwrap();
    assert_eq!(
        vm.run(100).unwrap(),
        RunOutcome::Completed(Some(Value::I32(9)))
    );
    vm.forget_completed_effect(id).unwrap();
    assert_eq!(vm.collect_garbage().unwrap(), 1); // Guest's LastText retains the response.
    assert_eq!(
        vm.heap().object(response).unwrap().string_units().unwrap(),
        expected
    );
    store.save(&vm).unwrap();
    let current = store.load().unwrap();
    assert_eq!(current.snapshot.restore().unwrap().effects().count(), 0);
}

struct CheckingProvider {
    path: PathBuf,
    calls: usize,
}
impl ModelProvider for CheckingProvider {
    fn capability(&self) -> &str {
        "local.echo"
    }
    fn submit(&mut self, request: &ModelRequest) -> Result<Vec<u16>> {
        self.calls += 1;
        let mut store = CheckpointStore::open(&self.path)?;
        let saved = store.load()?;
        let vm = saved.snapshot.restore()?;
        let effect = vm.effect(request.id)?;
        assert!(matches!(effect.state, EffectState::OutcomeUnknown { .. }));
        assert_eq!(
            vm.heap().object(effect.request)?.string_units().unwrap(),
            request.messages.last().unwrap().content
        );
        Ok(vec![111, 107])
    }
    fn reconcile(&mut self, _: &ModelRequest) -> Result<Option<Vec<u16>>> {
        Ok(None)
    }
}
#[test]
fn intent_and_dispatch_marker_commit_before_external_code_runs() {
    let program = program();
    let databases = Databases::new();
    let mut vm = Vm::new(&program);
    let id = queue(&mut vm, vec![1, 2, 3]);
    let mut store = CheckpointStore::open(&databases.universe).unwrap();
    let mut provider = CheckingProvider {
        path: databases.universe.clone(),
        calls: 0,
    };
    vm.dispatch_model(&mut store, id, &mut provider).unwrap();
    assert_eq!(provider.calls, 1);
    assert_eq!(store.generation(), 2);
}
struct FailingProvider {
    calls: usize,
}
impl ModelProvider for FailingProvider {
    fn capability(&self) -> &str {
        "local.echo"
    }
    fn submit(&mut self, _: &ModelRequest) -> Result<Vec<u16>> {
        self.calls += 1;
        Err(Error("connection lost after possible acceptance".into()))
    }
    fn reconcile(&mut self, _: &ModelRequest) -> Result<Option<Vec<u16>>> {
        Ok(None)
    }
}
#[test]
fn uncertain_requests_never_retry_and_missing_receipts_do_not_clear_uncertainty() {
    let program = program();
    let databases = Databases::new();
    let mut vm = Vm::new(&program);
    let id = queue(&mut vm, vec![7]);
    let mut store = CheckpointStore::open(&databases.universe).unwrap();
    let mut provider = FailingProvider { calls: 0 };
    assert!(
        vm.dispatch_model(&mut store, id, &mut provider)
            .unwrap_err()
            .0
            .contains("uncertain")
    );
    assert!(vm.dispatch_model(&mut store, id, &mut provider).is_err());
    assert_eq!(provider.calls, 1);
    let saved = store.load().unwrap();
    let mut vm = saved.snapshot.restore().unwrap();
    assert!(matches!(
        vm.effect(id).unwrap().state,
        EffectState::OutcomeUnknown { .. }
    ));
    assert_eq!(
        vm.reconcile_model(&mut store, id, &mut provider).unwrap(),
        None
    );
    assert!(vm.forget_completed_effect(id).is_err());
    assert!(vm.dispatch_model(&mut store, id, &mut provider).is_err());
    assert_eq!(provider.calls, 1);
}

#[test]
fn stale_universes_are_rejected_before_any_submission() {
    let program = program();
    let databases = Databases::new();
    let mut vm = Vm::new(&program);
    let id = queue(&mut vm, vec![7]);
    let mut store = CheckpointStore::open(&databases.universe).unwrap();
    store.save(&vm).unwrap();
    let old = store.load().unwrap();
    let mut stale = old.snapshot.restore().unwrap();
    store.save(&vm).unwrap();
    let mut provider = FailingProvider { calls: 0 };
    assert!(
        stale
            .dispatch_model(&mut store, id, &mut provider)
            .unwrap_err()
            .0
            .contains("conflict")
    );
    assert_eq!(provider.calls, 0);
}

struct RacingProvider {
    path: PathBuf,
}
impl ModelProvider for RacingProvider {
    fn capability(&self) -> &str {
        "local.echo"
    }
    fn submit(&mut self, _: &ModelRequest) -> Result<Vec<u16>> {
        let mut other = CheckpointStore::open(&self.path)?;
        let saved = other.load()?;
        let vm = saved.snapshot.restore()?;
        other.save(&vm)?; // Advance the universe while the original host awaits its response.
        Ok(vec![111, 107])
    }
    fn reconcile(&mut self, _: &ModelRequest) -> Result<Option<Vec<u16>>> {
        Ok(Some(vec![111, 107]))
    }
}
#[test]
fn a_failed_response_commit_cannot_return_an_uncommitted_cached_result() {
    let program = program();
    let databases = Databases::new();
    let mut vm = Vm::new(&program);
    let id = queue(&mut vm, vec![7]);
    let mut store = CheckpointStore::open(&databases.universe).unwrap();
    let mut provider = RacingProvider {
        path: databases.universe.clone(),
    };
    assert!(
        vm.dispatch_model(&mut store, id, &mut provider)
            .unwrap_err()
            .0
            .contains("conflict")
    );
    assert!(matches!(
        vm.effect(id).unwrap().state,
        EffectState::OutcomeUnknown { .. }
    ));
    assert!(
        vm.dispatch_model(&mut store, id, &mut provider)
            .unwrap_err()
            .0
            .contains("uncertain")
    );
    let saved = store.load().unwrap();
    let mut restored = saved.snapshot.restore().unwrap();
    assert!(
        restored
            .reconcile_model(&mut store, id, &mut provider)
            .unwrap()
            .is_some()
    );
}

struct Probe(Option<Child>);
impl Probe {
    fn start(databases: &Databases, gate: &str) -> Self {
        let child = Command::new(env!("CARGO_BIN_EXE_renraku-effect-probe"))
            .arg(&databases.universe)
            .arg(&databases.provider)
            .arg(gate)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap();
        let mut probe = Self(Some(child));
        let stdout = probe.0.as_mut().unwrap().stdout.take().unwrap();
        let (sender, receiver) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let line = line.unwrap();
                if line.starts_with("READY ") {
                    let _ = sender.send(line);
                    break;
                }
            }
        });
        receiver
            .recv_timeout(Duration::from_secs(20))
            .expect("provider probe did not reach gate");
        probe
    }
    fn kill(&mut self) {
        let mut child = self.0.take().unwrap();
        child.kill().unwrap();
        let status = child.wait().unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::process::ExitStatusExt;
            assert_eq!(status.signal(), Some(9));
        }
        #[cfg(not(unix))]
        assert!(!status.success());
    }
}
impl Drop for Probe {
    fn drop(&mut self) {
        if let Some(child) = &mut self.0 {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

#[test]
fn process_kills_before_and_after_provider_acceptance_recover_without_duplicate_submission() {
    let program = program();
    for gate in ["before", "after"] {
        let databases = Databases::new();
        let mut vm = Vm::new(&program);
        let id = queue(&mut vm, "hello".encode_utf16().collect());
        let mut store = CheckpointStore::open(&databases.universe).unwrap();
        store.save(&vm).unwrap();
        let mut probe = Probe::start(&databases, gate);
        probe.kill();
        let saved = store.load().unwrap();
        let mut vm = saved.snapshot.restore().unwrap();
        assert!(matches!(
            vm.effect(id).unwrap().state,
            EffectState::OutcomeUnknown { .. }
        ));
        let mut provider = LocalModelProvider::open(&databases.provider).unwrap();
        assert!(vm.dispatch_model(&mut store, id, &mut provider).is_err());
        let response = vm.reconcile_model(&mut store, id, &mut provider).unwrap();
        if gate == "before" {
            assert_eq!(response, None);
            assert_eq!(provider.submission_attempts(id).unwrap(), 0);
        } else {
            let response = response.unwrap();
            assert_eq!(
                vm.heap().object(response).unwrap().string_units().unwrap(),
                "echo: hello".encode_utf16().collect::<Vec<_>>()
            );
            assert_eq!(provider.submission_attempts(id).unwrap(), 1);
            let saved = store.load().unwrap();
            let mut vm = saved.snapshot.restore().unwrap();
            assert_eq!(
                vm.dispatch_model(&mut store, id, &mut provider).unwrap(),
                response
            );
            assert_eq!(provider.submission_attempts(id).unwrap(), 1);
        }
    }
}

#[test]
fn format_two_strings_restore_and_upgrade_without_losing_identity() {
    use rusqlite::{Connection, params};
    use sha2::{Digest, Sha256};
    let databases = Databases::new();
    let mut store = CheckpointStore::open(&databases.universe).unwrap();
    let bytes = include_bytes!("fixtures/checkpoint-v2.bin").as_slice();
    assert_eq!(&bytes[..12], b"RENRAKU\0\x02\0\0\0");
    Connection::open(&databases.universe)
        .unwrap()
        .execute(
            "INSERT INTO checkpoint VALUES(1,1,?1,2,?2,?3)",
            params![&bytes[12..28], Sha256::digest(bytes).as_slice(), bytes],
        )
        .unwrap();
    let saved = store.load().unwrap();
    let mut vm = saved.snapshot.restore().unwrap();
    let universe = vm.universe_id();
    let execution = vm.context().id;
    assert_eq!(vm.effects().count(), 0);
    assert_eq!(vm.instructions_executed(), 80);
    assert_eq!(
        vm.run(1000).unwrap(),
        RunOutcome::Completed(Some(Value::I32(fixtures().reference["Strings"] as i32)))
    );
    store.save(&vm).unwrap();
    let saved = store.load().unwrap();
    let vm = saved.snapshot.restore().unwrap();
    assert_eq!(vm.universe_id(), universe);
    assert_eq!(vm.context().id, execution);
}

#[test]
fn provider_bindings_and_idempotency_keys_cannot_be_reinterpreted() {
    struct WrongProvider;
    impl ModelProvider for WrongProvider {
        fn capability(&self) -> &str {
            "different.model"
        }
        fn submit(&mut self, _: &ModelRequest) -> Result<Vec<u16>> {
            panic!("wrong provider was invoked")
        }
        fn reconcile(&mut self, _: &ModelRequest) -> Result<Option<Vec<u16>>> {
            panic!("wrong provider was invoked")
        }
    }
    let program = program();
    let databases = Databases::new();
    let mut vm = Vm::new(&program);
    let id = queue(&mut vm, vec![7]);
    let mut store = CheckpointStore::open(&databases.universe).unwrap();
    assert!(
        vm.dispatch_model(&mut store, id, &mut WrongProvider)
            .unwrap_err()
            .0
            .contains("capability")
    );
    assert_eq!(vm.effect(id).unwrap().state, EffectState::Prepared);
    let mut provider = LocalModelProvider::open(&databases.provider).unwrap();
    let request = ModelRequest::echo(id, vec![7]);
    let first = provider.submit(&request).unwrap();
    assert_eq!(provider.submit(&request).unwrap(), first);
    assert_eq!(provider.submission_attempts(id).unwrap(), 2);
    assert!(provider.submit(&ModelRequest::echo(id, vec![8])).is_err());
    assert_eq!(provider.submission_attempts(id).unwrap(), 2);
}

#[test]
fn recovery_rejects_malformed_effect_records_even_with_a_repaired_checksum() {
    use rusqlite::{Connection, params};
    use sha2::{Digest, Sha256};
    let program = program();
    for case in 0..4 {
        let databases = Databases::new();
        let mut vm = Vm::new(&program);
        let id = queue(&mut vm, vec![7]);
        let mut store = CheckpointStore::open(&databases.universe).unwrap();
        store.save(&vm).unwrap();
        let connection = Connection::open(&databases.universe).unwrap();
        let mut bytes: Vec<u8> = connection
            .query_row("SELECT payload FROM checkpoint", [], |r| r.get(0))
            .unwrap();
        let positions: Vec<_> = bytes
            .windows(16)
            .enumerate()
            .filter(|(_, v)| *v == id.as_bytes())
            .map(|(i, _)| i)
            .collect();
        assert_eq!(positions.len(), 1);
        let start = positions[0];
        let name_len =
            u32::from_le_bytes(bytes[start + 32..start + 36].try_into().unwrap()) as usize;
        let state = start + 36 + name_len;
        match case {
            0 => bytes[state] = 255,
            1 => bytes[start + 16..start + 32].fill(0),
            2 => {
                assert_eq!(
                    u32::from_le_bytes(bytes[start - 4..start].try_into().unwrap()),
                    1
                );
                let metadata = state + 1;
                let model_length =
                    u32::from_le_bytes(bytes[metadata..metadata + 4].try_into().unwrap()) as usize;
                let count_offset = metadata + 4 + model_length;
                let messages =
                    u32::from_le_bytes(bytes[count_offset..count_offset + 4].try_into().unwrap())
                        as usize;
                let end = count_offset + 4 + messages * 17;
                let duplicate = bytes[start..end].to_vec();
                bytes[start - 4..start].copy_from_slice(&2u32.to_le_bytes());
                bytes.splice(end..end, duplicate);
            }
            3 => {
                bytes.drain(start + 36..state);
                bytes[start + 32..start + 36].copy_from_slice(&0u32.to_le_bytes());
            }
            _ => unreachable!(),
        }
        connection
            .execute(
                "UPDATE checkpoint SET payload=?1,digest=?2",
                params![&bytes, Sha256::digest(&bytes).as_slice()],
            )
            .unwrap();
        let error = store.load().unwrap_err();
        let expected = [
            "unknown effect state",
            "does not exist",
            "duplicate effect",
            "capability name",
        ][case];
        assert!(error.0.contains(expected), "case {case}: {error}");
    }
}
