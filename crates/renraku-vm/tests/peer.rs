#[allow(dead_code)]
mod common;
use renraku_vm::{
    checkpoint::CheckpointStore,
    identity::{GrantId, NodeId, ObjectId, UniverseId},
    owner::{OwnerCall, ReplyState},
    peer::{self, CallReport, Endpoint, OwnerServer, PeerKey},
    placement::{self, Argument, OwnerContext, PlacementPolicy},
    runtime::Vm,
    value::Value,
};
use rusqlite::Connection;
use std::{
    collections::BTreeSet,
    fs,
    io::{BufRead, BufReader},
    net::SocketAddr,
    path::PathBuf,
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};
struct Test {
    dir: PathBuf,
    owner: CheckpointStore,
    caller: CheckpointStore,
    node: NodeId,
    principal: UniverseId,
    object: ObjectId,
    call: OwnerCall,
    key: PeerKey,
}
impl Test {
    fn new() -> Self {
        let dir = common::fixtures()
            .output
            .join(format!("peer-{}", UniverseId::fresh()));
        fs::create_dir(&dir).unwrap();
        let p = renraku_vm::program::Program::load(&[
            &common::fixtures().core,
            &common::fixtures().guest,
            &fs::read(
                common::fixtures()
                    .output
                    .join("Renraku.ExportPolicyTests.dll"),
            )
            .unwrap(),
        ])
        .unwrap();
        let mut vm = Vm::new(&p);
        vm.start(common::entry(&p, "SaveObject"), vec![]).unwrap();
        vm.run(1000).unwrap();
        vm.start(
            p.entry("Renraku.Tests.ExportPolicyHarness::Initialize")
                .unwrap(),
            vec![],
        )
        .unwrap();
        vm.run(1000).unwrap();
        let ownerid = vm.universe_id();
        let ty = p.find_type("Renraku.Guest.Simulation").unwrap();
        let object = vm.static_fields()[&p
            .fields()
            .find(|f| f.owner == ty && f.name == "Saved")
            .unwrap()
            .id]
            .referenced_object()
            .unwrap();
        let mut owner = CheckpointStore::open(dir.join("owner.db")).unwrap();
        owner.save(&vm).unwrap();
        let vm = Vm::new(&p);
        let principal = vm.universe_id();
        let mut caller = CheckpointStore::open(dir.join("caller.db")).unwrap();
        caller.save(&vm).unwrap();
        let node = NodeId::fresh();
        placement::initialize(&mut owner, node, 1).unwrap();
        placement::register_object(&mut owner, object, PlacementPolicy::pinned(node)).unwrap();
        let method = p
            .method(p.entry("Renraku.Guest.Counter::Add").unwrap())
            .unwrap()
            .binding;
        let grant = GrantId::fresh();
        placement::export_method(&mut owner, grant, principal, object, method).unwrap();
        let call = OwnerCall {
            operation: "first".into(),
            owner: ownerid,
            epoch: 1,
            grant,
            object,
            method,
            arguments: vec![Argument::I32(7)],
        };
        let key = PeerKey::from_bytes([0xa5; 32]);
        key.write_new(dir.join("key")).unwrap();
        fs::write(dir.join("call.json"), peer::encode_call(&call).unwrap()).unwrap();
        Self {
            dir,
            owner,
            caller,
            node,
            principal,
            object,
            call,
            key,
        }
    }
    fn context(&self) -> OwnerContext {
        OwnerContext {
            node: self.node,
            epoch: 1,
            resources: BTreeSet::new(),
        }
    }
    fn server(&self, budget: u64) -> OwnerServer {
        OwnerServer::bind(
            self.dir.join("owner.db"),
            self.context(),
            "127.0.0.1:0".parse().unwrap(),
            vec![(self.principal, self.key.clone())],
            budget,
        )
        .unwrap()
    }
    fn endpoint(&self, address: SocketAddr) -> Endpoint {
        Endpoint {
            owner: self.call.owner,
            node: self.node,
            epoch: 1,
            address,
        }
    }
    fn value(&mut self) -> i32 {
        let saved = self.owner.load().unwrap();
        let vm = saved.snapshot.restore().unwrap();
        let field = saved
            .snapshot
            .program
            .fields()
            .find(|f| {
                f.owner == vm.heap().object(self.object).unwrap().type_id() && f.name == "Value"
            })
            .unwrap();
        let Value::I32(n) = vm.heap().object(self.object).unwrap().fields()[&field.id] else {
            panic!()
        };
        n
    }
    fn collect(&mut self) {
        for store in [&mut self.owner, &mut self.caller] {
            let saved = store.load().unwrap();
            let mut vm = saved.snapshot.restore().unwrap();
            vm.collect_garbage().unwrap();
            store.save(&vm).unwrap();
        }
    }
}
impl Drop for Test {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}
fn background(server: OwnerServer, count: usize) -> thread::JoinHandle<Vec<bool>> {
    thread::spawn(move || (0..count).map(|_| server.serve_one().is_ok()).collect())
}
struct Server {
    child: Child,
    address: SocketAddr,
}
impl Server {
    fn start(test: &Test, address: SocketAddr, budget: u64, count: usize, marker: &str) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_renraku-checkpoint-probe"))
            .args([
                "peer-server",
                test.dir.join("owner.db").to_str().unwrap(),
                &test.node.to_string(),
                &test.principal.to_string(),
                test.dir.join("key").to_str().unwrap(),
                &address.to_string(),
                &budget.to_string(),
                &count.to_string(),
                marker,
            ])
            .env("PATH", "/no-peer-sdk")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let mut line = String::new();
        BufReader::new(child.stdout.take().unwrap())
            .read_line(&mut line)
            .unwrap();
        let reply: serde_json::Value = serde_json::from_str(&line).unwrap();
        Self {
            child,
            address: reply["listening"].as_str().unwrap().parse().unwrap(),
        }
    }
    fn finish(&mut self) {
        assert!(self.child.wait().unwrap().success());
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
fn cli(args: &[&str]) -> serde_json::Value {
    let output = Command::new(env!("CARGO_BIN_EXE_renraku-vm"))
        .args(args)
        .env("PATH", "/no-peer-sdk")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}
#[test]
fn authenticated_network_call_uses_original_object_and_journals_reply_before_keyless_replay() {
    let mut test = Test::new();
    let server = test.server(1000);
    let endpoint = test.endpoint(server.address().unwrap());
    let worker = background(server, 2);
    let first = peer::call_once(&mut test.caller, &endpoint, &test.call, &test.key).unwrap();
    let CallReport::Owner(reply) = &first else {
        panic!()
    };
    assert_eq!(reply.state, ReplyState::Completed(Some(Argument::I32(26))));
    let mut later = test.call.clone();
    later.operation = "later".into();
    later.arguments = vec![Argument::I32(-3)];
    peer::call_once(&mut test.caller, &endpoint, &later, &test.key).unwrap();
    assert_eq!(worker.join().unwrap(), vec![true, true]);
    assert_eq!(test.value(), 23);
    test.collect();
    assert_eq!(
        peer::resume(&mut test.caller, "first", None).unwrap(),
        first
    );
    let saved = test.caller.load().unwrap();
    let vm = saved.snapshot.restore().unwrap();
    let counter = saved
        .snapshot
        .program
        .find_type("Renraku.Guest.Counter")
        .unwrap();
    assert_eq!(
        vm.heap()
            .objects()
            .filter(|o| o.type_id() == counter)
            .count(),
        0
    );
    let payload: Vec<u8> = Connection::open(test.dir.join("caller.db"))
        .unwrap()
        .query_row("SELECT payload FROM checkpoint", [], |r| r.get(0))
        .unwrap();
    assert!(!payload.windows(32).any(|w| w == [0xa5; 32]));
    let mut changed = test.call.clone();
    changed.arguments = vec![Argument::I32(8)];
    assert!(peer::call_once(&mut test.caller, &endpoint, &changed, &test.key).is_err());
}
#[test]
fn pending_acknowledgement_pins_execution_and_revoked_accepted_work_resumes_once() {
    let mut test = Test::new();
    let server = test.server(1);
    let endpoint = test.endpoint(server.address().unwrap());
    let worker = background(server, 64);
    let first = peer::call_once(&mut test.caller, &endpoint, &test.call, &test.key).unwrap();
    let CallReport::Owner(first) = first else {
        panic!()
    };
    assert_eq!(first.state, ReplyState::Pending);
    placement::revoke(&mut test.owner, test.call.grant).unwrap();
    let mut done = false;
    for _ in 0..32 {
        test.collect();
        let report = peer::resume(&mut test.caller, "first", Some(&test.key)).unwrap();
        let CallReport::Owner(reply) = report else {
            panic!()
        };
        assert_eq!(reply.execution, first.execution);
        if reply.state != ReplyState::Pending {
            assert_eq!(reply.state, ReplyState::Completed(Some(Argument::I32(26))));
            done = true;
            break;
        }
    }
    assert!(done);
    assert_eq!(test.value(), 26); // Close the bounded listener's remaining slots without guest work.
    use std::net::TcpStream;
    for _ in 0..64 {
        if TcpStream::connect(endpoint.address).is_err() {
            break;
        }
    }
    assert!(worker.join().is_ok());
}
#[test]
fn caller_terminal_save_failure_keeps_uncertain_intent_and_reconciles_original_owner_receipt() {
    let mut test = Test::new();
    let server = test.server(1000);
    let endpoint = test.endpoint(server.address().unwrap());
    let worker = background(server, 2);
    let db = Connection::open(test.dir.join("caller.db")).unwrap();
    db.execute_batch("CREATE TRIGGER fail_peer_reply BEFORE UPDATE ON checkpoint WHEN instr(NEW.payload,CAST('renraku.peer-v1/reply/' AS BLOB))>0 BEGIN SELECT RAISE(ABORT,'failed peer reply'); END;").unwrap();
    assert!(peer::call_once(&mut test.caller, &endpoint, &test.call, &test.key).is_err());
    assert_eq!(test.value(), 26);
    assert_eq!(
        peer::inspect(&mut test.caller, "first").unwrap(),
        Some(CallReport::Unresolved)
    );
    db.execute_batch("DROP TRIGGER fail_peer_reply").unwrap();
    let report = peer::resume(&mut test.caller, "first", Some(&test.key)).unwrap();
    assert!(matches!(report, CallReport::Owner(_)));
    assert_eq!(worker.join().unwrap(), vec![true, true]);
    assert_eq!(test.value(), 26);
}
#[test]
fn authentication_failure_cannot_create_owner_work_and_retry_never_turns_missing_receipt_into_accept()
 {
    let mut test = Test::new();
    let server = test.server(1000);
    let endpoint = test.endpoint(server.address().unwrap());
    let worker = background(server, 2);
    assert_eq!(
        peer::call_once(
            &mut test.caller,
            &endpoint,
            &test.call,
            &PeerKey::from_bytes([9; 32])
        )
        .unwrap(),
        CallReport::Unresolved
    );
    assert_eq!(test.value(), 19);
    assert_eq!(
        peer::resume(&mut test.caller, "first", Some(&test.key)).unwrap(),
        CallReport::Unresolved
    );
    let result = worker.join().unwrap();
    assert!(!result[0] && result[1]);
    assert_eq!(test.value(), 19);
    let saved = test.owner.load().unwrap();
    assert_eq!(saved.snapshot.restore().unwrap().invocations().count(), 0);
}
#[test]
fn native_owner_and_caller_recover_lost_reply_after_revocation_without_sdk_or_duplicate_mutation() {
    let mut test = Test::new();
    let marker = test.dir.join("before-write");
    let mut server = Server::start(
        &test,
        "127.0.0.1:0".parse().unwrap(),
        1000,
        1,
        marker.to_str().unwrap(),
    );
    let address = server.address;
    let endpoint = test.endpoint(address);
    let child = Command::new(env!("CARGO_BIN_EXE_renraku-vm"))
        .args([
            "peer-call",
            test.dir.join("caller.db").to_str().unwrap(),
            &endpoint.owner.to_string(),
            &endpoint.node.to_string(),
            "1",
            &address.to_string(),
            test.dir.join("key").to_str().unwrap(),
            test.dir.join("call.json").to_str().unwrap(),
        ])
        .env("PATH", "/no-peer-sdk")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(4);
    while !marker.exists() {
        assert!(Instant::now() < deadline);
        thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(test.value(), 26);
    server.child.kill().unwrap();
    server.child.wait().unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());
    let unresolved: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(unresolved["state"], "unresolved");
    placement::revoke(&mut test.owner, test.call.grant).unwrap();
    test.collect();
    fs::remove_file(test.dir.join("call.json")).unwrap();
    let mut server = Server::start(&test, address, 1000, 1, "-");
    let first = cli(&[
        "peer-resume",
        test.dir.join("caller.db").to_str().unwrap(),
        "first",
        test.dir.join("key").to_str().unwrap(),
    ]);
    assert_eq!(first["reply"]["state"], "completed");
    assert_eq!(first["reply"]["value"]["value"], 26);
    server.finish();
    fs::remove_file(test.dir.join("key")).unwrap();
    assert_eq!(
        cli(&[
            "peer-resume",
            test.dir.join("caller.db").to_str().unwrap(),
            "first"
        ]),
        first
    );
    assert_eq!(test.value(), 26);
}
#[test]
fn missing_authoritative_owner_ledger_stays_unresolved_after_an_acknowledged_pending_call() {
    let mut test = Test::new();
    let connection = Connection::open(test.dir.join("owner.db")).unwrap();
    let row: (i64, Vec<u8>, Vec<u8>) = connection
        .query_row(
            "SELECT generation,digest,payload FROM checkpoint",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    let server = test.server(0);
    let endpoint = test.endpoint(server.address().unwrap());
    let worker = background(server, 2);
    assert!(matches!(
        peer::call_once(&mut test.caller, &endpoint, &test.call, &test.key).unwrap(),
        CallReport::Owner(_)
    ));
    connection
        .execute(
            "UPDATE checkpoint SET generation=?1,digest=?2,payload=?3",
            rusqlite::params![row.0, row.1, row.2],
        )
        .unwrap();
    assert_eq!(
        peer::resume(&mut test.caller, "first", Some(&test.key)).unwrap(),
        CallReport::Unresolved
    );
    assert_eq!(worker.join().unwrap(), vec![true, true]);
    assert_eq!(test.value(), 19);
    let saved = test.owner.load().unwrap();
    assert_eq!(saved.snapshot.restore().unwrap().invocations().count(), 0);
}
#[test]
fn wire_call_codec_is_lossless_for_i64_utf16_and_rejects_unknown_or_oversized_values() {
    let test = Test::new();
    let mut call = test.call.clone();
    call.arguments = vec![
        Argument::I64(i64::MIN),
        Argument::I64(i64::MAX),
        Argument::Text(vec![0, 0xd800, 0xdc00, 0xdfff]),
        Argument::Null,
    ];
    let bytes = peer::encode_call(&call).unwrap();
    assert_eq!(peer::decode_call(&bytes).unwrap(), call);
    let mut value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(value["arguments"][0]["value"], i64::MIN.to_string());
    value["extra"] = true.into();
    assert!(peer::decode_call(&serde_json::to_vec(&value).unwrap()).is_err());
    call.arguments = vec![Argument::Text(vec![0; 4097])];
    assert!(peer::encode_call(&call).is_err());
    assert!(peer::decode_call(&vec![b' '; peer::MAX_FRAME + 1]).is_err());
}
#[test]
#[cfg(unix)]
fn keys_are_private_nonsymlink_files_and_public_bindings_or_duplicate_peer_keys_reject() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let test = Test::new();
    let path = test.dir.join("key");
    assert!(!format!("{:?}", test.key).contains("165"));
    PeerKey::load_private(&path).unwrap();
    symlink(&path, test.dir.join("key-link")).unwrap();
    assert!(PeerKey::load_private(test.dir.join("key-link")).is_err());
    fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
    assert!(PeerKey::load_private(&path).is_err());
    assert!(
        OwnerServer::bind(
            test.dir.join("owner.db"),
            test.context(),
            "0.0.0.0:0".parse().unwrap(),
            vec![(test.principal, test.key.clone())],
            1000
        )
        .is_err()
    );
    assert!(
        OwnerServer::bind(
            test.dir.join("owner.db"),
            test.context(),
            "127.0.0.1:0".parse().unwrap(),
            vec![
                (test.principal, test.key.clone()),
                (UniverseId::fresh(), test.key.clone())
            ],
            1000
        )
        .is_err()
    );
}

#[test]
fn actual_wire_calls_preserve_i64_extrema_and_exact_utf16_null_char_and_boolean_values() {
    let mut test = Test::new();
    let saved = test.owner.load().unwrap();
    let p = &saved.snapshot.program;
    let vm = saved.snapshot.restore().unwrap();
    let ty = p.find_type("Renraku.Tests.ExportPolicyHarness").unwrap();
    let subject = vm.static_fields()[&p
        .fields()
        .find(|f| f.owner == ty && f.name == "Saved")
        .unwrap()
        .id]
        .referenced_object()
        .unwrap();
    placement::register_object(&mut test.owner, subject, PlacementPolicy::pinned(test.node))
        .unwrap();
    let cases = [
        ("Wide", Argument::I64(i64::MIN)),
        ("Wide", Argument::I64(i64::MAX)),
        ("Echo", Argument::Text(vec![0, 0xd800, 0xdc00, 0xdfff])),
        ("Echo", Argument::Null),
        ("Unit", Argument::I32(0xd800)),
        ("Flag", Argument::I32(1)),
    ];
    let server = test.server(1000);
    let endpoint = test.endpoint(server.address().unwrap());
    let worker = background(server, cases.len());
    for (index, (name, argument)) in cases.into_iter().enumerate() {
        let binding = p
            .method(
                p.entry(&format!("Renraku.Tests.ExportSubject::{name}"))
                    .unwrap(),
            )
            .unwrap()
            .binding;
        let grant = GrantId::fresh();
        placement::export_method(&mut test.owner, grant, test.principal, subject, binding).unwrap();
        let mut call = test.call.clone();
        call.operation = format!("values-{index}");
        call.object = subject;
        call.method = binding;
        call.grant = grant;
        call.arguments = vec![argument.clone()];
        let CallReport::Owner(reply) =
            peer::call_once(&mut test.caller, &endpoint, &call, &test.key).unwrap()
        else {
            panic!()
        };
        assert_eq!(reply.state, ReplyState::Completed(Some(argument)));
        test.collect();
        assert_eq!(
            peer::resume(&mut test.caller, &call.operation, None).unwrap(),
            CallReport::Owner(reply)
        );
    }
    assert!(worker.join().unwrap().iter().all(|s| *s));
    assert_eq!(test.value(), 19);
}
#[test]
fn caller_journal_rejects_orphan_markers_changed_bindings_and_repaired_digest_corruption() {
    use sha2::{Digest, Sha256};
    let mut test = Test::new();
    let server = test.server(1000);
    let endpoint = test.endpoint(server.address().unwrap());
    let worker = background(server, 1);
    peer::call_once(&mut test.caller, &endpoint, &test.call, &test.key).unwrap();
    worker.join().unwrap();
    let saved = test.caller.load().unwrap();
    let vm = saved.snapshot.restore().unwrap();
    let name = "renraku.peer-v1/call/first";
    let id = vm.heap().roots()[name];
    let text = String::from_utf16(vm.heap().object(id).unwrap().string_units().unwrap()).unwrap();
    let mut corrupted = saved.snapshot.restore().unwrap();
    corrupted.remove_root(name);
    assert!(test.caller.save(&corrupted).is_err());
    let mut corrupted = saved.snapshot.restore().unwrap();
    corrupted.set_root("renraku.peer-v1/unknown", id).unwrap();
    assert!(test.caller.save(&corrupted).is_err());
    let changed = text.replacen("\"format\":1", "\"format\":2", 1);
    assert_ne!(text, changed);
    let mut corrupted = saved.snapshot.restore().unwrap();
    let bad = corrupted
        .new_string(changed.encode_utf16().collect())
        .unwrap();
    corrupted.set_root(name, bad).unwrap();
    assert!(test.caller.save(&corrupted).is_err());
    let db = Connection::open(test.dir.join("caller.db")).unwrap();
    let mut payload: Vec<u8> = db
        .query_row("SELECT payload FROM checkpoint", [], |r| r.get(0))
        .unwrap();
    let bytes: Vec<u8> = text.encode_utf16().flat_map(u16::to_le_bytes).collect();
    let changed: Vec<u8> = changed.encode_utf16().flat_map(u16::to_le_bytes).collect();
    let offset = payload
        .windows(bytes.len())
        .position(|b| b == bytes)
        .unwrap();
    payload[offset..offset + bytes.len()].copy_from_slice(&changed);
    db.execute(
        "UPDATE checkpoint SET payload=?1,digest=?2",
        rusqlite::params![payload, Sha256::digest(&payload).as_slice()],
    )
    .unwrap();
    assert!(test.caller.load().is_err());
}
