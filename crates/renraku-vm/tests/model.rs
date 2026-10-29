mod common;
use common::{entry, fixtures, program};
use renraku_vm::{
    checkpoint::CheckpointStore,
    driver::{DriverLimits, DriverStop},
    effects::{EffectRecord, EffectState, LocalModelProvider, ModelProvider, ModelRequest},
    execution::ExecutionStatus,
    identity::{EffectId, UniverseId},
    model::{MessageReference, MessageRole, ModelMessage},
    runtime::{RunOutcome, Vm},
    value::Value,
};
use std::{
    io::{BufRead, BufReader},
    path::PathBuf,
    process::{Command, Stdio},
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
fn limits() -> DriverLimits {
    DriverLimits {
        instructions: 2000,
        provider_operations: 2,
        checkpoint_interval: 7,
    }
}
fn text(vm: &Vm<'_>, record: &EffectRecord, index: usize) -> Vec<u16> {
    vm.heap()
        .object(record.messages[index].content)
        .unwrap()
        .string_units()
        .unwrap()
        .to_vec()
}
fn assert_chat(vm: &Vm<'_>) -> Vec<EffectRecord> {
    let records: Vec<_> = vm.effects().cloned().collect();
    assert_eq!(records.len(), 2);
    let first = records.iter().find(|e| e.messages.len() == 2).unwrap();
    let second = records.iter().find(|e| e.messages.len() == 4).unwrap();
    assert_eq!(first.capability, "local.echo");
    assert_eq!(first.model, "echo-v1");
    assert_eq!(second.model, "echo-v1");
    assert_eq!(
        text(vm, first, 0),
        "be brief".encode_utf16().collect::<Vec<_>>()
    );
    assert_eq!(
        text(vm, second, 0),
        "new instruction".encode_utf16().collect::<Vec<_>>()
    );
    assert_eq!(text(vm, first, 1), [104, 105, 0, 0xd800]);
    assert_eq!(
        second.messages.iter().map(|m| m.role).collect::<Vec<_>>(),
        [
            MessageRole::System,
            MessageRole::User,
            MessageRole::Assistant,
            MessageRole::User
        ]
    );
    let EffectState::Completed { response } = first.state else {
        panic!()
    };
    assert_eq!(second.messages[2].content, response);
    assert_eq!(first.messages[1].content, second.messages[1].content);
    vec![first.clone(), second.clone()]
}

#[test]
fn structured_chat_restores_every_instruction_and_freezes_history_and_binding() {
    let program = program();
    let databases = Databases::new();
    let mut store = CheckpointStore::open(&databases.universe).unwrap();
    let mut provider = LocalModelProvider::open(&databases.provider).unwrap();
    let mut vm = Vm::new(&program);
    vm.start(entry(&program, "ChatHarnessScore"), Vec::new())
        .unwrap();
    let execution = vm.context().id;
    store.save(&vm).unwrap();
    for _ in 0..2000 {
        let saved = store.load().unwrap();
        let mut vm = saved.snapshot.restore().unwrap();
        vm.set_model_calls_enabled(true);
        assert_eq!(vm.context().id, execution);
        let outcome = vm.run(1).unwrap();
        store.save(&vm).unwrap();
        match outcome {
            RunOutcome::Paused => {}
            RunOutcome::Waiting(id) => {
                let context = vm.context().clone();
                let record = vm.effect(id).unwrap().clone();
                let saved = store.load().unwrap();
                let mut vm = saved.snapshot.restore().unwrap();
                assert_eq!(vm.context(), &context);
                assert_eq!(vm.effect(id).unwrap(), &record);
                vm.collect_garbage().unwrap();
                vm.dispatch_model(&mut store, id, &mut provider).unwrap();
                assert_eq!(provider.submission_attempts(id).unwrap(), 1);
            }
            RunOutcome::Completed(result) => {
                assert_eq!(
                    result,
                    Some(Value::I32(fixtures().reference["ChatHarnessScore"] as i32))
                );
                let records = assert_chat(&vm);
                let model_field = saved
                    .snapshot
                    .program
                    .fields()
                    .find(|f| f.name == "Model")
                    .unwrap()
                    .id;
                let binding_field = saved
                    .snapshot
                    .program
                    .fields()
                    .find(|f| f.name == "LastBinding")
                    .unwrap()
                    .id;
                let Value::Reference(binding) = vm.static_fields()[&binding_field] else {
                    panic!()
                };
                let Value::Reference(model) =
                    vm.heap().object(binding).unwrap().fields()[&model_field]
                else {
                    panic!()
                };
                assert_eq!(
                    vm.heap().object(model).unwrap().string_units().unwrap(),
                    "changed-after-completion"
                        .encode_utf16()
                        .collect::<Vec<_>>()
                );
                let old_system_content = records[0].messages[0].content;
                vm.collect_garbage().unwrap();
                assert!(vm.heap().object(old_system_content).is_ok());
                vm.forget_completed_effect(records[0].id).unwrap();
                vm.collect_garbage().unwrap();
                assert!(vm.heap().object(old_system_content).is_err());
                store.save(&vm).unwrap();
                return;
            }
        }
    }
    panic!("chat harness did not complete");
}

#[test]
fn malformed_guest_chains_fault_atomically_without_creating_effects() {
    let program = program();
    for (kind, expected) in [
        (0, "cycle"),
        (1, "role"),
        (2, "non-null"),
        (3, "identifier"),
        (4, "user message"),
        (5, "non-null"),
        (6, "non-null"),
        (7, "limit"),
    ] {
        let mut vm = Vm::new(&program);
        vm.set_model_calls_enabled(true);
        vm.start(entry(&program, "BadChat"), vec![Value::I32(kind)])
            .unwrap();
        for step in 0..10000 {
            let before = vm.context().frames.clone();
            match vm.run(1) {
                Ok(RunOutcome::Paused) => assert!(step < 9999),
                Err(error) => {
                    assert!(error.0.contains(expected), "case {kind}: {error}");
                    assert_eq!(vm.context().frames, before);
                    assert_eq!(vm.effects().count(), 0);
                    break;
                }
                other => panic!("case {kind}: {other:?}"),
            }
        }
    }
}

#[test]
fn receipts_bind_all_roles_contents_order_and_model_not_only_the_final_prompt() {
    let databases = Databases::new();
    let mut provider = LocalModelProvider::open(&databases.provider).unwrap();
    let request = ModelRequest {
        id: EffectId::fresh(),
        capability: "local.echo".into(),
        model: "echo-v1".into(),
        messages: vec![
            ModelMessage {
                role: MessageRole::System,
                content: "context".encode_utf16().collect(),
            },
            ModelMessage {
                role: MessageRole::User,
                content: vec![0, 0xd800],
            },
        ],
    };
    let response = provider.submit(&request).unwrap();
    assert_eq!(
        provider.reconcile(&request).unwrap(),
        Some(response.clone())
    );
    for case in 0..4 {
        let mut changed = request.clone();
        match case {
            0 => changed.messages[0].content.push(1),
            1 => changed.messages[0].role = MessageRole::Assistant,
            2 => changed.messages.insert(
                1,
                ModelMessage {
                    role: MessageRole::Assistant,
                    content: vec![2],
                },
            ),
            3 => changed.model = "different-model".into(),
            _ => unreachable!(),
        }
        assert!(provider.submit(&changed).is_err());
        assert!(provider.reconcile(&changed).is_err());
        assert_eq!(provider.submission_attempts(request.id).unwrap(), 1);
    }
    assert_eq!(provider.submit(&request).unwrap(), response);
    assert_eq!(provider.submission_attempts(request.id).unwrap(), 2);
}

#[test]
fn unsupported_saved_model_is_rejected_before_dispatch_or_reconciliation() {
    struct NeverCalled;
    impl ModelProvider for NeverCalled {
        fn capability(&self) -> &str {
            "local.echo"
        }
        fn submit(&mut self, _: &ModelRequest) -> renraku_vm::error::Result<Vec<u16>> {
            panic!("unsupported model submitted")
        }
        fn reconcile(&mut self, _: &ModelRequest) -> renraku_vm::error::Result<Option<Vec<u16>>> {
            panic!("unsupported model looked up")
        }
    }
    let program = program();
    let databases = Databases::new();
    let mut store = CheckpointStore::open(&databases.universe).unwrap();
    let mut vm = Vm::new(&program);
    let content = vm.new_string(vec![104]).unwrap();
    let id = vm
        .queue_chat_request(
            "local.echo".into(),
            "unavailable".into(),
            vec![MessageReference {
                role: MessageRole::User,
                content,
            }],
        )
        .unwrap();
    store.save(&vm).unwrap();
    let generation = store.generation();
    assert!(
        vm.dispatch_model(&mut store, id, &mut NeverCalled)
            .unwrap_err()
            .0
            .contains("saved model")
    );
    assert!(
        vm.reconcile_model(&mut store, id, &mut NeverCalled)
            .unwrap_err()
            .0
            .contains("saved model")
    );
    assert_eq!(vm.effect(id).unwrap().state, EffectState::Prepared);
    assert_eq!(store.generation(), generation);
}

#[test]
fn request_limits_reject_oversized_aggregates_and_counts_without_publishing_intent() {
    let program = program();
    let mut vm = Vm::new(&program);
    let large = vm.new_string(vec![1; 1_000_000]).unwrap();
    let small = vm.new_string(vec![2]).unwrap();
    let messages = vec![
        MessageReference {
            role: MessageRole::System,
            content: large,
        },
        MessageReference {
            role: MessageRole::User,
            content: small,
        },
    ];
    assert!(
        vm.queue_chat_request("local.echo".into(), "echo-v1".into(), messages)
            .unwrap_err()
            .0
            .contains("UTF-16 unit limit")
    );
    let messages = vec![
        MessageReference {
            role: MessageRole::User,
            content: small
        };
        129
    ];
    assert!(
        vm.queue_chat_request("local.echo".into(), "echo-v1".into(), messages)
            .unwrap_err()
            .0
            .contains("message count")
    );
    assert!(
        vm.queue_chat_request("local.echo".into(), "echo-v1".into(), Vec::new())
            .is_err()
    );
    assert_eq!(vm.effects().count(), 0);
}

fn kill_probe(databases: &Databases, mode: &str) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_renraku-effect-probe"))
        .arg(&databases.universe)
        .arg(&databases.provider)
        .arg(mode)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .unwrap();
    let stdout = child.stdout.take().unwrap();
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            if line.unwrap().starts_with("READY ") {
                let _ = sender.send(());
                break;
            }
        }
    });
    let ready = receiver.recv_timeout(Duration::from_secs(20));
    child.kill().unwrap();
    let status = child.wait().unwrap();
    ready.expect("chat probe did not reach gate");
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        assert_eq!(status.signal(), Some(9));
    }
}

#[test]
fn structured_histories_survive_process_kills_during_both_deliveries() {
    let program = program();
    for mode in ["driver-after", "driver-second-after"] {
        let databases = Databases::new();
        let mut store = CheckpointStore::open(&databases.universe).unwrap();
        let mut vm = Vm::new(&program);
        vm.start(entry(&program, "ChatHarnessScore"), Vec::new())
            .unwrap();
        store.save(&vm).unwrap();
        kill_probe(&databases, mode);
        let saved = store.load().unwrap();
        let mut recovered = saved.snapshot.restore().unwrap();
        assert_eq!(recovered.context().id, vm.context().id);
        let ExecutionStatus::Waiting { effect } = recovered.context().status else {
            panic!()
        };
        let request = recovered.effect(effect).unwrap().clone();
        assert!(matches!(request.state, EffectState::OutcomeUnknown { .. }));
        recovered.set_model_calls_enabled(true);
        let mut provider = LocalModelProvider::open(&databases.provider).unwrap();
        let report = recovered
            .drive_model(&mut store, &mut provider, limits())
            .unwrap();
        assert_eq!(
            report.stop,
            DriverStop::Completed(Some(Value::I32(
                fixtures().reference["ChatHarnessScore"] as i32
            )))
        );
        assert_eq!(recovered.effect(effect).unwrap().messages, request.messages);
        for record in assert_chat(&recovered) {
            assert_eq!(provider.submission_attempts(record.id).unwrap(), 1);
        }
    }
}

#[test]
fn legacy_waiting_checkpoint_and_receipt_upgrade_and_reconcile_without_resubmission() {
    use rusqlite::{Connection, params};
    use sha2::{Digest, Sha256};
    let databases = Databases::new();
    std::fs::write(
        &databases.provider,
        include_bytes!("fixtures/local-provider-v1.db"),
    )
    .unwrap();
    let mut store = CheckpointStore::open(&databases.universe).unwrap();
    let bytes = include_bytes!("fixtures/checkpoint-v4.bin").as_slice();
    Connection::open(&databases.universe)
        .unwrap()
        .execute(
            "INSERT INTO checkpoint VALUES(1,1,?1,4,?2,?3)",
            params![&bytes[12..28], Sha256::digest(bytes).as_slice(), bytes],
        )
        .unwrap();
    let saved = store.load().unwrap();
    let mut vm = saved.snapshot.restore().unwrap();
    let execution = vm.context().id;
    let ExecutionStatus::Waiting { effect } = vm.context().status else {
        panic!()
    };
    assert_eq!(vm.effect(effect).unwrap().model, "echo-v1");
    assert_eq!(vm.effect(effect).unwrap().messages.len(), 1);
    vm.set_model_calls_enabled(true);
    let mut provider = LocalModelProvider::open(&databases.provider).unwrap();
    let report = vm.drive_model(&mut store, &mut provider, limits()).unwrap();
    assert_eq!(report.stop, DriverStop::Completed(Some(Value::I32(417))));
    assert_eq!(report.provider_operations, 2);
    assert_eq!(provider.submission_attempts(effect).unwrap(), 1);
    assert_eq!(vm.context().id, execution);
    let connection = Connection::open(&databases.provider).unwrap();
    assert_eq!(
        connection
            .pragma_query_value(None, "user_version", |r| r.get::<_, i64>(0))
            .unwrap(),
        2
    );
    let formats: Vec<i64> = connection
        .prepare("SELECT request_format FROM receipt ORDER BY request_format")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert_eq!(formats, [1, 2]);
    let connection = Connection::open(&databases.universe).unwrap();
    assert_eq!(
        connection
            .query_row("SELECT format FROM checkpoint", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        6
    );
}

#[test]
fn restored_envelopes_reject_invalid_models_roles_counts_and_last_message_references() {
    use rusqlite::{Connection, params};
    use sha2::{Digest, Sha256};
    let program = program();
    for case in 0..5 {
        let databases = Databases::new();
        let mut vm = Vm::new(&program);
        let prompt = vm.new_string(vec![7]).unwrap();
        let effect = vm.queue_model_request(prompt).unwrap();
        let mut store = CheckpointStore::open(&databases.universe).unwrap();
        store.save(&vm).unwrap();
        let connection = Connection::open(&databases.universe).unwrap();
        let mut bytes: Vec<u8> = connection
            .query_row("SELECT payload FROM checkpoint", [], |r| r.get(0))
            .unwrap();
        let start = bytes
            .windows(16)
            .position(|b| b == effect.as_bytes())
            .unwrap();
        let capability_length =
            u32::from_le_bytes(bytes[start + 32..start + 36].try_into().unwrap()) as usize;
        let model_length = start + 36 + capability_length + 1;
        let length =
            u32::from_le_bytes(bytes[model_length..model_length + 4].try_into().unwrap()) as usize;
        let count = model_length + 4 + length;
        match case {
            0 => bytes[model_length + 4] = b' ',
            1 => bytes[count + 4] = 255,
            2 => bytes[count..count + 4].copy_from_slice(&129u32.to_le_bytes()),
            3 => bytes[count + 4] = 2, // Last assistant messages are not supported.
            4 => bytes[count + 5..count + 21].fill(0),
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
            "case {case} accepted malformed request metadata"
        );
    }
}
