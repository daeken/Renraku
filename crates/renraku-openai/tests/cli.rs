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
