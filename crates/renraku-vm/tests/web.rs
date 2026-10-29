#[allow(dead_code)]
mod common;
use renraku_vm::{
    artifact::install_once,
    checkpoint::CheckpointStore,
    compiler::{CompilationRequest, CompilationStore, CompilerToolchain},
    error::Error,
    invocation::InvocationState,
    runtime::{RunOutcome, Vm},
    value::Value,
    web::{
        WebHost, WebRequest, WebServer, WebStage, inspect_publications, publish_handler,
        publish_handler_with_observer,
    },
};
use rusqlite::{Connection, params};
use std::{
    fs,
    io::{Read, Write},
    net::{Shutdown, SocketAddr, TcpStream},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::OnceLock,
    thread,
    time::{Duration, Instant},
};

const HANDLER: &str = "Renraku.Guest.Website::Handle";
struct Test {
    directory: PathBuf,
    database: PathBuf,
}

fn toolchain() -> &'static CompilerToolchain {
    static TOOLCHAIN: OnceLock<CompilerToolchain> = OnceLock::new();
    TOOLCHAIN.get_or_init(|| {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .canonicalize()
            .unwrap();
        let output = root
            .join("target")
            .join(format!("web-compiler-tests-{}", std::process::id()));
        let built = Command::new("dotnet")
            .args(["build", "--nologo", "--verbosity", "quiet", "--output"])
            .arg(&output)
            .arg(root.join("tools/Compiler"))
            .output()
            .unwrap();
        assert!(
            built.status.success(),
            "{}",
            String::from_utf8_lossy(&built.stdout)
        );
        let dotnet = std::env::split_paths(&std::env::var_os("PATH").unwrap())
            .map(|path| path.join("dotnet"))
            .find(|path| path.is_file())
            .unwrap()
            .canonicalize()
            .unwrap();
        CompilerToolchain::open(dotnet, output.join("Renraku.Compiler.dll")).unwrap()
    })
}
fn web_cli(args: &[&str]) -> serde_json::Value {
    let result = Command::new(env!("CARGO_BIN_EXE_renraku-vm"))
        .args(args)
        .env("PATH", "/no-website-tools")
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    serde_json::from_slice(&result.stdout).unwrap()
}

#[test]
fn object_map_page_and_frozen_caller_keep_the_original_counter_across_sdk_free_recovery() {
    use renraku_vm::{cil::Op, execution::Continuation, identity::ObjectId, program::Program};
    fn field(vm: &Vm<'_>, p: &Program, id: ObjectId, name: &str) -> Value {
        let object = vm.heap().object(id).unwrap();
        object.fields()[&p
            .fields()
            .find(|f| f.owner == object.type_id() && f.name == name)
            .unwrap()
            .id]
    }
    let test = Test::new();
    let mut store = CheckpointStore::open(&test.database).unwrap();
    let original = store.load().unwrap();
    let p = &original.snapshot.program;
    let counter_type = p.find_type("Renraku.Guest.Counter").unwrap();
    let root = p
        .fields()
        .find(|f| f.owner == p.find_type("Renraku.Guest.Simulation").unwrap() && f.name == "Saved")
        .unwrap()
        .id;
    let mut vm = original.snapshot.restore().unwrap();
    vm.start(common::entry(p, "SaveObject"), vec![]).unwrap();
    assert_eq!(
        vm.run(1000).unwrap(),
        RunOutcome::Completed(Some(Value::I32(19)))
    );
    let counter = vm.static_fields()[&root].referenced_object().unwrap();
    store.save(&vm).unwrap();
    let baseline = test
        .host()
        .process(&request("before-objects"), 10000)
        .unwrap();
    let compiler_path = test.directory.join("objects-compiler.db");
    let mut compiler = CompilationStore::open(&compiler_path).unwrap();
    let universe = test.host().universe();
    let mut installations = Vec::new();
    for (id, name, file, source) in [
        (
            "text",
            "Renraku.Library",
            "IntegerText.cs",
            include_bytes!("../../../guest/IntegerText.cs").as_slice(),
        ),
        (
            "objects",
            "Renraku.ObjectCollections",
            "ObjectMap.cs",
            include_bytes!("../../../guest/ObjectMap.cs").as_slice(),
        ),
    ] {
        let saved = store.load().unwrap();
        let input = CompilationRequest::capture(
            &saved.snapshot.program,
            universe,
            toolchain(),
            name.into(),
            vec![(file.into(), source.to_vec())],
            vec![],
        )
        .unwrap();
        compiler.prepare(id, &input).unwrap();
        let built = compiler.drive(universe, id, toolchain()).unwrap();
        assert!(built.artifact.is_some(), "{:?}", built.report);
        let artifact = built.artifact.unwrap();
        let receipt = install_once(&mut store, id, &artifact).unwrap();
        installations.push((id, artifact, receipt));
    }
    let saved = store.load().unwrap();
    let p = &saved.snapshot.program;
    let selected = [
        "Renraku.Guest.Counter",
        "Renraku.Guest.Website",
        "Renraku.Collections.ObjectMap",
        "Renraku.Text.IntegerText",
    ]
    .map(|name| p.find_type(name).unwrap());
    let views = p.reference_set(&selected).unwrap();
    assert_eq!(views.views.len(), 3);
    let input = CompilationRequest::capture(
        p,
        universe,
        toolchain(),
        "Generated.ObjectWebsite".into(),
        vec![(
            "ObjectWebsite.cs".into(),
            include_bytes!("../../../guest/examples/ObjectWebsite.cs").to_vec(),
        )],
        views.views,
    )
    .unwrap();
    compiler.prepare("page", &input).unwrap();
    let built = compiler.drive(universe, "page", toolchain()).unwrap();
    assert!(built.artifact.is_some(), "{:?}", built.report);
    let page = built.artifact.unwrap();
    let page_receipt = install_once(&mut store, "page", &page).unwrap();
    let saved = store.load().unwrap();
    let p = &saved.snapshot.program;
    let mut vm = saved.snapshot.restore().unwrap();
    vm.invoke_once(
        &mut store,
        "page-init",
        p.entry("ObjectWebsite::Initialize").unwrap(),
        vec![Value::Reference(counter)],
        10000,
        100,
    )
    .unwrap();
    let owner = p.find_type("ObjectWebsite").unwrap();
    let state_field = p
        .fields()
        .find(|f| f.owner == owner && f.name == "State")
        .unwrap()
        .id;
    let map = vm.static_fields()[&state_field]
        .referenced_object()
        .unwrap();
    let map_type = vm.heap().object(map).unwrap().type_id();
    let counter_node = field(&vm, p, map, "head").referenced_object().unwrap();
    let counter_key = field(&vm, p, counter_node, "Key");
    assert_eq!(
        field(&vm, p, counter_node, "Value"),
        Value::Reference(counter)
    );
    publish_handler(&mut store, "object-page", "ObjectWebsite::Handle").unwrap();
    let host = WebHost::open_published(&test.database).unwrap();
    let mut first_request = request("object-one");
    first_request.body = "7".into();
    let intent = host.process(&first_request, 0).unwrap();
    assert_eq!(intent.status, 202);
    let mut cut = false;
    for _ in 0..2000 {
        collect(&test.database);
        let saved = store.load().unwrap();
        let p = &saved.snapshot.program;
        let vm = saved.snapshot.restore().unwrap();
        let head = field(&vm, p, map, "head").referenced_object().unwrap();
        if head != counter_node {
            assert_eq!(field(&vm, p, map, "count"), Value::I32(1));
            assert_eq!(field(&vm, p, head, "Next"), Value::Reference(counter_node));
            assert_eq!(field(&vm, p, counter, "Value"), Value::I32(19));
            cut = true;
            break;
        }
        let partial = host.process(&first_request, 1).unwrap();
        assert_eq!(partial.status, 202);
        assert_eq!(partial.execution, intent.execution);
    }
    assert!(
        cut,
        "pause after linking last-body, before Count/counter/visit changes"
    );
    assert_eq!(state(&test.database).0, 1);
    assert_eq!(
        host.process(&request("excluded-object-request"), 10000)
            .unwrap_err()
            .status,
        503
    );
    // Freeze the complete page dependency set and compile a separate caller.
    // Installation runs no guest instructions and retains the HTTP continuation.
    let saved = store.load().unwrap();
    let p = &saved.snapshot.program;
    let set = p.reference_set(&[owner]).unwrap();
    assert_eq!(set.views.len(), 3);
    let set_json = set.to_json().unwrap();
    let source = "public static class ObjectCaller { public static int Calls; public static Renraku.Guest.Counter Last; public static int Run(int delta) { object value=ObjectWebsite.State.Get(\"counter\").Value; Renraku.Guest.Counter same=value as Renraku.Guest.Counter; if(same==null)return -1; Last=(Renraku.Guest.Counter)value; Calls=Calls+1; return same.Add(delta); } }";
    let input = CompilationRequest::capture(
        p,
        universe,
        toolchain(),
        "Generated.ObjectCaller".into(),
        vec![("Caller.cs".into(), source.as_bytes().to_vec())],
        set.views,
    )
    .unwrap();
    compiler.prepare("caller", &input).unwrap();
    let built = compiler.drive(universe, "caller", toolchain()).unwrap();
    assert!(built.artifact.is_some(), "{:?}", built.report);
    let caller = built.artifact.unwrap();
    let caller_receipt = install_once(&mut store, "caller", &caller).unwrap();
    drop(compiler);
    let published = web_cli(&[
        "compile-resume",
        test.database.to_str().unwrap(),
        compiler_path.to_str().unwrap(),
        "caller",
        "/missing/compiler.dll",
    ]);
    assert_eq!(published["state"], "compiled");
    fs::remove_file(compiler_path).unwrap();
    // A fresh SDK-free TCP edge resumes the partial linked update and replays
    // its immutable response after a later mutation of the exact same Counter.
    let mut child = Command::new(env!("CARGO_BIN_EXE_renraku-vm"))
        .args([
            "web-serve",
            test.database.to_str().unwrap(),
            "@published",
            "127.0.0.1:0",
            "20000",
            "3",
        ])
        .env("PATH", "/no-object-page-tools")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    use std::io::BufRead;
    let mut ready = std::io::BufReader::new(child.stdout.take().unwrap());
    let mut line = String::new();
    ready.read_line(&mut line).unwrap();
    let metadata: serde_json::Value = serde_json::from_str(&line).unwrap();
    let address = metadata["listening"].as_str().unwrap().parse().unwrap();
    let first = exchange_at(address, &wire("POST", "/visit", Some("object-one"), b"7"));
    assert!(
        body(std::str::from_utf8(&first).unwrap()).contains("Counter: 26; visits: 2; entries: 2")
    );
    let second = exchange_at(address, &wire("POST", "/visit", Some("object-two"), b"-3"));
    assert!(
        body(std::str::from_utf8(&second).unwrap()).contains("Counter: 23; visits: 3; entries: 2")
    );
    assert_eq!(
        exchange_at(address, &wire("POST", "/visit", Some("object-one"), b"7")),
        first
    );
    assert!(child.wait().unwrap().success());
    let mut invalid = request("object-invalid");
    invalid.body = "11".into();
    assert!(
        host.process(&invalid, 20000)
            .unwrap()
            .body
            .contains("Delta must be")
    );
    assert_eq!(state(&test.database).0, 3);
    let mut conflict = first_request.clone();
    conflict.body = "6".into();
    assert_eq!(host.process(&conflict, 10000).unwrap_err().status, 409);
    assert_eq!(
        host.process(&request("before-objects"), 10000)
            .unwrap()
            .body,
        baseline.body
    );
    let saved = store.load().unwrap();
    let p = &saved.snapshot.program;
    let mut vm = saved.snapshot.restore().unwrap();
    let method = p.entry("ObjectCaller::Run").unwrap();
    let binding = p.method(method).unwrap().binding;
    let invoke = vm
        .invoke_once(
            &mut store,
            "object-caller",
            method,
            vec![Value::I32(1)],
            0,
            1,
        )
        .unwrap();
    let mut cast_cut = false;
    for _ in 0..2000 {
        let frame = vm.context().frames.last().unwrap();
        if frame.method.method == method
            && let Continuation::Execute { next } = frame.continuation
            && matches!(
                p.method(method).unwrap().instructions[&next].op,
                Op::ReferenceCast { test: true, .. }
            )
        {
            assert_eq!(frame.evaluation_stack, vec![Value::Reference(counter)]);
            cast_cut = true;
            break;
        }
        vm.invoke_once(
            &mut store,
            "object-caller",
            method,
            vec![Value::I32(1)],
            1,
            1,
        )
        .unwrap();
    }
    assert!(cast_cut);
    vm.collect_garbage().unwrap();
    store.save(&vm).unwrap();
    let run = || {
        web_cli(&[
            "invoke-once",
            test.database.to_str().unwrap(),
            "object-caller",
            "ObjectCaller::Run",
            "20000",
            "i32:1",
        ])
    };
    let completed = run();
    assert_eq!(completed["execution"], invoke.execution.to_string());
    assert_eq!(completed["state"], "completed");
    let mut later = request("object-later");
    later.body = "".into();
    assert!(
        host.process(&later, 20000)
            .unwrap()
            .body
            .contains("Counter: 25; visits: 4; entries: 2")
    );
    collect(&test.database);
    let replay = run();
    assert_eq!(completed["execution"], replay["execution"]);
    assert_eq!(completed["result"], replay["result"]);
    let saved = store.load().unwrap();
    let p = &saved.snapshot.program;
    let vm = saved.snapshot.restore().unwrap();
    assert_eq!(
        vm.invocation("object-caller").unwrap().state,
        InvocationState::Completed {
            result: Some(Value::I32(24))
        }
    );
    assert_eq!(p.method(method).unwrap().binding, binding);
    assert_eq!(
        p.reference_set(&[owner]).unwrap().to_json().unwrap(),
        set_json
    );
    assert_eq!(p.find_type("Renraku.Guest.Counter").unwrap(), counter_type);
    assert_eq!(vm.static_fields()[&root], Value::Reference(counter));
    assert_eq!(vm.static_fields()[&state_field], Value::Reference(map));
    assert_eq!(field(&vm, p, counter_node, "Key"), counter_key);
    assert_eq!(
        field(&vm, p, counter_node, "Value"),
        Value::Reference(counter)
    );
    assert_eq!(field(&vm, p, counter, "Value"), Value::I32(25));
    assert_eq!(
        vm.heap()
            .objects()
            .filter(|o| o.type_id() == counter_type)
            .count(),
        1
    );
    assert_eq!(
        vm.heap()
            .objects()
            .filter(|o| o.type_id() == map_type)
            .count(),
        1
    );
    let caller_type = p.find_type("ObjectCaller").unwrap();
    let caller_field = |name| {
        p.fields()
            .find(|f| f.owner == caller_type && f.name == name)
            .unwrap()
            .id
    };
    assert_eq!(vm.static_fields()[&caller_field("Calls")], Value::I32(1));
    assert_eq!(
        vm.static_fields()[&caller_field("Last")],
        Value::Reference(counter)
    );
    for old in original.snapshot.program.methods() {
        assert_eq!(p.method(old.binding.method).unwrap().binding, old.binding);
    }
    for old in original.snapshot.program.fields() {
        assert!(
            p.fields()
                .any(|f| f.id == old.id && f.owner == old.owner && f.value_type == old.value_type)
        );
    }
    let code = vm.read_code(&method.to_string()).unwrap();
    assert_eq!(code["sourceAvailable"], true);
    assert_eq!(code["sources"][0]["text"], source);
    assert_eq!(host.process(&first_request, 10000).unwrap().instructions, 0);
    assert_eq!(
        host.process(&first_request, 10000).unwrap().body,
        body(std::str::from_utf8(&first).unwrap())
    );
    assert_eq!(
        install_once(&mut store, "page", &page).unwrap(),
        page_receipt
    );
    assert_eq!(
        install_once(&mut store, "caller", &caller).unwrap(),
        caller_receipt
    );
    for (id, artifact, receipt) in installations {
        assert_eq!(install_once(&mut store, id, &artifact).unwrap(), receipt);
    }
}

#[test]
fn managed_map_page_recovers_between_group_and_visit_mutations_then_replays_over_native_tcp() {
    let test = Test::new();
    let mut store = CheckpointStore::open(&test.database).unwrap();
    let baseline = test
        .host()
        .process(&request("before-groups"), 10000)
        .unwrap();
    publish_handler(&mut store, "original", HANDLER).unwrap();
    let compiler_path = test.directory.join("groups-compiler.db");
    let mut compiler = CompilationStore::open(&compiler_path).unwrap();
    for (id, name, file, source) in [
        (
            "text",
            "Renraku.Library",
            "IntegerText.cs",
            include_bytes!("../../../guest/IntegerText.cs").as_slice(),
        ),
        (
            "map",
            "Renraku.Collections",
            "StringMap.cs",
            include_bytes!("../../../guest/StringMap.cs").as_slice(),
        ),
    ] {
        let saved = store.load().unwrap();
        let input = CompilationRequest::capture(
            &saved.snapshot.program,
            test.host().universe(),
            toolchain(),
            name.into(),
            vec![(file.into(), source.to_vec())],
            vec![],
        )
        .unwrap();
        compiler.prepare(id, &input).unwrap();
        let built = compiler
            .drive(test.host().universe(), id, toolchain())
            .unwrap();
        install_once(
            &mut store,
            &format!("{id}-install"),
            built.artifact.as_ref().unwrap(),
        )
        .unwrap();
    }
    let saved = store.load().unwrap();
    let program = &saved.snapshot.program;
    let views = [
        "Renraku.Guest.Website",
        "Renraku.Text.IntegerText",
        "Renraku.Collections.StringMap",
    ]
    .map(|name| {
        program
            .reference_view(&[program.find_type(name).unwrap()])
            .unwrap()
    });
    let page = CompilationRequest::capture(
        program,
        test.host().universe(),
        toolchain(),
        "Generated.GroupedWebsite".into(),
        vec![(
            "GroupedWebsite.cs".into(),
            include_bytes!("../../../guest/examples/GroupedWebsite.cs").to_vec(),
        )],
        views.to_vec(),
    )
    .unwrap();
    compiler.prepare("group-page", &page).unwrap();
    let page = compiler
        .drive(test.host().universe(), "group-page", toolchain())
        .unwrap();
    assert!(page.artifact.is_some(), "{:?}", page.report);
    install_once(
        &mut store,
        "group-page-install",
        page.artifact.as_ref().unwrap(),
    )
    .unwrap();
    publish_handler(&mut store, "groups", "GroupedWebsite::Handle").unwrap();
    let exported = store.load().unwrap();
    assert!(
        exported
            .snapshot
            .program
            .reference_view(&[exported
                .snapshot
                .program
                .find_type("GroupedWebsite")
                .unwrap()])
            .unwrap_err()
            .0
            .contains("closure across non-core modules"),
        "the legacy single-view API remains explicitly module-scoped"
    );
    let dependency_set = exported
        .snapshot
        .program
        .reference_set(&[exported
            .snapshot
            .program
            .find_type("GroupedWebsite")
            .unwrap()])
        .unwrap();
    assert_eq!(dependency_set.views.len(), 2);
    dependency_set.validate(&exported.snapshot.program).unwrap();
    drop(compiler);
    fs::remove_file(compiler_path).unwrap();
    let host = WebHost::open_published(&test.database).unwrap();
    let mut red = request("red-one");
    red.body = "red".into();
    let pending = host.process(&red, 0).unwrap();
    assert_eq!(pending.status, 202);
    let group_value = |key: &str| {
        let mut store = CheckpointStore::open(&test.database).unwrap();
        let saved = store.load().unwrap();
        let program = &saved.snapshot.program;
        let vm = saved.snapshot.restore().unwrap();
        let owner = program.find_type("GroupedWebsite").unwrap();
        let field = program
            .fields()
            .find(|f| f.owner == owner && f.name == "Counts")
            .unwrap();
        let map = vm.static_fields()[&field.id].referenced_object()?;
        let field = |id, name| {
            let object = vm.heap().object(id).unwrap();
            let field = program
                .fields()
                .find(|f| f.owner == object.type_id() && f.name == name)
                .unwrap();
            object.fields()[&field.id]
        };
        let mut node = field(map, "head").referenced_object();
        while let Some(id) = node {
            let key_id = field(id, "Key").referenced_object().unwrap();
            if vm.heap().object(key_id).unwrap().string_units().unwrap()
                == key.encode_utf16().collect::<Vec<_>>()
            {
                let value = field(id, "Value").referenced_object().unwrap();
                return Some(
                    String::from_utf16(vm.heap().object(value).unwrap().string_units().unwrap())
                        .unwrap(),
                );
            }
            node = field(id, "Next").referenced_object();
        }
        None
    };
    let mut cut = false;
    for _ in 0..2000 {
        collect(&test.database);
        if group_value("red").as_deref() == Some("1") {
            cut = true;
            break;
        }
        let partial = host.process(&red, 1).unwrap();
        assert_eq!(partial.status, 202);
        assert_eq!(partial.execution, pending.execution);
    }
    assert!(cut);
    assert_eq!(state(&test.database).0, 1);
    assert_eq!(
        host.process(&request("excluded"), 10000)
            .unwrap_err()
            .status,
        503
    );
    // The checkpoint contains a new group link and the original pending CIL
    // frames, before the original website visit executes. Resume in a process.
    let mut child = Command::new(env!("CARGO_BIN_EXE_renraku-vm"))
        .args([
            "web-serve",
            test.database.to_str().unwrap(),
            "@published",
            "127.0.0.1:0",
            "10000",
            "3",
        ])
        .env("PATH", "/no-map-page-tools")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    use std::io::BufRead;
    let mut ready = std::io::BufReader::new(child.stdout.take().unwrap());
    let mut line = String::new();
    ready.read_line(&mut line).unwrap();
    let metadata: serde_json::Value = serde_json::from_str(&line).unwrap();
    let address = metadata["listening"].as_str().unwrap().parse().unwrap();
    let first = exchange_at(address, &wire("POST", "/visit", Some("red-one"), b"red"));
    assert!(
        body(std::str::from_utf8(&first).unwrap())
            .contains("Visits: 2; groups: 1; selected group: 1")
    );
    let second = exchange_at(address, &wire("POST", "/visit", Some("red-two"), b"red"));
    assert!(
        body(std::str::from_utf8(&second).unwrap())
            .contains("Visits: 3; groups: 1; selected group: 2")
    );
    assert_eq!(
        exchange_at(address, &wire("POST", "/visit", Some("red-one"), b"red")),
        first
    );
    assert!(child.wait().unwrap().success());
    assert_eq!(group_value("red").as_deref(), Some("2"));
    assert_eq!(state(&test.database).0, 3);
    let mut blue = request("blue");
    blue.body = "blue".into();
    assert!(
        host.process(&blue, 10000)
            .unwrap()
            .body
            .contains("Visits: 4; groups: 2; selected group: 1")
    );
    assert!(
        host.process(&request("default"), 10000)
            .unwrap()
            .body
            .contains("Visits: 5; groups: 3; selected group: 1")
    );
    let mut invalid = request("invalid-key");
    invalid.body = "x".repeat(129);
    assert!(
        host.process(&invalid, 10000)
            .unwrap()
            .body
            .contains("Invalid group key.")
    );
    assert_eq!(state(&test.database).0, 5);
    let replay = host.process(&red, 10000).unwrap();
    assert_eq!(replay.execution, pending.execution);
    assert_eq!(replay.instructions, 0);
    assert_eq!(replay.body, body(std::str::from_utf8(&first).unwrap()));
    let mut conflict = red.clone();
    conflict.body = "blue".into();
    assert_eq!(host.process(&conflict, 10000).unwrap_err().status, 409);
    assert_eq!(
        host.process(&request("before-groups"), 10000).unwrap().body,
        baseline.body
    );
    // Export a dependency set through the native read-only command, then compile
    // a caller against the page's already-created map rather than a facade copy.
    let saved = store.load().unwrap();
    let program = &saved.snapshot.program;
    let vm = saved.snapshot.restore().unwrap();
    let owner = program.find_type("GroupedWebsite").unwrap();
    let counts = program
        .fields()
        .find(|f| f.owner == owner && f.name == "Counts")
        .unwrap()
        .id;
    let map = vm.static_fields()[&counts].referenced_object().unwrap();
    let map_type = vm.heap().object(map).unwrap().type_id();
    let envelope = web_cli(&[
        "export-reference",
        test.database.to_str().unwrap(),
        "GroupedWebsite",
    ]);
    let set =
        renraku_vm::workbench::ReferenceSet::from_json(&serde_json::to_vec(&envelope).unwrap())
            .unwrap();
    assert_eq!(set.set_hash, dependency_set.set_hash);
    let path = test.directory.join("caller-compiler.db");
    let mut compiler = CompilationStore::open(&path).unwrap();
    let source=b"public static class ExistingGroups { public static string Run() { var map=GroupedWebsite.Counts; var old=map.Get(\"red\"); if (!old.Success || old.Value!=\"2\") return \"unexpected replay\"; return map.Set(\"red\",\"3\").Value; } }";
    let input = CompilationRequest::capture(
        program,
        host.universe(),
        toolchain(),
        "Generated.ExistingGroups".into(),
        vec![("Caller.cs".into(), source.to_vec())],
        set.views.clone(),
    )
    .unwrap();
    compiler.prepare("caller-build", &input).unwrap();
    let built = compiler
        .drive(host.universe(), "caller-build", toolchain())
        .unwrap();
    assert!(built.artifact.is_some(), "{:?}", built.report);
    let artifact = built.artifact.unwrap();
    let generation = store.generation();
    let connection = Connection::open(&test.database).unwrap();
    connection.execute_batch("CREATE TRIGGER reject_cross BEFORE UPDATE ON checkpoint BEGIN SELECT RAISE(ABORT,'injected dependency installation failure'); END;").unwrap();
    assert!(install_once(&mut store, "caller-install", &artifact).is_err());
    assert_eq!(store.generation(), generation);
    assert!(
        store
            .load()
            .unwrap()
            .snapshot
            .program
            .entry("ExistingGroups::Run")
            .is_err()
    );
    connection
        .execute_batch("DROP TRIGGER reject_cross")
        .unwrap();
    install_once(&mut store, "caller-install", &artifact).unwrap();
    drop(compiler);
    let published = web_cli(&[
        "compile-resume",
        test.database.to_str().unwrap(),
        path.to_str().unwrap(),
        "caller-build",
        "/missing/compiler.dll",
    ]);
    assert_eq!(published["state"], "compiled");
    fs::remove_file(path).unwrap();
    let caller_intent = web_cli(&[
        "invoke-once",
        test.database.to_str().unwrap(),
        "caller-invoke",
        "ExistingGroups::Run",
        "0",
    ]);
    assert_eq!(caller_intent["state"], "running");
    let completed = web_cli(&[
        "invoke-once",
        test.database.to_str().unwrap(),
        "caller-invoke",
        "ExistingGroups::Run",
        "10000",
    ]);
    let replay = web_cli(&[
        "invoke-once",
        test.database.to_str().unwrap(),
        "caller-invoke",
        "ExistingGroups::Run",
        "10000",
    ]);
    assert_eq!(completed["execution"], caller_intent["execution"]);
    assert_eq!(replay["execution"], completed["execution"]);
    assert_eq!(completed["result"], replay["result"]);
    collect(&test.database);
    let saved = store.load().unwrap();
    let vm = saved.snapshot.restore().unwrap();
    assert_eq!(
        saved.snapshot.program.find_type("GroupedWebsite").unwrap(),
        owner
    );
    assert_eq!(vm.static_fields()[&counts], Value::Reference(map));
    assert_eq!(
        vm.heap()
            .objects()
            .filter(|o| o.type_id() == map_type)
            .count(),
        1
    );
    assert_eq!(group_value("red").as_deref(), Some("3"));
    assert_eq!(state(&test.database).0, 5);
    assert_eq!(
        host.process(&red, 10000).unwrap().body,
        body(std::str::from_utf8(&first).unwrap())
    );
}

#[test]
fn append_only_decimal_library_and_page_reuse_saved_visits_with_durable_numeric_preconditions() {
    let test = Test::new();
    let mut store = CheckpointStore::open(&test.database).unwrap();
    let host = test.host();
    let original = host.process(&request("before-library"), 10000).unwrap();
    assert_eq!(state(&test.database).0, 1);
    publish_handler(&mut store, "original", HANDLER).unwrap();
    let compiler_path = test.directory.join("integer-compiler.db");
    let mut compiler = CompilationStore::open(&compiler_path).unwrap();
    let saved = store.load().unwrap();
    assert!(
        saved
            .snapshot
            .program
            .find_type("Renraku.Text.IntegerText")
            .is_err()
    );
    // An ordinary CIL module installs against the universe's original core.
    let library = CompilationRequest::capture(
        &saved.snapshot.program,
        host.universe(),
        toolchain(),
        "Renraku.Library".into(),
        vec![(
            "IntegerText.cs".into(),
            include_bytes!("../../../guest/IntegerText.cs").to_vec(),
        )],
        vec![],
    )
    .unwrap();
    compiler.prepare("decimal-library", &library).unwrap();
    let library = compiler
        .drive(host.universe(), "decimal-library", toolchain())
        .unwrap();
    install_once(
        &mut store,
        "decimal-library-install",
        library.artifact.as_ref().unwrap(),
    )
    .unwrap();
    let saved = store.load().unwrap();
    let program = &saved.snapshot.program;
    let views = ["Renraku.Guest.Website", "Renraku.Text.IntegerText"].map(|name| {
        program
            .reference_view(&[program.find_type(name).unwrap()])
            .unwrap()
    });
    let page = CompilationRequest::capture(
        program,
        host.universe(),
        toolchain(),
        "Generated.IntegerWebsite".into(),
        vec![(
            "IntegerWebsite.cs".into(),
            include_bytes!("../../../guest/examples/IntegerWebsite.cs").to_vec(),
        )],
        views.to_vec(),
    )
    .unwrap();
    compiler.prepare("decimal-page", &page).unwrap();
    let page = compiler
        .drive(host.universe(), "decimal-page", toolchain())
        .unwrap();
    assert!(page.artifact.is_some(), "{:?}", page.report);
    install_once(
        &mut store,
        "decimal-page-install",
        page.artifact.as_ref().unwrap(),
    )
    .unwrap();
    assert_eq!(
        inspect_publications(&mut store).unwrap()["current"]["id"],
        "original"
    );
    publish_handler(&mut store, "decimal-page", "IntegerWebsite::Handle").unwrap();
    drop(compiler);
    fs::remove_file(compiler_path).unwrap();
    let host = WebHost::open_published(&test.database).unwrap();
    let mut numeric = request("numeric-precondition");
    numeric.body = "+0001".into();
    let pending = host.process(&numeric, 7).unwrap();
    assert_eq!(pending.status, 202);
    assert_eq!(
        host.process(&request("excluded"), 10000)
            .unwrap_err()
            .status,
        503
    );
    let mut response = pending.clone();
    for _ in 0..1000 {
        collect(&test.database);
        response = WebHost::open_published(&test.database)
            .unwrap()
            .process(&numeric, 7)
            .unwrap();
        assert_eq!(response.execution, pending.execution);
        if response.status == 200 {
            break;
        }
        assert_eq!(response.status, 202);
    }
    assert_eq!(response.status, 200);
    assert!(
        response.body.contains("Visits: 2")
            && response.body.contains("before-library")
            && response.body.contains("numeric-precondition")
    );
    assert_eq!(state(&test.database).0, 2);
    assert!(
        host.process(&request("after-numeric"), 10000)
            .unwrap()
            .body
            .contains("Visits: 3")
    );
    for (index, invalid) in ["1", "3 ", "-1", "2147483648", "\u{0663}", &"0".repeat(65)]
        .into_iter()
        .enumerate()
    {
        let mut invalid_request = request(&format!("invalid-{index}"));
        invalid_request.body = invalid.into();
        let rejected = host.process(&invalid_request, 10000).unwrap();
        assert_eq!(rejected.status, 200); // HTTP-v1 handler validation is HTML.
        assert!(!rejected.body.contains("Visit recorded."));
        assert_eq!(state(&test.database).0, 3);
    }
    let replay = host.process(&numeric, 10000).unwrap();
    assert_eq!(replay.body, response.body);
    assert_eq!(replay.execution, response.execution);
    assert_eq!(replay.instructions, 0);
    let mut changed = numeric.clone();
    changed.body = "3".into();
    assert_eq!(host.process(&changed, 10000).unwrap_err().status, 409);
    let baseline = host.process(&request("before-library"), 10000).unwrap();
    assert_eq!(baseline.body, original.body);
    assert_eq!(baseline.execution, original.execution);
    assert_eq!(baseline.instructions, 0);
    // A fresh native edge needs neither the compiler ledger nor the SDK.
    let mut child = Command::new(env!("CARGO_BIN_EXE_renraku-vm"))
        .args([
            "web-serve",
            test.database.to_str().unwrap(),
            "@published",
            "127.0.0.1:0",
            "10000",
            "3",
        ])
        .env("PATH", "/no-integer-page-tools")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    use std::io::BufRead;
    let mut ready = std::io::BufReader::new(child.stdout.take().unwrap());
    let mut line = String::new();
    ready.read_line(&mut line).unwrap();
    let metadata: serde_json::Value = serde_json::from_str(&line).unwrap();
    let address = metadata["listening"].as_str().unwrap().parse().unwrap();
    let read = String::from_utf8(exchange_at(
        address,
        &wire("GET", "/page", Some("native-read"), b""),
    ))
    .unwrap();
    assert!(body(&read).contains("Visits: 3"));
    let first = exchange_at(
        address,
        &wire("POST", "/visit", Some("native-numbered"), b"3"),
    );
    assert!(body(std::str::from_utf8(&first).unwrap()).contains("Visits: 4"));
    assert_eq!(
        exchange_at(
            address,
            &wire("POST", "/visit", Some("native-numbered"), b"3")
        ),
        first
    );
    assert!(child.wait().unwrap().success());
    assert_eq!(state(&test.database).0, 4);
    let saved = store.load().unwrap();
    assert_eq!(saved.snapshot.restore().unwrap().effects().count(), 0);
}

#[test]
fn compiled_publication_changes_new_requests_while_paused_code_and_shared_state_recover() {
    let test = Test::new();
    let mut store = CheckpointStore::open(&test.database).unwrap();
    let first = publish_handler(&mut store, "release-one", HANDLER).unwrap();
    // The same running host must observe later publications without a restart.
    let host = test.host();
    let mut paused = host.process(&request("old"), 1).unwrap();
    while state(&test.database).0 == 0 {
        paused = host.process(&request("old"), 1).unwrap();
    }
    assert_eq!(paused.status, 202);
    let original = state(&test.database).1[0].1.clone();
    let saved = store.load().unwrap();
    let program = &saved.snapshot.program;
    let view = program
        .reference_view(&[program.find_type("Renraku.Guest.Website").unwrap()])
        .unwrap();
    let source = include_bytes!("../../../guest/examples/WebsiteV2.cs");
    let input = CompilationRequest::capture(
        program,
        host.universe(),
        toolchain(),
        "Generated.Website".into(),
        vec![("WebsiteV2.cs".into(), source.to_vec())],
        vec![view],
    )
    .unwrap();
    let compiler_path = test.directory.join("compiler.db");
    let mut compiler = CompilationStore::open(&compiler_path).unwrap();
    compiler.prepare("page-build", &input).unwrap();
    let compiled = compiler
        .drive(host.universe(), "page-build", toolchain())
        .unwrap();
    let artifact = compiled.artifact.expect("website compilation succeeds");
    // Compilation and append-only installation do not publish the route.
    assert_eq!(
        inspect_publications(&mut store).unwrap()["current"]["id"],
        "release-one"
    );
    install_once(&mut store, "page-install", &artifact).unwrap();
    assert_eq!(
        inspect_publications(&mut store).unwrap()["current"]["id"],
        "release-one"
    );
    assert_eq!(state(&test.database).1[0].1, original);
    let saved = store.load().unwrap();
    let vm = saved.snapshot.restore().unwrap();
    assert_eq!(vm.context().id, original.execution);
    let second = publish_handler(&mut store, "release-two", "PublishedPage::Handle").unwrap();
    assert_ne!(second.method, first.method);
    assert_ne!(second.code, first.code);
    assert_eq!(second.sequence, 2);
    let generation = store.generation();
    // A lost reply for the older publication must never roll back the route.
    assert_eq!(
        publish_handler(&mut store, "release-one", HANDLER).unwrap(),
        first
    );
    assert_eq!(store.generation(), generation);
    assert_eq!(
        inspect_publications(&mut store).unwrap()["current"]["id"],
        "release-two"
    );
    assert_eq!(
        host.process(&request("new"), 10000).unwrap_err().status,
        503
    );
    drop(compiler);
    fs::remove_file(compiler_path).unwrap();
    collect(&test.database);
    let recovered = host.process(&request("old"), 10000).unwrap();
    assert_eq!(recovered.status, 200);
    assert_eq!(recovered.execution, paused.execution);
    assert!(!recovered.body.contains("Published v2"));
    assert_eq!(state(&test.database).0, 1);
    let new = exchange(&host, 10000, &wire("POST", "/visit", Some("new"), b""));
    assert!(body(&new).starts_with("<h2>Published v2</h2>"));
    assert!(body(&new).contains("<li>old</li>") && body(&new).contains("<li>new</li>"));
    assert_eq!(state(&test.database).0, 2);
    let replay = host.process(&request("old"), 10000).unwrap();
    assert_eq!(replay.body, recovered.body);
    assert_eq!(replay.instructions, 0);
    // The compiler ledger, SDK, original files, and host fallback are unnecessary.
    let database = test.database.to_str().unwrap();
    assert_eq!(
        web_cli(&[
            "web-publish",
            database,
            "release-two",
            "PublishedPage::Handle"
        ]),
        serde_json::to_value(&second).unwrap()
    );
    collect(&test.database);
    let inspected = web_cli(&["web-inspect", database]);
    assert_eq!(inspected["current"]["id"], "release-two");
    assert_eq!(inspected["publications"].as_array().unwrap().len(), 2);
    let saved = store.load().unwrap();
    let vm = saved.snapshot.restore().unwrap();
    assert!(
        vm.read_code("PublishedPage::Handle")
            .unwrap()
            .to_string()
            .contains("WebsiteV2.cs")
    );
    assert_eq!(vm.context().frames.len(), 0);
    let server = WebHost::open_published(&test.database).unwrap();
    assert_eq!(
        body(&exchange(
            &server,
            10000,
            &wire("POST", "/visit", Some("new"), b"")
        )),
        body(&new)
    );

    let mut child = Command::new(env!("CARGO_BIN_EXE_renraku-vm"))
        .args([
            "web-serve",
            database,
            "@published",
            "127.0.0.1:0",
            "10000",
            "2",
        ])
        .env("PATH", "/no-website-tools")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    use std::io::BufRead;
    let mut ready = String::new();
    std::io::BufReader::new(child.stdout.take().unwrap())
        .read_line(&mut ready)
        .unwrap();
    let ready: serde_json::Value = serde_json::from_str(&ready).unwrap();
    let address = ready["listening"].as_str().unwrap().parse().unwrap();
    let wire = wire("GET", "/page", Some("after-restart"), b"");
    let response = exchange_at(address, &wire);
    assert!(String::from_utf8_lossy(&response).contains("Published v2"));
    assert_eq!(exchange_at(address, &wire), response);
    assert!(child.wait().unwrap().success());
    assert_eq!(state(&test.database).0, 2);
}

#[test]
fn publication_ids_limits_and_corrupt_routes_reject_without_silent_fallback() {
    let test = Test::new();
    assert!(WebHost::open_published(&test.database).is_err());
    let mut store = CheckpointStore::open(&test.database).unwrap();
    let empty = inspect_publications(&mut store).unwrap();
    assert!(empty["current"].is_null());
    let before = store.generation();
    for id in ["", "bad/id", "\n", &"x".repeat(65)] {
        assert!(publish_handler(&mut store, id, HANDLER).is_err());
    }
    assert!(publish_handler(&mut store, "wrong-abi", "Renraku.Guest.Simulation::Run").is_err());
    assert!(publish_handler(&mut store, "not-installed", "Missing::Handle").is_err());
    assert_eq!(store.generation(), before);
    let original = publish_handler(&mut store, "release", HANDLER).unwrap();
    let before = store.generation();
    assert!(publish_handler(&mut store, "release", "Renraku.Guest.Website::Alternate").is_err());
    assert_eq!(store.generation(), before);
    for n in 1..128 {
        publish_handler(&mut store, &format!("release-{n}"), HANDLER).unwrap();
    }
    let before = store.generation();
    assert!(
        publish_handler(&mut store, "overflow", HANDLER)
            .unwrap_err()
            .0
            .contains("limit")
    );
    assert_eq!(
        publish_handler(&mut store, "release", HANDLER).unwrap(),
        original
    );
    assert_eq!(store.generation(), before);
    collect(&test.database);
    // A syntactically valid heap checkpoint can still contain an unsupported
    // application journal. Validate its schema and bindings before selecting it.
    for (field, value) in [
        ("format", serde_json::json!(2)),
        ("universe", serde_json::json!("another-universe")),
        ("route", serde_json::json!("other")),
        ("id", serde_json::json!("changed")),
        ("sequence", serde_json::json!(0)),
        ("method", serde_json::json!("Renraku.Guest.Website::Handle")),
        ("code", serde_json::json!("missing-code")),
        ("futureField", serde_json::json!(true)),
    ] {
        let database = test.directory.join(format!("bad-{field}.db"));
        copy_world(&test.database, &database);
        let mut broken = CheckpointStore::open(&database).unwrap();
        let saved = broken.load().unwrap();
        let mut vm = saved.snapshot.restore().unwrap();
        let name = "renraku.http-v1/publication/release-127";
        let object = vm.heap().roots()[name];
        let text =
            String::from_utf16(vm.heap().object(object).unwrap().string_units().unwrap()).unwrap();
        let mut record: serde_json::Value = serde_json::from_str(&text).unwrap();
        record[field] = value;
        let object = vm
            .new_string(record.to_string().encode_utf16().collect())
            .unwrap();
        vm.set_root(name, object).unwrap();
        vm.set_root("renraku.http-v1/current", object).unwrap();
        broken.save(&vm).unwrap();
        let before = broken.generation();
        assert!(WebHost::open(&database, HANDLER).is_err(), "{field}");
        assert!(inspect_publications(&mut broken).is_err(), "{field}");
        assert!(
            publish_handler(&mut broken, "repair", HANDLER).is_err(),
            "{field}"
        );
        assert_eq!(broken.generation(), before);
    }
    let saved = store.load().unwrap();
    let mut vm = saved.snapshot.restore().unwrap();
    vm.remove_root("renraku.http-v1/current");
    store.save(&vm).unwrap();
    assert!(
        WebHost::open(&test.database, HANDLER)
            .unwrap_err()
            .0
            .contains("publication root")
    );
    assert!(inspect_publications(&mut store).is_err());
    assert!(publish_handler(&mut store, "repair-by-guessing", HANDLER).is_err());
}

#[test]
fn publication_commit_errors_and_stale_writers_keep_one_coherent_route_and_execution() {
    use renraku_vm::checkpoint::CommitStage;
    for stop in [
        CommitStage::Prepared,
        CommitStage::Written,
        CommitStage::Committed,
    ] {
        let test = Test::new();
        let host = test.host();
        let paused = host.process(&request("active"), 1).unwrap();
        let mut store = CheckpointStore::open(&test.database).unwrap();
        let old = publish_handler(&mut store, "one", HANDLER).unwrap();
        let error = publish_handler_with_observer(
            &mut store,
            "two",
            "Renraku.Guest.Website::Alternate",
            |stage| {
                if stage == stop {
                    Err(Error("lost publication reply".into()))
                } else {
                    Ok(())
                }
            },
        )
        .unwrap_err();
        assert!(error.0.contains("lost publication reply"));
        let current = inspect_publications(&mut store).unwrap();
        assert_eq!(
            current["current"]["id"],
            if stop == CommitStage::Committed {
                "two"
            } else {
                "one"
            }
        );
        assert_eq!(
            state(&test.database).1[0].1.execution.to_string(),
            paused.execution
        );
        let resumed = host.process(&request("active"), 10000).unwrap();
        assert_eq!(resumed.execution, paused.execution);
        assert!(resumed.body.contains("<li>active</li>"));
        publish_handler(&mut store, "two", "Renraku.Guest.Website::Alternate").unwrap();
        let before = store.generation();
        assert_eq!(publish_handler(&mut store, "one", HANDLER).unwrap(), old);
        assert_eq!(store.generation(), before);
        assert_eq!(
            host.process(&request("new"), 10000).unwrap().body,
            "alternate handler"
        );
        assert_eq!(state(&test.database).0, 1);
    }
    let test = Test::new();
    let host = test.host();
    host.process(&request("active"), 1).unwrap();
    let mut store = CheckpointStore::open(&test.database).unwrap();
    publish_handler(&mut store, "one", HANDLER).unwrap();
    let error = publish_handler_with_observer(
        &mut store,
        "two",
        "Renraku.Guest.Website::Alternate",
        |stage| {
            if stage == CommitStage::Prepared {
                host.process(&request("active"), 10000)
                    .map_err(|e| Error(e.to_string()))?;
            }
            Ok(())
        },
    )
    .unwrap_err();
    assert!(error.0.contains("checkpoint conflict"));
    assert_eq!(
        inspect_publications(&mut store).unwrap()["current"]["id"],
        "one"
    );
    assert_eq!(state(&test.database).0, 1);
}

#[test]
fn sigkill_publication_keeps_receipt_route_and_paused_frames_atomic() {
    for phase in ["prepared", "written", "committed"] {
        let test = Test::new();
        let host = test.host();
        let paused = host.process(&request("active"), 1).unwrap();
        let mut store = CheckpointStore::open(&test.database).unwrap();
        publish_handler(&mut store, "one", HANDLER).unwrap();
        let mut child = Command::new(env!("CARGO_BIN_EXE_renraku-checkpoint-probe"))
            .args([
                "web-publish",
                test.database.to_str().unwrap(),
                "two",
                "Renraku.Guest.Website::Alternate",
                phase,
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        use std::io::BufRead;
        let mut ready = std::io::BufReader::new(child.stdout.take().unwrap());
        let (send, receive) = std::sync::mpsc::channel();
        let reader = thread::spawn(move || {
            let mut line = String::new();
            ready.read_line(&mut line).unwrap();
            send.send(line).unwrap();
        });
        let line = receive.recv_timeout(Duration::from_secs(20));
        child.kill().unwrap();
        child.wait().unwrap();
        reader.join().unwrap();
        assert!(line.unwrap().starts_with("READY"));
        collect(&test.database);
        let current = inspect_publications(&mut store).unwrap();
        assert_eq!(
            current["current"]["id"],
            if phase == "committed" { "two" } else { "one" }
        );
        let resumed = host.process(&request("active"), 10000).unwrap();
        assert_eq!(resumed.execution, paused.execution);
        assert_eq!(state(&test.database).0, 1);
        assert!(resumed.body.contains("<li>active</li>"));
        publish_handler(&mut store, "two", "Renraku.Guest.Website::Alternate").unwrap();
        assert_eq!(
            inspect_publications(&mut store).unwrap()["publications"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
        assert_eq!(
            host.process(&request("new"), 10000).unwrap().body,
            "alternate handler"
        );
    }
}
impl Test {
    fn new() -> Self {
        let directory = std::env::temp_dir().join(format!("renraku-web-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&directory).unwrap();
        let database = directory.join("universe.db");
        let program = common::program();
        let mut vm = Vm::new(&program);
        vm.start(
            program.entry("Renraku.Guest.Website::Initialize").unwrap(),
            vec![],
        )
        .unwrap();
        let mut store = CheckpointStore::open(&database).unwrap();
        assert!(matches!(
            vm.run_checkpointed(&mut store, 1000, 100).unwrap(),
            RunOutcome::Completed(None)
        ));
        Self {
            directory,
            database,
        }
    }
    fn host(&self) -> WebHost {
        WebHost::open(&self.database, HANDLER).unwrap()
    }
}
impl Drop for Test {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.directory);
    }
}
fn request(id: &str) -> WebRequest {
    WebRequest {
        id: id.into(),
        method: "POST".into(),
        target: "/visit".into(),
        body: String::new(),
    }
}
fn state(database: &Path) -> (i32, Vec<(String, renraku_vm::invocation::InvocationRecord)>) {
    let mut store = CheckpointStore::open(database).unwrap();
    let saved = store.load().unwrap();
    let vm = saved.snapshot.restore().unwrap();
    let owner = saved
        .snapshot
        .program
        .find_type("Renraku.Guest.Website")
        .unwrap();
    let field = saved
        .snapshot
        .program
        .fields()
        .find(|f| f.owner == owner && f.name == "Visits")
        .unwrap()
        .id;
    let Value::I32(visits) = vm.static_fields()[&field] else {
        panic!()
    };
    (
        visits,
        vm.invocations()
            .map(|(id, r)| (id.to_owned(), r.clone()))
            .collect(),
    )
}
fn collect(database: &Path) {
    let mut store = CheckpointStore::open(database).unwrap();
    let saved = store.load().unwrap();
    let mut vm = saved.snapshot.restore().unwrap();
    vm.collect_garbage().unwrap();
    store.save(&vm).unwrap();
}
fn copy_world(source: &Path, target: &Path) {
    let row: (Vec<u8>, u32, Vec<u8>, Vec<u8>) = Connection::open(source)
        .unwrap()
        .query_row(
            "SELECT universe,format,digest,payload FROM checkpoint",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .unwrap();
    let _store = CheckpointStore::open(target).unwrap();
    Connection::open(target)
        .unwrap()
        .execute(
            "INSERT INTO checkpoint VALUES(1,1,?1,?2,?3,?4)",
            params![row.0, row.1, row.2, row.3],
        )
        .unwrap();
}
fn wire(method: &str, target: &str, id: Option<&str>, body: &[u8]) -> Vec<u8> {
    let mut text = format!(
        "{method} {target} HTTP/1.1\r\nHost: localhost\r\nContent-Length: {}\r\n",
        body.len()
    );
    if let Some(id) = id {
        text.push_str(&format!("X-Renraku-Request-Id: {id}\r\n"));
    }
    text.push_str("\r\n");
    let mut bytes = text.into_bytes();
    bytes.extend_from_slice(body);
    bytes
}
fn exchange_at(address: SocketAddr, bytes: &[u8]) -> Vec<u8> {
    let mut socket = TcpStream::connect(address).unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    socket.write_all(bytes).unwrap();
    socket.shutdown(Shutdown::Write).unwrap();
    let mut response = Vec::new();
    socket.read_to_end(&mut response).unwrap();
    response
}
fn exchange(host: &WebHost, budget: u64, bytes: &[u8]) -> String {
    let server = WebServer::bind(host.clone(), "127.0.0.1:0".parse().unwrap(), budget).unwrap();
    let address = server.address().unwrap();
    let worker = thread::spawn(move || server.serve_one());
    let response = exchange_at(address, bytes);
    worker.join().unwrap().unwrap();
    String::from_utf8(response).unwrap()
}
fn body(response: &str) -> &str {
    response.split_once("\r\n\r\n").unwrap().1
}

#[test]
fn localhost_page_retains_managed_state_and_replays_the_original_response_after_later_requests() {
    let test = Test::new();
    let host = test.host();
    let bootstrap = exchange(&host, 10000, &wire("GET", "/", None, b""));
    assert!(
        bootstrap.starts_with("HTTP/1.1 200")
            && bootstrap.contains("sessionStorage")
            && bootstrap.contains(&host.universe().to_string())
    );
    assert_eq!(state(&test.database), (0, vec![]));
    let first = exchange(
        &host,
        10000,
        &wire("POST", "/visit", Some("visit-one"), b""),
    );
    assert!(first.starts_with("HTTP/1.1 200") && body(&first).contains("<li>visit-one</li>"));
    let original = state(&test.database).1[0].1.clone();
    let second = exchange(
        &host,
        10000,
        &wire("POST", "/visit", Some("visit-two"), b""),
    );
    assert!(body(&second).contains("visit-one") && body(&second).contains("visit-two"));
    collect(&test.database);
    // A server restart can select a new handler for new IDs. Existing IDs keep
    // their original pinned method and immutable response string.
    let alternate = WebHost::open(&test.database, "Renraku.Guest.Website::Alternate").unwrap();
    let replay = exchange(
        &alternate,
        10000,
        &wire("POST", "/visit", Some("visit-one"), b""),
    );
    assert_eq!(first, replay);
    assert!(!body(&replay).contains("visit-two"));
    assert_eq!(state(&test.database).0, 2);
    assert_eq!(
        state(&test.database)
            .1
            .iter()
            .find(|(_, r)| r.execution == original.execution)
            .unwrap()
            .1,
        original
    );
    let conflict = exchange(
        &host,
        10000,
        &wire("POST", "/visit", Some("visit-one"), b"changed"),
    );
    assert!(conflict.starts_with("HTTP/1.1 409"));
    let mut read = request("view");
    read.method = "GET".into();
    read.target = "/page".into();
    let page = host.process(&read, 10000).unwrap();
    assert!(page.body.contains("visit-two"));
    assert_eq!(host.process(&read, 10000).unwrap().instructions, 0);
    assert_eq!(state(&test.database).0, 2);
}

#[test]
fn every_handler_instruction_restores_with_gc_and_the_same_request_execution() {
    let test = Test::new();
    let prototype = test.directory.join("prototype.db");
    copy_world(&test.database, &prototype);
    let complete = WebHost::open(&prototype, HANDLER)
        .unwrap()
        .process(&request("cut"), 10000)
        .unwrap();
    assert!(complete.instructions > 20);
    for cut in 0..=complete.instructions {
        let database = test.directory.join(format!("cut-{cut}.db"));
        copy_world(&test.database, &database);
        let host = WebHost::open(&database, HANDLER).unwrap();
        let first = host.process(&request("cut"), cut).unwrap();
        assert!(first.instructions <= cut);
        collect(&database);
        let resumed = host.process(&request("cut"), 10000).unwrap();
        assert_eq!(resumed.status, 200);
        assert_eq!(resumed.execution, first.execution);
        assert_eq!(
            first.instructions + resumed.instructions,
            complete.instructions
        );
        assert_eq!(resumed.body, complete.body);
        assert_eq!(state(&database).0, 1);
        assert_eq!(state(&database).1.len(), 1);
    }
}

#[test]
fn pending_requests_exclude_new_work_and_handler_faults_or_invalid_text_are_terminal() {
    let test = Test::new();
    let host = test.host();
    let pending = host.process(&request("one"), 0).unwrap();
    assert_eq!((pending.status, pending.state), (202, "pending"));
    assert_eq!(host.process(&request("two"), 1000).unwrap_err().status, 503);
    assert_eq!(state(&test.database).1.len(), 1);
    assert_eq!(host.process(&request("one"), 10000).unwrap().status, 200);
    for (method, state_name) in [
        ("Denied", "faulted"),
        ("NullResponse", "invalid_response"),
        ("SurrogateResponse", "invalid_response"),
        ("LargeResponse", "invalid_response"),
    ] {
        let host =
            WebHost::open(&test.database, &format!("Renraku.Guest.Website::{method}")).unwrap();
        let first = host.process(&request(method), 10000).unwrap();
        assert_eq!(first.status, 500);
        assert_eq!(first.state, state_name);
        let replay = host.process(&request(method), 10000).unwrap();
        assert_eq!(replay.instructions, 0);
        assert_eq!(first.body, replay.body);
        assert_eq!(first.execution, replay.execution);
    }
    let mut store = CheckpointStore::open(&test.database).unwrap();
    let saved = store.load().unwrap();
    let vm = saved.snapshot.restore().unwrap();
    assert_eq!(vm.effects().count(), 0);
    assert_eq!(state(&test.database).0, 1);
    assert!(WebHost::open(&test.database, "Renraku.Guest.Simulation::Run").is_err());
    assert!(WebServer::bind(host, "0.0.0.0:0".parse().unwrap(), 1000).is_err());
}

#[test]
fn malformed_or_unsupported_http_is_rejected_before_acceptance() {
    let test = Test::new();
    let host = test.host();
    let cases:Vec<(u16,Vec<u8>)>=vec![
        (400,b"POST /visit HTTP/1.1\r\nHost: localhost\r\nContent-Length: 0\r\ncontent-length: 0\r\nX-Renraku-Request-Id: bad\r\n\r\n".to_vec()),
        (400,b"POST /visit HTTP/1.1\r\nHost: localhost\r\nTransfer-Encoding: chunked\r\nContent-Length: 0\r\n\r\n".to_vec()),
        (400,b"GET /page HTTP/1.1\r\nHost : localhost\r\n\r\n".to_vec()),
        (400,b"GET /page HTTP/1.1\r\n\r\n".to_vec()),
        (400,b"POST /visit HTTP/1.1\r\nHost: localhost\r\nContent-Length: 5\r\n\r\nhi".to_vec()),
        (400,wire("POST","/visit",Some("bad/id"),b"")),
        (400,wire("POST","/visit",Some("binary"),&[0xff])),
        (400,wire("GET","/page",None,b"")),
        (405,wire("PUT","/page",Some("put"),b"")),
        (413,b"POST /visit HTTP/1.1\r\nHost: localhost\r\nContent-Length: 8193\r\n\r\n".to_vec()),
        (417,b"POST /visit HTTP/1.1\r\nHost: localhost\r\nExpect: 100-continue\r\n\r\n".to_vec()),
        (431,format!("GET /page HTTP/1.1\r\nHost: localhost\r\nLong: {}\r\n\r\n","x".repeat(8192)).into_bytes()),
    ];
    for (status, wire) in cases {
        let response = exchange(&host, 1000, &wire);
        assert!(
            response.starts_with(&format!("HTTP/1.1 {status}")),
            "{response}"
        );
    }
    assert_eq!(state(&test.database), (0, vec![]));
    let mut changed = request("same");
    host.process(&changed, 1000).unwrap();
    changed.method = "GET".into();
    assert_eq!(host.process(&changed, 1000).unwrap_err().status, 409);
}

fn wait_file(path: &Path, child: &mut std::process::Child) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !path.exists() {
        if Instant::now() > deadline || child.try_wait().unwrap().is_some() {
            let _ = child.kill();
            panic!("web probe did not reach {}", path.display());
        }
        thread::sleep(Duration::from_millis(5));
    }
}
#[test]
fn sigkill_before_during_and_after_handler_commit_preserves_one_mutation_and_response() {
    let template = Test::new();
    let prototype = template.directory.join("prototype.db");
    copy_world(&template.database, &prototype);
    let complete = WebHost::open(&prototype, HANDLER)
        .unwrap()
        .process(&request("crash"), 10000)
        .unwrap();
    let cut = (1..complete.instructions)
        .find(|cut| {
            let database = template.directory.join(format!("find-{cut}.db"));
            copy_world(&template.database, &database);
            let reply = WebHost::open(&database, HANDLER)
                .unwrap()
                .process(&request("crash"), *cut)
                .unwrap();
            reply.status == 202 && state(&database).0 == 1
        })
        .unwrap();
    for phase in ["accepted", "paused", "completed"] {
        let test = Test::new();
        let ready = test.directory.join("ready");
        let marker = test.directory.join("marker");
        let mut child = Command::new(env!("CARGO_BIN_EXE_renraku-web-probe"))
            .arg(&test.database)
            .arg(&ready)
            .arg(&marker)
            .arg(phase)
            .arg(if phase == "paused" { cut } else { 10000 }.to_string())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        wait_file(&ready, &mut child);
        let address: SocketAddr = fs::read_to_string(&ready).unwrap().parse().unwrap();
        let client = thread::spawn(move || {
            exchange_at(address, &wire("POST", "/visit", Some("crash"), b""))
        });
        wait_file(&marker, &mut child);
        child.kill().unwrap();
        let status = child.wait().unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::process::ExitStatusExt;
            assert_eq!(status.signal(), Some(9));
        }
        assert!(client.join().unwrap().is_empty());
        let before = state(&test.database);
        assert_eq!(before.1.len(), 1);
        assert_eq!(before.0, if phase == "accepted" { 0 } else { 1 });
        if phase == "paused" {
            assert_eq!(before.1[0].1.state, InvocationState::Running);
        }
        collect(&test.database);
        // Rebind the same port and recover through the actual HTTP edge.
        let server = WebServer::bind(test.host(), address, 10000).unwrap();
        let worker = thread::spawn(move || server.serve_one());
        let response = String::from_utf8(exchange_at(
            address,
            &wire("POST", "/visit", Some("crash"), b""),
        ))
        .unwrap();
        worker.join().unwrap().unwrap();
        assert!(response.starts_with("HTTP/1.1 200") && body(&response).contains("<li>crash</li>"));
        let after = state(&test.database);
        assert_eq!(after.0, 1);
        assert_eq!(after.1.len(), 1);
        assert_eq!(before.1[0].1.execution, after.1[0].1.execution);
        assert_eq!(
            test.host()
                .process(&request("crash"), 10000)
                .unwrap()
                .instructions,
            0
        );
    }
}

#[test]
fn native_cli_serves_bootstrap_and_managed_requests_with_bounded_connections() {
    let test = Test::new();
    let mut child = Command::new(env!("CARGO_BIN_EXE_renraku-vm"))
        .args([
            "web-serve",
            test.database.to_str().unwrap(),
            HANDLER,
            "127.0.0.1:0",
            "10000",
            "3",
        ])
        .env("PATH", "/no-website-tools")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    use std::io::BufRead;
    let mut ready = std::io::BufReader::new(child.stdout.take().unwrap());
    let mut line = String::new();
    ready.read_line(&mut line).unwrap();
    let metadata: serde_json::Value = serde_json::from_str(&line).unwrap();
    let address = metadata["listening"].as_str().unwrap().parse().unwrap();
    assert!(
        String::from_utf8(exchange_at(address, &wire("GET", "/", None, b"")))
            .unwrap()
            .starts_with("HTTP/1.1 200")
    );
    let first = exchange_at(address, &wire("POST", "/visit", Some("cli"), b""));
    let replay = exchange_at(address, &wire("POST", "/visit", Some("cli"), b""));
    assert_eq!(first, replay);
    assert!(child.wait().unwrap().success());
    assert_eq!(state(&test.database).0, 1);
}

#[test]
fn failed_response_checkpoint_leaves_a_resumable_receipt_instead_of_sending_unsaved_data() {
    let test = Test::new();
    let host = test.host();
    let failure=host.process_with_observer(&request("save-failure"),10000,|stage| {
        if stage==WebStage::Accepted {
            let mut store=CheckpointStore::open(&test.database)?;let generation=store.load()?.generation;
            Connection::open(&test.database)?.execute_batch(&format!("CREATE TRIGGER fail_web_save BEFORE UPDATE ON checkpoint WHEN NEW.generation={} BEGIN SELECT RAISE(FAIL,'web save failure'); END;",generation+2))?;
        }
        Ok(())
    }).unwrap_err();
    assert_eq!(failure.status, 503);
    let pending = state(&test.database);
    assert_eq!(pending.1.len(), 1);
    assert_eq!(pending.1[0].1.state, InvocationState::Running);
    Connection::open(&test.database)
        .unwrap()
        .execute_batch("DROP TRIGGER fail_web_save;")
        .unwrap();
    let recovered = host.process(&request("save-failure"), 10000).unwrap();
    assert_eq!(recovered.status, 200);
    assert_eq!(recovered.execution, pending.1[0].1.execution.to_string());
    assert_eq!(state(&test.database).0, 1);
    assert_eq!(
        host.process(&request("save-failure"), 10000)
            .unwrap()
            .instructions,
        0
    );
}

#[test]
fn fragmented_utf8_and_nul_bodies_are_exact_receipt_inputs_and_response_bytes() {
    let test = Test::new();
    let host = WebHost::open(&test.database, "Renraku.Guest.Website::Echo").unwrap();
    let server = WebServer::bind(host.clone(), "127.0.0.1:0".parse().unwrap(), 1000).unwrap();
    let address = server.address().unwrap();
    let worker = thread::spawn(move || server.serve_one());
    let content = "hello\0🦀";
    let bytes = wire("POST", "/echo", Some("unicode"), content.as_bytes());
    let mut socket = TcpStream::connect(address).unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    let split = bytes.len() - 3;
    socket.write_all(&bytes[..split]).unwrap();
    thread::sleep(Duration::from_millis(10));
    socket.write_all(&bytes[split..]).unwrap();
    socket.shutdown(Shutdown::Write).unwrap();
    let mut reply = String::new();
    socket.read_to_string(&mut reply).unwrap();
    worker.join().unwrap().unwrap();
    assert_eq!(body(&reply), content);
    collect(&test.database);
    let retry = exchange(&host, 1000, &bytes);
    assert_eq!(reply, retry);
    let changed = exchange(
        &host,
        1000,
        &wire("POST", "/echo", Some("unicode"), b"hello"),
    );
    assert!(changed.starts_with("HTTP/1.1 409"));
    assert_eq!(state(&test.database).0, 0);
    assert_eq!(state(&test.database).1.len(), 1);
}

#[test]
fn local_preview_shares_the_http_inbox_and_revokes_model_authority_on_the_supplied_vm() {
    use renraku_vm::web::preview_handler;
    let test = Test::new();
    let mut store = CheckpointStore::open(&test.database).unwrap();
    let selected = publish_handler(&mut store, "original", HANDLER).unwrap();
    let saved = store.load().unwrap();
    let mut vm = saved.snapshot.restore().unwrap();
    let pending = preview_handler(&mut vm, &mut store, &request("direct"), &selected, 0).unwrap();
    assert_eq!((pending.status, pending.instructions), (202, 0));
    publish_handler(&mut store, "newer", "Renraku.Guest.Website::Alternate").unwrap();
    collect(&test.database);
    let saved = store.load().unwrap();
    let mut vm = saved.snapshot.restore().unwrap();
    let old = preview_handler(&mut vm, &mut store, &request("direct"), &selected, 10000).unwrap();
    assert_eq!(old.execution, pending.execution);
    assert_eq!(old.status, 200);
    assert_eq!(state(&test.database).0, 1);
    // Socket delivery of the same client ID sees exactly the same accepted call.
    let host = WebHost::open_published(&test.database).unwrap();
    let response = exchange(&host, 10000, &wire("POST", "/visit", Some("direct"), b""));
    assert_eq!(body(&response), old.body);
    assert!(response.contains(&old.execution));
    let saved = store.load().unwrap();
    let mut vm = saved.snapshot.restore().unwrap();
    let before = store.generation();
    assert_eq!(
        preview_handler(
            &mut vm,
            &mut store,
            &request("not-yet-accepted"),
            &selected,
            10000
        )
        .unwrap_err()
        .status,
        409
    );
    assert_eq!(store.generation(), before);
    let denied = publish_handler(&mut store, "denied", "Renraku.Guest.Website::Denied").unwrap();
    let saved = store.load().unwrap();
    let mut vm = saved.snapshot.restore().unwrap();
    vm.set_model_calls_enabled(true);
    let reply = preview_handler(&mut vm, &mut store, &request("denied"), &denied, 10000).unwrap();
    assert_eq!((reply.status, reply.state), (500, "faulted"));
    assert_eq!(vm.effects().count(), 0);
    vm.collect_garbage().unwrap();
    store.save(&vm).unwrap();
    let replay = preview_handler(&mut vm, &mut store, &request("denied"), &denied, 10000).unwrap();
    assert_eq!(replay.execution, reply.execution);
    assert_eq!(replay.instructions, 0);
}
