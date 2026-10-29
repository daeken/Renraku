#[allow(dead_code)]
mod common;
use renraku_vm::{
    checkpoint::CheckpointStore,
    cil::Op,
    execution::{Continuation, MethodVersion},
    identity::{CodeVersionId, GrantId, NodeId, ObjectId, UniverseId},
    invocation::{InstanceCall, InvocationState},
    owner::{self, OwnerCall, ReplyState},
    placement::{self, Argument, OwnerContext, PlacementPolicy},
    program::Program,
    runtime::Vm,
    value::Value,
};
use rusqlite::Connection;
use std::{collections::BTreeSet, fs, path::PathBuf, process::Command};
struct Test {
    directory: PathBuf,
    store: CheckpointStore,
    receiver: ObjectId,
    counter: ObjectId,
    node: NodeId,
    caller: UniverseId,
}
impl Test {
    fn new() -> Self {
        let directory = common::fixtures()
            .output
            .join(format!("owner-{}", UniverseId::fresh()));
        fs::create_dir(&directory).unwrap();
        let p = Program::load(&[
            &common::fixtures().core,
            &common::fixtures().guest,
            &fs::read(common::fixtures().output.join("Renraku.OwnerTests.dll")).unwrap(),
        ])
        .unwrap();
        let mut vm = Vm::new(&p);
        vm.start(common::entry(&p, "SaveObject"), vec![]).unwrap();
        vm.run(1000).unwrap();
        vm.start(
            p.entry("Renraku.Tests.OwnerInvocationHarness::Initialize")
                .unwrap(),
            vec![],
        )
        .unwrap();
        vm.run(1000).unwrap();
        let receiver = static_root(&vm, &p, "Renraku.Tests.OwnerInvocationHarness");
        let counter = static_root(&vm, &p, "Renraku.Guest.Simulation");
        let mut store = CheckpointStore::open(directory.join("universe.db")).unwrap();
        store.save(&vm).unwrap();
        Self {
            directory,
            store,
            receiver,
            counter,
            node: NodeId::fresh(),
            caller: UniverseId::fresh(),
        }
    }
    fn context(&self) -> OwnerContext {
        OwnerContext {
            node: self.node,
            epoch: 1,
            resources: BTreeSet::new(),
        }
    }
    fn export(&mut self, name: &str, arguments: Vec<Argument>) -> OwnerCall {
        placement::initialize(&mut self.store, self.node, 1).unwrap();
        let saved = self.store.load().unwrap();
        let p = &saved.snapshot.program;
        let target = if name == "Counter::Add" {
            self.counter
        } else {
            self.receiver
        };
        let selector = if name == "Counter::Add" {
            "Renraku.Guest.Counter::Add".into()
        } else {
            format!("Renraku.Tests.OwnerSubject::{name}")
        };
        let method = p.method(p.entry(&selector).unwrap()).unwrap().binding;
        placement::register_object(&mut self.store, target, PlacementPolicy::pinned(self.node))
            .unwrap();
        let grant = GrantId::fresh();
        placement::export_method(&mut self.store, grant, self.caller, target, method).unwrap();
        OwnerCall {
            operation: format!("call-{name}").replace(':', "-"),
            owner: saved.snapshot.restore().unwrap().universe_id(),
            epoch: 1,
            grant,
            object: target,
            method,
            arguments,
        }
    }
    fn collect(&mut self) {
        let saved = self.store.load().unwrap();
        let mut vm = saved.snapshot.restore().unwrap();
        vm.collect_garbage().unwrap();
        self.store.save(&vm).unwrap();
    }
    fn value(&mut self, name: &str) -> Value {
        let saved = self.store.load().unwrap();
        let vm = saved.snapshot.restore().unwrap();
        field(&vm, &saved.snapshot.program, self.receiver, name)
    }
}
impl Drop for Test {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.directory);
    }
}
fn static_root(vm: &Vm<'_>, p: &Program, name: &str) -> ObjectId {
    let owner = p.find_type(name).unwrap();
    vm.static_fields()[&p
        .fields()
        .find(|f| f.owner == owner && f.name == "Saved")
        .unwrap()
        .id]
        .referenced_object()
        .unwrap()
}
fn field(vm: &Vm<'_>, p: &Program, id: ObjectId, name: &str) -> Value {
    let object = vm.heap().object(id).unwrap();
    object.fields()[&p
        .fields()
        .find(|f| f.owner == object.type_id() && f.name == name)
        .unwrap()
        .id]
}
fn method(p: &Program, name: &str) -> MethodVersion {
    p.method(
        p.entry(&format!("Renraku.Tests.OwnerSubject::{name}"))
            .unwrap(),
    )
    .unwrap()
    .binding
}
fn root(call: &OwnerCall, caller: UniverseId) -> String {
    format!("renraku.owner-v1/{caller}/{}", call.operation)
}
#[test]
fn explicit_instance_entries_restore_every_instruction_with_original_receivers_and_static_receipts()
{
    for (name, args, expected) in [
        ("Run", vec![Value::I32(7)], Some(Value::I32(26))),
        ("Fault", vec![Value::I32(0)], None),
        ("Overflow", vec![Value::I32(1)], None),
        ("Null", vec![], Some(Value::Null)),
        ("Void", vec![], None),
    ] {
        let mut test = Test::new();
        let saved = test.store.load().unwrap();
        let p = &saved.snapshot.program;
        let mut vm = saved.snapshot.restore().unwrap();
        let call = InstanceCall {
            method: method(p, name),
            receiver: test.receiver,
            arguments: args,
        };
        assert!(
            vm.start(call.method.method, call.arguments.clone())
                .is_err()
        );
        let intent = vm
            .invoke_instance_once(&mut test.store, "instance", call.clone(), 0, 1)
            .unwrap();
        assert_eq!(intent.arguments[0], Value::Reference(test.receiver));
        assert_eq!(intent.receiver(&vm), Some(test.receiver));
        for _ in 0..1000 {
            let saved = test.store.load().unwrap();
            let mut vm = saved.snapshot.restore().unwrap();
            let before = vm.context().clone();
            vm.collect_garbage().unwrap();
            let result = vm
                .invoke_instance_once(&mut test.store, "instance", call.clone(), 1, 1)
                .unwrap();
            assert_eq!(result.execution, intent.execution);
            assert_eq!(result.method, call.method);
            if result.state != InvocationState::Running {
                if name == "Fault" || name == "Overflow" {
                    assert!(matches!(result.state, InvocationState::Faulted { .. }));
                    assert_eq!(vm.context().frames, before.frames);
                } else {
                    assert_eq!(
                        result.state,
                        InvocationState::Completed { result: expected }
                    );
                }
                let count = vm.instructions_executed();
                assert_eq!(
                    vm.invoke_instance_once(&mut test.store, "instance", call.clone(), 1000, 1)
                        .unwrap(),
                    result
                );
                assert_eq!(vm.instructions_executed(), count);
                break;
            }
        }
        assert_eq!(test.value("Mutations"), Value::I32(1));
        let saved = test.store.load().unwrap();
        let p = &saved.snapshot.program;
        let mut vm = saved.snapshot.restore().unwrap();
        vm.start(
            p.entry("Renraku.Tests.OwnerInvocationHarness::Replace")
                .unwrap(),
            vec![],
        )
        .unwrap();
        vm.run(1000).unwrap();
        vm.invoke_once(
            &mut test.store,
            "old-static",
            common::entry(p, "Arithmetic"),
            vec![],
            1000,
            100,
        )
        .unwrap();
        vm.collect_garbage().unwrap();
        test.store.save(&vm).unwrap();
        assert!(vm.heap().object(test.receiver).is_ok());
        assert_ne!(
            static_root(&vm, p, "Renraku.Tests.OwnerInvocationHarness"),
            test.receiver
        );
        let before = vm.instructions_executed();
        let replay = vm
            .invoke_instance_once(&mut test.store, "instance", call, 1000, 1)
            .unwrap();
        assert_ne!(replay.state, InvocationState::Running);
        assert_eq!(vm.instructions_executed(), before);
    }
}
#[test]
fn owner_acceptance_resumes_after_revocation_and_replays_after_later_mutation_without_new_authority()
 {
    let mut test = Test::new();
    let context = test.context();
    let call = test.export("Counter::Add", vec![Argument::I32(7)]);
    let first = owner::accept(&mut test.store, &context, test.caller, &call, 0).unwrap();
    assert_eq!(first.state, ReplyState::Pending);
    let mut busy = call.clone();
    busy.operation = "busy".into();
    assert!(owner::accept(&mut test.store, &context, test.caller, &busy, 1000).is_err());
    placement::revoke(&mut test.store, call.grant).unwrap();
    assert!(owner::accept(&mut test.store, &context, test.caller, &busy, 1000).is_err());
    let completed = owner::accept(&mut test.store, &context, test.caller, &call, 1000).unwrap();
    assert_eq!(completed.execution, first.execution);
    assert_eq!(
        completed.state,
        ReplyState::Completed(Some(Argument::I32(26)))
    );
    let mut second = test.export("Counter::Add", vec![Argument::I32(-3)]);
    second.operation = "second".into();
    assert_eq!(
        owner::accept(&mut test.store, &context, test.caller, &second, 1000)
            .unwrap()
            .state,
        ReplyState::Completed(Some(Argument::I32(23)))
    );
    test.collect();
    assert_eq!(
        owner::accept(&mut test.store, &context, test.caller, &call, 1000).unwrap(),
        completed
    );
    let saved = test.store.load().unwrap();
    let vm = saved.snapshot.restore().unwrap();
    assert_eq!(
        field(&vm, &saved.snapshot.program, test.counter, "Value"),
        Value::I32(23)
    );
    for variant in 0..5 {
        let mut wrong = call.clone();
        match variant {
            0 => wrong.arguments = vec![Argument::I32(8)],
            1 => wrong.grant = second.grant,
            2 => wrong.object = test.receiver,
            3 => wrong.method.code = CodeVersionId::fresh(),
            _ => wrong.epoch = 2,
        };
        assert!(owner::accept(&mut test.store, &context, test.caller, &wrong, 1000).is_err());
    }
    assert!(owner::accept(&mut test.store, &context, UniverseId::fresh(), &call, 1000).is_err());
    let mut wrong = context.clone();
    wrong.node = NodeId::fresh();
    assert!(owner::accept(&mut test.store, &wrong, test.caller, &call, 1000).is_err());
    let saved = test.store.load().unwrap();
    let mut vm = saved.snapshot.restore().unwrap();
    vm.start(common::entry(&saved.snapshot.program, "Arithmetic"), vec![])
        .unwrap();
    vm.run(1).unwrap();
    test.store.save(&vm).unwrap();
    let frames = vm.context().clone();
    let instructions = vm.instructions_executed();
    assert_eq!(
        owner::accept(&mut test.store, &context, test.caller, &call, 1000).unwrap(),
        completed
    );
    let saved = test.store.load().unwrap();
    let vm = saved.snapshot.restore().unwrap();
    assert_eq!(vm.context(), &frames);
    assert_eq!(vm.instructions_executed(), instructions);
}
#[test]
fn owner_cut_recovery_keeps_frozen_inputs_original_objects_and_fault_continuations() {
    for (name, args) in [
        ("Run", vec![Argument::I32(7)]),
        (
            "Echo",
            vec![Argument::Text(vec![0, 0xd800, 0xdc00, 0xdfff])],
        ),
        ("Fault", vec![Argument::I32(0)]),
        ("Overflow", vec![Argument::I32(1)]),
    ] {
        let mut test = Test::new();
        let context = test.context();
        let call = test.export(name, args.clone());
        let first = owner::accept(&mut test.store, &context, test.caller, &call, 0).unwrap();
        let id = root(&call, test.caller);
        let initial = test.store.load().unwrap();
        let vm = initial.snapshot.restore().unwrap();
        let frozen = vm.invocation(&id).unwrap().arguments.clone();
        let method_id = call.method;
        let mut done = false;
        for _ in 0..1000 {
            test.collect();
            let before = test.store.load().unwrap();
            let before_vm = before.snapshot.restore().unwrap();
            let frames = before_vm.context().frames.clone();
            let reply = owner::accept(&mut test.store, &context, test.caller, &call, 1).unwrap();
            assert_eq!(reply.execution, first.execution);
            assert_eq!(reply.method, method_id);
            let saved = test.store.load().unwrap();
            let vm = saved.snapshot.restore().unwrap();
            assert_eq!(vm.invocation(&id).unwrap().arguments, frozen);
            if reply.state != ReplyState::Pending {
                if matches!(reply.state, ReplyState::Faulted { .. }) {
                    assert_eq!(vm.context().frames, frames);
                }
                if name == "Echo" {
                    assert_eq!(reply.state, ReplyState::Completed(Some(args[0].clone())));
                }
                assert_eq!(
                    owner::accept(&mut test.store, &context, test.caller, &call, 1000).unwrap(),
                    reply
                );
                done = true;
                break;
            }
        }
        assert!(done);
        assert_eq!(test.value("Mutations"), Value::I32(1));
    }
}
#[test]
fn failed_acceptance_and_terminal_saves_recover_without_repeating_committed_mutations() {
    let mut test = Test::new();
    let context = test.context();
    let call = test.export("Fault", vec![Argument::I32(0)]);
    let connection = Connection::open(test.directory.join("universe.db")).unwrap();
    let generation = test.store.generation();
    connection.execute_batch("CREATE TRIGGER fail_owner BEFORE UPDATE ON checkpoint BEGIN SELECT RAISE(ABORT,'failed owner intent'); END;").unwrap();
    assert!(owner::accept(&mut test.store, &context, test.caller, &call, 1000).is_err());
    connection.execute_batch("DROP TRIGGER fail_owner").unwrap();
    assert_eq!(test.store.load().unwrap().generation, generation);
    assert_eq!(test.value("Mutations"), Value::I32(0));
    owner::accept(&mut test.store, &context, test.caller, &call, 0).unwrap();
    let mut cut = false;
    for _ in 0..1000 {
        let saved = test.store.load().unwrap();
        let vm = saved.snapshot.restore().unwrap();
        let frame = vm.context().frames.last().unwrap();
        if let Continuation::Execute { next } = frame.continuation
            && matches!(
                saved
                    .snapshot
                    .program
                    .method(frame.method.method)
                    .unwrap()
                    .instructions[&next]
                    .op,
                Op::Divide { .. }
            )
        {
            cut = true;
            break;
        }
        owner::accept(&mut test.store, &context, test.caller, &call, 1).unwrap();
    }
    assert!(cut);
    assert_eq!(test.value("Mutations"), Value::I32(1));
    assert_eq!(test.value("Value"), Value::I32(20));
    connection.execute_batch("CREATE TRIGGER fail_terminal BEFORE UPDATE ON checkpoint WHEN instr(NEW.payload,CAST('integer division or remainder by zero' AS BLOB))>0 BEGIN SELECT RAISE(ABORT,'failed owner terminal'); END;").unwrap();
    assert!(owner::accept(&mut test.store, &context, test.caller, &call, 1).is_err());
    assert_eq!(test.value("Mutations"), Value::I32(1));
    assert_eq!(test.value("Value"), Value::I32(20));
    connection
        .execute_batch("DROP TRIGGER fail_terminal")
        .unwrap();
    let result = owner::accept(&mut test.store, &context, test.caller, &call, 1000).unwrap();
    assert!(matches!(result.state, ReplyState::Faulted { .. }));
    assert_eq!(
        owner::accept(&mut test.store, &context, test.caller, &call, 1000).unwrap(),
        result
    );
    assert_eq!(test.value("Mutations"), Value::I32(1));
}
#[test]
fn owner_denies_transitive_external_effects_and_projects_null_void_and_oversized_results_durably() {
    let mut test = Test::new();
    let context = test.context();
    for name in ["Denied", "IndirectDenied"] {
        let call = test.export(name, vec![]);
        let generation = test.store.generation();
        assert!(
            owner::accept(&mut test.store, &context, test.caller, &call, 1000)
                .unwrap_err()
                .0
                .contains("external capabilities")
        );
        assert_eq!(test.store.generation(), generation);
        assert_eq!(test.value("Mutations"), Value::I32(0));
        let saved = test.store.load().unwrap();
        assert_eq!(saved.snapshot.restore().unwrap().effects().count(), 0);
    }
    for (name, state) in [
        ("Null", ReplyState::Completed(Some(Argument::Null))),
        ("Void", ReplyState::Completed(None)),
        (
            "Large",
            ReplyState::InvalidResult {
                diagnostic: "owner result exceeds 4096 UTF-16 units".into(),
            },
        ),
    ] {
        let call = test.export(name, vec![]);
        let result = owner::accept(&mut test.store, &context, test.caller, &call, 10000).unwrap();
        assert_eq!(result.state, state);
        test.collect();
        assert_eq!(
            owner::accept(&mut test.store, &context, test.caller, &call, 10000).unwrap(),
            result
        );
    }
    assert_eq!(test.value("Mutations"), Value::I32(3));
    assert_eq!(test.value("Value"), Value::I32(21));
    let call = test.export("Echo", vec![Argument::Text(vec![0; 4097])]);
    let generation = test.store.generation();
    assert!(owner::accept(&mut test.store, &context, test.caller, &call, 1000).is_err());
    assert_eq!(test.store.generation(), generation);
}
#[test]
fn owner_metadata_and_original_receiver_bindings_are_validated_on_save_and_repaired_digest_recovery()
 {
    use sha2::{Digest, Sha256};
    let mut test = Test::new();
    let context = test.context();
    let call = test.export("Run", vec![Argument::I32(7)]);
    owner::accept(&mut test.store, &context, test.caller, &call, 0).unwrap();
    let saved = test.store.load().unwrap();
    let vm = saved.snapshot.restore().unwrap();
    let key = root(&call, test.caller);
    let metadata = vm.heap().roots()[&key];
    let text =
        String::from_utf16(vm.heap().object(metadata).unwrap().string_units().unwrap()).unwrap();
    let value: serde_json::Value = serde_json::from_str(&text).unwrap();
    for keyname in [
        "format",
        "caller",
        "owner",
        "grant",
        "object",
        "method",
        "code",
        "arguments",
        "epoch",
        "registryHash",
    ] {
        let mut bad = value.clone();
        bad[keyname] = serde_json::Value::Null;
        let mut vm = saved.snapshot.restore().unwrap();
        let string = vm
            .new_string(
                serde_json::to_string(&bad)
                    .unwrap()
                    .encode_utf16()
                    .collect(),
            )
            .unwrap();
        vm.set_root(key.clone(), string).unwrap();
        assert!(test.store.save(&vm).is_err());
    }
    let mut vm = saved.snapshot.restore().unwrap();
    vm.set_root("renraku.owner-v1/invalid", metadata).unwrap();
    assert!(test.store.save(&vm).is_err());
    let db = Connection::open(test.directory.join("universe.db")).unwrap();
    let mut payload: Vec<u8> = db
        .query_row("SELECT payload FROM checkpoint", [], |r| r.get(0))
        .unwrap();
    let bytes: Vec<u8> = text.encode_utf16().flat_map(u16::to_le_bytes).collect();
    let changed: Vec<u8> = text
        .replacen("\"format\":1", "\"format\":2", 1)
        .encode_utf16()
        .flat_map(u16::to_le_bytes)
        .collect();
    let start = payload
        .windows(bytes.len())
        .position(|b| b == bytes)
        .unwrap();
    payload[start..start + bytes.len()].copy_from_slice(&changed);
    db.execute(
        "UPDATE checkpoint SET payload=?1,digest=?2",
        rusqlite::params![payload, Sha256::digest(&payload).as_slice()],
    )
    .unwrap();
    assert!(
        test.store
            .load()
            .unwrap_err()
            .0
            .contains("owner acceptance format")
    );
}
#[test]
fn native_owner_and_instance_cli_resume_the_same_counter_without_sdk_and_preserve_receipt_reports()
{
    let mut test = Test::new();
    let context = test.context();
    let call = test.export("Counter::Add", vec![Argument::I32(7)]);
    let pending = owner::accept(&mut test.store, &context, test.caller, &call, 0).unwrap();
    placement::revoke(&mut test.store, call.grant).unwrap();
    test.collect();
    let run = |call: &OwnerCall, caller: UniverseId| {
        let Argument::I32(delta) = call.arguments[0] else {
            panic!()
        };
        Command::new(env!("CARGO_BIN_EXE_renraku-checkpoint-probe"))
            .args([
                "owner",
                test.directory.join("universe.db").to_str().unwrap(),
                &caller.to_string(),
                &call.grant.to_string(),
                &call.operation,
                "1000",
                &delta.to_string(),
            ])
            .env("PATH", "/no-owner-tools")
            .output()
            .unwrap()
    };
    let output = run(&call, test.caller);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let first: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(first["execution"], pending.execution.to_string());
    assert_eq!(first["reply"]["result"]["value"], 26);
    assert!(!run(&call, UniverseId::fresh()).status.success());
    let cli = || {
        Command::new(env!("CARGO_BIN_EXE_renraku-vm"))
            .args([
                "invoke-instance-once",
                test.directory.join("universe.db").to_str().unwrap(),
                "native-instance",
                "Renraku.Guest.Counter::Add",
                "1000",
                &test.counter.to_string(),
                "i32:-3",
            ])
            .env("PATH", "/no-owner-tools")
            .output()
            .unwrap()
    };
    let output = cli();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let second: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(second["format"], 2);
    assert_eq!(second["receiver"]["id"], test.counter.to_string());
    assert_eq!(second["result"]["value"], 23);
    let repeated: serde_json::Value = serde_json::from_slice(&cli().stdout).unwrap();
    assert_eq!(repeated["execution"], second["execution"]);
    assert_eq!(repeated["result"], second["result"]);
    let old: serde_json::Value = serde_json::from_slice(&run(&call, test.caller).stdout).unwrap();
    assert_eq!(old, first);
    let saved = test.store.load().unwrap();
    let vm = saved.snapshot.restore().unwrap();
    assert_eq!(
        field(&vm, &saved.snapshot.program, test.counter, "Value"),
        Value::I32(23)
    );
}
#[test]
fn instance_entry_rejects_wrong_receivers_code_constructors_and_static_methods_before_intent() {
    let mut test = Test::new();
    let saved = test.store.load().unwrap();
    let p = &saved.snapshot.program;
    let mut vm = saved.snapshot.restore().unwrap();
    let valid = InstanceCall {
        method: method(p, "Run"),
        receiver: test.receiver,
        arguments: vec![Value::I32(1)],
    };
    let text = vm.new_string(vec![0xd800]).unwrap();
    for variant in 0..5 {
        let mut call = valid.clone();
        match variant {
            0 => call.receiver = text,
            1 => call.receiver = ObjectId::fresh(),
            2 => call.method.code = CodeVersionId::fresh(),
            3 => {
                call.method = p
                    .methods()
                    .find(|m| {
                        m.owner == p.find_type("Renraku.Tests.OwnerSubject").unwrap()
                            && m.name == ".ctor"
                    })
                    .unwrap()
                    .binding
            }
            _ => call.method = p.method(common::entry(p, "Arithmetic")).unwrap().binding,
        };
        let before = vm.context().clone();
        assert!(
            vm.invoke_instance_once(&mut test.store, "bad", call, 1000, 1)
                .is_err()
        );
        assert_eq!(vm.context(), &before);
        assert!(vm.invocation("bad").is_none());
    }
}

#[test]
fn owner_operation_and_receipt_limits_reject_without_guest_progress_or_partial_acceptance() {
    let mut test = Test::new();
    let context = test.context();
    let mut call = test.export("Counter::Add", vec![Argument::I32(0)]);
    for bad in [
        "".to_owned(),
        "x".repeat(65),
        "slash/id".into(),
        "colon:id".into(),
        "nul\0id".into(),
        "é".into(),
    ] {
        call.operation = bad;
        let generation = test.store.generation();
        assert!(owner::accept(&mut test.store, &context, test.caller, &call, 1000).is_err());
        assert_eq!(test.store.generation(), generation);
    }
    for n in 0..owner::MAX_CALLS {
        call.operation = format!("bounded-{n}");
        let reply = owner::accept(&mut test.store, &context, test.caller, &call, 1000).unwrap();
        assert_eq!(reply.state, ReplyState::Completed(Some(Argument::I32(19))));
    }
    let generation = test.store.generation();
    call.operation = "overflow".into();
    assert!(owner::accept(&mut test.store, &context, test.caller, &call, 1000).is_err());
    assert_eq!(test.store.generation(), generation);
    let saved = test.store.load().unwrap();
    assert_eq!(
        saved
            .snapshot
            .restore()
            .unwrap()
            .invocations()
            .filter(|(id, _)| id.starts_with("renraku.owner-v1/"))
            .count(),
        owner::MAX_CALLS
    );
}
