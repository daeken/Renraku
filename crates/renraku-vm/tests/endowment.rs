#[allow(dead_code)]
mod common;
use renraku_vm::{
    checkpoint::CheckpointStore,
    endowment::{self, Endowment},
    execution::ExecutionStatus,
    identity::{CodeVersionId, FieldId, ObjectId, UniverseId},
    program::Program,
    runtime::{RunOutcome, Vm},
    value::Value,
};
use std::{fs, path::PathBuf};
struct Test {
    dir: PathBuf,
    store: CheckpointStore,
    shared: ObjectId,
    hidden: ObjectId,
}
impl Test {
    fn new(raw: bool) -> Self {
        let f = common::fixtures();
        let dll = fs::read(f.output.join(if raw {
            "Renraku.RawChildTests.dll"
        } else {
            "Renraku.ChildTests.dll"
        }))
        .unwrap();
        let p = Program::load(&[&f.core, &dll]).unwrap();
        let mut vm = Vm::new(&p);
        vm.start(p.entry("Renraku.ChildTests.Child::Seed").unwrap(), vec![])
            .unwrap();
        vm.run(10000).unwrap();
        let object = |ty: &str, name: &str| {
            let ty = p.find_type(ty).unwrap();
            let id = p
                .fields()
                .find(|f| f.owner == ty && f.name == name)
                .unwrap()
                .id;
            vm.static_fields()[&id].referenced_object().unwrap()
        };
        let shared = object("Renraku.ChildTests.Child", "Shared");
        let hidden = object("Renraku.ChildTests.PrivateParent", "Hidden");
        let dir = f.output.join(format!("child-{}", UniverseId::fresh()));
        fs::create_dir(&dir).unwrap();
        let mut store = CheckpointStore::open(dir.join("child.db")).unwrap();
        store.save(&vm).unwrap();
        Self {
            dir,
            store,
            shared,
            hidden,
        }
    }
    fn code(&mut self, name: &str) -> CodeVersionId {
        let s = self.store.load().unwrap();
        let p = &s.snapshot.program;
        p.method(p.entry(name).unwrap()).unwrap().binding.code
    }
    fn field(&mut self, ty: &str, name: &str) -> FieldId {
        let s = self.store.load().unwrap();
        let p = &s.snapshot.program;
        let ty = p.find_type(ty).unwrap();
        p.fields()
            .find(|f| f.owner == ty && f.name == name)
            .unwrap()
            .id
    }
    fn base(&mut self, names: &[&str]) -> Endowment {
        let s = self.store.load().unwrap();
        let p = &s.snapshot.program;
        let mut e = Endowment::default();
        for n in [
            "System.Object",
            "Renraku.ChildTests.Child",
            "Renraku.ChildTests.Node",
        ] {
            e.types.insert(p.find_type(n).unwrap());
        }
        for n in names {
            e.methods
                .insert(p.method(p.entry(n).unwrap()).unwrap().binding.code);
        }
        e
    }
    fn use_policy(&mut self) -> Endowment {
        let mut e = self.base(&["Renraku.ChildTests.Child::Use"]);
        e.objects.insert(self.shared);
        for (t, n) in [
            ("Renraku.ChildTests.Node", "Value"),
            ("Renraku.ChildTests.Child", "Runs"),
        ] {
            let f = self.field(t, n);
            e.read.insert(f);
            e.write.insert(f);
        }
        e
    }
    fn seal(&mut self, e: &Endowment) {
        endowment::seal(&mut self.store, e).unwrap();
    }
    fn node_value(&mut self, id: ObjectId) -> Value {
        let s = self.store.load().unwrap();
        let vm = s.snapshot.restore().unwrap();
        let f = s
            .snapshot
            .program
            .fields()
            .find(|f| f.owner == vm.heap().object(id).unwrap().type_id() && f.name == "Value")
            .unwrap();
        vm.heap().object(id).unwrap().fields()[&f.id]
    }
}
impl Drop for Test {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}
#[test]
fn explicit_objects_fields_methods_and_scoped_inspection_keep_private_state_out() {
    let mut t = Test::new(false);
    let e = t.use_policy();
    t.seal(&e);
    let code = t.code("Renraku.ChildTests.Child::Use");
    let r = endowment::invoke_once(
        &mut t.store,
        "first",
        code,
        vec![Value::Reference(t.shared), Value::I32(7)],
        1000,
    )
    .unwrap();
    assert_eq!(r["result"]["value"], 26);
    assert_eq!(t.node_value(t.shared), Value::I32(26));
    assert_eq!(t.node_value(t.hidden), Value::I32(77));
    let view = endowment::inspect_object(&mut t.store, t.shared).unwrap();
    assert_eq!(view["fields"].as_array().unwrap().len(), 1);
    assert_eq!(view["fields"][0]["name"], "Value");
    let data = endowment::search(&mut t.store, "", 100)
        .unwrap()
        .to_string();
    assert!(
        !data.contains("PrivateParent")
            && !data.contains("Hidden")
            && !data.contains("Seed")
            && !data.contains("Next")
    );
    let err = endowment::inspect_object(&mut t.store, t.hidden)
        .unwrap_err()
        .to_string();
    assert_eq!(
        err,
        endowment::inspect_object(&mut t.store, ObjectId::fresh())
            .unwrap_err()
            .to_string()
    );
    let number = t.field("Renraku.ChildTests.PrivateParent", "Number");
    assert!(endowment::inspect_static(&mut t.store, number).is_err());
    assert!(endowment::read_source_or_code(&mut t.store, "Renraku.ChildTests.Child::Use").is_err());
    let r2 = endowment::invoke_once(
        &mut t.store,
        "first",
        code,
        vec![Value::Reference(t.shared), Value::I32(7)],
        1000,
    )
    .unwrap();
    assert_eq!(r, r2);
    assert_eq!(t.node_value(t.shared), Value::I32(26));
}
#[test]
fn ungranted_methods_and_object_ids_cannot_be_used_as_entry_authority() {
    let mut t = Test::new(false);
    let e = t.use_policy();
    t.seal(&e);
    let attack = t.code("Renraku.ChildTests.Child::AttackStatic");
    assert!(
        endowment::invoke_once(&mut t.store, "attack", attack, vec![Value::I32(0)], 1000).is_err()
    );
    let code = t.code("Renraku.ChildTests.Child::Use");
    assert!(
        endowment::invoke_once(
            &mut t.store,
            "hidden",
            code,
            vec![Value::Reference(t.hidden), Value::I32(1)],
            1000
        )
        .is_err()
    );
    assert_eq!(t.node_value(t.hidden), Value::I32(77));
    let s = t.store.load().unwrap();
    let mut vm = s.snapshot.restore().unwrap();
    assert!(
        vm.start(
            s.snapshot
                .program
                .entry("Renraku.ChildTests.Child::AttackStatic")
                .unwrap(),
            vec![Value::I32(0)]
        )
        .is_err()
    );
}
#[test]
fn admission_checks_unreachable_cil_and_handcrafted_private_tokens_before_sealing() {
    for (raw, name) in [
        (false, "AttackStatic"),
        (false, "AttackCall"),
        (false, "AttackNew"),
        (false, "AttackCast"),
        (false, "Dead"),
        (true, "RawCall"),
        (true, "RawField"),
        (true, "RawNew"),
        (true, "RawCast"),
    ] {
        let mut t = Test::new(raw);
        let mut e = t.base(&[&format!("Renraku.ChildTests.Child::{name}")]);
        if name.contains("Cast") || name.contains("New") {
            let s = t.store.load().unwrap();
            e.types
                .insert(s.snapshot.program.find_type("System.Object").unwrap());
        }
        let generation = t.store.generation();
        assert!(endowment::seal(&mut t.store, &e).is_err(), "{name}");
        assert_eq!(t.store.generation(), generation);
    }
    let mut t = Test::new(true);
    let s = t.store.load().unwrap();
    let mut vm = s.snapshot.restore().unwrap();
    vm.start(
        s.snapshot
            .program
            .entry("Renraku.ChildTests.Child::RawCall")
            .unwrap(),
        vec![Value::I32(0)],
    )
    .unwrap();
    assert_eq!(
        vm.run(1000).unwrap(),
        RunOutcome::Completed(Some(Value::I32(8675309)))
    );
}
#[test]
fn reading_an_allowed_edge_does_not_endow_its_target_or_disclose_it_in_tools() {
    let mut t = Test::new(false);
    let mut e = t.base(&["Renraku.ChildTests.Child::Leak"]);
    e.objects.insert(t.shared);
    let next = t.field("Renraku.ChildTests.Node", "Next");
    e.read.insert(next);
    let runs = t.field("Renraku.ChildTests.Child", "Runs");
    e.read.insert(runs);
    e.write.insert(runs);
    t.seal(&e);
    let view = endowment::inspect_object(&mut t.store, t.shared).unwrap();
    assert_eq!(view["fields"][0]["value"]["kind"], "denied");
    assert!(!view.to_string().contains(&t.hidden.to_string()));
    let code = t.code("Renraku.ChildTests.Child::Leak");
    let r = endowment::invoke_once(
        &mut t.store,
        "leak",
        code,
        vec![Value::Reference(t.shared)],
        1000,
    )
    .unwrap();
    assert_eq!(r["state"], "faulted");
    let s = t.store.load().unwrap();
    let vm = s.snapshot.restore().unwrap();
    assert_eq!(
        vm.context().frames.last().unwrap().evaluation_stack,
        vec![Value::Reference(t.shared)]
    );
    assert_eq!(vm.static_fields()[&runs], Value::I32(1));
}
fn construction(t: &mut Test, names: &[&str]) -> Endowment {
    let mut e = t.base(names);
    let s = t.store.load().unwrap();
    let p = &s.snapshot.program;
    e.construct
        .insert(p.find_type("Renraku.ChildTests.Node").unwrap());
    for n in ["Renraku.ChildTests.Node::.ctor", "System.Object::.ctor"] {
        e.methods
            .insert(p.method(p.entry(n).unwrap()).unwrap().binding.code);
    }
    for f in [
        t.field("Renraku.ChildTests.Node", "Value"),
        t.field("Renraku.ChildTests.Node", "Next"),
        t.field("Renraku.ChildTests.Child", "Kept"),
    ] {
        e.read.insert(f);
        e.write.insert(f);
    }
    e
}
#[test]
fn owned_allocation_cycles_and_frames_recover_each_instruction_without_granting_old_instances() {
    let mut t = Test::new(false);
    let e = construction(&mut t, &["Renraku.ChildTests.Child::Create"]);
    t.seal(&e);
    let s = t.store.load().unwrap();
    let mut vm = s.snapshot.restore().unwrap();
    let method = s
        .snapshot
        .program
        .entry("Renraku.ChildTests.Child::Create")
        .unwrap();
    vm.invoke_once(&mut t.store, "create", method, vec![Value::I32(42)], 0, 1)
        .unwrap();
    let execution = vm.context().id;
    drop(vm);
    let mut result = None;
    for _ in 0..100 {
        let s = t.store.load().unwrap();
        let mut vm = s.snapshot.restore().unwrap();
        let out = vm.run(1).unwrap();
        vm.collect_garbage().unwrap();
        let context = vm.context().clone();
        t.store.save(&vm).unwrap();
        let after = t.store.load().unwrap();
        assert_eq!(*after.snapshot.restore().unwrap().context(), context);
        assert_eq!(context.id, execution);
        if let RunOutcome::Completed(Some(Value::Reference(id))) = out {
            result = Some(id);
            break;
        }
    }
    let id = result.unwrap();
    let v = endowment::inspect_object(&mut t.store, id).unwrap();
    assert!(
        v["fields"]
            .as_array()
            .unwrap()
            .iter()
            .any(|f| f["value"]["id"] == id.to_string())
    );
    assert!(endowment::inspect_object(&mut t.store, t.shared).is_err());
    assert!(endowment::inspect_object(&mut t.store, t.hidden).is_err());
}
#[test]
fn collected_child_objects_leave_nonreusable_bounded_allocation_provenance() {
    let mut t = Test::new(false);
    let e = construction(&mut t, &["Renraku.ChildTests.Child::Allocate"]);
    t.seal(&e);
    let code = t.code("Renraku.ChildTests.Child::Allocate");
    let r =
        endowment::invoke_once(&mut t.store, "allocate", code, vec![Value::I32(8)], 1000).unwrap();
    assert_eq!(r["result"]["value"], 8);
    let s = t.store.load().unwrap();
    let mut vm = s.snapshot.restore().unwrap();
    vm.collect_garbage().unwrap();
    t.store.save(&vm).unwrap();
    let root = vm.heap().roots()["renraku.child-v1/owned"];
    let doc: serde_json::Value = serde_json::from_str(
        &String::from_utf16(vm.heap().object(root).unwrap().string_units().unwrap()).unwrap(),
    )
    .unwrap();
    assert_eq!(doc["allocations"].as_array().unwrap().len(), 8);
    for row in doc["allocations"].as_array().unwrap() {
        let id = ObjectId::from_bytes(
            *uuid::Uuid::parse_str(row[0].as_str().unwrap())
                .unwrap()
                .as_bytes(),
        );
        assert!(vm.heap().object(id).is_err());
    }
}
#[test]
fn effects_require_exact_pairs_and_model_graph_reads_require_object_and_field_authority() {
    for allowed in [false, true] {
        let mut t = Test::new(false);
        let mut e = t.base(&["Renraku.ChildTests.Child::Echo", "Renraku.Model::Complete"]);
        let s = t.store.load().unwrap();
        for n in ["System.String", "Renraku.Model"] {
            e.types.insert(s.snapshot.program.find_type(n).unwrap());
        }
        if allowed {
            e.effects.insert(("local.echo".into(), "echo-v1".into()));
        }
        t.seal(&e);
        let s = t.store.load().unwrap();
        let mut vm = s.snapshot.restore().unwrap();
        vm.set_model_calls_enabled(true);
        vm.start(
            s.snapshot
                .program
                .entry("Renraku.ChildTests.Child::Echo")
                .unwrap(),
            vec![],
        )
        .unwrap();
        let result = vm.run(1000);
        if allowed {
            assert!(matches!(result, Ok(RunOutcome::Waiting(_))));
            let effect = vm.effects().next().unwrap().id;
            let mut provider =
                renraku_vm::effects::LocalModelProvider::open(t.dir.join("echo.db")).unwrap();
            vm.dispatch_model(&mut t.store, effect, &mut provider)
                .unwrap();
            assert!(matches!(
                vm.run_checkpointed(&mut t.store, 1000, 1).unwrap(),
                RunOutcome::Completed(Some(Value::Reference(_)))
            ));
        } else {
            assert!(
                result
                    .unwrap_err()
                    .to_string()
                    .contains("child authority denied")
            );
            assert_eq!(vm.effects().count(), 0);
            t.store.save(&vm).unwrap();
        }
    }
    let mut t = Test::new(false);
    let mut e = t.base(&["Renraku.ChildTests.Child::Chat", "Renraku.Model::Chat"]);
    let s = t.store.load().unwrap();
    let p = &s.snapshot.program;
    let vm = s.snapshot.restore().unwrap();
    for n in [
        "System.String",
        "Renraku.Model",
        "Renraku.ModelBinding",
        "Renraku.ChatMessage",
    ] {
        e.types.insert(p.find_type(n).unwrap());
    }
    let mut args = Vec::new();
    for name in ["Binding", "Head"] {
        let f = p
            .fields()
            .find(|f| f.owner == p.find_type("Renraku.ChildTests.Child").unwrap() && f.name == name)
            .unwrap();
        let id = vm.static_fields()[&f.id].referenced_object().unwrap();
        e.objects.insert(id);
        args.push(Value::Reference(id));
    }
    let binding = args[0].referenced_object().unwrap();
    for name in ["Capability", "Model"] {
        let f = p
            .fields()
            .find(|f| f.owner == p.find_type("Renraku.ModelBinding").unwrap() && f.name == name)
            .unwrap();
        e.objects.insert(
            vm.heap().object(binding).unwrap().fields()[&f.id]
                .referenced_object()
                .unwrap(),
        );
    }
    for f in p.fields().filter(|f| {
        [
            p.find_type("Renraku.ModelBinding").unwrap(),
            p.find_type("Renraku.ChatMessage").unwrap(),
        ]
        .contains(&f.owner)
    }) {
        e.read.insert(f.id);
    }
    e.effects.insert(("local.echo".into(), "echo-v1".into()));
    t.seal(&e);
    let s = t.store.load().unwrap();
    let mut vm = s.snapshot.restore().unwrap();
    vm.set_model_calls_enabled(true);
    vm.start(
        s.snapshot
            .program
            .entry("Renraku.ChildTests.Child::Chat")
            .unwrap(),
        args.clone(),
    )
    .unwrap();
    assert!(
        vm.run(1000)
            .unwrap_err()
            .to_string()
            .contains("child authority denied")
    );
    assert_eq!(vm.effects().count(), 0);
    assert_eq!(vm.context().frames.last().unwrap().evaluation_stack, args);
    t.store.save(&vm).unwrap();
}
#[test]
fn immutable_registry_tools_limits_and_failed_seal_publish_no_new_authority() {
    let mut t = Test::new(false);
    let e = t.use_policy();
    let db = rusqlite::Connection::open(t.dir.join("child.db")).unwrap();
    db.execute_batch("CREATE TRIGGER fail_seal BEFORE UPDATE ON checkpoint BEGIN SELECT RAISE(ABORT,'fail seal'); END;").unwrap();
    assert!(endowment::seal(&mut t.store, &e).is_err());
    db.execute_batch("DROP TRIGGER fail_seal").unwrap();
    let s = t.store.load().unwrap();
    assert!(
        !s.snapshot
            .restore()
            .unwrap()
            .heap()
            .roots()
            .contains_key("renraku.child-v1/policy")
    );
    t.seal(&e);
    assert!(endowment::seal(&mut t.store, &e).is_err());
    let s = t.store.load().unwrap();
    let vm = s.snapshot.restore().unwrap();
    let addon = fs::read(common::fixtures().output.join("Renraku.RemoteTests.dll")).unwrap();
    let linked = s.snapshot.program.link_modules(&[&addon], &[]).unwrap();
    assert!(vm.install_program(&mut t.store, &linked).is_err());
    for input in [
        br#"{"action":"read_code","code":"x"}"#.as_slice(),
        br#"{"action":"search","query":"","limit":10,"hostPath":"/private"}"#,
        br#"{"action":"install","path":"private.dll"}"#,
        br#"{"action":"search","query":"","limit":101}"#,
    ] {
        assert!(endowment::tool(&mut t.store, input).is_err());
    }
    assert!(endowment::tool(&mut t.store, &vec![32; endowment::MAX_TOOL_BYTES + 1]).is_err());
    let s = t.store.load().unwrap();
    let mut vm = s.snapshot.restore().unwrap();
    assert!(vm.new_string(vec![65; 4097]).is_err());
}
#[test]
fn allocation_limit_is_lifetime_scoped_even_after_collection() {
    let mut t = Test::new(false);
    let e = construction(&mut t, &["Renraku.ChildTests.Child::Allocate"]);
    t.seal(&e);
    let code = t.code("Renraku.ChildTests.Child::Allocate");
    let report = endowment::invoke_once(
        &mut t.store,
        "exhaust",
        code,
        vec![Value::I32(endowment::MAX_ALLOCATIONS as i32 + 1)],
        150000,
    )
    .unwrap();
    assert_eq!(report["state"], "faulted");
    let s = t.store.load().unwrap();
    let mut vm = s.snapshot.restore().unwrap();
    vm.collect_garbage().unwrap();
    t.store.save(&vm).unwrap();
    let report =
        endowment::invoke_once(&mut t.store, "new-goal", code, vec![Value::I32(1)], 1000).unwrap();
    assert_eq!(report["state"], "faulted");
}
#[test]
fn child_metadata_corruption_and_reserved_root_inspection_fail_closed() {
    use sha2::{Digest, Sha256};
    let mut t = Test::new(false);
    let mut e = t.use_policy();
    let s = t.store.load().unwrap();
    e.types
        .insert(s.snapshot.program.find_type("System.String").unwrap());
    t.seal(&e);
    let s = t.store.load().unwrap();
    let vm = s.snapshot.restore().unwrap();
    let id = vm.heap().roots()["renraku.child-v1/policy"];
    assert!(endowment::inspect_object(&mut t.store, id).is_err());
    let text = String::from_utf16(vm.heap().object(id).unwrap().string_units().unwrap()).unwrap();
    let changed = text.replacen("\"format\":1", "\"format\":2", 1);
    let mut bad = s.snapshot.restore().unwrap();
    let replacement = bad.new_string(changed.encode_utf16().collect()).unwrap();
    bad.set_root("renraku.child-v1/policy", replacement)
        .unwrap();
    assert!(t.store.save(&bad).is_err());
    let db = rusqlite::Connection::open(t.dir.join("child.db")).unwrap();
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
    assert!(t.store.load().is_err());
}
#[test]
fn failed_child_terminal_checkpoint_resumes_prior_mutation_without_starting_again() {
    let mut t = Test::new(false);
    let e = t.use_policy();
    t.seal(&e);
    let s = t.store.load().unwrap();
    let mut vm = s.snapshot.restore().unwrap();
    let entry = s
        .snapshot
        .program
        .entry("Renraku.ChildTests.Child::Use")
        .unwrap();
    vm.invoke_once(
        &mut t.store,
        "saved",
        entry,
        vec![Value::Reference(t.shared), Value::I32(7)],
        0,
        1,
    )
    .unwrap();
    drop(vm);
    let mut found = false;
    for _ in 0..100 {
        let s = t.store.load().unwrap();
        let mut vm = s.snapshot.restore().unwrap();
        let previous = vm.context().clone();
        let out = vm.run(1).unwrap();
        if matches!(out, RunOutcome::Completed(_)) {
            let db = rusqlite::Connection::open(t.dir.join("child.db")).unwrap();
            db.execute_batch("CREATE TRIGGER fail_terminal BEFORE UPDATE ON checkpoint BEGIN SELECT RAISE(ABORT,'terminal failure'); END;").unwrap();
            assert!(t.store.save(&vm).is_err());
            db.execute_batch("DROP TRIGGER fail_terminal").unwrap();
            let s = t.store.load().unwrap();
            assert_eq!(*s.snapshot.restore().unwrap().context(), previous);
            found = true;
            break;
        }
        t.store.save(&vm).unwrap();
    }
    assert!(found);
    let code = t.code("Renraku.ChildTests.Child::Use");
    let result = endowment::invoke_once(
        &mut t.store,
        "saved",
        code,
        vec![Value::Reference(t.shared), Value::I32(7)],
        1000,
    )
    .unwrap();
    assert_eq!(result["result"]["value"], 26);
    assert_eq!(t.node_value(t.shared), Value::I32(26));
    let runs = t.field("Renraku.ChildTests.Child", "Runs");
    assert_eq!(
        endowment::inspect_static(&mut t.store, runs).unwrap()["value"]["value"],
        1
    );
}
fn native_tool(t: &Test, request: serde_json::Value) -> serde_json::Value {
    let file = t.dir.join("tool.json");
    fs::write(&file, serde_json::to_vec(&request).unwrap()).unwrap();
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_renraku-vm"))
        .args([
            "child-tool",
            t.dir.join("child.db").to_str().unwrap(),
            file.to_str().unwrap(),
        ])
        .env("PATH", "/no-child-sdk")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).unwrap()
}
#[test]
fn native_sealed_child_uses_only_its_remote_endowment_and_keeps_parent_tools_private() {
    use renraku_vm::{
        identity::{GrantId, NodeId},
        peer::{Endpoint, PeerKey},
        placement::{self, PlacementPolicy},
        remote,
    };
    use std::{
        io::{BufRead, BufReader},
        process::{Command, Stdio},
    };
    let mut t = Test::new(false);
    let f = common::fixtures();
    let op = Program::load(&[&f.core, &f.guest]).unwrap();
    let mut ovm = Vm::new(&op);
    ovm.start(
        op.entry("Renraku.Guest.Simulation::SaveObject").unwrap(),
        vec![],
    )
    .unwrap();
    ovm.run(1000).unwrap();
    let counter = op
        .fields()
        .find(|f| f.owner == op.find_type("Renraku.Guest.Simulation").unwrap() && f.name == "Saved")
        .unwrap();
    let object = ovm.static_fields()[&counter.id]
        .referenced_object()
        .unwrap();
    let owner_id = ovm.universe_id();
    let mut owner = CheckpointStore::open(t.dir.join("owner.db")).unwrap();
    owner.save(&ovm).unwrap();
    let s = t.store.load().unwrap();
    let principal = s.snapshot.restore().unwrap().universe_id();
    let node = NodeId::fresh();
    placement::initialize(&mut owner, node, 1).unwrap();
    placement::register_object(&mut owner, object, PlacementPolicy::pinned(node)).unwrap();
    let method = op
        .method(op.entry("Renraku.Guest.Counter::Add").unwrap())
        .unwrap()
        .binding;
    let grant = GrantId::fresh();
    placement::export_method(&mut owner, grant, principal, object, method).unwrap();
    let key = PeerKey::from_bytes([0x42; 32]);
    key.write_new(t.dir.join("key")).unwrap();
    let mut server = NativeGuard(
        Command::new(env!("CARGO_BIN_EXE_renraku-checkpoint-probe"))
            .args([
                "peer-server",
                t.dir.join("owner.db").to_str().unwrap(),
                &node.to_string(),
                &principal.to_string(),
                t.dir.join("key").to_str().unwrap(),
                "127.0.0.1:0",
                "1000",
                "1",
                "-",
            ])
            .env("PATH", "/no-child-sdk")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap(),
    );
    let mut line = String::new();
    BufReader::new(server.0.stdout.take().unwrap())
        .read_line(&mut line)
        .unwrap();
    let address = serde_json::from_str::<serde_json::Value>(&line).unwrap()["listening"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    let contract = s
        .snapshot
        .program
        .method(
            s.snapshot
                .program
                .entry("Renraku.ChildTests.CounterPort::Add")
                .unwrap(),
        )
        .unwrap()
        .binding;
    let handle = remote::import_method(
        &mut owner,
        &mut t.store,
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
    let mut e = t.base(&[
        "Renraku.ChildTests.Child::Remote",
        "Renraku.ChildTests.CounterPort::Add",
    ]);
    e.types.insert(
        s.snapshot
            .program
            .find_type("Renraku.ChildTests.CounterPort")
            .unwrap(),
    );
    e.objects.insert(handle);
    let runs = t.field("Renraku.ChildTests.Child", "Runs");
    e.read.insert(runs);
    e.write.insert(runs);
    t.seal(&e);
    let code = t.code("Renraku.ChildTests.Child::Remote");
    let request = serde_json::json!({"action":"invoke","operation":"remote","code":code.to_string(),"budget":1000,"arguments":[{"kind":"object","value":handle.to_string()},{"kind":"i32","value":7}]});
    let r = native_tool(&t, request.clone());
    assert_eq!(r["state"], "running");
    assert_eq!(r["waitingRemote"], true);
    let s = t.store.load().unwrap();
    let mut vm = s.snapshot.restore().unwrap();
    assert!(matches!(
        vm.context().status,
        ExecutionStatus::WaitingRemote { .. }
    ));
    vm.collect_garbage().unwrap();
    t.store.save(&vm).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_renraku-vm"))
        .args([
            "remote-drive",
            t.dir.join("child.db").to_str().unwrap(),
            t.dir.join("key").to_str().unwrap(),
        ])
        .env("PATH", "/no-child-sdk")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(server.0.wait().unwrap().success());
    fs::remove_file(t.dir.join("key")).unwrap();
    let r = native_tool(&t, request.clone());
    assert_eq!(r["result"]["value"], 27);
    assert_eq!(native_tool(&t, request), r);
    let search = native_tool(
        &t,
        serde_json::json!({"action":"search","query":"PrivateParent","limit":100}),
    );
    assert_eq!(search["symbols"], serde_json::json!([]));
    let s = owner.load().unwrap();
    let vm = s.snapshot.restore().unwrap();
    let value = op
        .fields()
        .find(|f| f.owner == op.find_type("Renraku.Guest.Counter").unwrap() && f.name == "Value")
        .unwrap();
    assert_eq!(
        vm.heap().object(object).unwrap().fields()[&value.id],
        Value::I32(26)
    );
}

struct NativeGuard(std::process::Child);
impl Drop for NativeGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
#[test]
fn child_layout_and_frame_limits_reject_valid_but_oversized_runtime_metadata() {
    let mut t = Test::new(false);
    let mut e = t.base(&[
        "Renraku.ChildTests.BigLayout::.ctor",
        "System.Object::.ctor",
    ]);
    let s = t.store.load().unwrap();
    let big = s
        .snapshot
        .program
        .find_type("Renraku.ChildTests.BigLayout")
        .unwrap();
    e.types.insert(big);
    e.construct.insert(big);
    assert!(
        endowment::seal(&mut t.store, &e)
            .unwrap_err()
            .to_string()
            .contains("64 fields")
    );
    let f = common::fixtures();
    let p = Program::load(&[
        &f.core,
        &fs::read(f.output.join("Renraku.LargeStackChildTests.dll")).unwrap(),
    ])
    .unwrap();
    let vm = Vm::new(&p);
    let mut large = CheckpointStore::open(t.dir.join("large.db")).unwrap();
    large.save(&vm).unwrap();
    let code = p
        .method(p.entry("Renraku.ChildTests.Child::RawCast").unwrap())
        .unwrap()
        .binding
        .code;
    let mut e = Endowment::default();
    for ty in ["System.Object", "Renraku.ChildTests.Child"] {
        e.types.insert(p.find_type(ty).unwrap());
    }
    e.methods.insert(code);
    assert!(
        endowment::seal(&mut large, &e)
            .unwrap_err()
            .to_string()
            .contains("256 slots")
    );
    let e = t.use_policy();
    t.seal(&e);
    let s = t.store.load().unwrap();
    let mut vm = s.snapshot.restore().unwrap();
    vm.set_frame_limit(65).unwrap();
    assert!(t.store.save(&vm).is_err());
}
