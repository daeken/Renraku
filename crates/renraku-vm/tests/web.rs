#[allow(dead_code)]
mod common;
use renraku_vm::{
    checkpoint::CheckpointStore,
    invocation::InvocationState,
    runtime::{RunOutcome, Vm},
    value::Value,
    web::{WebHost, WebRequest, WebServer, WebStage},
};
use rusqlite::{Connection, params};
use std::{
    fs,
    io::{Read, Write},
    net::{Shutdown, SocketAddr, TcpStream},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

const HANDLER: &str = "Renraku.Guest.Website::Handle";
struct Test {
    directory: PathBuf,
    database: PathBuf,
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
