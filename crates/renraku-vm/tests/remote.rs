#[allow(dead_code)]
mod common;
use renraku_vm::{
    checkpoint::CheckpointStore,
    execution::{Continuation, ExecutionStatus},
    identity::{GrantId, NodeId, ObjectId, UniverseId},
    invocation::InvocationState,
    owner::ReplyState,
    peer::{CallReport, Endpoint, OwnerServer, PeerKey},
    placement::{self, OwnerContext, PlacementPolicy},
    program::Program,
    remote,
    runtime::{RunOutcome, Vm},
    value::Value,
};
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
    grant: GrantId,
    key: PeerKey,
    owner_id: UniverseId,
}
impl Test {
    fn new(subject: bool, method: &str) -> Self {
        Self::kind(u8::from(subject), method)
    }
    fn kind(kind: u8, method: &str) -> Self {
        let subject = kind == 1;
        let f = common::fixtures();
        let dir = f.output.join(format!("remote-{}", UniverseId::fresh()));
        fs::create_dir(&dir).unwrap();
        let ot = fs::read(f.output.join("Renraku.OwnerTests.dll")).unwrap();
        let profile = fs::read(f.output.join("Renraku.RemoteOwnerTests.dll")).unwrap();
        let op = Program::load(&[&f.core, &f.guest, &ot, &profile]).unwrap();
        let mut vm = Vm::new(&op);
        let init = if kind == 2 {
            "Renraku.RemoteOwnerTests.Harness::Initialize"
        } else if subject {
            "Renraku.Tests.OwnerInvocationHarness::Initialize"
        } else {
            "Renraku.Guest.Simulation::SaveObject"
        };
        vm.start(op.entry(init).unwrap(), vec![]).unwrap();
        vm.run(1000).unwrap();
        let ty = op
            .find_type(if kind == 2 {
                "Renraku.RemoteOwnerTests.Harness"
            } else if subject {
                "Renraku.Tests.OwnerInvocationHarness"
            } else {
                "Renraku.Guest.Simulation"
            })
            .unwrap();
        let field = op
            .fields()
            .find(|f| f.owner == ty && f.name == "Saved")
            .unwrap()
            .id;
        let object = vm.static_fields()[&field].referenced_object().unwrap();
        let owner_id = vm.universe_id();
        let mut owner = CheckpointStore::open(dir.join("owner.db")).unwrap();
        owner.save(&vm).unwrap();
        // Entirely independent registry/core IDs. No Counter declaration is installed here.
        let cp = Program::load(&[
            &f.core,
            &fs::read(f.output.join("Renraku.RemoteTests.dll")).unwrap(),
        ])
        .unwrap();
        let vm = Vm::new(&cp);
        let principal = vm.universe_id();
        assert_ne!(op.find_type("System.String"), cp.find_type("System.String"));
        assert!(cp.find_type("Renraku.Guest.Counter").is_err());
        let mut caller = CheckpointStore::open(dir.join("caller.db")).unwrap();
        caller.save(&vm).unwrap();
        let node = NodeId::fresh();
        placement::initialize(&mut owner, node, 1).unwrap();
        placement::register_object(&mut owner, object, PlacementPolicy::pinned(node)).unwrap();
        let owner_type = if kind == 2 {
            "Renraku.RemoteOwnerTests.Subject"
        } else if subject {
            "Renraku.Tests.OwnerSubject"
        } else {
            "Renraku.Guest.Counter"
        };
        let binding = op
            .method(op.entry(&format!("{owner_type}::{method}")).unwrap())
            .unwrap()
            .binding;
        let grant = GrantId::fresh();
        placement::export_method(&mut owner, grant, principal, object, binding).unwrap();
        let key = PeerKey::from_bytes([0x73; 32]);
        key.write_new(dir.join("key")).unwrap();
        Self {
            dir,
            owner,
            caller,
            node,
            principal,
            object,
            grant,
            key,
            owner_id,
        }
    }
    fn server(&self, budget: u64) -> OwnerServer {
        OwnerServer::bind(
            self.dir.join("owner.db"),
            OwnerContext {
                node: self.node,
                epoch: 1,
                resources: BTreeSet::new(),
            },
            "127.0.0.1:0".parse().unwrap(),
            vec![(self.principal, self.key.clone())],
            budget,
        )
        .unwrap()
    }
    fn endpoint(&self, address: SocketAddr) -> Endpoint {
        Endpoint {
            owner: self.owner_id,
            node: self.node,
            epoch: 1,
            address,
        }
    }
    fn import(&mut self, address: SocketAddr, ty: &str, method: &str) -> ObjectId {
        let saved = self.caller.load().unwrap();
        let p = &saved.snapshot.program;
        let binding = p
            .method(
                p.entry(&format!("Renraku.RemoteTests.{ty}::{method}"))
                    .unwrap(),
            )
            .unwrap()
            .binding;
        {
            let endpoint = self.endpoint(address);
            remote::import_method(
                &mut self.owner,
                &mut self.caller,
                &endpoint,
                self.grant,
                binding,
            )
            .unwrap()
        }
    }
    fn start(&mut self, request: &str, name: &str, args: Vec<Value>) {
        let saved = self.caller.load().unwrap();
        let mut vm = saved.snapshot.restore().unwrap();
        let entry = saved
            .snapshot
            .program
            .entry(&format!("Renraku.RemoteTests.Caller::{name}"))
            .unwrap();
        vm.invoke_once(&mut self.caller, request, entry, args, 0, 1)
            .unwrap();
    }
    fn cut_to_wait(&mut self) -> ObjectId {
        for _ in 0..1000 {
            let saved = self.caller.load().unwrap();
            let mut vm = saved.snapshot.restore().unwrap();
            if let ExecutionStatus::WaitingRemote { request } = vm.context().status {
                return request;
            }
            assert!(matches!(
                vm.run(1).unwrap(),
                RunOutcome::Paused | RunOutcome::WaitingRemote(_)
            ));
            vm.collect_garbage().unwrap();
            let context = vm.context().clone();
            self.caller.save(&vm).unwrap();
            let next = self.caller.load().unwrap();
            assert_eq!(*next.snapshot.restore().unwrap().context(), context);
        }
        panic!("no wait")
    }
    fn finish(&mut self) -> Value {
        let saved = self.caller.load().unwrap();
        let mut vm = saved.snapshot.restore().unwrap();
        let result = vm.run_checkpointed(&mut self.caller, 10000, 1).unwrap();
        let RunOutcome::Completed(Some(v)) = result else {
            panic!("{result:?}")
        };
        v
    }
    fn value(&mut self) -> i32 {
        let saved = self.owner.load().unwrap();
        let vm = saved.snapshot.restore().unwrap();
        let obj = vm.heap().object(self.object).unwrap();
        let field = saved
            .snapshot
            .program
            .fields()
            .find(|f| f.owner == obj.type_id() && f.name == "Value")
            .unwrap();
        let Value::I32(n) = obj.fields()[&field.id] else {
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
fn background(server: OwnerServer, count: usize) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        for _ in 0..count {
            server.serve_one().unwrap();
        }
    })
}
#[test]
fn cil_instance_call_parks_exact_frames_with_distinct_registries_and_wakes_once() {
    let mut t = Test::new(false, "Add");
    let server = t.server(1000);
    let address = server.address().unwrap();
    let h = t.import(address, "CounterPort", "Add");
    assert_eq!(t.import(address, "CounterPort", "Add"), h);
    t.start("first", "Run", vec![Value::Reference(h), Value::I32(7)]);
    let request = t.cut_to_wait();
    let saved = t.caller.load().unwrap();
    let vm = saved.snapshot.restore().unwrap();
    let context = vm.context().clone();
    assert_eq!(context.frames.len(), 2);
    assert!(
        matches!(context.frames.last().unwrap().continuation,Continuation::AwaitRemote{request:r,..} if r==request)
    );
    let execution = context.id;
    assert!(vm.effects().next().is_none());
    assert_eq!(
        remote::drive(&mut t.caller, None).unwrap(),
        CallReport::Unresolved
    );
    let saved = t.caller.load().unwrap();
    assert_eq!(*saved.snapshot.restore().unwrap().context(), context);
    let work = background(server, 2);
    let r = remote::drive(&mut t.caller, Some(&t.key)).unwrap();
    assert!(
        matches!(r,CallReport::Owner(ref r) if r.state==ReplyState::Completed(Some(placement::Argument::I32(26))))
    );
    assert_eq!(t.finish(), Value::I32(27));
    assert_eq!(t.value(), 26);
    t.start("second", "Run", vec![Value::Reference(h), Value::I32(-3)]);
    t.cut_to_wait();
    remote::drive(&mut t.caller, Some(&t.key)).unwrap();
    assert_eq!(t.finish(), Value::I32(24));
    work.join().unwrap();
    t.collect();
    assert_eq!(t.value(), 23);
    let saved = t.caller.load().unwrap();
    let mut vm = saved.snapshot.restore().unwrap();
    let entry = saved
        .snapshot
        .program
        .entry("Renraku.RemoteTests.Caller::Run")
        .unwrap();
    let receipt = vm
        .invoke_once(
            &mut t.caller,
            "first",
            entry,
            vec![Value::Reference(h), Value::I32(7)],
            1000,
            1,
        )
        .unwrap();
    assert_eq!(receipt.execution, execution);
    assert_eq!(
        receipt.state,
        InvocationState::Completed {
            result: Some(Value::I32(27))
        }
    );
    assert_eq!(t.value(), 23);
    assert_eq!(
        vm.heap()
            .objects()
            .filter(|o| o.type_id() == vm.heap().object(h).unwrap().type_id())
            .count(),
        1
    );
}
#[test]
fn repeated_call_site_mints_unique_durable_operations_across_loop_iterations() {
    let mut t = Test::new(false, "Add");
    let server = t.server(1000);
    let h = t.import(server.address().unwrap(), "CounterPort", "Add");
    let work = background(server, 3);
    t.start("loop", "Repeat", vec![Value::Reference(h), Value::I32(3)]);
    let mut ids = BTreeSet::new();
    for _ in 0..3 {
        ids.insert(t.cut_to_wait());
        remote::drive(&mut t.caller, Some(&t.key)).unwrap();
    }
    assert_eq!(ids.len(), 3);
    assert_eq!(t.finish(), Value::I32(22));
    assert_eq!(t.value(), 22);
    work.join().unwrap();
    let saved = t.caller.load().unwrap();
    let vm = saved.snapshot.restore().unwrap();
    assert_eq!(
        vm.heap()
            .roots()
            .keys()
            .filter(|n| n.starts_with("renraku.peer-v1/call/"))
            .count(),
        3
    );
}
#[test]
fn caller_reply_and_continuation_commit_together_after_failed_save() {
    let mut t = Test::new(false, "Add");
    let server = t.server(1000);
    let h = t.import(server.address().unwrap(), "CounterPort", "Add");
    let work = background(server, 2);
    t.start("first", "Run", vec![Value::Reference(h), Value::I32(7)]);
    t.cut_to_wait();
    let db = rusqlite::Connection::open(t.dir.join("caller.db")).unwrap();
    db.execute_batch("CREATE TRIGGER fail_reply BEFORE UPDATE ON checkpoint WHEN instr(NEW.payload,CAST('renraku.peer-v1/reply/' AS BLOB))>0 BEGIN SELECT RAISE(ABORT,'fail remote wake'); END;").unwrap();
    assert!(remote::drive(&mut t.caller, Some(&t.key)).is_err());
    assert_eq!(t.value(), 26);
    let saved = t.caller.load().unwrap();
    let vm = saved.snapshot.restore().unwrap();
    assert!(matches!(
        vm.context().status,
        ExecutionStatus::WaitingRemote { .. }
    ));
    assert!(
        !vm.heap()
            .roots()
            .keys()
            .any(|n| n.starts_with("renraku.peer-v1/reply/"))
    );
    db.execute_batch("DROP TRIGGER fail_reply").unwrap();
    remote::drive(&mut t.caller, Some(&t.key)).unwrap();
    assert_eq!(t.finish(), Value::I32(27));
    assert_eq!(t.value(), 26);
    work.join().unwrap();
}
#[test]
fn bound_contract_construction_null_and_bad_import_shapes_reject_without_local_fallback() {
    let mut t = Test::new(false, "Add");
    let server = t.server(1000);
    let address = server.address().unwrap();
    for ty in ["FieldPort", "ExtraPort", "UnsealedPort", "MismatchPort"] {
        let saved = t.caller.load().unwrap();
        let p = &saved.snapshot.program;
        let method = p
            .method(p.entry(&format!("Renraku.RemoteTests.{ty}::Add")).unwrap())
            .unwrap()
            .binding;
        let before = t.caller.generation();
        let endpoint = t.endpoint(address);
        assert!(
            remote::import_method(&mut t.owner, &mut t.caller, &endpoint, t.grant, method).is_err()
        );
        assert_eq!(t.caller.generation(), before);
    }
    let h = t.import(address, "CounterPort", "Add");
    let saved = t.caller.load().unwrap();
    let mut vm = saved.snapshot.restore().unwrap();
    let entry = saved
        .snapshot
        .program
        .entry("Renraku.RemoteTests.Caller::Construct")
        .unwrap();
    vm.start(entry, vec![]).unwrap();
    assert!(
        vm.run(100)
            .unwrap_err()
            .to_string()
            .contains("construction")
    );
    assert_eq!(
        vm.heap()
            .objects()
            .filter(|o| o.type_id() == vm.heap().object(h).unwrap().type_id())
            .count(),
        1
    );
    t.caller.save(&vm).unwrap();
    t.start("null", "Run", vec![Value::Null, Value::I32(7)]);
    let saved = t.caller.load().unwrap();
    let mut vm = saved.snapshot.restore().unwrap();
    assert!(
        vm.run(1000)
            .unwrap_err()
            .to_string()
            .contains("non-null handle")
    );
    assert_eq!(
        vm.context().frames.last().unwrap().evaluation_stack,
        vec![Value::Null, Value::I32(7)]
    );
    t.caller.save(&vm).unwrap();
    assert_eq!(t.value(), 19);
}
#[test]
fn owner_terminal_fault_keeps_caller_prior_mutation_and_frozen_wait_receipt() {
    let mut t = Test::new(true, "Fault");
    let server = t.server(1000);
    let h = t.import(server.address().unwrap(), "FaultPort", "Fault");
    let work = background(server, 1);
    t.start("fault", "Fault", vec![Value::Reference(h)]);
    t.cut_to_wait();
    remote::drive(&mut t.caller, Some(&t.key)).unwrap();
    work.join().unwrap();
    assert_eq!(t.value(), 20);
    t.collect();
    let saved = t.caller.load().unwrap();
    let vm = saved.snapshot.restore().unwrap();
    assert!(matches!(
        vm.context().status,
        ExecutionStatus::Faulted { .. }
    ));
    assert!(matches!(
        vm.invocation("fault").unwrap().state,
        InvocationState::Faulted { .. }
    ));
    assert!(matches!(
        vm.context().frames.last().unwrap().continuation,
        Continuation::AwaitRemote { .. }
    ));
    let f = saved
        .snapshot
        .program
        .fields()
        .find(|f| f.name == "Before")
        .unwrap();
    assert_eq!(vm.static_fields()[&f.id], Value::I32(1));
}
#[test]
fn string_reply_allocation_preserves_exact_units_and_null_in_the_callers_core() {
    for text in [None, Some(vec![0, 0xd800, 65, 0xdfff])] {
        let mut t = Test::new(true, "Echo");
        let server = t.server(1000);
        let h = t.import(server.address().unwrap(), "EchoPort", "Echo");
        let work = background(server, 1);
        let saved = t.caller.load().unwrap();
        let mut vm = saved.snapshot.restore().unwrap();
        let arg = if let Some(units) = &text {
            Value::Reference(vm.new_string(units.clone()).unwrap())
        } else {
            Value::Null
        };
        if let Value::Reference(id) = arg {
            vm.set_root("original-input", id).unwrap();
        }
        t.caller.save(&vm).unwrap();
        t.start("echo", "Echo", vec![Value::Reference(h), arg]);
        t.cut_to_wait();
        remote::drive(&mut t.caller, Some(&t.key)).unwrap();
        let v = t.finish();
        work.join().unwrap();
        let saved = t.caller.load().unwrap();
        let vm = saved.snapshot.restore().unwrap();
        if let Some(units) = text {
            let Value::Reference(id) = v else { panic!() };
            assert_eq!(
                vm.heap().object(id).unwrap().string_units(),
                Some(units.as_slice())
            );
            assert_eq!(
                vm.heap().object(id).unwrap().type_id(),
                saved.snapshot.program.find_type("System.String").unwrap()
            );
        } else {
            assert_eq!(v, Value::Null);
        }
    }
}
struct Native {
    child: Child,
    address: SocketAddr,
}
impl Native {
    fn start(t: &Test, address: SocketAddr, marker: &str) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_renraku-checkpoint-probe"))
            .args([
                "peer-server",
                t.dir.join("owner.db").to_str().unwrap(),
                &t.node.to_string(),
                &t.principal.to_string(),
                t.dir.join("key").to_str().unwrap(),
                &address.to_string(),
                "1000",
                "1",
                marker,
            ])
            .env("PATH", "/no-remote-sdk")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let mut line = String::new();
        BufReader::new(child.stdout.take().unwrap())
            .read_line(&mut line)
            .unwrap();
        let value: serde_json::Value = serde_json::from_str(&line).unwrap();
        Self {
            child,
            address: value["listening"].as_str().unwrap().parse().unwrap(),
        }
    }
}
impl Drop for Native {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
#[test]
fn native_guest_wait_survives_owner_kill_reply_loss_revocation_gc_and_sdk_free_resume() {
    let mut t = Test::new(false, "Add");
    let marker = t.dir.join("committed-before-reply");
    let mut server = Native::start(&t, "127.0.0.1:0".parse().unwrap(), marker.to_str().unwrap());
    let address = server.address;
    let h = t.import(address, "CounterPort", "Add");
    t.start("native", "Run", vec![Value::Reference(h), Value::I32(7)]);
    let request = t.cut_to_wait();
    t.collect();
    let report = Command::new(env!("CARGO_BIN_EXE_renraku-vm"))
        .args([
            "invocation",
            t.dir.join("caller.db").to_str().unwrap(),
            "native",
        ])
        .env("PATH", "/no-remote-sdk")
        .output()
        .unwrap();
    assert!(report.status.success());
    let report: serde_json::Value = serde_json::from_slice(&report.stdout).unwrap();
    assert_eq!(report["format"], 3);
    assert_eq!(report["waitingRemote"]["request"], request.to_string());
    let mut child = Command::new(env!("CARGO_BIN_EXE_renraku-vm"))
        .args([
            "remote-drive",
            t.dir.join("caller.db").to_str().unwrap(),
            t.dir.join("key").to_str().unwrap(),
        ])
        .env("PATH", "/no-remote-sdk")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while !marker.exists() {
        assert!(Instant::now() < deadline);
        thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(t.value(), 26);
    child.kill().unwrap();
    assert!(!child.wait().unwrap().success());
    server.child.kill().unwrap();
    server.child.wait().unwrap();
    let saved = t.caller.load().unwrap();
    assert_eq!(
        saved.snapshot.restore().unwrap().context().status,
        ExecutionStatus::WaitingRemote { request }
    );
    placement::revoke(&mut t.owner, t.grant).unwrap();
    t.collect();
    let mut server = Native::start(&t, address, "-");
    let output = Command::new(env!("CARGO_BIN_EXE_renraku-vm"))
        .args([
            "remote-drive",
            t.dir.join("caller.db").to_str().unwrap(),
            t.dir.join("key").to_str().unwrap(),
        ])
        .env("PATH", "/no-remote-sdk")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(server.child.wait().unwrap().success());
    fs::remove_file(t.dir.join("key")).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_renraku-checkpoint-probe"))
        .args(["remote-run", t.dir.join("caller.db").to_str().unwrap()])
        .env("PATH", "/no-remote-sdk")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["outcome"], "Completed(Some(I32(27)))");
    assert_eq!(t.value(), 26);
    assert_eq!(t.finish(), Value::I32(27));
}
#[test]
fn typed_wakeups_preserve_i64_boolean_char_and_void_without_owner_core_ids() {
    let cases = [
        ("Wide", Value::I64(i64::MIN)),
        ("Wide", Value::I64(i64::MAX)),
        ("Wide", Value::I64(-1)),
        ("Flag", Value::I32(0)),
        ("Flag", Value::I32(1)),
        ("Unit", Value::I32(0)),
        ("Unit", Value::I32(0xd800)),
        ("Unit", Value::I32(0xdfff)),
        ("Unit", Value::I32(65535)),
        ("Void", Value::I32(73)),
    ];
    for (name, value) in cases {
        let mut t = Test::kind(2, name);
        let server = t.server(1000);
        let h = t.import(server.address().unwrap(), &format!("{name}Port"), name);
        let work = background(server, 1);
        let mut args = vec![Value::Reference(h)];
        if name != "Void" {
            args.push(value);
        }
        t.start("typed", name, args);
        t.cut_to_wait();
        remote::drive(&mut t.caller, Some(&t.key)).unwrap();
        assert_eq!(t.finish(), value);
        work.join().unwrap();
        let saved = t.owner.load().unwrap();
        let vm = saved.snapshot.restore().unwrap();
        let object = vm.heap().object(t.object).unwrap();
        let field = saved
            .snapshot
            .program
            .fields()
            .find(|f| f.owner == object.type_id() && f.name == "Mutations")
            .unwrap();
        assert_eq!(object.fields()[&field.id], Value::I32(1));
    }
}
#[test]
fn remote_fault_save_failure_and_old_fault_replay_never_repeat_owner_mutation() {
    let mut t = Test::new(true, "Fault");
    let server = t.server(1000);
    let h = t.import(server.address().unwrap(), "FaultPort", "Fault");
    let work = background(server, 2);
    t.start("fault", "Fault", vec![Value::Reference(h)]);
    t.cut_to_wait();
    let db = rusqlite::Connection::open(t.dir.join("caller.db")).unwrap();
    db.execute_batch("CREATE TRIGGER fail_fault BEFORE UPDATE ON checkpoint WHEN instr(NEW.payload,CAST('renraku.peer-v1/reply/' AS BLOB))>0 BEGIN SELECT RAISE(ABORT,'fail remote fault'); END;").unwrap();
    assert!(remote::drive(&mut t.caller, Some(&t.key)).is_err());
    assert_eq!(t.value(), 20);
    let saved = t.caller.load().unwrap();
    assert!(matches!(
        saved.snapshot.restore().unwrap().context().status,
        ExecutionStatus::WaitingRemote { .. }
    ));
    db.execute_batch("DROP TRIGGER fail_fault").unwrap();
    remote::drive(&mut t.caller, Some(&t.key)).unwrap();
    work.join().unwrap();
    t.collect();
    let saved = t.caller.load().unwrap();
    let mut vm = saved.snapshot.restore().unwrap();
    let original = vm.invocation("fault").unwrap().clone();
    let entry = saved
        .snapshot
        .program
        .entry("Renraku.RemoteTests.Caller::Zero")
        .unwrap();
    vm.start(entry, vec![]).unwrap();
    assert_eq!(
        vm.run(1000).unwrap(),
        RunOutcome::Completed(Some(Value::I32(123)))
    );
    vm.collect_garbage().unwrap();
    t.caller.save(&vm).unwrap();
    let entry = saved
        .snapshot
        .program
        .entry("Renraku.RemoteTests.Caller::Fault")
        .unwrap();
    let receipt = vm
        .invoke_once(
            &mut t.caller,
            "fault",
            entry,
            vec![Value::Reference(h)],
            1000,
            1,
        )
        .unwrap();
    assert_eq!(receipt, original);
    assert_eq!(t.value(), 20);
}
#[test]
fn revoked_new_calls_and_wrong_credentials_keep_the_same_durable_wait() {
    for revoke in [false, true] {
        let mut t = Test::new(false, "Add");
        let server = t.server(1000);
        let h = t.import(server.address().unwrap(), "CounterPort", "Add");
        if revoke {
            placement::revoke(&mut t.owner, t.grant).unwrap();
        }
        t.start("denied", "Run", vec![Value::Reference(h), Value::I32(7)]);
        let request = t.cut_to_wait();
        let worker = thread::spawn(move || {
            let _ = server.serve_one();
            let _ = server.serve_one();
        });
        let wrong = PeerKey::from_bytes([0x55; 32]);
        let key = if revoke { &t.key } else { &wrong };
        assert_eq!(
            remote::drive(&mut t.caller, Some(key)).unwrap(),
            CallReport::Unresolved
        );
        assert_eq!(
            remote::drive(&mut t.caller, Some(&t.key)).unwrap(),
            CallReport::Unresolved
        );
        worker.join().unwrap();
        assert_eq!(t.value(), 19);
        let saved = t.caller.load().unwrap();
        let vm = saved.snapshot.restore().unwrap();
        assert_eq!(
            vm.context().status,
            ExecutionStatus::WaitingRemote { request }
        );
        assert_eq!(
            vm.heap()
                .roots()
                .keys()
                .filter(|n| n.starts_with("renraku.peer-v1/call/"))
                .count(),
            1
        );
        let saved = t.owner.load().unwrap();
        assert_eq!(saved.snapshot.restore().unwrap().invocations().count(), 0);
    }
}
#[test]
fn oversized_guest_inputs_fail_at_call_before_any_remote_intent_is_created() {
    let mut t = Test::new(true, "Echo");
    let server = t.server(1000);
    let h = t.import(server.address().unwrap(), "EchoPort", "Echo");
    let saved = t.caller.load().unwrap();
    let mut vm = saved.snapshot.restore().unwrap();
    let input = vm.new_string(vec![65; 4097]).unwrap();
    vm.set_root("large-input", input).unwrap();
    t.caller.save(&vm).unwrap();
    t.start(
        "large",
        "Echo",
        vec![Value::Reference(h), Value::Reference(input)],
    );
    let saved = t.caller.load().unwrap();
    let mut vm = saved.snapshot.restore().unwrap();
    assert!(vm.run(1000).unwrap_err().to_string().contains("4096"));
    assert_eq!(
        vm.context().frames.last().unwrap().evaluation_stack,
        vec![Value::Reference(h), Value::Reference(input)]
    );
    assert!(
        !vm.heap()
            .roots()
            .keys()
            .any(|n| n.starts_with("renraku.peer-v1/call/"))
    );
    t.caller.save(&vm).unwrap();
}
#[test]
fn remote_import_roots_validate_on_save_and_repaired_digest_recovery() {
    use sha2::{Digest, Sha256};
    let mut t = Test::new(false, "Add");
    let server = t.server(1000);
    t.import(server.address().unwrap(), "CounterPort", "Add");
    let saved = t.caller.load().unwrap();
    let vm = saved.snapshot.restore().unwrap();
    let (name, id) = vm
        .heap()
        .roots()
        .iter()
        .find(|(n, _)| n.starts_with("renraku.remote-v1/import/"))
        .unwrap();
    let name = name.clone();
    let text = String::from_utf16(vm.heap().object(*id).unwrap().string_units().unwrap()).unwrap();
    let changed = text.replacen("\"format\":1", "\"format\":2", 1);
    assert_ne!(text, changed);
    let mut bad = saved.snapshot.restore().unwrap();
    let record = bad.new_string(changed.encode_utf16().collect()).unwrap();
    bad.set_root(name, record).unwrap();
    assert!(t.caller.save(&bad).is_err());
    let db = rusqlite::Connection::open(t.dir.join("caller.db")).unwrap();
    let mut payload: Vec<u8> = db
        .query_row("SELECT payload FROM checkpoint", [], |r| r.get(0))
        .unwrap();
    let old: Vec<u8> = text.encode_utf16().flat_map(u16::to_le_bytes).collect();
    let new: Vec<u8> = changed.encode_utf16().flat_map(u16::to_le_bytes).collect();
    let offset = payload.windows(old.len()).position(|b| b == old).unwrap();
    payload[offset..offset + old.len()].copy_from_slice(&new);
    db.execute(
        "UPDATE checkpoint SET payload=?1,digest=?2",
        rusqlite::params![payload, Sha256::digest(&payload).as_slice()],
    )
    .unwrap();
    assert!(t.caller.load().is_err());
}
#[test]
fn a_second_remote_call_preserves_the_first_result_below_its_frozen_arguments() {
    let mut t = Test::new(false, "Add");
    let server = t.server(1000);
    let h = t.import(server.address().unwrap(), "CounterPort", "Add");
    let worker = background(server, 2);
    t.start("stack", "Twice", vec![Value::Reference(h), Value::I32(1)]);
    t.cut_to_wait();
    remote::drive(&mut t.caller, Some(&t.key)).unwrap();
    t.cut_to_wait();
    let saved = t.caller.load().unwrap();
    assert_eq!(
        saved
            .snapshot
            .restore()
            .unwrap()
            .context()
            .frames
            .last()
            .unwrap()
            .evaluation_stack,
        vec![Value::I32(20)]
    );
    remote::drive(&mut t.caller, Some(&t.key)).unwrap();
    assert_eq!(t.finish(), Value::I32(41));
    assert_eq!(t.value(), 21);
    worker.join().unwrap();
}
#[test]
fn import_scope_denials_publish_no_handle_or_rebound_identity() {
    let mut t = Test::new(false, "Add");
    let server = t.server(1000);
    let endpoint = t.endpoint(server.address().unwrap());
    let saved = t.caller.load().unwrap();
    let p = &saved.snapshot.program;
    let binding = p
        .method(p.entry("Renraku.RemoteTests.CounterPort::Add").unwrap())
        .unwrap()
        .binding;
    let generation = t.caller.generation();
    for i in 0..4 {
        let mut e = endpoint.clone();
        let mut grant = t.grant;
        match i {
            0 => e.epoch = 2,
            1 => e.node = NodeId::fresh(),
            2 => e.owner = UniverseId::fresh(),
            _ => grant = GrantId::fresh(),
        };
        assert!(remote::import_method(&mut t.owner, &mut t.caller, &e, grant, binding).is_err());
        assert_eq!(t.caller.generation(), generation);
    }
    let os = t.owner.load().unwrap();
    let ovm = os.snapshot.restore().unwrap();
    let original = placement::read(&ovm)
        .unwrap()
        .unwrap()
        .grants()
        .next()
        .unwrap()
        .clone();
    let wrong = GrantId::fresh();
    placement::export_method(
        &mut t.owner,
        wrong,
        UniverseId::fresh(),
        t.object,
        original.method,
    )
    .unwrap();
    assert!(remote::import_method(&mut t.owner, &mut t.caller, &endpoint, wrong, binding).is_err());
    placement::revoke(&mut t.owner, t.grant).unwrap();
    assert!(
        remote::import_method(&mut t.owner, &mut t.caller, &endpoint, t.grant, binding).is_err()
    );
    assert_eq!(t.caller.generation(), generation);
}
#[test]
fn a_waiting_checkpoint_with_unknown_frame_code_is_an_error_even_with_repaired_digest() {
    use sha2::{Digest, Sha256};
    let mut t = Test::new(false, "Add");
    let server = t.server(1000);
    let h = t.import(server.address().unwrap(), "CounterPort", "Add");
    t.start("bad-frame", "Run", vec![Value::Reference(h), Value::I32(7)]);
    t.cut_to_wait();
    let saved = t.caller.load().unwrap();
    let vm = saved.snapshot.restore().unwrap();
    let code = vm.context().frames.last().unwrap().method.code;
    let db = rusqlite::Connection::open(t.dir.join("caller.db")).unwrap();
    let mut payload: Vec<u8> = db
        .query_row("SELECT payload FROM checkpoint", [], |r| r.get(0))
        .unwrap();
    let offset = payload
        .windows(16)
        .rposition(|b| b == code.as_bytes())
        .unwrap();
    payload[offset..offset + 16]
        .copy_from_slice(renraku_vm::identity::CodeVersionId::fresh().as_bytes());
    db.execute(
        "UPDATE checkpoint SET payload=?1,digest=?2",
        rusqlite::params![payload, Sha256::digest(&payload).as_slice()],
    )
    .unwrap();
    assert!(t.caller.load().is_err());
}
#[test]
fn a_busy_owner_does_not_turn_a_saved_guest_operation_into_later_fresh_acceptance() {
    let mut t = Test::new(false, "Add");
    let server = t.server(1000);
    let h = t.import(server.address().unwrap(), "CounterPort", "Add");
    let worker = background(server, 2);
    let saved = t.owner.load().unwrap();
    let mut owner = saved.snapshot.restore().unwrap();
    owner
        .start(
            saved
                .snapshot
                .program
                .entry("Renraku.Guest.Simulation::Run")
                .unwrap(),
            vec![],
        )
        .unwrap();
    t.owner.save(&owner).unwrap();
    t.start("busy", "Run", vec![Value::Reference(h), Value::I32(7)]);
    let request = t.cut_to_wait();
    assert_eq!(
        remote::drive(&mut t.caller, Some(&t.key)).unwrap(),
        CallReport::Unresolved
    );
    let saved = t.owner.load().unwrap();
    let mut owner = saved.snapshot.restore().unwrap();
    owner.run(1000).unwrap();
    t.owner.save(&owner).unwrap();
    assert_eq!(
        remote::drive(&mut t.caller, Some(&t.key)).unwrap(),
        CallReport::Unresolved
    );
    worker.join().unwrap();
    assert_eq!(t.value(), 19);
    let saved = t.caller.load().unwrap();
    assert_eq!(
        saved.snapshot.restore().unwrap().context().status,
        ExecutionStatus::WaitingRemote { request }
    );
}
