#[allow(dead_code)]
mod common;
use common::fixtures;
use renraku_vm::{
    program::Program,
    workbench::{ReferenceSet, ReferenceView, check_reference_views},
};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::fs;

fn program(names: &[&str]) -> Program {
    let mut bytes = vec![fixtures().core.clone()];
    bytes.extend(
        names
            .iter()
            .map(|name| fs::read(fixtures().output.join(format!("{name}.dll"))).unwrap()),
    );
    Program::load(&bytes.iter().map(Vec::as_slice).collect::<Vec<_>>()).unwrap()
}
fn digest(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}
fn rewrite(view: &mut ReferenceView, change: impl FnOnce(&mut Value)) {
    let mut data: Value = serde_json::from_str(&view.schema_json).unwrap();
    change(&mut data);
    view.schema_json = serde_json::to_string(&data).unwrap();
    view.schema_hash = digest(view.schema_json.as_bytes());
}
#[test]
fn dependency_sets_merge_roots_and_preserve_isolated_legacy_wire() {
    let program = program(&["Renraku.Collections", "Cross.Owner"]);
    let owner = program.find_type("CrossOwner").unwrap();
    let map = program.find_type("Renraku.Collections.StringMap").unwrap();
    let set = program.reference_set(&[owner]).unwrap();
    assert_eq!(set.views.len(), 2);
    set.validate(&program).unwrap();
    let a = serde_json::from_str::<Value>(&set.views[0].schema_json).unwrap();
    let b = serde_json::from_str::<Value>(&set.views[1].schema_json).unwrap();
    assert_eq!(a["moduleName"], "Renraku.Collections");
    assert_eq!(b["moduleName"], "Cross.Owner");
    assert_eq!(b["dependencies"][0]["schemaHash"], set.views[0].schema_hash);
    assert!(a["declaration"].as_str().unwrap().contains("StringMap"));
    assert!(
        !b["declaration"]
            .as_str()
            .unwrap()
            .contains("class @StringMap")
    );
    let same = program.reference_set(&[map, owner, owner]).unwrap();
    assert_eq!(same.to_json().unwrap(), set.to_json().unwrap());
    assert_eq!(
        ReferenceSet::from_json(
            &serde_json::to_vec_pretty(
                &serde_json::from_slice::<Value>(&set.to_json().unwrap()).unwrap()
            )
            .unwrap()
        )
        .unwrap()
        .set_hash,
        set.set_hash
    );
    assert_eq!(
        ReferenceSet::parse_input(&set.export_json().unwrap())
            .unwrap()
            .len(),
        2
    );
    let legacy = program.reference_view(&[map]).unwrap();
    let isolated = program.reference_set(&[map]).unwrap();
    assert_eq!(legacy.to_json().unwrap(), isolated.export_json().unwrap());
    legacy.validate(&program).unwrap();
    assert_eq!(
        ReferenceSet::parse_input(&legacy.to_json().unwrap())
            .unwrap()
            .len(),
        1
    );
    let mut reversed = set.views.clone();
    reversed.reverse();
    assert_eq!(
        ReferenceSet::from_views(reversed).unwrap().set_hash,
        set.set_hash
    );
}
#[test]
fn missing_changed_ambiguous_cyclic_and_wrong_universe_dependencies_reject() {
    let live = program(&["Renraku.Collections", "Cross.Owner"]);
    let root = live.find_type("CrossOwner").unwrap();
    let set = live.reference_set(&[root]).unwrap();
    assert!(
        check_reference_views(&set.views[1..])
            .unwrap_err()
            .0
            .contains("missing")
    );
    let mut changed = set.views.clone();
    rewrite(&mut changed[0], |s| {
        s["declaration"] = Value::String("public class Changed {}".into())
    });
    assert!(
        check_reference_views(&changed)
            .unwrap_err()
            .0
            .contains("hash")
    );
    let mut changed = set.views.clone();
    rewrite(&mut changed[1], |s| {
        s["dependencies"][0]["types"][0] = Value::String("missing-type".into())
    });
    assert!(
        check_reference_views(&changed)
            .unwrap_err()
            .0
            .contains("type binding")
    );
    let mut wrong_body = set.views.clone();
    rewrite(&mut wrong_body[1], |s| {
        s["declaration"] = Value::String("public class Forged {}".into())
    });
    let forged = ReferenceSet::from_views(wrong_body).unwrap();
    assert!(forged.validate(&live).is_err());
    assert!(
        set.validate(&program(&["Renraku.Collections", "Cross.Owner"]))
            .is_err()
    );
    let mut envelope: Value = serde_json::from_slice(&set.to_json().unwrap()).unwrap();
    envelope["setHash"] = Value::String("wrong".into());
    assert!(ReferenceSet::from_json(&serde_json::to_vec(&envelope).unwrap()).is_err());
    let cyclic = program(&["Cycle.Left", "Cycle.Right"]);
    assert!(
        cyclic
            .reference_set(&[cyclic.find_type("Cycle.Left.Node").unwrap()])
            .unwrap_err()
            .0
            .contains("cycle")
    );
    let ambiguous = program(&["Ambiguous.One", "Ambiguous.Two"]);
    assert!(ambiguous.find_type("Duplicate.Node").is_err());
    let roots = ambiguous
        .types()
        .filter(|t| t.name == "Duplicate.Node")
        .map(|t| t.id)
        .collect::<Vec<_>>();
    assert!(
        ambiguous
            .reference_set(&roots)
            .unwrap_err()
            .0
            .contains("ambiguous")
    );
    // Preserve old independent-view contracts. New sets reject ambiguous names,
    // but previously captured isolated views still validate independently.
    let legacy = roots
        .iter()
        .map(|id| ambiguous.reference_view(&[*id]).unwrap())
        .collect::<Vec<_>>();
    check_reference_views(&legacy).unwrap();
    renraku_vm::workbench::validate_reference_views(&legacy, &ambiguous).unwrap();
    assert!(live.reference_set(&[]).is_err());
    assert!(
        live.reference_set(&[live.find_type("System.Object").unwrap()])
            .is_err()
    );
}
