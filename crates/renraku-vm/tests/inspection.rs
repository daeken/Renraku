#[allow(dead_code)]
mod common;
use renraku_vm::{
    checkpoint::CheckpointStore,
    identity::UniverseId,
    inspection::InspectionLimits,
    runtime::{RunOutcome, Vm},
};
use serde_json::{Value as Json, json};
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};
struct Directory(PathBuf);
impl Directory {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("renraku-inspection-{}", UniverseId::fresh()));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
}
impl Drop for Directory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn cli(database: &Path, command: &str, args: &[&str]) -> Json {
    let result = Command::new(env!("CARGO_BIN_EXE_renraku-vm"))
        .arg(command)
        .arg(database)
        .args(args)
        .env("PATH", "/no-inspection-tools")
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    serde_json::from_slice(&result.stdout).unwrap()
}
fn field<'a>(view: &'a Json, name: &str) -> &'a Json {
    view["fields"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["name"] == name)
        .unwrap()
}
#[test]
fn cyclic_objects_static_roots_and_cli_views_keep_identity_without_writing_a_checkpoint() {
    let directory = Directory::new();
    let database = directory.0.join("universe.db");
    let program = common::program();
    let mut vm = Vm::new(&program);
    vm.start(common::entry(&program, "DurableRun"), vec![])
        .unwrap();
    let mut store = CheckpointStore::open(&database).unwrap();
    assert!(matches!(
        vm.run_checkpointed(&mut store, 10000, 100).unwrap(),
        RunOutcome::Completed(_)
    ));
    let before_heap = vm.heap().clone();
    let before_statics = vm.static_fields().clone();
    let before_instructions = vm.instructions_executed();
    let generation = store.generation();
    let static_ = vm
        .inspect_static("Renraku.Guest.Simulation::Saved", 64)
        .unwrap();
    let id = static_["field"]["value"]["id"].as_str().unwrap();
    let first = vm.inspect_object(id, InspectionLimits::default()).unwrap();
    let second_id = field(&first, "Next")["value"]["id"].as_str().unwrap();
    let second = vm
        .inspect_object(second_id, InspectionLimits::default())
        .unwrap();
    assert_eq!(field(&second, "Next")["value"]["id"], id);
    assert_eq!(first["checkpoint"], generation);
    assert_eq!(vm.heap(), &before_heap);
    assert_eq!(vm.static_fields(), &before_statics);
    assert_eq!(vm.instructions_executed(), before_instructions);
    assert_eq!(store.generation(), generation);
    assert_eq!(
        cli(
            &database,
            "inspect-static",
            &[static_["field"]["id"].as_str().unwrap()]
        ),
        static_
    );
    assert_eq!(cli(&database, "inspect-object", &[id]), first);
    let saved = store.load().unwrap();
    let mut restored = saved.snapshot.restore().unwrap();
    restored.collect_garbage().unwrap();
    store.save(&restored).unwrap();
    let mut expected = first.clone();
    expected["checkpoint"] = json!(store.generation());
    assert_eq!(cli(&database, "inspect-object", &[id]), expected);
    let generation = store.generation();
    let rejected = Command::new(env!("CARGO_BIN_EXE_renraku-vm"))
        .args(["inspect-object", database.to_str().unwrap(), id, "33"])
        .output()
        .unwrap();
    assert!(!rejected.status.success());
    assert_eq!(store.load().unwrap().generation, generation);
}
#[test]
fn inherited_owners_and_exact_surrogate_units_survive_gc_restore_and_native_cli_without_sdk() {
    let directory = Directory::new();
    let database = directory.0.join("universe.db");
    let program = common::program();
    let mut vm = Vm::new(&program);
    vm.start(common::entry(&program, "Inheritance"), vec![])
        .unwrap();
    assert!(matches!(vm.run(1000).unwrap(), RunOutcome::Completed(_)));
    let ty = program.find_type("Renraku.Guest.DerivedCounter").unwrap();
    let id = vm
        .heap()
        .objects()
        .find(|object| object.type_id() == ty)
        .unwrap()
        .id();
    vm.set_root("inspection.derived", id).unwrap();
    let units = vec![65, 0, 0xd800, 66, 0xd83e, 0xdd80];
    let string = vm.new_string(units.clone()).unwrap();
    vm.set_root("inspection.string", string).unwrap();
    vm.collect_garbage().unwrap();
    let mut store = CheckpointStore::open(&database).unwrap();
    store.save(&vm).unwrap();
    let derived = cli(&database, "inspect-object", &[&id.to_string()]);
    assert_eq!(
        field(&derived, "BaseValue")["owner"]["name"],
        "Renraku.Guest.BaseCounter"
    );
    assert_eq!(
        field(&derived, "Extra")["owner"]["name"],
        "Renraku.Guest.DerivedCounter"
    );
    let view = cli(
        &database,
        "inspect-object",
        &[&string.to_string(), "16", "6"],
    );
    assert_eq!(view["stringPreview"]["units"], json!(units));
    assert!(view["stringPreview"]["text"].is_null());
    assert_eq!(view["stringPreview"]["truncated"], false);
    let short = cli(
        &database,
        "inspect-object",
        &[&string.to_string(), "16", "2"],
    );
    assert_eq!(short["stringPreview"]["text"], "A\0");
    assert_eq!(short["stringPreview"]["truncated"], true);
    let saved = store.load().unwrap();
    let restored = saved.snapshot.restore().unwrap();
    assert_eq!(
        restored
            .heap()
            .object(string)
            .unwrap()
            .string_units()
            .unwrap(),
        units
    );
}
