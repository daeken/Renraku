#[allow(dead_code)]
mod common;
use common::{entry, fixtures, program};
use renraku_vm::{
    artifact::install_once,
    checkpoint::CheckpointStore,
    compiler::{CompilationRequest, CompilationStage, CompilationStore, CompilerToolchain},
    error::Error,
    identity::UniverseId,
    runtime::{RunOutcome, Vm},
    value::Value,
};
use serde_json::Value as Json;
use std::{
    fs,
    io::{BufRead, BufReader},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{OnceLock, mpsc},
    time::Duration,
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
            .join(format!("durable-compiler-tests-{}", std::process::id()));
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
fn dotnet() -> PathBuf {
    std::env::split_paths(&std::env::var_os("PATH").unwrap())
        .map(|path| path.join("dotnet"))
        .find(|path| path.is_file())
        .unwrap()
        .canonicalize()
        .unwrap()
}
fn toolchain() -> &'static CompilerToolchain {
    static TOOLCHAIN: OnceLock<CompilerToolchain> = OnceLock::new();
    TOOLCHAIN.get_or_init(|| CompilerToolchain::open(dotnet(), compiler()).unwrap())
}
struct Directory(PathBuf);
impl Directory {
    fn new() -> Self {
        let path = fixtures().output.join(UniverseId::fresh().to_string());
        fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn ledger(&self) -> CompilationStore {
        CompilationStore::open(self.0.join("compiler.db")).unwrap()
    }
}
impl Drop for Directory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn request(universe: UniverseId, source: &str) -> CompilationRequest {
    CompilationRequest::capture(
        &program(),
        universe,
        toolchain(),
        "Generated.Example".into(),
        vec![("Generated.cs".into(), source.as_bytes().to_vec())],
        vec![],
    )
    .unwrap()
}
const GOOD: &str = "public static class Generated { public static int Run() { return 41; } }";
fn cli(args: &[&str], empty_path: bool) -> Json {
    let mut command = Command::new(env!("CARGO_BIN_EXE_renraku-vm"));
    command.args(args);
    if empty_path {
        command.env("PATH", "");
    }
    let output = command.output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

#[test]
fn cli_prepared_inputs_compile_after_deleting_sources_then_install_without_the_sdk_or_ledger() {
    let directory = Directory::new();
    let program = program();
    let database = directory.0.join("universe.db");
    let mut store = CheckpointStore::open(&database).unwrap();
    let mut vm = Vm::new(&program);
    vm.start(entry(&program, "SaveObject"), vec![]).unwrap();
    assert_eq!(
        vm.run_checkpointed(&mut store, 1000, 10).unwrap(),
        RunOutcome::Completed(Some(Value::I32(19)))
    );
    drop(vm);
    drop(store);
    let source = "// retained source Ω\npublic static class Generated { public static int Run(Renraku.Guest.Counter value) { return value.Add(7); } }";
    let source_file = directory.0.join("Generated.cs");
    fs::write(&source_file, source).unwrap();
    let view_file = directory.0.join("view.json");
    fs::write(
        &view_file,
        program
            .reference_view(&[program.find_type("Renraku.Guest.Counter").unwrap()])
            .unwrap()
            .to_json()
            .unwrap(),
    )
    .unwrap();
    let ledger = directory.0.join("compiler.db");
    let database = database.to_str().unwrap();
    let ledger = ledger.to_str().unwrap();
    let prepared = cli(
        &[
            "compile-prepare",
            database,
            ledger,
            "turn/compile",
            compiler().to_str().unwrap(),
            "Generated.Caller",
            "--reference-view",
            view_file.to_str().unwrap(),
            source_file.to_str().unwrap(),
        ],
        false,
    );
    assert_eq!(prepared["state"], "prepared");
    fs::remove_file(source_file).unwrap();
    fs::remove_file(view_file).unwrap();
    let compiled = cli(
        &[
            "compile-resume",
            database,
            ledger,
            "turn/compile",
            compiler().to_str().unwrap(),
        ],
        false,
    );
    assert_eq!(compiled["state"], "compiled");
    assert_eq!(compiled["inputId"], prepared["inputId"]);
    let replay = cli(
        &[
            "compile-resume",
            database,
            ledger,
            "turn/compile",
            "/missing/Renraku.Compiler.dll",
        ],
        true,
    );
    assert_eq!(compiled, replay);
    let query = cli(&["compilation", database, ledger, "turn/compile"], true);
    assert_eq!(compiled, query);
    let installed = cli(
        &[
            "install-compilation",
            database,
            ledger,
            "turn/compile",
            "turn/install",
        ],
        true,
    );
    assert_eq!(installed["artifactId"], compiled["artifactId"]);
    fs::remove_file(ledger).unwrap();
    let result = cli(
        &[
            "invoke-once",
            database,
            "turn/invoke",
            "Generated::Run",
            "1000",
            "static:Renraku.Guest.Simulation::Saved",
        ],
        true,
    );
    assert_eq!(result["result"]["value"], 26);
    let code = cli(&["read-code", database, "Generated::Run"], true);
    assert_eq!(code["sources"][0]["text"], source);
}

#[test]
fn compiler_errors_are_terminal_and_replaying_completed_jobs_does_not_launch_the_compiler() {
    let directory = Directory::new();
    let universe = UniverseId::fresh();
    let mut store = directory.ledger();
    let rejected = request(
        universe,
        "public static class Generated { public static int Run() { return ; } }",
    );
    store.prepare("bad", &rejected).unwrap();
    let result = store.drive(universe, "bad", toolchain()).unwrap();
    assert!(result.artifact.is_none());
    let report = result.report.as_ref().unwrap();
    assert_eq!(report["success"], false);
    assert!(
        report["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .any(|diagnostic| diagnostic["code"] == "CS0126")
    );
    assert!(report.get("outputDirectory").is_none());
    assert_eq!(
        store
            .drive_with_observer(universe, "bad", toolchain(), |_| panic!(
                "terminal job was relaunched"
            ))
            .unwrap(),
        result
    );
    assert!(store.prepare("bad", &request(universe, GOOD)).is_err());
    let good = request(universe, GOOD);
    store.prepare("good", &good).unwrap();
    let result = store.drive(universe, "good", toolchain()).unwrap();
    assert_eq!(store.get(universe, "good").unwrap(), result);
    assert_eq!(
        result.artifact.as_ref().unwrap(),
        &renraku_vm::artifact::BuildArtifact::from_bytes(
            &result.artifact.as_ref().unwrap().to_bytes().unwrap()
        )
        .unwrap()
    );
    let other = UniverseId::fresh();
    assert!(store.get(other, "good").is_err());
    store.prepare("good", &request(other, GOOD)).unwrap();
    let program = program();
    let view = program
        .reference_view(&[program.find_type("Renraku.Guest.Counter").unwrap()])
        .unwrap();
    let collision = CompilationRequest::capture(
        &program,
        universe,
        toolchain(),
        "Renraku.Simulation".into(),
        vec![(
            "Generated.cs".into(),
            b"public class Generated { }".to_vec(),
        )],
        vec![view],
    )
    .unwrap();
    store.prepare("bad-reference", &collision).unwrap();
    let rejected = store.drive(universe, "bad-reference", toolchain()).unwrap();
    assert_eq!(rejected.report.as_ref().unwrap()["failureKind"], "input");
    assert!(rejected.artifact.is_none());
}

#[test]
fn captured_inputs_and_compiler_identity_cannot_change_under_an_existing_request() {
    let directory = Directory::new();
    let universe = UniverseId::fresh();
    let mut store = directory.ledger();
    let original = request(universe, GOOD);
    store.prepare("build", &original).unwrap();
    assert!(
        store
            .prepare("build", &request(universe, "public class Different { }"))
            .is_err()
    );
    let package = directory.0.join("package");
    fs::create_dir(&package).unwrap();
    for name in [
        "Renraku.Compiler.dll",
        "Renraku.Compiler.deps.json",
        "Renraku.Compiler.runtimeconfig.json",
        "Microsoft.CodeAnalysis.dll",
        "Microsoft.CodeAnalysis.CSharp.dll",
    ] {
        fs::copy(compiler().parent().unwrap().join(name), package.join(name)).unwrap();
    }
    let copied = CompilerToolchain::open(dotnet(), package.join("Renraku.Compiler.dll")).unwrap();
    assert_eq!(copied.identity(), toolchain().identity());
    let config = package.join("Renraku.Compiler.runtimeconfig.json");
    let bytes = fs::read(&config).unwrap();
    let mut changed = bytes.clone();
    changed.push(b'\n');
    fs::write(&config, changed).unwrap();
    assert!(
        store
            .drive(universe, "build", &copied)
            .unwrap_err()
            .0
            .contains("changed")
    );
    assert!(store.get(universe, "build").unwrap().report.is_none());
    let changed = CompilerToolchain::open(dotnet(), package.join("Renraku.Compiler.dll")).unwrap();
    assert!(store.drive(universe, "build", &changed).is_err());
    fs::write(&config, bytes).unwrap();
    let completed = store.drive(universe, "build", &copied).unwrap();
    fs::remove_dir_all(package).unwrap();
    assert_eq!(store.drive(universe, "build", &copied).unwrap(), completed);
}

#[test]
fn failed_preparation_or_publication_leaves_no_uncommitted_compiler_result() {
    let directory = Directory::new();
    let universe = UniverseId::fresh();
    let mut store = directory.ledger();
    let connection = rusqlite::Connection::open(directory.0.join("compiler.db")).unwrap();
    connection.execute_batch("CREATE TRIGGER reject_prepare BEFORE INSERT ON compilation BEGIN SELECT RAISE(ABORT,'injected prepare failure'); END;").unwrap();
    let request = request(universe, GOOD);
    assert!(store.prepare("build", &request).is_err());
    assert!(store.drive(universe, "build", toolchain()).is_err());
    connection
        .execute_batch("DROP TRIGGER reject_prepare")
        .unwrap();
    store.prepare("build", &request).unwrap();
    for failure in [
        CompilationStage::Prepared,
        CompilationStage::Started,
        CompilationStage::Captured,
        CompilationStage::Written,
    ] {
        assert!(
            store
                .drive_with_observer(
                    universe,
                    "build",
                    toolchain(),
                    |stage| if stage == failure {
                        Err(Error("injected publication failure".into()))
                    } else {
                        Ok(())
                    }
                )
                .is_err()
        );
        assert!(store.get(universe, "build").unwrap().report.is_none());
    }
    connection.execute_batch("CREATE TRIGGER reject_result BEFORE UPDATE ON compilation BEGIN SELECT RAISE(ABORT,'injected result failure'); END;").unwrap();
    assert!(store.drive(universe, "build", toolchain()).is_err());
    assert!(store.get(universe, "build").unwrap().artifact.is_none());
    connection
        .execute_batch("DROP TRIGGER reject_result")
        .unwrap();
    assert!(
        store
            .drive_with_observer(universe, "build", toolchain(), |stage| {
                if stage == CompilationStage::Committed {
                    Err(Error("lost reply after commit".into()))
                } else {
                    Ok(())
                }
            })
            .is_err()
    );
    let saved = store.get(universe, "build").unwrap();
    assert!(saved.artifact.is_some());
    assert_eq!(
        store
            .drive_with_observer(universe, "build", toolchain(), |_| panic!(
                "published job reran"
            ))
            .unwrap(),
        saved
    );
}

#[test]
fn actual_process_kills_recover_only_published_compiler_results() {
    let directory = Directory::new();
    let universe = UniverseId::fresh();
    let request = request(universe, GOOD);
    for stage in ["prepared", "started", "captured", "written", "committed"] {
        let path = directory.0.join(format!("{stage}.db"));
        let mut store = CompilationStore::open(&path).unwrap();
        store.prepare("build", &request).unwrap();
        drop(store);
        let mut child = Command::new(env!("CARGO_BIN_EXE_renraku-checkpoint-probe"))
            .arg("compile")
            .arg(&path)
            .arg(universe.to_string())
            .arg("build")
            .arg(dotnet())
            .arg(compiler())
            .arg(stage)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let stdout = child.stdout.take().unwrap();
        let (send, receive) = mpsc::channel();
        let reader = std::thread::spawn(move || {
            let mut line = String::new();
            let read = BufReader::new(stdout).read_line(&mut line);
            let _ = send.send((read, line));
        });
        let ready = receive.recv_timeout(Duration::from_secs(30));
        child.kill().unwrap();
        child.wait().unwrap();
        reader.join().unwrap();
        let (read, line) = ready.expect("compile probe timed out");
        assert!(read.unwrap() > 0 && line.starts_with("READY"), "{line}");
        let mut store = CompilationStore::open(&path).unwrap();
        let prior = store.get(universe, "build").unwrap();
        assert_eq!(prior.artifact.is_some(), stage == "committed");
        let result = store.drive(universe, "build", toolchain()).unwrap();
        if prior.report.is_some() {
            assert_eq!(result, prior);
        }
        assert!(result.artifact.is_some());
    }
}

#[test]
fn foreign_ledgers_and_corrupted_input_or_terminal_blobs_are_rejected() {
    let directory = Directory::new();
    let universe = UniverseId::fresh();
    let request = request(universe, GOOD);
    assert!(CompilationStore::open(":memory:").is_err());
    let path = directory.0.join("universe.db");
    CheckpointStore::open(&path).unwrap();
    assert!(CompilationStore::open(path).is_err());
    for case in 0..3 {
        let path = directory.0.join(format!("corrupt-{case}.db"));
        let mut store = CompilationStore::open(&path).unwrap();
        store.prepare("build", &request).unwrap();
        store.drive(universe, "build", toolchain()).unwrap();
        let connection = rusqlite::Connection::open(path).unwrap();
        let column = ["inputs", "report", "artifact"][case];
        let query = format!("SELECT {column} FROM compilation");
        let mut bytes: Vec<u8> = connection.query_row(&query, [], |row| row.get(0)).unwrap();
        let last = bytes.len() - 1;
        bytes[last] ^= 1;
        connection
            .execute(
                &format!("UPDATE compilation SET {column}=?1"),
                rusqlite::params![bytes],
            )
            .unwrap();
        assert!(store.get(universe, "build").is_err());
    }
}

#[test]
fn pure_compiler_recovery_does_not_imply_guest_cil_acceptance() {
    let directory = Directory::new();
    let program = program();
    let mut universe_store = CheckpointStore::open(directory.0.join("universe.db")).unwrap();
    let vm = Vm::new(&program);
    let universe = vm.universe_id();
    universe_store.save(&vm).unwrap();
    let request = CompilationRequest::capture(
        &program,
        universe,
        toolchain(),
        "Generated.Checked".into(),
        vec![(
            "Checked.cs".into(),
            b"public static class Generated { public static object Box(int value) { return value; } }"
                .to_vec(),
        )],
        vec![],
    )
    .unwrap();
    let mut store = directory.ledger();
    store.prepare("checked", &request).unwrap();
    let compiled = store.drive(universe, "checked", toolchain()).unwrap();
    assert!(compiled.artifact.is_some());
    assert!(
        install_once(
            &mut universe_store,
            "install",
            compiled.artifact.as_ref().unwrap()
        )
        .unwrap_err()
        .0
        .contains("unsupported CIL")
    );
    assert_eq!(universe_store.generation(), 1);
    assert!(
        universe_store
            .load()
            .unwrap()
            .snapshot
            .restore()
            .unwrap()
            .installation("install")
            .is_none()
    );
}
