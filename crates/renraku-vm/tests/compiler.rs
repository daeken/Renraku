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
fn ordinal_library_searches_existing_saved_text_through_frozen_callers_and_sdk_free_replay() {
    let directory = Directory::new();
    let program = common::program();
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
    let database = directory.0.join("ordinal.db");
    let mut store = CheckpointStore::open(&database).unwrap();
    let mut vm = Vm::new(&program);
    vm.start(common::entry(&program, "SaveObject"), vec![])
        .unwrap();
    assert_eq!(
        vm.run(1000).unwrap(),
        RunOutcome::Completed(Some(Value::I32(19)))
    );
    let counter = vm.static_fields()[&root];
    let units = vec![115, 97, 118, 101, 100, 0, 0xd800, 45, 116, 97, 105, 108];
    let text = vm.new_string(units.clone()).unwrap();
    vm.set_root("saved-text", text).unwrap();
    store.save(&vm).unwrap();
    let source = directory.source(
        "OrdinalText.cs",
        include_str!("../../../guest/OrdinalText.cs"),
    );
    let (output, report) = directory.compile_named(
        std::slice::from_ref(&source),
        "ordinal-library",
        &[],
        "Renraku.OrdinalText",
    );
    assert_success(&output, &report);
    let library = BuildArtifact::from_bundle(directory.0.join("ordinal-library")).unwrap();
    let installed = install_once(&mut store, "ordinal/install", &library).unwrap();
    assert_eq!(
        install_once(&mut store, "ordinal/install", &library).unwrap(),
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
    let ordinal_type = registry.find_type("Renraku.Text.OrdinalText").unwrap();
    let result_type = registry
        .find_type("Renraku.Text.StringMatchResult")
        .unwrap();
    let set = registry
        .reference_set(&[ordinal_type, counter_type])
        .unwrap();
    assert_eq!(set.views.len(), 2);
    let view = directory.0.join("ordinal-set.json");
    fs::write(&view, set.to_json().unwrap()).unwrap();
    let caller = directory.source(
        "OrdinalCaller.cs",
        r#"
using Renraku.Text;
public static class OrdinalCaller {
 public static int Calls;
 public static StringMatchResult Last;
 public static int Run(Renraku.Guest.Counter counter, string text) {
  StringMatchResult result = OrdinalText.IndexOf(text, "\0\ud800", 0);
  Last = result;
  if (!result.Success || !result.Found) return result.Index;
  Calls = Calls + 1;
  counter.Add(result.Index);
  return result.Index;
 }
}"#,
    );
    let caller_source = fs::read_to_string(&caller).unwrap();
    let (output, report) = directory.compile_with_views(
        std::slice::from_ref(&caller),
        "ordinal-caller",
        std::slice::from_ref(&view),
    );
    assert_success(&output, &report);
    let artifact = BuildArtifact::from_bundle(directory.0.join("ordinal-caller")).unwrap();
    assert_eq!(
        manifest(&directory.0.join("ordinal-caller"))["references"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    install_once(&mut store, "ordinal-caller/install", &artifact).unwrap();
    let saved = store.load().unwrap();
    let registry = &saved.snapshot.program;
    let mut vm = saved.snapshot.restore().unwrap();
    assert_eq!(vm.heap().roots()["saved-text"], text);
    assert_eq!(
        vm.heap().object(text).unwrap().string_units(),
        Some(units.as_slice())
    );
    let method = registry.entry("OrdinalCaller::Run").unwrap();
    let args = vec![counter, Value::Reference(text)];
    let intent = vm
        .invoke_once(
            &mut store,
            "ordinal-caller/invoke",
            method,
            args.clone(),
            0,
            1,
        )
        .unwrap();
    let mut saw_loop = false;
    for _ in 0..500 {
        let receipt = vm
            .invoke_once(
                &mut store,
                "ordinal-caller/invoke",
                method,
                args.clone(),
                1,
                1,
            )
            .unwrap();
        assert_eq!(receipt.execution, intent.execution);
        assert_eq!(
            receipt.state,
            renraku_vm::invocation::InvocationState::Running
        );
        if vm.context().frames.iter().any(|f| {
            let m = registry.method(f.method.method).unwrap();
            m.owner == ordinal_type && m.name == "MatchAt"
        }) {
            saw_loop = true;
            break;
        }
    }
    assert!(saw_loop);
    vm.collect_garbage().unwrap();
    store.save(&vm).unwrap();
    drop(store);
    for path in ["ordinal-library", "ordinal-caller"] {
        fs::remove_dir_all(directory.0.join(path)).unwrap();
    }
    for path in [source, caller, view] {
        fs::remove_file(path).unwrap();
    }
    let counter_arg = format!("object:{}", counter.referenced_object().unwrap());
    let text_arg = format!("object:{text}");
    let native = || -> Json {
        let output = Command::new(env!("CARGO_BIN_EXE_renraku-vm"))
            .args([
                "invoke-once",
                database.to_str().unwrap(),
                "ordinal-caller/invoke",
                &method.to_string(),
                "10000",
                &counter_arg,
                &text_arg,
            ])
            .env("PATH", "/no-ordinal-compiler-or-sdk")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice(&output.stdout).unwrap()
    };
    let first = native();
    assert_eq!(first["state"], "completed");
    assert_eq!(first["execution"], intent.execution.to_string());
    assert_eq!(first["result"]["value"], 5);
    let mut store = CheckpointStore::open(&database).unwrap();
    let saved = store.load().unwrap();
    let registry = &saved.snapshot.program;
    let mut vm = saved.snapshot.restore().unwrap();
    let receipt = vm.invocation("ordinal-caller/invoke").unwrap().clone();
    let calls = registry
        .fields()
        .find(|f| f.owner == registry.find_type("OrdinalCaller").unwrap() && f.name == "Calls")
        .unwrap()
        .id;
    let last = registry
        .fields()
        .find(|f| f.owner == registry.find_type("OrdinalCaller").unwrap() && f.name == "Last")
        .unwrap()
        .id;
    let result = vm
        .heap()
        .object(vm.static_fields()[&last].referenced_object().unwrap())
        .unwrap();
    assert_eq!(result.type_id(), result_type);
    let index = registry
        .fields()
        .find(|f| f.owner == result_type && f.name == "Index")
        .unwrap()
        .id;
    assert_eq!(result.fields()[&index], Value::I32(5));
    vm.start(common::entry(registry, "Arithmetic"), vec![])
        .unwrap();
    vm.run(1000).unwrap();
    vm.collect_garbage().unwrap();
    store.save(&vm).unwrap();
    drop(store);
    let replay = native();
    for key in [
        "state",
        "execution",
        "result",
        "methodId",
        "codeVersion",
        "requestId",
    ] {
        assert_eq!(first[key], replay[key]);
    }
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
        Value::I32(24)
    );
    assert_eq!(vm.heap().roots()["saved-text"], text);
    assert_eq!(
        vm.heap().object(text).unwrap().string_units(),
        Some(units.as_slice())
    );
    assert_eq!(vm.invocation("ordinal-caller/invoke").unwrap(), &receipt);
    let code = vm.read_code(&method.to_string()).unwrap();
    assert_eq!(code["sourceAvailable"], true);
    assert_eq!(code["sources"][0]["text"], caller_source);
}

#[test]
fn hex_library_reuses_frozen_foreign_results_and_saved_objects_without_implementation_references() {
    let directory = Directory::new();
    let program = Program::load(&[
        &fixtures().core,
        &fixtures().guest,
        &fs::read(fixtures().output.join("Renraku.UnsignedText.dll")).unwrap(),
    ])
    .unwrap();
    let r32 = program.find_type("Renraku.Text.UInt32ParseResult").unwrap();
    let r64 = program.find_type("Renraku.Text.UInt64ParseResult").unwrap();
    let counter_type = program.find_type("Renraku.Guest.Counter").unwrap();
    let counter_value = program
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
    let database = directory.0.join("hex.db");
    let mut store = CheckpointStore::open(&database).unwrap();
    let mut vm = Vm::new(&program);
    vm.start(common::entry(&program, "SaveObject"), vec![])
        .unwrap();
    assert_eq!(
        vm.run(1000).unwrap(),
        RunOutcome::Completed(Some(Value::I32(19)))
    );
    let counter = vm.static_fields()[&root];
    let decimal_input = Value::Reference(
        vm.new_string("18446744073709551615".encode_utf16().collect())
            .unwrap(),
    );
    let decimal = vm
        .invoke_once(
            &mut store,
            "decimal-before-hex",
            program
                .entry("Renraku.Text.UnsignedIntegerText::ParseUInt64")
                .unwrap(),
            vec![decimal_input],
            5000,
            7,
        )
        .unwrap();
    let result_view = directory.0.join("unsigned-results.json");
    let result_set = program.reference_set(&[r32, r64]).unwrap();
    assert_eq!(result_set.views.len(), 1);
    fs::write(&result_view, result_set.to_json().unwrap()).unwrap();
    let hex_source = directory.source(
        "HexIntegerText.cs",
        include_str!("../../../guest/HexIntegerText.cs"),
    );
    let (output, report) = directory.compile_named(
        std::slice::from_ref(&hex_source),
        "hex-library",
        std::slice::from_ref(&result_view),
        "Renraku.HexText",
    );
    assert_success(&output, &report);
    let library = BuildArtifact::from_bundle(directory.0.join("hex-library")).unwrap();
    assert_eq!(
        manifest(&directory.0.join("hex-library"))["references"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    let installed = install_once(&mut store, "hex/install", &library).unwrap();
    assert_eq!(
        install_once(&mut store, "hex/install", &library).unwrap(),
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
    for name in [
        "Renraku.Text.UInt32ParseResult",
        "Renraku.Text.UInt64ParseResult",
    ] {
        assert_eq!(registry.types().filter(|t| t.name == name).count(), 1);
        assert_eq!(
            registry.find_type(name).unwrap(),
            program.find_type(name).unwrap()
        );
    }
    assert_eq!(
        saved
            .snapshot
            .restore()
            .unwrap()
            .invocation("decimal-before-hex")
            .unwrap(),
        &decimal
    );
    let hex_type = registry.find_type("Renraku.Text.HexIntegerText").unwrap();
    let set = registry.reference_set(&[hex_type, counter_type]).unwrap();
    assert_eq!(set.views.len(), 3);
    let hex_view = set
        .views
        .iter()
        .find(|v| {
            serde_json::from_str::<Json>(&v.schema_json).unwrap()["moduleName"] == "Renraku.HexText"
        })
        .unwrap();
    assert_eq!(
        serde_json::from_slice::<Json>(&hex_view.to_json().unwrap()).unwrap()["format"],
        2
    );
    assert_eq!(
        serde_json::from_str::<Json>(&hex_view.schema_json).unwrap()["dependencies"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    let caller_view = directory.0.join("hex-set.json");
    fs::write(&caller_view, set.to_json().unwrap()).unwrap();
    let caller = directory.source(
        "HexCaller.cs",
        r#"
using Renraku.Text;
public static class HexCaller {
 public static int Calls;
 public static UInt64ParseResult Last;
 public static string Run(Renraku.Guest.Counter counter, string text) {
  UInt64ParseResult parsed = HexIntegerText.ParseUInt64(text);
  Last = parsed;
  if (!parsed.Success) return parsed.Error;
  Calls = Calls + 1;
  counter.Add(1);
  return string.Concat(HexIntegerText.FormatUInt64(parsed.Value),
    string.Concat("/", HexIntegerText.FormatUInt32((uint)counter.Value)));
 }
}"#,
    );
    let caller_source = fs::read_to_string(&caller).unwrap();
    let (output, report) = directory.compile_with_views(
        std::slice::from_ref(&caller),
        "hex-caller",
        std::slice::from_ref(&caller_view),
    );
    assert_success(&output, &report);
    let artifact = BuildArtifact::from_bundle(directory.0.join("hex-caller")).unwrap();
    assert_eq!(
        manifest(&directory.0.join("hex-caller"))["references"]
            .as_array()
            .unwrap()
            .len(),
        3
    );
    install_once(&mut store, "hex-caller/install", &artifact).unwrap();
    let saved = store.load().unwrap();
    let registry = &saved.snapshot.program;
    let method = registry.entry("HexCaller::Run").unwrap();
    let mut vm = saved.snapshot.restore().unwrap();
    let input = vm
        .new_string("0Xffffffffffffffff".encode_utf16().collect())
        .unwrap();
    let args = vec![counter, Value::Reference(input)];
    let intent = vm
        .invoke_once(&mut store, "hex-caller/invoke", method, args.clone(), 0, 7)
        .unwrap();
    assert_eq!(
        intent.state,
        renraku_vm::invocation::InvocationState::Running
    );
    // Persist an actual foreign constructor, including its partially initialized
    // old-type result and the caller's portable newobj continuation.
    let mut saw_ctor = false;
    for _ in 0..5000 {
        let receipt = vm
            .invoke_once(&mut store, "hex-caller/invoke", method, args.clone(), 1, 1)
            .unwrap();
        assert_eq!(receipt.execution, intent.execution);
        if vm.context().frames.iter().any(|f| {
            let m = registry.method(f.method.method).unwrap();
            m.owner == r64 && m.name == ".ctor"
        }) {
            saw_ctor = true;
            break;
        }
        assert_eq!(
            receipt.state,
            renraku_vm::invocation::InvocationState::Running
        );
    }
    assert!(saw_ctor);
    vm.collect_garbage().unwrap();
    assert!(vm.heap().object(input).is_ok());
    store.save(&vm).unwrap();
    drop(store);
    for path in ["hex-library", "hex-caller"] {
        fs::remove_dir_all(directory.0.join(path)).unwrap();
    }
    for path in [hex_source, result_view, caller, caller_view] {
        fs::remove_file(path).unwrap();
    }
    let counter_arg = format!("object:{}", counter.referenced_object().unwrap());
    let input_arg = format!("object:{input}");
    let native = || -> Json {
        let output = Command::new(env!("CARGO_BIN_EXE_renraku-vm"))
            .args([
                "invoke-once",
                database.to_str().unwrap(),
                "hex-caller/invoke",
                &method.to_string(),
                "10000",
                &counter_arg,
                &input_arg,
            ])
            .env("PATH", "/no-hex-compiler-or-sdk")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice(&output.stdout).unwrap()
    };
    let first = native();
    assert_eq!(first["state"], "completed");
    assert_eq!(first["execution"], intent.execution.to_string());
    let mut store = CheckpointStore::open(&database).unwrap();
    let saved = store.load().unwrap();
    let registry = &saved.snapshot.program;
    let mut vm = saved.snapshot.restore().unwrap();
    let receipt = vm.invocation("hex-caller/invoke").unwrap().clone();
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
        "ffffffffffffffff/14"
    );
    let calls = registry
        .fields()
        .find(|f| f.owner == registry.find_type("HexCaller").unwrap() && f.name == "Calls")
        .unwrap()
        .id;
    let last = registry
        .fields()
        .find(|f| f.owner == registry.find_type("HexCaller").unwrap() && f.name == "Last")
        .unwrap()
        .id;
    let parsed = vm
        .heap()
        .object(vm.static_fields()[&last].referenced_object().unwrap())
        .unwrap();
    assert_eq!(parsed.type_id(), r64);
    let value_field = registry
        .fields()
        .find(|f| f.owner == r64 && f.name == "Value")
        .unwrap()
        .id;
    assert_eq!(parsed.fields()[&value_field], Value::I64(-1));
    vm.start(common::entry(registry, "Arithmetic"), vec![])
        .unwrap();
    vm.run(1000).unwrap();
    vm.collect_garbage().unwrap();
    store.save(&vm).unwrap();
    drop(store);
    let replay = native();
    for key in [
        "state",
        "execution",
        "result",
        "methodId",
        "codeVersion",
        "requestId",
    ] {
        assert_eq!(first[key], replay[key]);
    }
    let mut store = CheckpointStore::open(&database).unwrap();
    let saved = store.load().unwrap();
    let vm = saved.snapshot.restore().unwrap();
    assert_eq!(vm.static_fields()[&root], counter);
    assert_eq!(vm.static_fields()[&calls], Value::I32(1));
    assert_eq!(
        vm.heap()
            .object(counter.referenced_object().unwrap())
            .unwrap()
            .fields()[&counter_value],
        Value::I32(20)
    );
    assert_eq!(vm.invocation("hex-caller/invoke").unwrap(), &receipt);
    assert_eq!(vm.invocation("decimal-before-hex").unwrap(), &decimal);
    let source = vm.read_code(&method.to_string()).unwrap();
    assert_eq!(source["sourceAvailable"], true);
    assert_eq!(source["sources"][0]["text"], caller_source);
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
        "public static class Extra { public static object Run(int value) { return value; } }",
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
        "public static class App { public static object Run(int value) { return value; } }",
    );
    let (output, report) = directory.compile(&[source], "bundle");
    assert_success(&output, &report);
    let guest = fs::read(directory.0.join("bundle/Compiled.Example.dll")).unwrap();
    assert!(
        Program::load(&[&fixtures().core, &guest])
            .unwrap_err()
            .0
            .contains("unsupported CIL opcode 8c")
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

#[test]
fn frozen_reference_cast_caller_keeps_saved_counter_identity_provenance_and_sdk_free_receipts() {
    use renraku_vm::{cil::Op, execution::Continuation, invocation::InvocationState};
    let directory = Directory::new();
    let program = common::program();
    let counter_type = program.find_type("Renraku.Guest.Counter").unwrap();
    let root = program
        .fields()
        .find(|f| {
            f.owner == program.find_type("Renraku.Guest.Simulation").unwrap() && f.name == "Saved"
        })
        .unwrap()
        .id;
    let value = program
        .fields()
        .find(|f| f.owner == counter_type && f.name == "Value")
        .unwrap()
        .id;
    let view = program
        .reference_set(&[counter_type])
        .unwrap()
        .to_json()
        .unwrap();
    let view_path = directory.0.join("counter.view.json");
    fs::write(&view_path, &view).unwrap();
    let database = directory.0.join("reference-cast.db");
    let mut store = CheckpointStore::open(&database).unwrap();
    let mut vm = Vm::new(&program);
    vm.start(common::entry(&program, "SaveObject"), vec![])
        .unwrap();
    vm.run(1000).unwrap();
    let counter = vm.static_fields()[&root];
    store.save(&vm).unwrap();
    let source_text = "public static class CastCaller { public static int Calls; public static Renraku.Guest.Counter Last; public static Renraku.Guest.Counter Cast(object value) { return (Renraku.Guest.Counter)value; } public static int Run(object value, int delta) { Renraku.Guest.Counter same=value as Renraku.Guest.Counter; if(same==null)return -1; Renraku.Guest.Counter exact=Cast(value); if(same!=exact)return -2; Last=exact; Calls=Calls+1; exact.Add(delta); return exact.Value; } }";
    let source = directory.source("CastCaller.cs", source_text);
    let (output, report) = directory.compile_with_views(
        std::slice::from_ref(&source),
        "cast-caller",
        std::slice::from_ref(&view_path),
    );
    assert_success(&output, &report);
    let artifact = BuildArtifact::from_bundle(directory.0.join("cast-caller")).unwrap();
    let installation = install_once(&mut store, "cast/install", &artifact).unwrap();
    assert_eq!(
        install_once(&mut store, "cast/install", &artifact).unwrap(),
        installation
    );
    let saved = store.load().unwrap();
    let registry = &saved.snapshot.program;
    let mut vm = saved.snapshot.restore().unwrap();
    assert_eq!(
        registry
            .reference_set(&[counter_type])
            .unwrap()
            .to_json()
            .unwrap(),
        view
    );
    for old in program.methods() {
        assert_eq!(
            registry.method(old.binding.method).unwrap().binding,
            old.binding
        );
    }
    for old in program.types() {
        let now = registry.types().find(|t| t.id == old.id).unwrap();
        assert_eq!(
            (&now.name, now.base, &now.fields),
            (&old.name, old.base, &old.fields)
        );
    }
    for old in program.fields() {
        let now = registry.fields().find(|f| f.id == old.id).unwrap();
        assert_eq!(
            (&now.name, now.owner, now.value_type),
            (&old.name, old.owner, old.value_type)
        );
    }
    let method = registry.entry("CastCaller::Run").unwrap();
    let helper = registry.entry("CastCaller::Cast").unwrap();
    let binding = registry.method(method).unwrap().binding;
    let code = vm.read_code(&helper.to_string()).unwrap();
    let cast = code["instructions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["target"]["kind"] == "type")
        .unwrap();
    assert_eq!(cast["target"]["id"], counter_type.to_string());
    let args = vec![counter, Value::I32(7)];
    let intent = vm
        .invoke_once(&mut store, "cast/invoke", method, args.clone(), 0, 1)
        .unwrap();
    let mut paused = false;
    for _ in 0..100 {
        let frame = vm.context().frames.last().unwrap();
        if let Continuation::Execute { next } = frame.continuation
            && frame.method.method == helper
            && matches!(
                registry.method(helper).unwrap().instructions[&next].op,
                Op::ReferenceCast { test: false, .. }
            )
        {
            assert_eq!(frame.evaluation_stack, vec![counter]);
            paused = true;
            break;
        }
        vm.invoke_once(&mut store, "cast/invoke", method, args.clone(), 1, 1)
            .unwrap();
    }
    assert!(paused);
    vm.collect_garbage().unwrap();
    store.save(&vm).unwrap();
    drop(store);
    fs::remove_file(source).unwrap();
    fs::remove_file(view_path).unwrap();
    fs::remove_dir_all(directory.0.join("cast-caller")).unwrap();
    let argument = format!("object:{}", counter.referenced_object().unwrap());
    let invoke = || -> Json {
        let output = Command::new(env!("CARGO_BIN_EXE_renraku-vm"))
            .args([
                "invoke-once",
                database.to_str().unwrap(),
                "cast/invoke",
                &method.to_string(),
                "1000",
                &argument,
                "i32:7",
            ])
            .env("PATH", "/no-cast-compiler-or-sdk")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice(&output.stdout).unwrap()
    };
    let first = invoke();
    assert_eq!(first["execution"], intent.execution.to_string());
    assert_eq!(first["result"]["value"], 26);
    let mut store = CheckpointStore::open(&database).unwrap();
    let saved = store.load().unwrap();
    let registry = &saved.snapshot.program;
    let mut vm = saved.snapshot.restore().unwrap();
    let owner = registry.find_type("CastCaller").unwrap();
    let calls = registry
        .fields()
        .find(|f| f.owner == owner && f.name == "Calls")
        .unwrap()
        .id;
    let last = registry
        .fields()
        .find(|f| f.owner == owner && f.name == "Last")
        .unwrap()
        .id;
    assert_eq!(vm.static_fields()[&last], counter);
    assert_eq!(vm.static_fields()[&calls], Value::I32(1));
    let receipt = vm.invocation("cast/invoke").unwrap().clone();
    let second = vm
        .invoke_once(
            &mut store,
            "cast/later",
            method,
            vec![counter, Value::I32(-3)],
            1000,
            7,
        )
        .unwrap();
    assert_eq!(
        second.state,
        InvocationState::Completed {
            result: Some(Value::I32(23))
        }
    );
    vm.start(common::entry(registry, "Arithmetic"), vec![])
        .unwrap();
    vm.run(1000).unwrap();
    vm.collect_garbage().unwrap();
    store.save(&vm).unwrap();
    drop(store);
    let replay = invoke();
    for key in ["execution", "state", "result", "methodId", "codeVersion"] {
        assert_eq!(first[key], replay[key]);
    }
    let mut store = CheckpointStore::open(&database).unwrap();
    let saved = store.load().unwrap();
    let registry = &saved.snapshot.program;
    let vm = saved.snapshot.restore().unwrap();
    assert_eq!(vm.invocation("cast/invoke").unwrap(), &receipt);
    assert_eq!(registry.method(method).unwrap().binding, binding);
    assert_eq!(
        registry
            .reference_set(&[counter_type])
            .unwrap()
            .to_json()
            .unwrap(),
        view
    );
    assert_eq!(vm.static_fields()[&root], counter);
    assert_eq!(vm.static_fields()[&last], counter);
    assert_eq!(vm.static_fields()[&calls], Value::I32(2));
    let object = vm
        .heap()
        .object(counter.referenced_object().unwrap())
        .unwrap();
    assert_eq!(object.type_id(), counter_type);
    assert_eq!(object.fields()[&value], Value::I32(23));
    assert_eq!(
        vm.heap()
            .objects()
            .filter(|o| o.type_id() == counter_type)
            .count(),
        1
    );
    let code = vm.read_code(&method.to_string()).unwrap();
    assert_eq!(code["sourceAvailable"], true);
    assert_eq!(code["sources"][0]["text"], source_text);
}

#[test]
fn scoped_bootstrap_compiles_only_endowed_declarations_and_seals_original_objects() {
    use renraku_vm::{
        bootstrap,
        endowment::{self, Endowment},
    };
    let directory = Directory::new();
    let dll = fs::read(fixtures().output.join("Renraku.ChildTests.dll")).unwrap();
    let p = Program::load(&[&fixtures().core, &dll]).unwrap();
    let mut vm = Vm::new(&p);
    vm.start(p.entry("Renraku.ChildTests.Child::Seed").unwrap(), vec![])
        .unwrap();
    vm.run(10000).unwrap();
    let node = p.find_type("Renraku.ChildTests.Node").unwrap();
    let field = p
        .fields()
        .find(|f| f.owner == node && f.name == "Value")
        .unwrap()
        .id;
    let shared_field = p.fields().find(|f| f.name == "Shared").unwrap().id;
    let shared = vm.static_fields()[&shared_field]
        .referenced_object()
        .unwrap();
    let mut e = Endowment::default();
    e.types
        .extend([p.find_type("System.Object").unwrap(), node]);
    e.read.insert(field);
    e.write.insert(field);
    e.objects.insert(shared);
    let mut store = CheckpointStore::open(directory.0.join("scoped.db")).unwrap();
    store.save(&vm).unwrap();
    let set = bootstrap::prepare(&mut store, &e).unwrap();
    set.validate(&p).unwrap();
    let json = set.to_json().unwrap();
    let text = String::from_utf8(json.clone()).unwrap();
    for name in [
        "PrivateParent",
        "Hidden",
        "Next",
        "Child::",
        "Seed",
        "Renraku.Model",
        "8675309",
        "parent-private-text",
    ] {
        assert!(!text.contains(name), "leaked {name}");
    }
    assert!(text.contains("Value") && text.contains("private extern @Node"));
    assert!(set.views.iter().all(|v| v.is_scoped()));
    let views = directory.0.join("scoped.json");
    fs::write(&views, json).unwrap();
    let source=directory.source("ScopedCaller.cs","public static class ScopedCaller { public static int Calls; public static int Run(Renraku.ChildTests.Node node,int delta) { Calls++; node.Value=node.Value+delta; return node.Value; } }");
    let (output, report) = directory.compile_named(
        &[source],
        "scoped-bundle",
        std::slice::from_ref(&views),
        "Scoped.Caller",
    );
    assert_success(&output, &report);
    let artifact = BuildArtifact::from_bundle(directory.0.join("scoped-bundle")).unwrap();
    artifact.validate().unwrap();
    let mut forged = set.views.clone();
    for view in &mut forged {
        let schema: Json = serde_json::from_str(&view.schema_json).unwrap();
        view.schema_json = view.schema_json.replace(
            schema["scope"]["approval"].as_str().unwrap(),
            &"0".repeat(64),
        );
        view.schema_hash = Sha256::digest(view.schema_json.as_bytes())
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
    }
    let forged = ReferenceSet::from_views(forged).unwrap();
    forged.validate(&p).unwrap();
    let fake_path = directory.0.join("fake-scope.json");
    fs::write(&fake_path, forged.to_json().unwrap()).unwrap();
    let fake_source=directory.source("Fake.cs","public static class Fake { public static int Run(Renraku.ChildTests.Node node) { return node.Value; } }");
    let (output, report) =
        directory.compile_named(&[fake_source], "fake-bundle", &[fake_path], "Fake.Caller");
    assert_success(&output, &report);
    let fake = BuildArtifact::from_bundle(directory.0.join("fake-bundle")).unwrap();
    assert!(
        install_once(&mut store, "fake/install", &fake)
            .unwrap_err()
            .to_string()
            .contains("outside trusted bootstrap approval")
    );
    let receipt = install_once(&mut store, "scoped-install", &artifact).unwrap();
    let saved = store.load().unwrap();
    let p = &saved.snapshot.program;
    assert_eq!(p.find_type("Renraku.ChildTests.Node").unwrap(), node);
    let caller = p.find_type("ScopedCaller").unwrap();
    e.types.insert(caller);
    let method = p
        .method(p.entry("ScopedCaller::Run").unwrap())
        .unwrap()
        .binding;
    e.methods.insert(method.code);
    let calls = p
        .fields()
        .find(|f| f.owner == caller && f.name == "Calls")
        .unwrap()
        .id;
    e.read.insert(calls);
    e.write.insert(calls);
    let mut widened = e.clone();
    widened.read.insert(
        p.fields()
            .find(|f| f.owner == node && f.name == "Next")
            .unwrap()
            .id,
    );
    assert!(
        endowment::seal(&mut store, &widened)
            .unwrap_err()
            .to_string()
            .contains("widens")
    );
    endowment::seal(&mut store, &e).unwrap();
    // Save and collect after every instruction, including a failed final save.
    endowment::invoke_once(
        &mut store,
        "scoped-first",
        method.code,
        vec![Value::Reference(shared), Value::I32(7)],
        0,
    )
    .unwrap();
    let mut cuts = 0;
    loop {
        let s = store.load().unwrap();
        let mut vm = s.snapshot.restore().unwrap();
        let previous = vm.context().clone();
        assert_eq!(vm.invocation("scoped-first").unwrap().method, method);
        assert_eq!(
            vm.invocation("scoped-first").unwrap().arguments[0],
            Value::Reference(shared)
        );
        let out = vm.run(1).unwrap();
        vm.collect_garbage().unwrap();
        cuts += 1;
        if matches!(out, RunOutcome::Completed(_)) {
            let db = rusqlite::Connection::open(directory.0.join("scoped.db")).unwrap();
            db.execute_batch("CREATE TRIGGER fail_scoped_terminal BEFORE UPDATE ON checkpoint BEGIN SELECT RAISE(ABORT,'scoped terminal failure'); END;").unwrap();
            assert!(store.save(&vm).is_err());
            db.execute_batch("DROP TRIGGER fail_scoped_terminal")
                .unwrap();
            assert_eq!(
                *store.load().unwrap().snapshot.restore().unwrap().context(),
                previous
            );
            break;
        }
        store.save(&vm).unwrap();
        assert!(cuts < 100);
    }
    assert!(cuts > 10);
    assert_eq!(
        endowment::invoke_once(
            &mut store,
            "scoped-first",
            method.code,
            vec![Value::Reference(shared), Value::I32(7)],
            1000
        )
        .unwrap()["result"]["value"],
        26
    );
    assert_eq!(
        endowment::invoke_once(
            &mut store,
            "scoped-second",
            method.code,
            vec![Value::Reference(shared), Value::I32(-3)],
            10000
        )
        .unwrap()["result"]["value"],
        23
    );
    assert_eq!(
        endowment::invoke_once(
            &mut store,
            "scoped-first",
            method.code,
            vec![Value::Reference(shared), Value::I32(7)],
            10000
        )
        .unwrap()["result"]["value"],
        26
    );
    let s = store.load().unwrap();
    let mut vm = s.snapshot.restore().unwrap();
    vm.collect_garbage().unwrap();
    store.save(&vm).unwrap();
    assert_eq!(
        vm.heap().object(shared).unwrap().fields()[&field],
        Value::I32(23)
    );
    assert_eq!(vm.static_fields()[&calls], Value::I32(2));
    assert_eq!(
        vm.installation("scoped-install").unwrap().assembly,
        receipt.assembly
    );
    assert!(
        vm.read_code("ScopedCaller::Run").unwrap()["sourceAvailable"]
            .as_bool()
            .unwrap()
    );
    // C# visibility is useful, but runtime admission remains the authority.
    for (i, body) in [
        "return node.Next;",
        "return Renraku.ChildTests.PrivateParent.Number;",
        "return new Renraku.ChildTests.Node(1);",
        "return Renraku.Model.Complete(null);",
    ]
    .iter()
    .enumerate()
    {
        let path=directory.source(&format!("Denied{i}.cs"),&format!("public static class Denied {{ public static object Run(Renraku.ChildTests.Node node) {{ {body} }} }}"));
        let (output, report) = directory.compile_named(
            &[path],
            &format!("denied-{i}"),
            std::slice::from_ref(&views),
            "Denied.Caller",
        );
        assert!(!output.status.success(), "{report}");
    }
    // A real artifact carrying an authentic-looking scope has no authority in
    // another universe, even if module images and captured source are identical.
    let mut other = CheckpointStore::open(directory.0.join("other.db")).unwrap();
    other.save(&Vm::new(&p_from_images(&dll))).unwrap();
    assert!(install_once(&mut other, "forged", &artifact).is_err());
    fs::remove_dir_all(directory.0.join("scoped-bundle")).unwrap();
    fs::remove_file(views).unwrap();
    assert_eq!(
        install_once(&mut store, "scoped-install", &artifact)
            .unwrap()
            .assembly,
        receipt.assembly
    );
}
fn p_from_images(dll: &[u8]) -> Program {
    Program::load(&[&fixtures().core, dll]).unwrap()
}

struct ScopedServer(std::process::Child);
impl Drop for ScopedServer {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
#[test]
fn scoped_captured_csharp_bootstrap_uses_original_remote_counter_and_recovers_sdk_free() {
    use renraku_vm::{
        bootstrap,
        compiler::{CompilationRequest, CompilationStore, CompilerToolchain},
        endowment::{self, Endowment},
        execution::ExecutionStatus,
        identity::{GrantId, NodeId},
        peer::{Endpoint, PeerKey},
        placement::{self, PlacementPolicy},
        remote,
    };
    let d = Directory::new();
    let dll = fs::read(fixtures().output.join("Renraku.ChildTests.dll")).unwrap();
    let p = p_from_images(&dll);
    let mut vm = Vm::new(&p);
    vm.start(p.entry("Renraku.ChildTests.Child::Seed").unwrap(), vec![])
        .unwrap();
    vm.run(10000).unwrap();
    let principal = vm.universe_id();
    let shared = vm.static_fields()[&p.fields().find(|f| f.name == "Shared").unwrap().id]
        .referenced_object()
        .unwrap();
    let db = d.0.join("child.db");
    let mut child = CheckpointStore::open(&db).unwrap();
    child.save(&vm).unwrap();
    let op = common::program();
    let mut ovm = Vm::new(&op);
    ovm.start(
        op.entry("Renraku.Guest.Simulation::SaveObject").unwrap(),
        vec![],
    )
    .unwrap();
    ovm.run(1000).unwrap();
    let object = ovm.static_fields()[&op
        .fields()
        .find(|f| f.owner == op.find_type("Renraku.Guest.Simulation").unwrap() && f.name == "Saved")
        .unwrap()
        .id]
        .referenced_object()
        .unwrap();
    let owner_id = ovm.universe_id();
    let ownerdb = d.0.join("owner.db");
    let mut owner = CheckpointStore::open(&ownerdb).unwrap();
    owner.save(&ovm).unwrap();
    let node = NodeId::fresh();
    placement::initialize(&mut owner, node, 1).unwrap();
    placement::register_object(&mut owner, object, PlacementPolicy::pinned(node)).unwrap();
    let owner_method = op
        .method(op.entry("Renraku.Guest.Counter::Add").unwrap())
        .unwrap()
        .binding;
    let grant = GrantId::fresh();
    placement::export_method(&mut owner, grant, principal, object, owner_method).unwrap();
    let key = d.0.join("key");
    PeerKey::from_bytes([0x63; 32]).write_new(&key).unwrap();
    let mut server = ScopedServer(
        Command::new(env!("CARGO_BIN_EXE_renraku-checkpoint-probe"))
            .args([
                "peer-server",
                ownerdb.to_str().unwrap(),
                &node.to_string(),
                &principal.to_string(),
                key.to_str().unwrap(),
                "127.0.0.1:0",
                "1000",
                "1",
                "-",
            ])
            .env("PATH", "/no-scoped-sdk")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap(),
    );
    let mut line = String::new();
    BufReader::new(server.0.stdout.take().unwrap())
        .read_line(&mut line)
        .unwrap();
    let address = serde_json::from_str::<Json>(&line).unwrap()["listening"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    let contract = p
        .method(p.entry("Renraku.ChildTests.CounterPort::Add").unwrap())
        .unwrap()
        .binding;
    let handle = remote::import_method(
        &mut owner,
        &mut child,
        &Endpoint {
            owner: owner_id,
            node,
            epoch: 1,
            address,
        },
        grant,
        contract,
    )
    .unwrap();
    let mut e = Endowment::default();
    for name in [
        "System.Object",
        "Renraku.ChildTests.Node",
        "Renraku.ChildTests.CounterPort",
    ] {
        e.types.insert(p.find_type(name).unwrap());
    }
    e.methods.insert(contract.code);
    e.objects.extend([shared, handle]);
    let value = p
        .fields()
        .find(|f| f.owner == p.find_type("Renraku.ChildTests.Node").unwrap() && f.name == "Value")
        .unwrap()
        .id;
    e.read.insert(value);
    e.write.insert(value);
    let set = bootstrap::prepare(&mut child, &e).unwrap();
    let saved = child.load().unwrap();
    let vm = saved.snapshot.restore().unwrap();
    let toolchain = CompilerToolchain::open("/usr/local/share/dotnet/dotnet", compiler()).unwrap();
    let source=b"public static class ScopedRemoteCaller { public static int Calls; public static int Run(Renraku.ChildTests.Node node,Renraku.ChildTests.CounterPort counter,int delta) { Calls++; node.Value++; return counter.Add(delta)+node.Value; } }".to_vec();
    let request = CompilationRequest::capture(
        &saved.snapshot.program,
        vm.universe_id(),
        &toolchain,
        "Scoped.RemoteCaller".into(),
        vec![("Caller.cs".into(), source.clone())],
        set.views.clone(),
    )
    .unwrap();
    let ledger = d.0.join("compiler.db");
    let mut service = CompilationStore::open(&ledger).unwrap();
    service.prepare("bootstrap", &request).unwrap();
    let result = service.drive(principal, "bootstrap", &toolchain).unwrap();
    assert!(
        result.report.as_ref().unwrap()["success"]
            .as_bool()
            .unwrap(),
        "{:?}",
        result.report
    );
    let artifact = result.artifact.unwrap();
    let installed = install_once(&mut child, "bootstrap/install", &artifact).unwrap();
    let s = child.load().unwrap();
    let cp = &s.snapshot.program;
    let caller = cp.find_type("ScopedRemoteCaller").unwrap();
    e.types.insert(caller);
    let code = cp
        .method(cp.entry("ScopedRemoteCaller::Run").unwrap())
        .unwrap()
        .binding
        .code;
    e.methods.insert(code);
    let calls = cp
        .fields()
        .find(|f| f.owner == caller && f.name == "Calls")
        .unwrap()
        .id;
    e.read.insert(calls);
    e.write.insert(calls);
    endowment::seal(&mut child, &e).unwrap();
    let tool_request = d.0.join("invoke.json");
    fs::write(&tool_request,serde_json::to_vec(&serde_json::json!({"action":"invoke","operation":"bootstrap/run","code":code.to_string(),"budget":1000,"arguments":[{"kind":"object","value":shared.to_string()},{"kind":"object","value":handle.to_string()},{"kind":"i32","value":7}]})).unwrap()).unwrap();
    let invoke = || {
        let output = Command::new(env!("CARGO_BIN_EXE_renraku-vm"))
            .args([
                "child-tool",
                db.to_str().unwrap(),
                tool_request.to_str().unwrap(),
            ])
            .env("PATH", "/no-scoped-sdk")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice::<Json>(&output.stdout).unwrap()
    };
    let first = invoke();
    assert_eq!(first["waitingRemote"], true);
    let s = child.load().unwrap();
    let mut vm = s.snapshot.restore().unwrap();
    assert!(matches!(
        vm.context().status,
        ExecutionStatus::WaitingRemote { .. }
    ));
    assert_eq!(
        vm.heap().object(shared).unwrap().fields()[&value],
        Value::I32(20)
    );
    vm.collect_garbage().unwrap();
    child.save(&vm).unwrap();
    // Compiler terminal records and bootstrap artifacts are no longer required by
    // either native process. The caller has only a handle, never Counter state.
    drop(service);
    fs::remove_file(&ledger).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_renraku-vm"))
        .args(["remote-drive", db.to_str().unwrap(), key.to_str().unwrap()])
        .env("PATH", "/no-scoped-sdk")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(server.0.wait().unwrap().success());
    fs::remove_file(key).unwrap();
    let terminal = invoke();
    assert_eq!(terminal["result"]["value"], 46);
    assert_eq!(invoke(), terminal);
    let s = child.load().unwrap();
    let vm = s.snapshot.restore().unwrap();
    assert_eq!(vm.static_fields()[&calls], Value::I32(1));
    assert_eq!(
        vm.heap().object(shared).unwrap().fields()[&value],
        Value::I32(20)
    );
    assert_eq!(
        vm.installation("bootstrap/install").unwrap().assembly,
        installed.assembly
    );
    assert!(
        s.snapshot
            .program
            .find_type("Renraku.Guest.Counter")
            .is_err()
    );
    assert_eq!(
        bootstrap::references(&mut child)
            .unwrap()
            .to_json()
            .unwrap(),
        set.to_json().unwrap()
    );
    let retained = vm.read_code("ScopedRemoteCaller::Run").unwrap();
    assert!(
        retained
            .to_string()
            .contains(std::str::from_utf8(&source).unwrap())
    );
    let s = owner.load().unwrap();
    let vm = s.snapshot.restore().unwrap();
    let value = op
        .fields()
        .find(|f| f.owner == op.find_type("Renraku.Guest.Counter").unwrap() && f.name == "Value")
        .unwrap()
        .id;
    assert_eq!(
        vm.heap().object(object).unwrap().fields()[&value],
        Value::I32(26)
    );
}

#[test]
fn scoped_bootstrap_approval_fails_closed_on_save_scope_rebinding_and_corruption() {
    use renraku_vm::{
        bootstrap,
        endowment::Endowment,
        identity::{CodeVersionId, FieldId},
    };
    let d = Directory::new();
    let dll = fs::read(fixtures().output.join("Renraku.ChildTests.dll")).unwrap();
    let p = p_from_images(&dll);
    let mut store = CheckpointStore::open(d.0.join("scope.db")).unwrap();
    store.save(&Vm::new(&p)).unwrap();
    let mut e = Endowment::default();
    e.types.extend([
        p.find_type("System.Object").unwrap(),
        p.find_type("Renraku.ChildTests.Node").unwrap(),
    ]);
    let field = p
        .fields()
        .find(|f| f.owner == p.find_type("Renraku.ChildTests.Node").unwrap() && f.name == "Value")
        .unwrap()
        .id;
    e.read.insert(field);
    let db = rusqlite::Connection::open(d.0.join("scope.db")).unwrap();
    db.execute_batch("CREATE TRIGGER fail_bootstrap BEFORE UPDATE ON checkpoint BEGIN SELECT RAISE(ABORT,'bootstrap failure'); END;").unwrap();
    assert!(bootstrap::prepare(&mut store, &e).is_err());
    db.execute_batch("DROP TRIGGER fail_bootstrap").unwrap();
    assert!(
        !store
            .load()
            .unwrap()
            .snapshot
            .restore()
            .unwrap()
            .heap()
            .roots()
            .keys()
            .any(|n| n.starts_with("renraku.bootstrap-v1/"))
    );
    let mut bad = e.clone();
    bad.methods.insert(CodeVersionId::fresh());
    assert!(bootstrap::prepare(&mut store, &bad).is_err());
    bad = e.clone();
    bad.read.insert(FieldId::fresh());
    assert!(bootstrap::prepare(&mut store, &bad).is_err());
    let set = bootstrap::prepare(&mut store, &e).unwrap();
    assert_eq!(
        bootstrap::prepare(&mut store, &e)
            .unwrap()
            .to_json()
            .unwrap(),
        set.to_json().unwrap()
    );
    bad = e.clone();
    bad.write.insert(field);
    assert!(bootstrap::prepare(&mut store, &bad).is_err());
    assert!(
        ReferenceSet::from_views(vec![
            set.views
                .iter()
                .find(|v| !v.schema_json.contains("\"moduleName\":\"Renraku.CoreLib\""))
                .unwrap()
                .clone()
        ])
        .is_err()
    );
    let mut changed = set.views.clone();
    let mut envelope: Json = serde_json::from_slice(&changed[0].to_json().unwrap()).unwrap();
    let mut schema: Json = serde_json::from_str(envelope["schemaJson"].as_str().unwrap()).unwrap();
    schema["scope"]["approval"] = Json::String("0".repeat(64));
    let text = serde_json::to_string(&schema).unwrap();
    envelope["schemaHash"] = Json::String(
        Sha256::digest(text.as_bytes())
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect(),
    );
    envelope["schemaJson"] = Json::String(text);
    changed[0] = ReferenceView::from_json(&serde_json::to_vec(&envelope).unwrap()).unwrap();
    assert!(ReferenceSet::from_views(changed).is_err());
    let s = store.load().unwrap();
    let mut executed = s.snapshot.restore().unwrap();
    executed
        .start(
            s.snapshot
                .program
                .entry("Renraku.ChildTests.Child::RawCall")
                .unwrap(),
            vec![Value::I32(1)],
        )
        .unwrap();
    executed.run(1000).unwrap();
    store.save(&executed).unwrap();
    assert!(
        renraku_vm::endowment::seal(&mut store, &e)
            .unwrap_err()
            .to_string()
            .contains("executed before sealing")
    );
    let s = store.load().unwrap();
    let vm = s.snapshot.restore().unwrap();
    let id = vm.heap().roots()["renraku.bootstrap-v1/approval"];
    let text = String::from_utf16(vm.heap().object(id).unwrap().string_units().unwrap()).unwrap();
    let changed = text.replacen("\"format\":1", "\"format\":2", 1);
    let mut badvm = s.snapshot.restore().unwrap();
    let replacement = badvm.new_string(changed.encode_utf16().collect()).unwrap();
    badvm
        .set_root("renraku.bootstrap-v1/approval", replacement)
        .unwrap();
    assert!(store.save(&badvm).is_err());
    let mut payload: Vec<u8> = db
        .query_row("SELECT payload FROM checkpoint", [], |r| r.get(0))
        .unwrap();
    let old: Vec<_> = text.encode_utf16().flat_map(u16::to_le_bytes).collect();
    let new: Vec<_> = changed.encode_utf16().flat_map(u16::to_le_bytes).collect();
    let offset = payload.windows(old.len()).position(|b| b == old).unwrap();
    payload[offset..offset + old.len()].copy_from_slice(&new);
    db.execute(
        "UPDATE checkpoint SET payload=?1,digest=?2",
        rusqlite::params![payload, Sha256::digest(&payload).as_slice()],
    )
    .unwrap();
    assert!(store.load().is_err());
}
#[test]
fn scoped_views_preserve_original_unsigned_readonly_fields_and_hide_other_result_types() {
    use renraku_vm::{
        bootstrap,
        endowment::{self, Endowment},
    };
    let d = Directory::new();
    let dll = fs::read(fixtures().output.join("Renraku.UnsignedText.dll")).unwrap();
    let p = Program::load(&[&fixtures().core, &dll]).unwrap();
    let mut vm = Vm::new(&p);
    let text = vm
        .new_string("4294967295".encode_utf16().collect())
        .unwrap();
    vm.start(
        p.entry("Renraku.Text.UnsignedIntegerText::ParseUInt32")
            .unwrap(),
        vec![Value::Reference(text)],
    )
    .unwrap();
    let RunOutcome::Completed(Some(Value::Reference(object))) = vm.run(150000).unwrap() else {
        panic!("parser did not finish")
    };
    let ty = p.find_type("Renraku.Text.UInt32ParseResult").unwrap();
    let field = p
        .fields()
        .find(|f| f.owner == ty && f.name == "Value")
        .unwrap()
        .id;
    let mut e = Endowment::default();
    e.types.extend([p.find_type("System.Object").unwrap(), ty]);
    e.read.insert(field);
    e.objects.insert(object);
    let mut store = CheckpointStore::open(d.0.join("unsigned.db")).unwrap();
    store.save(&vm).unwrap();
    let set = bootstrap::prepare(&mut store, &e).unwrap();
    let exported = String::from_utf8(set.to_json().unwrap()).unwrap();
    assert!(exported.contains("public readonly uint @Value;"));
    assert!(
        !exported.contains("UInt64ParseResult")
            && !exported.contains("ParseUInt32")
            && !exported.contains("@Success")
            && !exported.contains("@Error")
    );
    let views = d.0.join("scope.json");
    fs::write(&views, set.to_json().unwrap()).unwrap();
    let source=d.source("UnsignedCaller.cs","public static class UnsignedCaller { public static ulong Run(Renraku.Text.UInt32ParseResult item) { return item.Value; } }");
    let (output, report) = d.compile_named(
        &[source],
        "unsigned-bundle",
        std::slice::from_ref(&views),
        "Scoped.UnsignedCaller",
    );
    assert_success(&output, &report);
    let artifact = BuildArtifact::from_bundle(d.0.join("unsigned-bundle")).unwrap();
    install_once(&mut store, "unsigned/install", &artifact).unwrap();
    let s = store.load().unwrap();
    let p = &s.snapshot.program;
    let code = p
        .method(p.entry("UnsignedCaller::Run").unwrap())
        .unwrap()
        .binding
        .code;
    e.types.insert(p.find_type("UnsignedCaller").unwrap());
    e.methods.insert(code);
    endowment::seal(&mut store, &e).unwrap();
    assert_eq!(
        endowment::invoke_once(
            &mut store,
            "unsigned/run",
            code,
            vec![Value::Reference(object)],
            10000
        )
        .unwrap()["result"]["value"],
        "4294967295"
    );
    let source=d.source("WriteReadonly.cs","public static class Denied { public static void Run(Renraku.Text.UInt32ParseResult item) { item.Value=0; } }");
    let (output, report) =
        d.compile_named(&[source], "write-readonly", &[views], "Denied.Readonly");
    assert!(!output.status.success(), "{report}");
}

#[test]
fn forged_captured_private_field_tokens_cannot_escape_scoped_bootstrap_admission() {
    use renraku_vm::{
        bootstrap,
        endowment::{self, Endowment},
    };
    let d = Directory::new();
    let dll = fs::read(fixtures().output.join("Renraku.ChildTests.dll")).unwrap();
    let p = p_from_images(&dll);
    let mut vm = Vm::new(&p);
    vm.start(p.entry("Renraku.ChildTests.Child::Seed").unwrap(), vec![])
        .unwrap();
    vm.run(10000).unwrap();
    let mut store = CheckpointStore::open(d.0.join("forged.db")).unwrap();
    store.save(&vm).unwrap();
    let mut e = Endowment::default();
    e.types.extend([
        p.find_type("System.Object").unwrap(),
        p.find_type("Renraku.ChildTests.Node").unwrap(),
    ]);
    let field = p
        .fields()
        .find(|f| f.owner == p.find_type("Renraku.ChildTests.Node").unwrap() && f.name == "Value")
        .unwrap()
        .id;
    e.read.insert(field);
    let scope = bootstrap::prepare(&mut store, &e).unwrap();
    let filtered = d.0.join("scope.json");
    fs::write(&filtered, scope.to_json().unwrap()).unwrap();
    let source=d.source("Good.cs","public static class Good { public static int Run(Renraku.ChildTests.Node node) { return node.Value; } }");
    let (output, report) = d.compile_named(&[source], "good", &[filtered], "Good.Scoped");
    assert_success(&output, &report);
    // This test alone uses the trusted full view to generate hostile metadata;
    // the child-facing workflow never supplies this view or private source.
    let full = d.0.join("full.json");
    fs::write(
        &full,
        p.reference_view(&[p.find_type("Renraku.ChildTests.PrivateParent").unwrap()])
            .unwrap()
            .to_json()
            .unwrap(),
    )
    .unwrap();
    let source=d.source("Attack.cs","public static class Attack { public static int Run() { return Renraku.ChildTests.PrivateParent.Number; } }");
    let (output, report) = d.compile_named(&[source], "attack", &[full], "Attack.Scoped");
    assert_success(&output, &report);
    // Forge the captured inputs to claim the subset facades. Hashes and names
    // remain internally valid. Neither facade possession nor compiler visibility
    // may replace the final admission check over actual executable CIL.
    let mut manifest: Json =
        serde_json::from_slice(&fs::read(d.0.join("attack/renraku-build.json")).unwrap()).unwrap();
    let good: Json =
        serde_json::from_slice(&fs::read(d.0.join("good/renraku-build.json")).unwrap()).unwrap();
    manifest["references"] = good["references"].clone();
    fs::remove_dir_all(d.0.join("attack/references")).unwrap();
    fs::create_dir(d.0.join("attack/references")).unwrap();
    for reference in good["references"].as_array().unwrap() {
        for kind in ["view", "image"] {
            let path = reference[kind]["file"].as_str().unwrap();
            fs::copy(d.0.join("good").join(path), d.0.join("attack").join(path)).unwrap();
        }
    }
    fs::write(
        d.0.join("attack/renraku-build.json"),
        serde_json::to_vec(&manifest).unwrap(),
    )
    .unwrap();
    let artifact = BuildArtifact::from_bundle(d.0.join("attack")).unwrap();
    install_once(&mut store, "attack/install", &artifact).unwrap();
    let s = store.load().unwrap();
    let p = &s.snapshot.program;
    e.types.insert(p.find_type("Attack").unwrap());
    let method = p.entry("Attack::Run").unwrap();
    e.methods.insert(p.method(method).unwrap().binding.code);
    let mut raw = s.snapshot.restore().unwrap();
    raw.start(method, vec![]).unwrap();
    assert_eq!(
        raw.run(1000).unwrap(),
        RunOutcome::Completed(Some(Value::I32(8675309)))
    );
    assert!(
        endowment::seal(&mut store, &e)
            .unwrap_err()
            .to_string()
            .contains("child authority denied")
    );
    assert!(
        !store
            .load()
            .unwrap()
            .snapshot
            .restore()
            .unwrap()
            .heap()
            .roots()
            .contains_key("renraku.child-v1/policy")
    );
}
