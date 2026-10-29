#[allow(dead_code)]
mod common;
use common::fixtures;
use renraku_vm::{
    checkpoint::CheckpointStore,
    identity::UniverseId,
    program::Program,
    runtime::{RunOutcome, Vm},
    value::Value,
};
use serde_json::Value as Json;
use sha2::{Digest, Sha256};
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
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
            .join(format!("compiler-tests-{}", std::process::id()));
        let result = Command::new("dotnet")
            .args(["build", "--nologo", "--verbosity", "quiet", "--output"])
            .arg(&output)
            .arg(root.join("tools/Compiler"))
            .output()
            .expect("external compiler tests require the .NET 10 SDK");
        assert!(
            result.status.success(),
            "compiler tool failed to build: {}\n{}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
        output.join("Renraku.Compiler.dll")
    })
}
struct Directory(PathBuf);
impl Directory {
    fn new() -> Self {
        let directory = fixtures().output.join(UniverseId::fresh().to_string());
        fs::create_dir(&directory).unwrap();
        Self(directory)
    }
    fn source(&self, name: &str, text: &str) -> PathBuf {
        let path = self.0.join(name);
        fs::write(&path, text).unwrap();
        path
    }
    fn compile(&self, sources: &[PathBuf], output: &str) -> (Output, Json) {
        let result = Command::new("dotnet")
            .arg(compiler())
            .arg("--core")
            .arg(fixtures().output.join("Renraku.CoreLib.dll"))
            .arg("--output")
            .arg(self.0.join(output))
            .args(["--name", "Compiled.Example"])
            .args(sources)
            .output()
            .unwrap();
        let json = serde_json::from_slice(&result.stdout).unwrap_or_else(|_| {
            panic!(
                "invalid compiler report: {}\n{}",
                String::from_utf8_lossy(&result.stdout),
                String::from_utf8_lossy(&result.stderr)
            )
        });
        (result, json)
    }
}
impl Drop for Directory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn assert_success(output: &Output, report: &Json) {
    assert!(
        output.status.success(),
        "{report}\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(report["success"], true);
}
fn manifest(directory: &Path) -> Json {
    serde_json::from_slice(&fs::read(directory.join("renraku-build.json")).unwrap()).unwrap()
}

#[test]
fn externally_compiled_code_checkpoints_nested_frames_and_runs_without_the_compiler_or_bundle() {
    let directory = Directory::new();
    let source = directory.source("Application.cs", r#"
namespace Example {
    public sealed class Box { public int Value; public Box(int n) { Value = n; } public int Add() { Value += 3; return Value; } }
    public static class App {
        public static int Visits; public static Box Saved; public static string Text;
        public static int Run() { Visits += 1; Saved = new Box(40); Text = "runtime\0compilation"; return Step(Saved) + Visits; }
        static int Step(Box box) { return box.Add(); }
    }
}"#);
    let (output, report) = directory.compile(std::slice::from_ref(&source), "bundle");
    assert_success(&output, &report);
    let bundle = directory.0.join("bundle");
    let core = fs::read(bundle.join("Renraku.CoreLib.dll")).unwrap();
    let guest = fs::read(bundle.join("Compiled.Example.dll")).unwrap();
    let program = Program::load(&[&core, &guest]).unwrap();
    let mut vm = Vm::new(&program);
    vm.start(program.entry("Example.App::Run").unwrap(), Vec::new())
        .unwrap();
    let mut store = CheckpointStore::open(directory.0.join("universe.db")).unwrap();
    loop {
        assert_eq!(vm.run(1).unwrap(), RunOutcome::Paused);
        if vm.context().frames.len() >= 3 {
            break;
        }
        assert!(vm.instructions_executed() < 100);
    }
    store.save(&vm).unwrap();
    let universe = vm.universe_id();
    let execution = vm.context().id;
    let frames = vm.context().frames.clone();
    drop(vm);
    drop(program);
    drop(core);
    drop(guest);
    fs::remove_dir_all(&bundle).unwrap();
    fs::remove_file(source).unwrap();
    let stored = store.load().unwrap();
    let mut vm = stored.snapshot.restore().unwrap();
    assert_eq!(vm.universe_id(), universe);
    assert_eq!(vm.context().id, execution);
    assert_eq!(vm.context().frames, frames);
    assert_eq!(
        vm.run_checkpointed(&mut store, 1000, 7).unwrap(),
        RunOutcome::Completed(Some(Value::I32(44)))
    );
    vm.collect_garbage().unwrap();
    vm.start(
        stored.snapshot.program.entry("Example.App::Run").unwrap(),
        Vec::new(),
    )
    .unwrap();
    assert_eq!(
        vm.run_checkpointed(&mut store, 1000, 7).unwrap(),
        RunOutcome::Completed(Some(Value::I32(45)))
    );
    // Also prove the standalone CLI needs only the checkpoint at this stage.
    let output = Command::new(env!("CARGO_BIN_EXE_renraku-vm"))
        .env("PATH", "")
        .arg("resume")
        .arg(directory.0.join("universe.db"))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stdout)
            .lines()
            .any(|line| line == "result: 45")
    );
}

#[test]
fn bundles_capture_exact_sources_core_and_options_and_emit_reproducible_assemblies() {
    let first = Directory::new();
    let second = Directory::new();
    for directory in [&first, &second] {
        let a = directory.source("Entry.cs", "namespace Example { public static class App { public static int Run() { return Helper.Value(); } } }");
        let b = directory.source("Helper.cs", "namespace Example { public static class Helper { public static int Value() { return 61; } } }");
        let (output, report) = directory.compile(&[a, b], "bundle");
        assert_success(&output, &report);
        let bundle = directory.0.join("bundle");
        let manifest = manifest(&bundle);
        assert_eq!(manifest["format"], 1);
        assert_eq!(manifest["languageVersion"], "CSharp12");
        assert_eq!(manifest["options"]["deterministic"], true);
        assert_eq!(manifest["options"]["allowUnsafe"], false);
        assert!(
            manifest["compiler"]["csharp"]["name"]
                .as_str()
                .unwrap()
                .starts_with("Microsoft.CodeAnalysis.CSharp,")
        );
        for component in ["csharp", "common", "driver"] {
            assert_eq!(
                manifest["compiler"][component]["sha256"]
                    .as_str()
                    .unwrap()
                    .len(),
                64
            );
        }
        let records = [manifest["core"].clone(), manifest["assembly"].clone()];
        for record in records
            .iter()
            .chain(manifest["sources"].as_array().unwrap())
        {
            let file = bundle.join(record["file"].as_str().unwrap());
            let bytes = fs::read(&file).unwrap();
            assert_eq!(record["bytes"], bytes.len());
            let hash: String = Sha256::digest(&bytes)
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect();
            assert_eq!(record["sha256"], hash);
            if let Some(name) = file
                .file_name()
                .filter(|_| file.parent().unwrap().ends_with("sources"))
            {
                assert_eq!(bytes, fs::read(directory.0.join(name)).unwrap());
            }
        }
        assert_eq!(
            fs::read(bundle.join("Renraku.CoreLib.dll")).unwrap(),
            fixtures().core
        );
    }
    assert_eq!(
        fs::read(first.0.join("bundle/Compiled.Example.dll")).unwrap(),
        fs::read(second.0.join("bundle/Compiled.Example.dll")).unwrap()
    );
    assert_eq!(
        manifest(&first.0.join("bundle")),
        manifest(&second.0.join("bundle"))
    );
}

#[test]
fn compiler_errors_are_structured_and_do_not_publish_bundles_or_inherit_host_framework_references()
{
    let directory = Directory::new();
    for (name, source, code) in [
        (
            "Syntax.cs",
            "namespace Example {\n public static class Broken { public static int Run() { return ; } } }",
            "CS0126",
        ),
        (
            "Host.cs",
            "namespace Example { public static class Host { public static void Run() { System.Console.WriteLine(1); } } }",
            "CS0234",
        ),
    ] {
        let source = directory.source(name, source);
        let (output, report) = directory.compile(&[source], "rejected");
        assert_eq!(output.status.code(), Some(1));
        assert_eq!(report["success"], false);
        let diagnostic = report["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .find(|d| d["code"] == code)
            .unwrap();
        assert_eq!(diagnostic["file"], name);
        assert!(diagnostic["line"].as_u64().unwrap() > 0);
        assert!(!directory.0.join("rejected").exists());
        assert!(!fs::read_dir(&directory.0).unwrap().any(|e| {
            e.unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(".renraku-compile-")
        }));
    }
}

#[test]
fn emitting_a_dll_does_not_imply_support_for_its_cil() {
    let directory = Directory::new();
    let source = directory.source(
        "Unsupported.cs",
        "public static class App { public static int Run(int a, int b) { return a / b; } }",
    );
    let (output, report) = directory.compile(&[source], "bundle");
    assert_success(&output, &report);
    let guest = fs::read(directory.0.join("bundle/Compiled.Example.dll")).unwrap();
    assert!(
        Program::load(&[&fixtures().core, &guest])
            .unwrap_err()
            .0
            .contains("unsupported CIL opcode 5b")
    );
}

#[test]
fn invalid_inputs_limits_and_existing_outputs_are_rejected_without_replacing_artifacts() {
    let directory = Directory::new();
    let source = directory.source(
        "Good.cs",
        "public static class App { public static int Run() { return 9; } }",
    );
    fs::create_dir(directory.0.join("bundle")).unwrap();
    fs::write(directory.0.join("bundle/marker"), b"keep").unwrap();
    let (output, report) = directory.compile(std::slice::from_ref(&source), "bundle");
    assert_eq!(output.status.code(), Some(2));
    assert!(report["error"].as_str().unwrap().contains("already exists"));
    assert_eq!(
        fs::read(directory.0.join("bundle/marker")).unwrap(),
        b"keep"
    );
    let too_large = directory.source("Large.cs", &" ".repeat(1024 * 1024 + 1));
    let (output, _) = directory.compile(&[too_large], "large");
    assert_eq!(output.status.code(), Some(2));
    assert!(!directory.0.join("large").exists());
    let (output, report) = directory.compile(&[source.clone(), source], "duplicate");
    assert_eq!(output.status.code(), Some(2));
    assert!(report["error"].as_str().unwrap().contains("filenames"));
    let invalid = directory.source("BadUtf8.cs", "");
    fs::write(&invalid, [0xff, 0xfe, 0xff]).unwrap();
    let (output, _) = directory.compile(&[invalid], "encoding");
    assert_eq!(output.status.code(), Some(2));
    let output = Command::new("dotnet")
        .arg(compiler())
        .arg("--core")
        .arg(fixtures().output.join("Renraku.Simulation.dll"))
        .arg("--output")
        .arg(directory.0.join("wrong-core"))
        .args(["--name", "Compiled.Example"])
        .arg(directory.0.join("Good.cs"))
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(!directory.0.join("wrong-core").exists());
    let (output, report) = directory.compile(&[directory.0.join("Good.cs")], "trailing-slash/");
    assert_success(&output, &report);
    assert!(
        directory
            .0
            .join("trailing-slash/Compiled.Example.dll")
            .exists()
    );
}
