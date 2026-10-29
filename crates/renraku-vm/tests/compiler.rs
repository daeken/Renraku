#[allow(dead_code)]
mod common;
use common::fixtures;
use renraku_vm::{
    artifact::{BuildArtifact, install_once, install_once_with_observer},
    checkpoint::CheckpointStore,
    effects::{ModelProvider, ModelRequest},
    error::{Error, Result},
    identity::UniverseId,
    program::Program,
    runtime::{RunOutcome, Vm},
    value::Value,
    workbench::{ReferenceSet, ReferenceView},
};

#[test]
fn external_compiler_emits_dependency_facades_from_set_envelopes_and_reversed_inputs() {
    let directory = Directory::new();
    let map = fs::read(fixtures().output.join("Renraku.Collections.dll")).unwrap();
    let owner = fs::read(fixtures().output.join("Cross.Owner.dll")).unwrap();
    let program = Program::load(&[&fixtures().core, &map, &owner]).unwrap();
    let set = program
        .reference_set(&[program.find_type("CrossOwner").unwrap()])
        .unwrap();
    let path = directory.0.join("set.json");
    fs::write(
        &path,
        serde_json::to_vec_pretty(
            &serde_json::from_slice::<Json>(&set.to_json().unwrap()).unwrap(),
        )
        .unwrap(),
    )
    .unwrap();
    let source=directory.source("Caller.cs","public static class Caller { public static int Run() { return CrossOwner.Saved == null ? 0 : CrossOwner.Saved.Count(); } }");
    let (output, report) = directory.compile_with_views(
        std::slice::from_ref(&source),
        "set-bundle",
        std::slice::from_ref(&path),
    );
    assert_success(&output, &report);
    let artifact = BuildArtifact::from_bundle(directory.0.join("set-bundle")).unwrap();
    artifact.validate().unwrap();
    let mut reordered: Json = serde_json::from_slice(&set.to_json().unwrap()).unwrap();
    reordered["views"].as_array_mut().unwrap().reverse();
    let reordered_path = directory.0.join("reordered-set.json");
    fs::write(
        &reordered_path,
        serde_json::to_vec_pretty(&reordered).unwrap(),
    )
    .unwrap();
    let (output, report) = directory.compile_with_views(
        std::slice::from_ref(&source),
        "reordered-set-bundle",
        &[reordered_path],
    );
    assert_success(&output, &report);
    let mut paths = Vec::new();
    for (index, view) in set.views.iter().rev().enumerate() {
        let path = directory.0.join(format!("view-{index}.json"));
        fs::write(&path, view.to_json().unwrap()).unwrap();
        paths.push(path);
    }
    let (output, report) = directory.compile_with_views(&[source], "reversed-bundle", &paths);
    assert_success(&output, &report);
    assert_eq!(
        fs::read(directory.0.join("set-bundle/Compiled.Example.dll")).unwrap(),
        fs::read(directory.0.join("reversed-bundle/Compiled.Example.dll")).unwrap()
    );
    let missing = ReferenceSet::from_views(vec![set.views[1].clone()]);
    assert!(missing.is_err());
    let (output, report) = directory.compile_with_views(
        &[directory.source("Missing.cs", "public class Missing {}")],
        "missing",
        &paths[..1],
    );
    assert!(!output.status.success());
    assert!(report["error"].as_str().unwrap().contains("missing"));
    assert!(!directory.0.join("missing").exists());
}
use serde_json::Value as Json;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs,
    io::{BufRead, BufReader},
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
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
        self.compile_with_views(sources, output, &[])
    }
    fn compile_with_views(
        &self,
        sources: &[PathBuf],
        output: &str,
        views: &[PathBuf],
    ) -> (Output, Json) {
        self.compile_named(sources, output, views, "Compiled.Example")
    }
    fn compile_named(
        &self,
        sources: &[PathBuf],
        output: &str,
        views: &[PathBuf],
        name: &str,
    ) -> (Output, Json) {
        let mut command = Command::new("dotnet");
        command
            .arg(compiler())
            .arg("--core")
            .arg(fixtures().output.join("Renraku.CoreLib.dll"))
            .arg("--output")
            .arg(self.0.join(output))
            .args(["--name", name]);
        for view in views {
            command.arg("--reference-view").arg(view);
        }
        let result = command.args(sources).output().unwrap();
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

#[test]
fn unsigned_library_installs_append_only_and_frozen_callers_replay_without_sdk() {
    let directory = Directory::new();
    // Start with the original signed library, heap and saved counter already present.
    let program = Program::load(&[
        &fixtures().core,
        &fixtures().guest,
        &fs::read(fixtures().output.join("Renraku.Library.dll")).unwrap(),
    ])
    .unwrap();
    let counter_type = program.find_type("Renraku.Guest.Counter").unwrap();
    let value_field = program
        .fields()
        .find(|f| f.owner == counter_type && f.name == "Value")
        .unwrap()
        .id;
    let root = program
        .fields()
        .find(|f| {
            f.owner == program.find_type("Renraku.Guest.Simulation").unwrap() && f.name == "Saved"
        })
        .unwrap()
        .id;
    let database = directory.0.join("unsigned.db");
    let mut store = CheckpointStore::open(&database).unwrap();
    let mut vm = Vm::new(&program);
    vm.start(common::entry(&program, "SaveObject"), vec![])
        .unwrap();
    assert_eq!(
        vm.run(1000).unwrap(),
        RunOutcome::Completed(Some(Value::I32(19)))
    );
    let counter = vm.static_fields()[&root];
    store.save(&vm).unwrap();
    let source = directory.source(
        "UnsignedIntegerText.cs",
        include_str!("../../../guest/UnsignedIntegerText.cs"),
    );
    let (output, report) = directory.compile_named(
        std::slice::from_ref(&source),
        "unsigned-library",
        &[],
        "Renraku.UnsignedText",
    );
    assert_success(&output, &report);
    let library = BuildArtifact::from_bundle(directory.0.join("unsigned-library")).unwrap();
    let installed = install_once(&mut store, "unsigned/install", &library).unwrap();
    assert_eq!(
        install_once(&mut store, "unsigned/install", &library).unwrap(),
        installed
    );
    let saved = store.load().unwrap();
    let registry = &saved.snapshot.program;
    for method in program.methods() {
        assert_eq!(
            registry.method(method.binding.method).unwrap().binding,
            method.binding
        );
    }
    for before in program.types() {
        let after = registry.types().find(|t| t.id == before.id).unwrap();
        assert_eq!(
            (&after.name, after.base, &after.fields),
            (&before.name, before.base, &before.fields)
        );
    }
    for before in program.fields() {
        let after = registry.fields().find(|f| f.id == before.id).unwrap();
        assert_eq!(
            (&after.name, after.owner, after.value_type),
            (&before.name, before.owner, before.value_type)
        );
    }
    let set = registry
        .reference_set(&[
            registry
                .find_type("Renraku.Text.UnsignedIntegerText")
                .unwrap(),
            registry.find_type("Renraku.Text.IntegerText").unwrap(),
            counter_type,
        ])
        .unwrap();
    assert_eq!(set.views.len(), 3);
    let view = directory.0.join("unsigned-set.json");
    fs::write(&view, set.to_json().unwrap()).unwrap();
    let caller = directory.source(
        "UnsignedCaller.cs",
        r#"
using Renraku.Text;
public static class UnsignedCaller {
 public static int Calls;
 public static UInt64ParseResult Last;
 public static string Run(Renraku.Guest.Counter counter, string text) {
  UInt64ParseResult parsed = UnsignedIntegerText.ParseUInt64(text);
  Last = parsed;
  if (!parsed.Success) return parsed.Error;
  Calls = Calls + 1;
  counter.Add(1);
  return string.Concat(UnsignedIntegerText.FormatUInt64(parsed.Value),
    string.Concat("/", IntegerText.FormatInt32(counter.Value)));
 }
}"#,
    );
    let (output, report) = directory.compile_with_views(
        std::slice::from_ref(&caller),
        "unsigned-caller",
        std::slice::from_ref(&view),
    );
    assert_success(&output, &report);
    let artifact = BuildArtifact::from_bundle(directory.0.join("unsigned-caller")).unwrap();
    assert_eq!(
        manifest(&directory.0.join("unsigned-caller"))["references"]
            .as_array()
            .unwrap()
            .len(),
        3
    );
    install_once(&mut store, "unsigned-caller/install", &artifact).unwrap();
    let saved = store.load().unwrap();
    let registry = &saved.snapshot.program;
    let method = registry.entry("UnsignedCaller::Run").unwrap();
    let mut vm = saved.snapshot.restore().unwrap();
    assert_eq!(vm.static_fields()[&root], counter);
    let text = vm
        .new_string("+00018446744073709551615".encode_utf16().collect())
        .unwrap();
    let args = vec![counter, Value::Reference(text)];
    let intent = vm
        .invoke_once(
            &mut store,
            "unsigned-caller/invoke",
            method,
            args.clone(),
            0,
            7,
        )
        .unwrap();
    assert_eq!(
        intent.state,
        renraku_vm::invocation::InvocationState::Running
    );
    // Pause in a foreign parser frame with typed arguments.
    let pending = vm
        .invoke_once(&mut store, "unsigned-caller/invoke", method, args, 7, 7)
        .unwrap();
    assert_eq!(pending.execution, intent.execution);
    assert!(vm.context().frames.len() > 1);
    vm.collect_garbage().unwrap();
    assert!(vm.heap().object(text).is_ok());
    store.save(&vm).unwrap();
    drop(store);
    let caller_source = fs::read_to_string(&caller).unwrap();
    for path in ["unsigned-library", "unsigned-caller"] {
        fs::remove_dir_all(directory.0.join(path)).unwrap();
    }
    for path in [source, caller, view] {
        fs::remove_file(path).unwrap();
    }
    let object_arg = format!("object:{}", counter.referenced_object().unwrap());
    let text_arg = format!("object:{text}");
    let native = |args: &[&str]| -> Json {
        let output = Command::new(env!("CARGO_BIN_EXE_renraku-vm"))
            .args(args)
            .env("PATH", "/no-unsigned-compiler-or-sdk")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice(&output.stdout).unwrap()
    };
    let first = native(&[
        "invoke-once",
        database.to_str().unwrap(),
        "unsigned-caller/invoke",
        &method.to_string(),
        "10000",
        &object_arg,
        &text_arg,
    ]);
    assert_eq!(first["state"], "completed");
    assert_eq!(first["execution"], intent.execution.to_string());
    let mut store = CheckpointStore::open(&database).unwrap();
    let saved = store.load().unwrap();
    let registry = &saved.snapshot.program;
    let mut vm = saved.snapshot.restore().unwrap();
    let receipt = vm.invocation("unsigned-caller/invoke").unwrap().clone();
    let renraku_vm::invocation::InvocationState::Completed {
        result: Some(value),
    } = receipt.state
    else {
        panic!()
    };
    assert_eq!(
        String::from_utf16(
            vm.heap()
                .object(value.referenced_object().unwrap())
                .unwrap()
                .string_units()
                .unwrap()
        )
        .unwrap(),
        "18446744073709551615/20"
    );
    let calls = registry
        .fields()
        .find(|f| f.owner == registry.find_type("UnsignedCaller").unwrap() && f.name == "Calls")
        .unwrap()
        .id;
    let last = registry
        .fields()
        .find(|f| f.owner == registry.find_type("UnsignedCaller").unwrap() && f.name == "Last")
        .unwrap()
        .id;
    let result = vm
        .heap()
        .object(vm.static_fields()[&last].referenced_object().unwrap())
        .unwrap();
    let result_value = registry
        .fields()
        .find(|f| f.owner == result.type_id() && f.name == "Value")
        .unwrap()
        .id;
    assert_eq!(result.fields()[&result_value], Value::I64(-1));
    assert_eq!(vm.static_fields()[&calls], Value::I32(1));
    assert_eq!(
        vm.heap()
            .object(counter.referenced_object().unwrap())
            .unwrap()
            .fields()[&value_field],
        Value::I32(20)
    );
    vm.start(common::entry(registry, "Arithmetic"), vec![])
        .unwrap();
    vm.run(1000).unwrap();
    vm.collect_garbage().unwrap();
    store.save(&vm).unwrap();
    drop(store);
    let replay = native(&[
        "invoke-once",
        database.to_str().unwrap(),
        "unsigned-caller/invoke",
        &method.to_string(),
        "10000",
        &object_arg,
        &text_arg,
    ]);
    assert_eq!(replay["execution"], first["execution"]);
    assert_eq!(replay["result"], first["result"]);
    let mut store = CheckpointStore::open(&database).unwrap();
    let saved = store.load().unwrap();
    let vm = saved.snapshot.restore().unwrap();
    assert_eq!(vm.static_fields()[&root], counter);
    assert_eq!(vm.static_fields()[&calls], Value::I32(1));
    assert_eq!(
        vm.heap()
            .object(counter.referenced_object().unwrap())
            .unwrap()
            .fields()[&value_field],
        Value::I32(20)
    );
    assert_eq!(vm.invocation("unsigned-caller/invoke").unwrap(), &receipt);
    let source = vm.read_code(&method.to_string()).unwrap();
    assert_eq!(source["sourceAvailable"], true);
    assert_eq!(source["sources"][0]["text"], caller_source);
}

#[test]
fn runtime_reference_view_links_callers_to_existing_objects_and_installs_with_live_frames() {
    let directory = Directory::new();
    let program = common::program();
    let counter = program.find_type("Renraku.Guest.Counter").unwrap();
    let view = program.reference_view(&[counter]).unwrap();
    let view_file = directory.0.join("counter.view.json");
    fs::write(&view_file, view.to_json().unwrap()).unwrap();
    let source = directory.source("Generated.cs", "namespace Generated { public static class Entry { public static int Calls; public static int Run(Renraku.Guest.Counter value) { Calls += 1; return value.Add(delta: 7); } } }");
    let (output, report) = directory.compile_with_views(&[source], "bundle", &[view_file]);
    assert_success(&output, &report);
    let bundle = directory.0.join("bundle");
    let guest = fs::read(bundle.join("Compiled.Example.dll")).unwrap();
    let facade = fs::read(bundle.join("references/Renraku.Simulation.dll")).unwrap();
    let error = Program::load(&[&fixtures().core, &facade]).unwrap_err();
    assert!(error.0.contains("reference assemblies"), "{error}");
    let linked = program
        .link_modules(&[&guest], std::slice::from_ref(&view))
        .unwrap();
    assert_eq!(linked.find_type("Renraku.Guest.Counter").unwrap(), counter);
    for method in program.methods() {
        assert_eq!(
            linked.method(method.binding.method).unwrap().binding,
            method.binding
        );
    }
    let mut vm = Vm::new(&program);
    let mut store = CheckpointStore::open(directory.0.join("universe.db")).unwrap();
    vm.start(common::entry(&program, "SaveObject"), vec![])
        .unwrap();
    assert_eq!(
        vm.run_checkpointed(&mut store, 1000, 5).unwrap(),
        RunOutcome::Completed(Some(Value::I32(19)))
    );
    let saved = program
        .fields()
        .find(|f| {
            f.name == "Saved" && f.owner == program.find_type("Renraku.Guest.Simulation").unwrap()
        })
        .unwrap()
        .id;
    let object = vm.static_fields()[&saved];
    let invocation = vm
        .invoke_once(
            &mut store,
            "running-original",
            common::entry(&program, "Run"),
            vec![],
            6,
            6,
        )
        .unwrap();
    assert_eq!(
        invocation.state,
        renraku_vm::invocation::InvocationState::Running
    );
    let frames = vm.context().frames.clone();
    let execution = vm.context().id;
    let mut vm = vm.install_program(&mut store, &linked).unwrap();
    assert_eq!(vm.context().frames, frames);
    assert_eq!(vm.context().id, execution);
    view.validate(&linked).unwrap();
    assert_eq!(
        vm.run_checkpointed(&mut store, 1000, 7).unwrap(),
        RunOutcome::Completed(Some(Value::I32(48)))
    );
    assert_eq!(
        vm.invocation("running-original").unwrap().state,
        renraku_vm::invocation::InvocationState::Completed {
            result: Some(Value::I32(48))
        }
    );
    vm.start(linked.entry("Generated.Entry::Run").unwrap(), vec![object])
        .unwrap();
    assert_eq!(
        vm.run_checkpointed(&mut store, 1000, 3).unwrap(),
        RunOutcome::Completed(Some(Value::I32(26)))
    );
    assert_eq!(vm.static_fields()[&saved], object);
    drop(vm);
    fs::remove_dir_all(bundle).unwrap();
    let stored = store.load().unwrap();
    let mut vm = stored.snapshot.restore().unwrap();
    view.validate(&stored.snapshot.program).unwrap();
    vm.start(
        stored
            .snapshot
            .program
            .entry("Generated.Entry::Run")
            .unwrap(),
        vec![object],
    )
    .unwrap();
    assert_eq!(
        vm.run_checkpointed(&mut store, 1000, 3).unwrap(),
        RunOutcome::Completed(Some(Value::I32(33)))
    );
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

fn artifact_inputs(bundle: &Path) -> (Vec<u8>, BTreeMap<String, Vec<u8>>) {
    let bytes = fs::read(bundle.join("renraku-build.json")).unwrap();
    let manifest: Json = serde_json::from_slice(&bytes).unwrap();
    let mut records = vec![manifest["core"].clone(), manifest["assembly"].clone()];
    records.extend(manifest["sources"].as_array().unwrap().iter().cloned());
    for reference in manifest["references"].as_array().unwrap() {
        records.extend([reference["image"].clone(), reference["view"].clone()]);
    }
    let files = records
        .iter()
        .map(|record| {
            let name = record["file"].as_str().unwrap();
            (name.to_owned(), fs::read(bundle.join(name)).unwrap())
        })
        .collect();
    (bytes, files)
}
fn digest_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}
fn compile_artifact(directory: &Directory, source: &str, output: &str) -> BuildArtifact {
    let source = directory.source("Captured.cs", source);
    let (result, report) = directory.compile(&[source], output);
    assert_success(&result, &report);
    BuildArtifact::from_bundle(directory.0.join(output)).unwrap()
}

#[test]
fn captured_artifacts_validate_all_declared_files_and_reject_unsafe_paths_and_changed_contracts() {
    let directory = Directory::new();
    let source = "\u{feff}// captured Ω 😀\npublic static class Extra { public static int Run() { return 9; } }";
    let artifact = compile_artifact(&directory, source, "bundle");
    let copied = compile_artifact(&directory, source, "copy");
    assert_eq!(artifact.id(), copied.id());
    let bundle = directory.0.join("bundle");
    let (manifest, files) = artifact_inputs(&bundle);
    assert_eq!(
        BuildArtifact::from_files(manifest.clone(), files.clone())
            .unwrap()
            .id(),
        artifact.id()
    );
    for case in 0..6 {
        let mut manifest: Json = serde_json::from_slice(&manifest).unwrap();
        let mut files = files.clone();
        match case {
            0 => {
                files.get_mut("sources/Captured.cs").unwrap().push(b' ');
            }
            1 => manifest["sources"][0]["file"] = serde_json::json!("sources/../../outside.cs"),
            2 => manifest["options"]["allowUnsafe"] = serde_json::json!(true),
            3 => {
                files.insert("extra.bin".into(), vec![]);
            }
            4 => {
                let data = vec![0xff];
                manifest["sources"][0]["bytes"] = serde_json::json!(data.len());
                manifest["sources"][0]["sha256"] = serde_json::json!(digest_hex(&data));
                files.insert("sources/Captured.cs".into(), data);
            }
            5 => manifest["sources"][0]["bytes"] = serde_json::json!(1024 * 1024 + 1),
            _ => unreachable!(),
        }
        assert!(
            BuildArtifact::from_files(serde_json::to_vec(&manifest).unwrap(), files).is_err(),
            "case {case}"
        );
    }
    #[cfg(unix)]
    {
        fs::remove_file(bundle.join("sources/Captured.cs")).unwrap();
        std::os::unix::fs::symlink(
            directory.0.join("Captured.cs"),
            bundle.join("sources/Captured.cs"),
        )
        .unwrap();
        assert!(
            BuildArtifact::from_bundle(bundle)
                .unwrap_err()
                .0
                .contains("symlinks")
        );
    }
}

#[test]
fn recorded_installation_retains_source_frames_and_invocations_and_replays_without_new_identities()
{
    let directory = Directory::new();
    let program = common::program();
    let source = "\u{feff}// original source Ω 😀\npublic static class Extra { public static int Run() { return 9; } }";
    let artifact = compile_artifact(&directory, source, "bundle");
    let mut store = CheckpointStore::open(directory.0.join("universe.db")).unwrap();
    let mut vm = Vm::new(&program);
    vm.invoke_once(
        &mut store,
        "original-call",
        common::entry(&program, "Run"),
        vec![],
        6,
        6,
    )
    .unwrap();
    let frames = vm.context().frames.clone();
    let execution = vm.context().id;
    let instructions = vm.instructions_executed();
    drop(vm);
    let receipt = install_once(&mut store, "install-1", &artifact).unwrap();
    let generation = store.generation();
    assert_eq!(
        install_once(&mut store, "install-1", &artifact).unwrap(),
        receipt
    );
    assert_eq!(store.generation(), generation);
    assert!(install_once(&mut store, "original-call", &artifact).is_err());
    assert!(install_once(&mut store, "install-2", &artifact).is_err()); // existing module name
    let other = compile_artifact(
        &directory,
        "public static class Extra { public static int Run() { return 10; } }",
        "other",
    );
    assert!(install_once(&mut store, "install-1", &other).is_err());
    fs::remove_dir_all(directory.0.join("bundle")).unwrap();
    fs::remove_file(directory.0.join("Captured.cs")).unwrap();
    let saved = store.load().unwrap();
    let mut vm = saved.snapshot.restore().unwrap();
    assert_eq!(vm.context().frames, frames);
    assert_eq!(vm.context().id, execution);
    assert_eq!(vm.instructions_executed(), instructions);
    assert!(
        vm.invoke_once(
            &mut store,
            "install-1",
            saved.snapshot.program.entry("Extra::Run").unwrap(),
            vec![],
            1000,
            5
        )
        .is_err()
    );
    assert_eq!(
        vm.run_checkpointed(&mut store, 1000, 5).unwrap(),
        RunOutcome::Completed(Some(Value::I32(48)))
    );
    let code = vm.read_code("Extra::Run").unwrap();
    assert_eq!(code["sourceAvailable"], true);
    assert_eq!(code["sources"][0]["text"], source);
    assert_eq!(code["artifactId"], artifact.id());
    assert_eq!(code["sourceScope"], "assembly");
    vm.collect_garbage().unwrap();
    store.save(&vm).unwrap();
    assert_eq!(
        vm.invoke_once(
            &mut store,
            "new-call",
            saved.snapshot.program.entry("Extra::Run").unwrap(),
            vec![],
            1000,
            5
        )
        .unwrap()
        .state,
        renraku_vm::invocation::InvocationState::Completed {
            result: Some(Value::I32(9))
        }
    );
    let next = store.load().unwrap();
    assert_eq!(
        next.snapshot.restore().unwrap().installation("install-1"),
        Some(&receipt)
    );
}

#[test]
fn installation_commit_failures_bad_cil_and_mismatched_cores_publish_no_partial_registry() {
    let directory = Directory::new();
    let program = common::program();
    let artifact = compile_artifact(
        &directory,
        "public static class Extra { public static int Run() { return 9; } }",
        "bundle",
    );
    let database = directory.0.join("universe.db");
    let mut store = CheckpointStore::open(&database).unwrap();
    store.save(&Vm::new(&program)).unwrap();
    for failure in [
        renraku_vm::checkpoint::CommitStage::Prepared,
        renraku_vm::checkpoint::CommitStage::Written,
    ] {
        assert!(
            install_once_with_observer(&mut store, "install-1", &artifact, |stage| {
                if stage == failure {
                    Err(Error("injected install error".into()))
                } else {
                    Ok(())
                }
            })
            .is_err()
        );
        let saved = store.load().unwrap();
        assert!(saved.snapshot.program.entry("Extra::Run").is_err());
        assert!(
            saved
                .snapshot
                .restore()
                .unwrap()
                .installation("install-1")
                .is_none()
        );
    }
    let connection = rusqlite::Connection::open(&database).unwrap();
    connection.execute_batch("CREATE TRIGGER reject_install BEFORE UPDATE ON checkpoint BEGIN SELECT RAISE(ABORT,'injected installation failure'); END;").unwrap();
    assert!(install_once(&mut store, "install-1", &artifact).is_err());
    assert_eq!(store.generation(), 1);
    connection
        .execute_batch("DROP TRIGGER reject_install")
        .unwrap();
    let bad = compile_artifact(
        &directory,
        "public static class Extra { public static int Run(int a,int b) { return a<<b; } }",
        "bad-cil",
    );
    assert!(
        install_once(&mut store, "bad-cil", &bad)
            .unwrap_err()
            .0
            .contains("unsupported CIL")
    );
    let (manifest, mut files) = artifact_inputs(&directory.0.join("bundle"));
    let mut manifest: Json = serde_json::from_slice(&manifest).unwrap();
    files.get_mut("Renraku.CoreLib.dll").unwrap().push(0);
    let bytes = &files["Renraku.CoreLib.dll"];
    manifest["core"]["bytes"] = serde_json::json!(bytes.len());
    manifest["core"]["sha256"] = serde_json::json!(digest_hex(bytes));
    let changed = BuildArtifact::from_files(serde_json::to_vec(&manifest).unwrap(), files).unwrap();
    assert!(
        install_once(&mut store, "wrong-core", &changed)
            .unwrap_err()
            .0
            .contains("core does not match")
    );
    assert_eq!(store.generation(), 1);
    assert!(
        install_once_with_observer(&mut store, "install-1", &artifact, |stage| {
            if stage == renraku_vm::checkpoint::CommitStage::Committed {
                Err(Error("lost post-commit reply".into()))
            } else {
                Ok(())
            }
        })
        .is_err()
    );
    let saved = store.load().unwrap();
    let first = saved.snapshot.program.entry("Extra::Run").unwrap();
    let receipt = install_once(&mut store, "install-1", &artifact).unwrap();
    assert_eq!(store.generation(), 2);
    assert_eq!(
        store
            .load()
            .unwrap()
            .snapshot
            .program
            .entry("Extra::Run")
            .unwrap(),
        first
    );
    assert_eq!(receipt.artifact, artifact);
}

#[test]
fn sigkill_installation_recovery_commits_source_code_and_receipt_together() {
    let directory = Directory::new();
    let program = common::program();
    let artifact = compile_artifact(
        &directory,
        "public static class Extra { public static int Run() { return 9; } }",
        "bundle",
    );
    for stage in ["prepared", "written", "committed"] {
        let database = directory.0.join(format!("{stage}.db"));
        let mut store = CheckpointStore::open(&database).unwrap();
        store.save(&Vm::new(&program)).unwrap();
        drop(store);
        let mut child = Command::new(env!("CARGO_BIN_EXE_renraku-checkpoint-probe"))
            .arg("install")
            .arg(&database)
            .arg("install-1")
            .arg(directory.0.join("bundle"))
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
            let result = BufReader::new(stdout).read_line(&mut line);
            let _ = send.send((result, line));
        });
        let ready = receive.recv_timeout(Duration::from_secs(20));
        child.kill().unwrap();
        child.wait().unwrap();
        reader.join().unwrap();
        let (read, line) = ready.expect("install probe did not reach commit boundary");
        assert!(read.unwrap() > 0 && line.starts_with("READY"), "{line}");
        let mut store = CheckpointStore::open(&database).unwrap();
        let saved = store.load().unwrap();
        let vm = saved.snapshot.restore().unwrap();
        assert_eq!(vm.installation("install-1").is_some(), stage == "committed");
        assert_eq!(
            saved.snapshot.program.entry("Extra::Run").is_ok(),
            stage == "committed"
        );
        let prior = vm.installation("install-1").map(|record| record.assembly);
        let record = install_once(&mut store, "install-1", &artifact).unwrap();
        if let Some(prior) = prior {
            assert_eq!(record.assembly, prior);
        }
        let next = store.load().unwrap();
        assert_eq!(
            next.snapshot
                .restore()
                .unwrap()
                .read_code("Extra::Run")
                .unwrap()["sourceAvailable"],
            true
        );
    }
}

#[test]
fn installation_references_cannot_be_retargeted_to_another_universe() {
    let directory = Directory::new();
    let original = common::program();
    let view = original
        .reference_view(&[original.find_type("Renraku.Guest.Counter").unwrap()])
        .unwrap();
    let view_file = directory.0.join("view.json");
    fs::write(&view_file, view.to_json().unwrap()).unwrap();
    let source=directory.source("Caller.cs","public static class Caller { public static int Run(Renraku.Guest.Counter value) { return value.Add(7); } }");
    let (result, report) = directory.compile_with_views(&[source], "bundle", &[view_file]);
    assert_success(&result, &report);
    let artifact = BuildArtifact::from_bundle(directory.0.join("bundle")).unwrap();
    let different = common::program();
    let mut other_store = CheckpointStore::open(directory.0.join("different.db")).unwrap();
    other_store.save(&Vm::new(&different)).unwrap();
    assert!(install_once(&mut other_store, "install-1", &artifact).is_err());
    assert_eq!(other_store.generation(), 1);
    assert!(
        other_store
            .load()
            .unwrap()
            .snapshot
            .program
            .entry("Caller::Run")
            .is_err()
    );
    let mut original_store = CheckpointStore::open(directory.0.join("original.db")).unwrap();
    original_store.save(&Vm::new(&original)).unwrap();
    install_once(&mut original_store, "install-1", &artifact).unwrap();
    let saved = original_store.load().unwrap();
    assert_eq!(
        saved
            .snapshot
            .program
            .find_type("Renraku.Guest.Counter")
            .unwrap(),
        original.find_type("Renraku.Guest.Counter").unwrap()
    );
}

#[test]
fn concurrent_writers_fence_installation_without_losing_their_newer_heap_state() {
    let directory = Directory::new();
    let program = common::program();
    let artifact = compile_artifact(
        &directory,
        "public static class Extra { public static int Run() { return 9; } }",
        "bundle",
    );
    let database = directory.0.join("universe.db");
    let mut store = CheckpointStore::open(&database).unwrap();
    store.save(&Vm::new(&program)).unwrap();
    assert!(
        install_once_with_observer(&mut store, "install-1", &artifact, |stage| {
            if stage == renraku_vm::checkpoint::CommitStage::Prepared {
                let mut other = CheckpointStore::open(&database)?;
                let saved = other.load()?;
                let mut vm = saved.snapshot.restore()?;
                vm.start(
                    common::entry(&saved.snapshot.program, "StaticRoundTrip"),
                    vec![],
                )?;
                vm.run_checkpointed(&mut other, 1000, 1000)?;
            }
            Ok(())
        })
        .unwrap_err()
        .0
        .contains("conflict")
    );
    let saved = store.load().unwrap();
    assert!(saved.snapshot.program.entry("Extra::Run").is_err());
    assert!(
        saved
            .snapshot
            .restore()
            .unwrap()
            .installation("install-1")
            .is_none()
    );
    install_once(&mut store, "install-1", &artifact).unwrap();
    let saved = store.load().unwrap();
    let vm = saved.snapshot.restore().unwrap();
    let total = saved
        .snapshot
        .program
        .fields()
        .find(|field| field.name == "Total")
        .unwrap()
        .id;
    assert_eq!(vm.static_fields()[&total], Value::I32(4));
}

#[test]
fn corrupted_installation_receipts_and_captured_source_fail_even_with_repaired_checksums() {
    let directory = Directory::new();
    let program = common::program();
    let source = "// corruption marker source\npublic static class Extra { public static int Run() { return 9; } }";
    let artifact = compile_artifact(&directory, source, "bundle");
    for case in 0..4 {
        let database = directory.0.join(format!("corrupt-{case}.db"));
        let mut store = CheckpointStore::open(&database).unwrap();
        let vm = Vm::new(&program);
        let execution = vm.context().id;
        store.save(&vm).unwrap();
        install_once(&mut store, "installation-corruption", &artifact).unwrap();
        let connection = rusqlite::Connection::open(&database).unwrap();
        let mut bytes: Vec<u8> = connection
            .query_row("SELECT payload FROM checkpoint", [], |row| row.get(0))
            .unwrap();
        let request = "installation-corruption";
        let start = bytes
            .windows(request.len())
            .position(|data| data == request.as_bytes())
            .unwrap();
        match case {
            0 => {
                let source_start = bytes
                    .windows(source.len())
                    .position(|data| data == source.as_bytes())
                    .unwrap();
                bytes[source_start] = b' ';
            }
            1 => bytes[start + request.len()..start + request.len() + 16].fill(0),
            2 => bytes[start] = b' ',
            3 => {
                let end = bytes
                    .windows(16)
                    .rposition(|data| data == execution.as_bytes())
                    .unwrap();
                let copy = bytes[start - 4..end].to_vec();
                bytes[start - 8..start - 4].copy_from_slice(&2u32.to_le_bytes());
                bytes.splice(end..end, copy);
            }
            _ => unreachable!(),
        }
        connection
            .execute(
                "UPDATE checkpoint SET payload=?1,digest=?2",
                rusqlite::params![&bytes, Sha256::digest(&bytes).as_slice()],
            )
            .unwrap();
        assert!(
            store.load().is_err(),
            "accepted malformed installation case {case}"
        );
    }
}

fn cli(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_renraku-vm"))
        .args(args)
        .output()
        .unwrap()
}
fn cli_ok(args: &[&str]) -> Vec<u8> {
    let output = cli(args);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    output.stdout
}

#[test]
fn scripted_codegen_receipt_compiles_installs_and_executes_through_the_tool_cli() {
    // A deterministic model fixture, not a claim about model intelligence. It
    // exercises the same durable CIL call/result boundary as the real adapter.
    struct CodeProvider {
        submitted: usize,
        receipt: Option<Vec<u16>>,
    }
    impl ModelProvider for CodeProvider {
        fn capability(&self) -> &str {
            "test.codegen"
        }
        fn supports_model(&self, model: &str) -> bool {
            model == "fixture-v1"
        }
        fn submit(&mut self, request: &ModelRequest) -> Result<Vec<u16>> {
            let prompt = String::from_utf16(&request.messages.last().unwrap().content).unwrap();
            assert!(prompt.contains("Renraku.Guest.Counter::Add"));
            assert!(prompt.contains("codeVersion"));
            assert!(prompt.contains("representation\":\"cil"));
            self.submitted += 1;
            let source="namespace Generated { public static class Entry { public static int Run(Renraku.Guest.Counter value) { return value.Add(7); } } }".encode_utf16().collect::<Vec<_>>();
            self.receipt = Some(source.clone());
            Ok(source)
        }
        fn reconcile(&mut self, _request: &ModelRequest) -> Result<Option<Vec<u16>>> {
            Ok(self.receipt.clone())
        }
    }
    let directory = Directory::new();
    let database = directory.0.join("universe.db");
    let database = database.to_str().unwrap();
    let program = common::program();
    let mut store = CheckpointStore::open(database).unwrap();
    let mut vm = Vm::new(&program);
    vm.start(common::entry(&program, "SaveObject"), vec![])
        .unwrap();
    vm.run_checkpointed(&mut store, 1000, 5).unwrap();
    let saved = program
        .fields()
        .find(|f| {
            f.name == "Saved" && f.owner == program.find_type("Renraku.Guest.Simulation").unwrap()
        })
        .unwrap()
        .id;
    let object = vm.static_fields()[&saved];
    let search: Json =
        serde_json::from_slice(&cli_ok(&["search", database, "counter", "20"])).unwrap();
    let code: Json = serde_json::from_slice(&cli_ok(&[
        "read-code",
        database,
        "Renraku.Guest.Counter::Add",
    ]))
    .unwrap();
    let prompt = format!(
        "Write C# only: Generated.Entry.Run(Counter value) returns value.Add(7). Available symbols: {search}. Exact current code: {code}"
    );
    let arguments = ["test.codegen", "fixture-v1", &prompt]
        .into_iter()
        .map(|text| Value::Reference(vm.new_string(text.encode_utf16().collect()).unwrap()))
        .collect();
    vm.set_model_calls_enabled(true);
    vm.start(common::entry(&program, "OpenAIHarness"), arguments)
        .unwrap();
    let RunOutcome::Waiting(effect) = vm.run_checkpointed(&mut store, 1000, 5).unwrap() else {
        panic!("expected suspended model call")
    };
    let mut provider = CodeProvider {
        submitted: 0,
        receipt: None,
    };
    vm.dispatch_model(&mut store, effect, &mut provider)
        .unwrap();
    drop(vm);
    let stored = store.load().unwrap();
    let mut vm = stored.snapshot.restore().unwrap();
    let RunOutcome::Completed(Some(Value::Reference(response))) =
        vm.run_checkpointed(&mut store, 1000, 5).unwrap()
    else {
        panic!("expected source result")
    };
    let source =
        String::from_utf16(vm.heap().object(response).unwrap().string_units().unwrap()).unwrap();
    assert_eq!(provider.submitted, 1);
    assert_eq!(vm.static_fields()[&saved], object);
    drop(vm);
    drop(store);
    let view_file = directory.0.join("view.json");
    let view = cli_ok(&["export-reference", database, "Renraku.Guest.Counter"]);
    fs::write(&view_file, view).unwrap();
    let source_file = directory.source("Generated.cs", &source);
    let (output, report) =
        directory.compile_with_views(&[source_file], "bundle", std::slice::from_ref(&view_file));
    assert_success(&output, &report);
    let bundle = directory.0.join("bundle");
    assert_eq!(manifest(&bundle)["format"], 2);
    let installed: Json = serde_json::from_slice(&cli_ok(&[
        "install-once",
        database,
        "model-turn-1/install",
        bundle.to_str().unwrap(),
    ]))
    .unwrap();
    assert_eq!(installed["state"], "installed");
    let replay: Json = serde_json::from_slice(&cli_ok(&[
        "install-once",
        database,
        "model-turn-1/install",
        bundle.to_str().unwrap(),
    ]))
    .unwrap();
    assert_eq!(replay["assemblyId"], installed["assemblyId"]);
    assert_eq!(replay["checkpoint"], installed["checkpoint"]);
    fs::remove_dir_all(bundle).unwrap();
    fs::remove_file(view_file).unwrap();
    let receipt: Json =
        serde_json::from_slice(&cli_ok(&["installation", database, "model-turn-1/install"]))
            .unwrap();
    assert_eq!(receipt["artifactId"], installed["artifactId"]);
    let output = cli_ok(&[
        "invoke-once",
        database,
        "model-turn-1/invoke",
        "Generated.Entry::Run",
        "1000",
        "static:Renraku.Guest.Simulation::Saved",
    ]);
    let first: Json = serde_json::from_slice(&output).unwrap();
    assert_eq!(first["state"], "completed");
    assert_eq!(first["result"]["value"], 26);
    let replay: Json = serde_json::from_slice(&cli_ok(&[
        "invoke-once",
        database,
        "model-turn-1/invoke",
        first["methodId"].as_str().unwrap(),
        "1000",
        "static:Renraku.Guest.Simulation::Saved",
    ]))
    .unwrap();
    assert_eq!(replay["execution"], first["execution"]);
    assert_eq!(replay["result"]["value"], 26);
    let receipt: Json =
        serde_json::from_slice(&cli_ok(&["invocation", database, "model-turn-1/invoke"])).unwrap();
    assert_eq!(receipt["execution"], first["execution"]);
    assert_eq!(receipt["state"], "completed");
    let second: Json = serde_json::from_slice(&cli_ok(&[
        "invoke-once",
        database,
        "model-turn-2/invoke",
        "Generated.Entry::Run",
        "1000",
        "static:Renraku.Guest.Simulation::Saved",
    ]))
    .unwrap();
    assert_eq!(second["result"]["value"], 33);
    assert_ne!(second["execution"], first["execution"]);
    let code: Json =
        serde_json::from_slice(&cli_ok(&["read-code", database, "Generated.Entry::Run"])).unwrap();
    assert_eq!(code["sourceAvailable"], true);
    assert_eq!(code["sources"][0]["text"], source);
    assert_eq!(code["artifactId"], installed["artifactId"]);
    assert!(
        code["instructions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|instruction| instruction["target"]["name"] == "Renraku.Guest.Counter::Add")
    );
    let mut store = CheckpointStore::open(database).unwrap();
    let restored = store.load().unwrap();
    assert_eq!(
        restored.snapshot.restore().unwrap().static_fields()[&saved],
        object
    );
}

#[test]
fn reference_and_installation_failures_preserve_the_published_registry_and_generation() {
    let directory = Directory::new();
    let program = common::program();
    let counter = program.find_type("Renraku.Guest.Counter").unwrap();
    let view = program.reference_view(&[counter]).unwrap();
    view.validate(&program).unwrap();
    assert!(view.validate(&common::program()).is_err()); // same DLLs, different logical universe IDs
    let mut tampered = view.clone();
    tampered.schema_json.push(' ');
    assert!(ReferenceView::from_json(&tampered.to_json().unwrap()).is_err());
    assert!(
        program
            .reference_view(&[program.find_type("System.Int32").unwrap()])
            .is_err()
    );
    let source = directory.source(
        "New.cs",
        "public static class Extra { public static int Run() { return 9; } }",
    );
    let (output, report) = directory.compile(&[source], "bundle");
    assert_success(&output, &report);
    let dll = fs::read(directory.0.join("bundle/Compiled.Example.dll")).unwrap();
    let linked = program
        .link_modules(&[&dll], std::slice::from_ref(&view))
        .unwrap();
    assert!(linked.link_modules(&[&dll], &[]).is_err());
    assert!(program.link_modules(&[b"bad DLL"], &[]).is_err());
    let database = directory.0.join("universe.db");
    let mut store = CheckpointStore::open(&database).unwrap();
    let mut vm = Vm::new(&program);
    vm.start(common::entry(&program, "SaveObject"), vec![])
        .unwrap();
    vm.run_checkpointed(&mut store, 1000, 5).unwrap();
    let generation = store.generation();
    let connection = rusqlite::Connection::open(&database).unwrap();
    connection.execute_batch("CREATE TRIGGER reject_install BEFORE UPDATE ON checkpoint BEGIN SELECT RAISE(ABORT,'injected installation failure'); END;").unwrap();
    assert!(vm.install_program(&mut store, &linked).is_err());
    assert_eq!(store.generation(), generation);
    let restored = store.load().unwrap();
    assert!(restored.snapshot.program.entry("Extra::Run").is_err());
    view.validate(&restored.snapshot.program).unwrap();
    connection
        .execute_batch("DROP TRIGGER reject_install")
        .unwrap();
    let stale = restored.snapshot.restore().unwrap();
    store.save(&restored.snapshot.restore().unwrap()).unwrap();
    assert!(stale.install_program(&mut store, &linked).is_err());
    assert!(
        store
            .load()
            .unwrap()
            .snapshot
            .program
            .entry("Extra::Run")
            .is_err()
    );
}

#[test]
fn symbol_tools_select_exact_logical_code_and_bound_search_results() {
    let program = common::program();
    let result = program.search_symbols("METHOD counter add", 1).unwrap();
    assert_eq!(result["symbols"].as_array().unwrap().len(), 1);
    assert!(result["total"].as_u64().unwrap() >= 1);
    let method = &result["symbols"][0];
    let by_id = program.read_code(method["id"].as_str().unwrap()).unwrap();
    let by_version = program
        .read_code(method["codeVersion"].as_str().unwrap())
        .unwrap();
    assert_eq!(by_id, by_version);
    assert_eq!(by_id["sourceAvailable"], false);
    assert_eq!(by_id["representation"], "cil");
    assert!(
        by_id["instructions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|instruction| instruction["target"]["kind"] == "field")
    );
    let all = program.search_symbols("", 1).unwrap();
    assert_eq!(all["truncated"], true);
    assert!(program.search_symbols("counter", 0).is_err());
    assert!(program.search_symbols(&"x".repeat(257), 20).is_err());
    assert!(program.search_symbols(&"x ".repeat(17), 20).is_err());
    assert!(program.read_code("missing code").is_err());
    assert!(program.find_type("missing type").is_err());
    assert!(program.reference_view(&[]).is_err());
}

#[test]
fn views_preserve_unsigned_types_named_parameters_and_readonly_fields_and_reject_incomplete_shapes()
{
    let directory = Directory::new();
    let source=directory.source("Numbers.cs","public sealed class Numbers { public readonly uint Seed; public uint Value; public Numbers(uint initial) { Seed=initial; Value=initial; } public uint Add(uint delta) { Value+=delta; return Value; } }");
    let (output, report) = directory.compile(&[source], "numbers");
    assert_success(&output, &report);
    let numbers = fs::read(directory.0.join("numbers/Compiled.Example.dll")).unwrap();
    let program = Program::load(&[&fixtures().core, &numbers]).unwrap();
    let view = program
        .reference_view(&[program.find_type("Numbers").unwrap()])
        .unwrap();
    assert!(view.schema_json.contains("readonly uint @Seed"));
    assert!(view.schema_json.contains("uint @delta"));
    let view_file = directory.0.join("view.json");
    fs::write(&view_file, view.to_json().unwrap()).unwrap();
    let source=directory.source("Caller.cs","public static class Caller { public static uint Run() { Numbers value=new Numbers(initial: 4294967295U); return value.Add(delta: 2U); } }");
    let (output, report) = directory.compile_named(
        &[source],
        "caller",
        std::slice::from_ref(&view_file),
        "Numbers.Caller",
    );
    assert_success(&output, &report);
    let caller = fs::read(directory.0.join("caller/Numbers.Caller.dll")).unwrap();
    let linked = program
        .link_modules(&[&caller], std::slice::from_ref(&view))
        .unwrap();
    let mut vm = Vm::new(&linked);
    vm.start(linked.entry("Caller::Run").unwrap(), vec![])
        .unwrap();
    assert_eq!(
        vm.run(1000).unwrap(),
        RunOutcome::Completed(Some(Value::I32(1)))
    );
    for (index, text, reason) in [
        (
            0,
            "public sealed class Shape { public int Value { get { return 3; } } }",
            "properties",
        ),
        (
            1,
            "public sealed class Shape { public const int Value=3; }",
            "literal",
        ),
        (
            2,
            "public sealed class Shape { public int Read(int value=3) { return value; } }",
            "parameter defaults",
        ),
        (
            3,
            "public sealed class Tag : System.Attribute { } [Tag] public sealed class Shape { }",
            "custom attributes",
        ),
    ] {
        let source = directory.source("Shape.cs", text);
        let output_name = format!("shape{index}");
        let (output, report) = directory.compile(&[source], &output_name);
        assert_success(&output, &report);
        let image = fs::read(directory.0.join(output_name).join("Compiled.Example.dll")).unwrap();
        let program = Program::load(&[&fixtures().core, &image]).unwrap();
        let error = program
            .reference_view(&[program.find_type("Shape").unwrap()])
            .unwrap_err();
        assert!(error.0.contains(reason), "{error}");
    }
    let mut malformed = view.clone();
    malformed.core_sha256 = "00".repeat(32);
    fs::write(&view_file, malformed.to_json().unwrap()).unwrap();
    let source = directory.source("BadReference.cs", "public class Caller { }");
    let (output, report) =
        directory.compile_named(&[source], "bad-view", &[view_file], "Numbers.Caller");
    assert_eq!(output.status.code(), Some(2));
    assert!(report["error"].as_str().unwrap().contains("hash"));
    assert!(!directory.0.join("bad-view").exists());
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
        "public static class App { public static int Run(int a, int b) { return a << b; } }",
    );
    let (output, report) = directory.compile(&[source], "bundle");
    assert_success(&output, &report);
    let guest = fs::read(directory.0.join("bundle/Compiled.Example.dll")).unwrap();
    assert!(
        Program::load(&[&fixtures().core, &guest])
            .unwrap_err()
            .0
            .contains("unsupported CIL opcode 62")
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
