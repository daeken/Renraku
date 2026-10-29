use renraku_vm::{checkpoint::CheckpointStore, effects::EffectState, execution::ExecutionStatus};
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};
use uuid::Uuid;
#[allow(dead_code)]
#[path = "../../renraku-vm/tests/common/mod.rs"]
mod common;

struct Directory(PathBuf);
impl Directory {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("renraku-openai-cli-{}", Uuid::new_v4()));
        fs::create_dir(&path).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
        }
        Self(path)
    }
}
impl Drop for Directory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn command(directory: &Path, arguments: &[String]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_renraku-openai"))
        .arg("--auth-dir")
        .arg(directory.join("auth"))
        .args(arguments)
        .output()
        .unwrap()
}

#[test]
fn cli_requires_opt_in_checkpoints_preflight_rejection_and_resumes_with_zero_provider_budget() {
    let directory = Directory::new();
    let fixture = common::fixtures();
    let profile = Uuid::new_v4();
    let universe = directory.0.join("universe.db");
    let receipts = directory.0.join("receipts");
    let mut args: Vec<String> = vec![
        "demo".into(),
        universe.display().to_string(),
        receipts.display().to_string(),
        fixture
            .output
            .join("Renraku.CoreLib.dll")
            .display()
            .to_string(),
        fixture
            .output
            .join("Renraku.Simulation.dll")
            .display()
            .to_string(),
        profile.to_string(),
        "chosen-model".into(),
        "hello".into(),
    ];
    let output = command(&directory.0, &args);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("--allow-model-calls"));
    assert!(!universe.exists());
    assert!(!directory.0.join("auth").exists());
    // Only test credentials, intentionally without the plan-usage grant. No
    // production inference/catalog request can pass this preflight.
    let auth = directory.0.join("auth");
    fs::create_dir(&auth).unwrap();
    let path = auth.join("profiles.json");
    fs::write(&path, serde_json::to_vec(&serde_json::json!({"version":1,"host_id":Uuid::new_v4().urn().to_string(),"active":profile,"profiles":[{
        "id":profile,"client_id":"oaiapp_cli_fixture","identity":{"issuer":"https://auth.openai.com","subject":"cli-test","email":null},
        "session":{"access_token":"cli-secret-access-token","refresh_token":null,"id_token":"cli-test-id-token","scopes":["openid"],"expires_at":9999999999u64},"refresh_pending":false}]})).unwrap()).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&auth, fs::Permissions::from_mode(0o700)).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    }
    args.push("--allow-model-calls".into());
    let output = command(&directory.0, &args);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("plan usage"));
    let mut store = CheckpointStore::open(&universe).unwrap();
    let saved = store.load().unwrap();
    let vm = saved.snapshot.restore().unwrap();
    assert!(matches!(
        vm.context().status,
        ExecutionStatus::Waiting { .. }
    ));
    assert_eq!(vm.effects().next().unwrap().state, EffectState::Prepared);
    let before = store.generation();
    drop(store);
    let args = [
        "drive".into(),
        universe.display().to_string(),
        receipts.display().to_string(),
        profile.to_string(),
        "chosen-model".into(),
        "0".into(),
        "0".into(),
        "--allow-model-calls".into(),
    ];
    let output = command(&directory.0, &args);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("provider operations: 0") && stdout.contains("waiting:"));
    assert!(!stdout.contains("cli-secret-access-token"));
    let mut store = CheckpointStore::open(&universe).unwrap();
    assert!(store.load().unwrap().generation > before);
    for path in [&universe, &receipts.join("receipts.db")] {
        assert!(
            !fs::read(path)
                .unwrap()
                .windows(b"cli-secret-access-token".len())
                .any(|b| b == b"cli-secret-access-token")
        );
    }
}

#[test]
fn agent_cli_requires_explicit_opt_in_and_validates_budgets_before_touching_state_or_auth() {
    let directory = Directory::new();
    let universe = directory.0.join("agent.db");
    let mut args = vec![
        "agent".into(),
        universe.display().to_string(),
        directory.0.join("receipts").display().to_string(),
        directory.0.join("compiler.db").display().to_string(),
        "/missing/Renraku.Compiler.dll".into(),
        "/missing/CoreLib.dll".into(),
        "/missing/guest.dll".into(),
        Uuid::new_v4().to_string(),
        "chosen-model".into(),
        "compile a caller".into(),
        "10000".into(),
        "6".into(),
        "6".into(),
    ];
    let output = command(&directory.0, &args);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("--allow-model-calls"));
    args[10] = "1000001".into();
    args.push("--allow-model-calls".into());
    let output = command(&directory.0, &args);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("limits exceed policy"));
    args[10] = "10000".into();
    args[12] = "33".into();
    let output = command(&directory.0, &args);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("limits exceed policy"));
    assert!(!universe.exists());
    assert!(!directory.0.join("auth").exists());
    assert!(!directory.0.join("compiler.db").exists());
    let help = command(&directory.0, &["--help".into()]);
    assert!(help.status.success());
    assert!(String::from_utf8_lossy(&help.stdout).contains("agent-drive"));
}

#[test]
fn agent_console_separates_progress_from_json_and_gives_durable_resume_or_failure_guidance() {
    use renraku_vm::{
        driver::{DriverLimits, DriverStop},
        effects::ModelProvider,
        error::Result,
        model::ModelRequest,
        runtime::RunOutcome,
        value::Value,
    };
    struct Finish(String, String);
    impl ModelProvider for Finish {
        fn capability(&self) -> &str {
            &self.0
        }
        fn supports_model(&self, model: &str) -> bool {
            model == "chosen-model"
        }
        fn submit(&mut self, _: &ModelRequest) -> Result<Vec<u16>> {
            Ok(serde_json::json!({"op":"finish","message":self.1})
                .to_string()
                .encode_utf16()
                .collect())
        }
        fn reconcile(&mut self, _: &ModelRequest) -> Result<Option<Vec<u16>>> {
            unreachable!()
        }
    }
    let directory = Directory::new();
    let profile = Uuid::new_v4();
    let auth = directory.0.join("auth");
    fs::create_dir(&auth).unwrap();
    let metadata = auth.join("profiles.json");
    fs::write(&metadata, serde_json::to_vec(&serde_json::json!({"version":1,"host_id":Uuid::new_v4().urn().to_string(),"active":profile,
        "profiles":[{"id":profile,"client_id":"offline_console_test","identity":{"issuer":"https://auth.openai.com","subject":"console-test","email":null},"session":null,"refresh_pending":false}]})).unwrap()).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&auth, fs::Permissions::from_mode(0o700)).unwrap();
        fs::set_permissions(&metadata, fs::Permissions::from_mode(0o600)).unwrap();
    }
    let fixture = common::fixtures();
    let universe = directory.0.join("world 'quoted'.db");
    let mut args = vec![
        "agent".into(),
        universe.display().to_string(),
        directory.0.join("receipts").display().to_string(),
        directory.0.join("compiler.db").display().to_string(),
        "/missing/compiler.dll".into(),
        fixture
            .output
            .join("Renraku.CoreLib.dll")
            .display()
            .to_string(),
        fixture
            .output
            .join("Renraku.Simulation.dll")
            .display()
            .to_string(),
        profile.to_string(),
        "chosen-model".into(),
        "finish".into(),
        "0".into(),
        "0".into(),
        "0".into(),
        "--allow-model-calls".into(),
    ];
    let output = command(&directory.0, &args);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["stop"], "instruction_budget");
    assert_eq!(report["recovery"]["kind"], "resume");
    assert!(
        !report["recovery"]["resumeArgs"]
            .as_array()
            .unwrap()
            .iter()
            .any(|arg| arg == "--allow-web-publication")
    );
    assert_eq!(
        report["recovery"]["resumeArgs"][4],
        universe.display().to_string()
    );
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("Paused") && stderr.contains("Resume:") && stderr.contains("Inspect:"));
    for flags in [
        ["--allow-model-calls", "--allow-web-publication"],
        ["--allow-web-publication", "--allow-model-calls"],
    ] {
        let mut resume: Vec<String> = vec![
            "agent-drive".into(),
            universe.display().to_string(),
            directory.0.join("receipts").display().to_string(),
            directory.0.join("compiler.db").display().to_string(),
            "/missing/compiler.dll".into(),
            profile.to_string(),
            "chosen-model".into(),
            "0".into(),
            "0".into(),
            "0".into(),
        ];
        resume.extend(flags.map(str::to_owned));
        let output = command(&directory.0, &resume);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(report["stop"], "instruction_budget");
        assert_eq!(
            report["recovery"]["resumeArgs"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|arg| *arg == "--allow-web-publication")
                .count(),
            1
        );
    }

    // Complete the managed finish turn locally. The CLI can then project saved
    // progress while signed out, with no credentials or inference HTTP available.
    let mut store = CheckpointStore::open(&universe).unwrap();
    let saved = store.load().unwrap();
    let program = &saved.snapshot.program;
    let mut vm = saved.snapshot.restore().unwrap();
    vm.set_model_calls_enabled(true);
    let message = "one\nline\u{1b}[31m🦀".to_owned() + &"x".repeat(600);
    let mut provider = Finish(format!("openai.chatgpt.{profile}"), message.clone());
    let model = vm
        .drive_model(
            &mut store,
            &mut provider,
            DriverLimits {
                instructions: 1000,
                provider_operations: 1,
                checkpoint_interval: 100,
            },
        )
        .unwrap();
    assert!(matches!(model.stop, DriverStop::Completed(_)));
    let input = vm
        .new_string(
            serde_json::json!({"op":"finish","message":message})
                .to_string()
                .encode_utf16()
                .collect(),
        )
        .unwrap();
    let mut freeze = vec![Value::Reference(input)];
    freeze.resize(9, Value::Null);
    vm.start(
        program.entry("Renraku.Guest.AgentHarness::Freeze").unwrap(),
        freeze,
    )
    .unwrap();
    assert!(matches!(
        vm.run_checkpointed(&mut store, 1000, 100).unwrap(),
        RunOutcome::Completed(_)
    ));
    let result = vm
        .new_string(
            serde_json::json!({"ok":true,"message":message})
                .to_string()
                .encode_utf16()
                .collect(),
        )
        .unwrap();
    vm.start(
        program.entry("Renraku.Guest.AgentHarness::Apply").unwrap(),
        vec![Value::Reference(result), Value::I32(1)],
    )
    .unwrap();
    vm.run_checkpointed(&mut store, 1000, 100).unwrap();
    let before = store.generation();
    assert_eq!(vm.agent_status().unwrap()["phase"], 3);
    drop(vm);
    drop(store);
    let args_drive = vec![
        "agent-drive".into(),
        universe.display().to_string(),
        directory.0.join("receipts").display().to_string(),
        directory.0.join("compiler.db").display().to_string(),
        "/missing/compiler.dll".into(),
        profile.to_string(),
        "chosen-model".into(),
        "0".into(),
        "0".into(),
        "0".into(),
        "--allow-model-calls".into(),
    ];
    let output = command(&directory.0, &args_drive);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["stop"], "finished");
    assert_eq!(report["instructions"], 0);
    assert_eq!(report["providerOperations"], 0);
    assert!(report["recovery"].get("resumeArgs").is_none());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        stderr.contains("turn 1 saved action [finish]")
            && stderr.contains("turn 1 saved result [finish]")
    );
    assert!(stderr.contains("one\\nline\\u{1b}[31m🦀") && !stderr.contains('\u{1b}'));
    for line in stderr.lines().filter(|l| l.starts_with("turn ")) {
        assert!(line.len() < 600);
    }
    assert_eq!(
        CheckpointStore::open(&universe)
            .unwrap()
            .load()
            .unwrap()
            .generation,
        before
    );

    // A preflight error remains an error and retains its original saved world.
    args[1] = directory.0.join("signed-out.db").display().to_string();
    args[10] = "1000".into();
    args[11] = "1".into();
    args[12] = "16".into();
    let output = command(&directory.0, &args);
    assert!(!output.status.success() && output.stdout.is_empty());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        stderr.contains("signed out")
            && stderr.contains("Saved work is retained")
            && stderr.contains("Frames:")
    );
}

#[test]
fn cli_rejects_unrelated_or_duplicate_publication_flags_and_keeps_model_opt_in_separate() {
    let directory = Directory::new();
    for (args, error) in [
        (vec!["status", "--allow-web-publication"], "only supported"),
        (
            vec![
                "--help",
                "--allow-web-publication",
                "--allow-web-publication",
            ],
            "duplicate authorization",
        ),
        (
            vec![
                "agent",
                "world.db",
                "receipts",
                "compiler.db",
                "compiler.dll",
                "core.dll",
                "guest.dll",
                "profile",
                "model",
                "goal",
                "1",
                "1",
                "1",
                "--allow-web-publication",
            ],
            "requires --allow-model-calls",
        ),
    ] {
        let args: Vec<String> = args.into_iter().map(str::to_owned).collect();
        let output = command(&directory.0, &args);
        assert!(!output.status.success());
        assert!(String::from_utf8_lossy(&output.stderr).contains(error));
        assert!(!directory.0.join("auth").exists());
    }
}
