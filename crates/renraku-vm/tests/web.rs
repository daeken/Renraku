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
