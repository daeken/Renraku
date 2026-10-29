mod common;
use common::{entry, fixtures, program};
use renraku_vm::{
    checkpoint::CheckpointStore,
    effects::{EffectState, LocalModelProvider, ModelProvider, ModelRequest},
    error::{Error, Result},
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
            request.prompt
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
    let request = ModelRequest {
        id,
        prompt: vec![7],
    };
    let first = provider.submit(&request).unwrap();
    assert_eq!(provider.submit(&request).unwrap(), first);
    assert_eq!(provider.submission_attempts(id).unwrap(), 2);
    assert!(
        provider
            .submit(&ModelRequest {
                id,
                prompt: vec![8]
            })
            .is_err()
    );
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
                let duplicate = bytes[start..state + 1].to_vec();
                bytes[start - 4..start].copy_from_slice(&2u32.to_le_bytes());
                bytes.splice(state + 1..state + 1, duplicate);
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
