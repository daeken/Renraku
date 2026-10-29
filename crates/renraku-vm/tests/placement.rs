#[allow(dead_code)]
mod common;
use renraku_vm::{
    checkpoint::CheckpointStore,
    execution::MethodVersion,
    identity::{CodeVersionId, GrantId, NodeId, ObjectId, UniverseId},
    placement::{self, Access, Argument, OwnerContext, PlacementPolicy},
    program::Program,
    runtime::{RunOutcome, Vm},
    value::Value,
};
use rusqlite::Connection;
use std::{collections::BTreeSet, fs, path::PathBuf, process::Command};
const REGISTRY: &str = "renraku.placement-v1/registry";
struct Test {
    directory: PathBuf,
    store: CheckpointStore,
    node: NodeId,
    caller: UniverseId,
    target: ObjectId,
    method: MethodVersion,
    grant: GrantId,
}
impl Test {
    fn new() -> Self {
        let directory = common::fixtures()
            .output
            .join(format!("placement-{}", UniverseId::fresh()));
        fs::create_dir(&directory).unwrap();
        let p = Program::load(&[
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
        assert_eq!(
            vm.run(1000).unwrap(),
            RunOutcome::Completed(Some(Value::I32(19)))
        );
        vm.start(
            p.entry("Renraku.Tests.ExportPolicyHarness::Initialize")
                .unwrap(),
            vec![],
        )
        .unwrap();
        vm.run(1000).unwrap();
        let owner = p.find_type("Renraku.Guest.Simulation").unwrap();
        let root = p
            .fields()
            .find(|f| f.owner == owner && f.name == "Saved")
            .unwrap()
            .id;
        let target = vm.static_fields()[&root].referenced_object().unwrap();
        let method = p
            .methods()
            .find(|m| m.owner == p.find_type("Renraku.Guest.Counter").unwrap() && m.name == "Add")
            .unwrap()
            .binding;
        let mut store = CheckpointStore::open(directory.join("universe.db")).unwrap();
        store.save(&vm).unwrap();
        Self {
            directory,
            store,
            node: NodeId::fresh(),
            caller: UniverseId::fresh(),
            target,
            method,
            grant: GrantId::fresh(),
        }
    }
    fn init(&mut self) {
        placement::initialize(&mut self.store, self.node, 1).unwrap();
        placement::register_object(
            &mut self.store,
            self.target,
            PlacementPolicy::pinned(self.node),
        )
        .unwrap();
    }
    fn export(&mut self) {
        self.init();
        placement::export_method(
            &mut self.store,
            self.grant,
            self.caller,
            self.target,
            self.method,
        )
        .unwrap();
    }
    fn context(&self) -> OwnerContext {
        OwnerContext {
            node: self.node,
            epoch: 1,
            resources: BTreeSet::new(),
        }
    }
    fn access(&mut self) -> Access {
        let saved = self.store.load().unwrap();
        Access::Invoke {
            owner: saved.snapshot.restore().unwrap().universe_id(),
            epoch: 1,
            object: self.target,
            method: self.method,
            arguments: vec![Argument::I32(7)],
        }
    }
    fn subject(&mut self) -> ObjectId {
        let saved = self.store.load().unwrap();
        let p = &saved.snapshot.program;
        let vm = saved.snapshot.restore().unwrap();
        let owner = p.find_type("Renraku.Tests.ExportPolicyHarness").unwrap();
        vm.static_fields()[&p
            .fields()
            .find(|f| f.owner == owner && f.name == "Saved")
            .unwrap()
            .id]
            .referenced_object()
            .unwrap()
    }
}
impl Drop for Test {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.directory);
    }
}
#[test]
fn default_denial_pinned_placement_and_independent_copy_mobility_require_explicit_resources() {
    let mut test = Test::new();
    let host_context = test.context();
    let access = test.access();
    let saved = test.store.load().unwrap();
    let vm = saved.snapshot.restore().unwrap();
    assert!(placement::read(&vm).unwrap().is_none());
    assert!(
        placement::authorize(
            &mut test.store,
            &host_context,
            test.caller,
            test.grant,
            &access
        )
        .is_err()
    );
    assert!(
        placement::register_object(
            &mut test.store,
            test.target,
            PlacementPolicy::pinned(test.node)
        )
        .is_err()
    );
    placement::initialize(&mut test.store, test.node, 1).unwrap();
    let mut policy = PlacementPolicy::pinned(test.node);
    policy.copyable = true;
    policy.required_resources.insert("camera.front".into());
    let record = placement::register_object(&mut test.store, test.target, policy.clone()).unwrap();
    assert!(record.placement.copyable && !record.placement.movable);
    placement::export_method(
        &mut test.store,
        test.grant,
        test.caller,
        test.target,
        test.method,
    )
    .unwrap();
    let saved = test.store.load().unwrap();
    let vm = saved.snapshot.restore().unwrap();
    let before = vm.instructions_executed();
    assert!(
        placement::authorize(
            &mut test.store,
            &host_context,
            test.caller,
            test.grant,
            &access
        )
        .is_err()
    );
    let mut context = test.context();
    context.resources.insert("camera.front".into());
    let accepted =
        placement::authorize(&mut test.store, &context, test.caller, test.grant, &access).unwrap();
    assert_eq!(accepted.object, test.target);
    assert_eq!(accepted.method, test.method);
    assert_eq!(accepted.observed_generation, saved.generation);
    assert_eq!(
        accepted.registry_hash,
        placement::read(&vm).unwrap().unwrap().hash().unwrap()
    );
    assert_eq!(vm.instructions_executed(), before);
    assert!(
        placement::authorize(
            &mut test.store,
            &context,
            test.caller,
            test.grant,
            &Access::CopyGraph(test.target)
        )
        .is_err()
    );
    context.node = NodeId::fresh();
    assert!(
        placement::authorize(&mut test.store, &context, test.caller, test.grant, &access).is_err()
    );
    policy.movable = true;
    assert!(placement::register_object(&mut test.store, test.target, policy).is_err());
    let subject = test.subject();
    let mut movable = PlacementPolicy::pinned(test.node);
    movable.movable = true;
    let second = placement::register_object(&mut test.store, subject, movable).unwrap();
    assert!(second.placement.movable && !second.placement.copyable);
}
#[test]
fn grant_pins_principal_target_code_epoch_and_confers_no_ambient_inspection_or_mutation_rights() {
    let mut test = Test::new();
    let host_context = test.context();
    test.export();
    let access = test.access();
    let saved = test.store.load().unwrap();
    let p = &saved.snapshot.program;
    let vm = saved.snapshot.restore().unwrap();
    let heap = vm.heap().clone();
    let context = vm.context().clone();
    placement::authorize(
        &mut test.store,
        &host_context,
        test.caller,
        test.grant,
        &access,
    )
    .unwrap();
    assert!(
        placement::authorize(
            &mut test.store,
            &host_context,
            UniverseId::fresh(),
            test.grant,
            &access
        )
        .is_err()
    );
    assert!(
        placement::authorize(
            &mut test.store,
            &host_context,
            test.caller,
            GrantId::fresh(),
            &access
        )
        .is_err()
    );
    let mut wrong_context = test.context();
    wrong_context.epoch = 2;
    assert!(
        placement::authorize(
            &mut test.store,
            &wrong_context,
            test.caller,
            test.grant,
            &access
        )
        .is_err()
    );
    for variant in 0..6 {
        let mut wrong = access.clone();
        if let Access::Invoke {
            owner,
            epoch,
            object,
            method,
            arguments,
        } = &mut wrong
        {
            match variant {
                0 => *owner = UniverseId::fresh(),
                1 => *epoch = 2,
                2 => *object = ObjectId::fresh(),
                3 => method.code = CodeVersionId::fresh(),
                4 => *arguments = vec![Argument::I64(7)],
                _ => arguments.clear(),
            }
        }
        assert!(
            placement::authorize(
                &mut test.store,
                &host_context,
                test.caller,
                test.grant,
                &wrong
            )
            .is_err()
        );
    }
    let field = p
        .fields()
        .find(|f| f.owner == p.find_type("Renraku.Guest.Counter").unwrap() && f.name == "Value")
        .unwrap()
        .id;
    for denied in [
        Access::InspectObject(test.target),
        Access::InspectType(p.find_type("Renraku.Guest.Counter").unwrap()),
        Access::ReadCode(test.method.code),
        Access::ReadStatic(field),
        Access::ReadField(test.target, field),
        Access::WriteField(test.target, field),
        Access::Discover,
        Access::CopyGraph(test.target),
        Access::MoveObject(test.target),
    ] {
        assert!(
            placement::authorize(
                &mut test.store,
                &host_context,
                test.caller,
                test.grant,
                &denied
            )
            .is_err()
        );
    }
    assert_eq!(vm.heap(), &heap);
    assert_eq!(vm.context(), &context);
    assert_eq!(test.store.generation(), saved.generation);
}
#[test]
fn declared_value_profile_preserves_utf16_and_rejects_width_boxing_reference_and_size_coercions() {
    let mut test = Test::new();
    let host_context = test.context();
    test.init();
    let subject = test.subject();
    placement::register_object(&mut test.store, subject, PlacementPolicy::pinned(test.node))
        .unwrap();
    let saved = test.store.load().unwrap();
    let p = &saved.snapshot.program;
    let owner = p.find_type("Renraku.Tests.ExportSubject").unwrap();
    for name in [".ctor", "Static", "Object", "Self", "Many"] {
        let binding = p
            .methods()
            .find(|m| m.owner == owner && m.name == name)
            .unwrap()
            .binding;
        assert!(
            placement::export_method(
                &mut test.store,
                GrantId::fresh(),
                test.caller,
                subject,
                binding
            )
            .is_err(),
            "{name}"
        );
    }
    for (name, good, bad) in [
        (
            "Echo",
            vec![Argument::Text(vec![0, 0xd800, 0xdc00, 0xdfff])],
            vec![Argument::I32(7)],
        ),
        (
            "Three",
            vec![
                Argument::Text(vec![0; 4096]),
                Argument::Text(vec![0; 4096]),
                Argument::Text(vec![]),
            ],
            vec![Argument::Text(vec![0; 4096]); 3],
        ),
        ("Wide", vec![Argument::I64(-1)], vec![Argument::I32(-1)]),
        ("Flag", vec![Argument::I32(1)], vec![Argument::I32(2)]),
        (
            "Unit",
            vec![Argument::I32(0xd800)],
            vec![Argument::I32(65536)],
        ),
    ] {
        let method = p
            .methods()
            .find(|m| m.owner == owner && m.name == name)
            .unwrap()
            .binding;
        let grant = GrantId::fresh();
        placement::export_method(&mut test.store, grant, test.caller, subject, method).unwrap();
        let saved = test.store.load().unwrap();
        let vm = saved.snapshot.restore().unwrap();
        let mut request = Access::Invoke {
            owner: vm.universe_id(),
            epoch: 1,
            object: subject,
            method,
            arguments: good.clone(),
        };
        let accepted =
            placement::authorize(&mut test.store, &host_context, test.caller, grant, &request)
                .unwrap();
        assert_eq!(accepted.arguments, good);
        if let Access::Invoke { arguments, .. } = &mut request {
            *arguments = bad;
        }
        assert!(
            placement::authorize(&mut test.store, &host_context, test.caller, grant, &request)
                .is_err(),
            "{name}"
        );
        if name == "Echo" {
            if let Access::Invoke { arguments, .. } = &mut request {
                *arguments = vec![Argument::Null];
            }
            placement::authorize(&mut test.store, &host_context, test.caller, grant, &request)
                .unwrap();
            if let Access::Invoke { arguments, .. } = &mut request {
                *arguments = vec![Argument::Text(vec![0; 4097])];
            }
            assert!(
                placement::authorize(&mut test.store, &host_context, test.caller, grant, &request)
                    .is_err()
            );
        }
    }
}
#[test]
fn registry_roots_keep_original_targets_through_static_replacement_gc_and_paused_frames() {
    let mut test = Test::new();
    let host_context = test.context();
    test.export();
    let access = test.access();
    let saved = test.store.load().unwrap();
    let p = &saved.snapshot.program;
    let frozen = p
        .reference_set(&[p.find_type("Renraku.Guest.Counter").unwrap()])
        .unwrap()
        .to_json()
        .unwrap();
    let mut vm = saved.snapshot.restore().unwrap();
    let hash = placement::read(&vm).unwrap().unwrap().hash().unwrap();
    let root = p
        .fields()
        .find(|f| f.owner == p.find_type("Renraku.Guest.Simulation").unwrap() && f.name == "Saved")
        .unwrap()
        .id;
    vm.start(common::entry(p, "SaveObject"), vec![]).unwrap();
    vm.run(1000).unwrap();
    assert_ne!(vm.static_fields()[&root], Value::Reference(test.target));
    vm.start(common::entry(p, "Arithmetic"), vec![]).unwrap();
    for _ in 0..8 {
        vm.run(1).unwrap();
        vm.collect_garbage().unwrap();
        test.store.save(&vm).unwrap();
    }
    let saved = test.store.load().unwrap();
    let vm = saved.snapshot.restore().unwrap();
    assert!(vm.heap().object(test.target).is_ok());
    assert_eq!(placement::read(&vm).unwrap().unwrap().hash().unwrap(), hash);
    assert_eq!(
        saved
            .snapshot
            .program
            .reference_set(&[p.find_type("Renraku.Guest.Counter").unwrap()])
            .unwrap()
            .to_json()
            .unwrap(),
        frozen
    );
    placement::authorize(
        &mut test.store,
        &host_context,
        test.caller,
        test.grant,
        &access,
    )
    .unwrap();
    assert_eq!(vm.context().frames.len(), 1);
    // Preflight does not enter the paused context; dispatch still needs the
    // existing one-context check in the future remote acceptance layer.
}
#[test]
fn registration_and_revocation_commit_atomically_and_retries_never_reactivate_or_rebind_grants() {
    let mut test = Test::new();
    let host_context = test.context();
    test.export();
    let access = test.access();
    let subject = test.subject();
    let generation = test.store.generation();
    let db = Connection::open(test.directory.join("universe.db")).unwrap();
    db.execute_batch("CREATE TRIGGER reject_policy BEFORE UPDATE ON checkpoint BEGIN SELECT RAISE(ABORT,'injected policy save failure'); END;").unwrap();
    assert!(
        placement::register_object(&mut test.store, subject, PlacementPolicy::pinned(test.node))
            .is_err()
    );
    assert!(
        placement::export_method(
            &mut test.store,
            GrantId::fresh(),
            test.caller,
            test.target,
            test.method
        )
        .is_err()
    );
    assert!(placement::revoke(&mut test.store, test.grant).is_err());
    let saved = test.store.load().unwrap();
    let vm = saved.snapshot.restore().unwrap();
    assert_eq!(saved.generation, generation);
    let registry = placement::read(&vm).unwrap().unwrap();
    assert_eq!(registry.objects().count(), 1);
    assert_eq!(registry.grants().count(), 1);
    placement::authorize(
        &mut test.store,
        &host_context,
        test.caller,
        test.grant,
        &access,
    )
    .unwrap();
    db.execute_batch("DROP TRIGGER reject_policy").unwrap();
    let revoked = placement::revoke(&mut test.store, test.grant).unwrap();
    assert!(revoked.revoked);
    // A previously restored VM cannot commit after revocation; fresh preflight
    // reloads the registry instead of trusting that old in-memory grant.
    assert!(test.store.save(&vm).is_err());
    assert!(
        placement::authorize(
            &mut test.store,
            &host_context,
            test.caller,
            test.grant,
            &access
        )
        .is_err()
    );
    let generation = test.store.generation();
    assert_eq!(
        placement::revoke(&mut test.store, test.grant).unwrap(),
        revoked
    );
    assert_eq!(
        placement::export_method(
            &mut test.store,
            test.grant,
            test.caller,
            test.target,
            test.method
        )
        .unwrap(),
        revoked
    );
    assert_eq!(test.store.generation(), generation);
    assert!(
        placement::export_method(
            &mut test.store,
            test.grant,
            UniverseId::fresh(),
            test.target,
            test.method
        )
        .is_err()
    );
    assert!(
        placement::authorize(
            &mut test.store,
            &host_context,
            test.caller,
            test.grant,
            &access
        )
        .is_err()
    );
    assert!(placement::initialize(&mut test.store, test.node, 2).is_err());
    assert!(placement::initialize(&mut test.store, NodeId::fresh(), 1).is_err());
}
#[test]
fn checkpoint_validation_rejects_malformed_policy_roots_bindings_formats_and_oversized_lists() {
    let mut test = Test::new();
    test.export();
    let saved = test.store.load().unwrap();
    let original = saved.snapshot.restore().unwrap();
    let metadata = original.heap().roots()[REGISTRY];
    let text = String::from_utf16(
        original
            .heap()
            .object(metadata)
            .unwrap()
            .string_units()
            .unwrap(),
    )
    .unwrap();
    let wire: serde_json::Value = serde_json::from_str(&text).unwrap();
    let mut corruptions = Vec::new();
    for n in 0..14 {
        let mut value = wire.clone();
        match n {
            0 => value["format"] = 2.into(),
            1 => value["epoch"] = 0.into(),
            2 => value["universe"] = UniverseId::fresh().to_string().into(),
            3 => {
                value["objects"][0]["typeId"] =
                    renraku_vm::identity::TypeId::fresh().to_string().into()
            }
            4 => value["objects"][0]["object"] = ObjectId::fresh().to_string().into(),
            5 => value["grants"][0]["code"] = CodeVersionId::fresh().to_string().into(),
            6 => value["grants"][0]["object"] = ObjectId::fresh().to_string().into(),
            7 => value["extra"] = true.into(),
            8 => {
                value["objects"][0]["allowedNodes"] =
                    serde_json::json!([test.node.to_string(), test.node.to_string()])
            }
            9 => value["objects"][0]["requiredResources"] = serde_json::json!(["a", "a"]),
            10 => {
                value["objects"] = serde_json::Value::Array(vec![value["objects"][0].clone(); 257])
            }
            11 => value["grants"] = serde_json::Value::Array(vec![value["grants"][0].clone(); 257]),
            12 => value["objects"][0]["allowedNodes"] = serde_json::json!([]),
            _ => value["objects"][0]["requiredResources"] = serde_json::json!(["../hardware"]),
        }
        corruptions.push(serde_json::to_string(&value).unwrap());
    }
    corruptions.extend([
        format!(" {text}"),
        "x".repeat(placement::MAX_REGISTRY_BYTES + 1),
        "\u{fffd}".into(),
    ]);
    for corrupted in corruptions {
        let mut vm = saved.snapshot.restore().unwrap();
        let metadata = vm.new_string(corrupted.encode_utf16().collect()).unwrap();
        vm.set_root(REGISTRY, metadata).unwrap();
        assert!(test.store.save(&vm).is_err());
        assert_eq!(test.store.generation(), saved.generation);
    }
    for root in [
        REGISTRY.to_owned(),
        format!("renraku.placement-v1/object/{}", test.target),
    ] {
        let mut vm = saved.snapshot.restore().unwrap();
        vm.remove_root(&root);
        assert!(test.store.save(&vm).is_err());
    }
    let mut vm = saved.snapshot.restore().unwrap();
    vm.set_root("renraku.placement-v1/unknown", metadata)
        .unwrap();
    assert!(test.store.save(&vm).is_err());
    let current = test.store.load().unwrap();
    assert_eq!(current.generation, saved.generation);
    assert_eq!(
        placement::read(&current.snapshot.restore().unwrap()).unwrap(),
        placement::read(&original).unwrap()
    );

    // A forged checkpoint with a repaired outer digest must still validate the
    // reserved metadata on recovery, rather than treating it as inert text.
    use sha2::{Digest, Sha256};
    let connection = Connection::open(test.directory.join("universe.db")).unwrap();
    let mut payload: Vec<u8> = connection
        .query_row("SELECT payload FROM checkpoint", [], |row| row.get(0))
        .unwrap();
    let bytes: Vec<u8> = text.encode_utf16().flat_map(u16::to_le_bytes).collect();
    let changed = text.replacen("\"format\":1", "\"format\":2", 1);
    assert_ne!(changed, text);
    let changed: Vec<u8> = changed.encode_utf16().flat_map(u16::to_le_bytes).collect();
    let offset = payload
        .windows(bytes.len())
        .position(|part| part == bytes)
        .unwrap();
    payload[offset..offset + bytes.len()].copy_from_slice(&changed);
    connection
        .execute(
            "UPDATE checkpoint SET payload=?1,digest=?2",
            rusqlite::params![payload, Sha256::digest(&payload).as_slice()],
        )
        .unwrap();
    let error = test.store.load().unwrap_err();
    assert!(error.0.contains("placement format"), "{error}");
}
#[test]
fn native_sdk_free_policy_recovery_authorizes_only_the_original_bound_principal_without_execution()
{
    let mut test = Test::new();
    test.export();
    let saved = test.store.load().unwrap();
    let vm = saved.snapshot.restore().unwrap();
    let hash = placement::read(&vm).unwrap().unwrap().hash().unwrap();
    let run = |caller: UniverseId| {
        Command::new(env!("CARGO_BIN_EXE_renraku-checkpoint-probe"))
            .args([
                "placement",
                test.directory.join("universe.db").to_str().unwrap(),
                &test.grant.to_string(),
                &caller.to_string(),
            ])
            .env("PATH", "/no-placement-sdk")
            .output()
            .unwrap()
    };
    for _ in 0..2 {
        let output = run(test.caller);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let result: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(result["object"], test.target.to_string());
        assert_eq!(result["code"], test.method.code.to_string());
        assert_eq!(result["registryHash"], hash);
        assert_eq!(result["generation"], saved.generation);
        assert_eq!(result["instructions"], vm.instructions_executed());
    }
    assert!(!run(UniverseId::fresh()).status.success());
    placement::revoke(&mut test.store, test.grant).unwrap();
    assert!(!run(test.caller).status.success());
}

#[test]
fn canonical_hashes_ignore_registration_order_and_fixed_policy_lists_have_explicit_bounds() {
    let mut test = Test::new();
    let copy = test.directory.join("copy.db");
    Connection::open(test.directory.join("universe.db"))
        .unwrap()
        .execute("VACUUM INTO ?1", [copy.to_str().unwrap()])
        .unwrap();
    let mut other = CheckpointStore::open(copy).unwrap();
    let subject = test.subject();
    let saved = test.store.load().unwrap();
    let p = &saved.snapshot.program;
    let second_method = p
        .methods()
        .find(|m| m.owner == p.find_type("Renraku.Tests.ExportSubject").unwrap() && m.name == "Add")
        .unwrap()
        .binding;
    let second_grant = GrantId::fresh();
    for (store, reverse) in [(&mut test.store, false), (&mut other, true)] {
        placement::initialize(store, test.node, u64::MAX).unwrap();
        let mut entries = vec![
            (test.target, test.method, test.grant),
            (subject, second_method, second_grant),
        ];
        if reverse {
            entries.reverse();
        }
        for (target, method, grant) in entries {
            placement::register_object(store, target, PlacementPolicy::pinned(test.node)).unwrap();
            placement::export_method(store, grant, test.caller, target, method).unwrap();
        }
    }
    let saved = test.store.load().unwrap();
    let vm = saved.snapshot.restore().unwrap();
    let copied = other.load().unwrap();
    let second = copied.snapshot.restore().unwrap();
    let registry = placement::read(&vm).unwrap().unwrap();
    assert_eq!(
        registry.hash().unwrap(),
        placement::read(&second).unwrap().unwrap().hash().unwrap()
    );
    assert_eq!(registry.owner().epoch, u64::MAX);
    let generation = test.store.generation();
    assert!(placement::initialize(&mut test.store, test.node, 0).is_err());
    let target = vm.heap().roots()[REGISTRY]; // A known object, with no existing target policy.
    let mut policy = PlacementPolicy::pinned(test.node);
    policy.allowed_nodes.clear();
    assert!(placement::register_object(&mut test.store, target, policy).is_err());
    let mut policy = PlacementPolicy::pinned(test.node);
    policy
        .allowed_nodes
        .extend((0..16).map(|_| NodeId::fresh()));
    assert!(placement::register_object(&mut test.store, target, policy).is_err());
    for invalid in [
        "",
        "../camera",
        "camera/front",
        "é",
        "a\0b",
        &"x".repeat(65),
    ] {
        let mut policy = PlacementPolicy::pinned(test.node);
        policy.required_resources.insert(invalid.into());
        assert!(placement::register_object(&mut test.store, target, policy).is_err());
    }
    let mut policy = PlacementPolicy::pinned(test.node);
    policy
        .required_resources
        .extend((0..17).map(|n| format!("r{n}")));
    assert!(placement::register_object(&mut test.store, target, policy).is_err());
    assert_eq!(test.store.generation(), generation);
}
#[test]
fn record_capacity_is_bounded_and_rejected_insertions_leave_the_committed_registry_unchanged() {
    let mut test = Test::new();
    test.init();
    let saved = test.store.load().unwrap();
    let mut vm = saved.snapshot.restore().unwrap();
    let objects: Vec<_> = (0..placement::MAX_OBJECTS)
        .map(|n| {
            let object = vm
                .new_string(n.to_string().encode_utf16().collect())
                .unwrap();
            vm.set_root(format!("test-placement/{n}"), object).unwrap();
            object
        })
        .collect();
    test.store.save(&vm).unwrap();
    for object in &objects[..placement::MAX_OBJECTS - 1] {
        placement::register_object(&mut test.store, *object, PlacementPolicy::pinned(test.node))
            .unwrap();
    }
    let generation = test.store.generation();
    assert!(
        placement::register_object(
            &mut test.store,
            objects[placement::MAX_OBJECTS - 1],
            PlacementPolicy::pinned(test.node)
        )
        .is_err()
    );
    assert_eq!(test.store.generation(), generation);
    for n in 0..placement::MAX_GRANTS {
        let grant = GrantId::from_bytes((n as u128).to_be_bytes());
        placement::export_method(
            &mut test.store,
            grant,
            test.caller,
            test.target,
            test.method,
        )
        .unwrap();
    }
    let generation = test.store.generation();
    assert!(
        placement::export_method(
            &mut test.store,
            GrantId::fresh(),
            test.caller,
            test.target,
            test.method
        )
        .is_err()
    );
    assert_eq!(test.store.generation(), generation);
    let saved = test.store.load().unwrap();
    let vm = saved.snapshot.restore().unwrap();
    let registry = placement::read(&vm).unwrap().unwrap();
    assert_eq!(registry.objects().count(), placement::MAX_OBJECTS);
    assert_eq!(registry.grants().count(), placement::MAX_GRANTS);
}
