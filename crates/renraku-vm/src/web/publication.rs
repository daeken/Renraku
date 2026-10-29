//! Immutable publication receipts in reserved named roots. The current route
//! aliases the latest receipt string; both roots commit in the universe snapshot.
use super::{handler, string};
use crate::{
    checkpoint::{CheckpointStore, CommitStage},
    error::{Error, Result, require},
    identity::MethodId,
    runtime::Vm,
    value::Value,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value as Json, json};

const NAMESPACE: &str = "renraku.http-v1/";
const CURRENT: &str = "renraku.http-v1/current";
const RECEIPTS: &str = "renraku.http-v1/publication/";
pub const MAX_PUBLICATIONS: usize = 128;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PublicationRecord {
    pub format: u32,
    pub universe: String,
    pub id: String,
    pub route: String,
    pub sequence: u32,
    pub method: String,
    pub code: String,
}
fn validate_id(id: &str) -> Result<()> {
    require(
        !id.is_empty()
            && id.len() <= 64
            && id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b)),
        "publication ID must contain 1..64 ASCII letters, digits, dots, underscores, or hyphens",
    )
}
fn pinned_method(vm: &Vm<'_>, record: &PublicationRecord) -> Result<MethodId> {
    let entry = handler(vm.program, &record.method)?;
    let binding = vm.program.method(entry)?.binding;
    require(
        record.method == binding.method.to_string() && record.code == binding.code.to_string(),
        "published handler binding changed or is invalid",
    )?;
    Ok(entry)
}
fn journal(vm: &Vm<'_>) -> Result<Vec<PublicationRecord>> {
    let roots = vm.heap().roots();
    let mut records = Vec::new();
    for (name, object) in roots.iter().filter(|(name, _)| name.starts_with(NAMESPACE)) {
        if name == CURRENT {
            continue;
        }
        let id = name
            .strip_prefix(RECEIPTS)
            .ok_or_else(|| Error("unsupported HTTP publication root".into()))?;
        validate_id(id)?;
        require(
            records.len() < MAX_PUBLICATIONS,
            "HTTP publication limit exceeded",
        )?;
        require(
            vm.heap()
                .object(*object)?
                .string_units()
                .is_some_and(|s| s.len() <= 1024),
            "invalid HTTP publication receipt text",
        )?;
        let record: PublicationRecord =
            serde_json::from_str(&string(vm, Value::Reference(*object))?)
                .map_err(|_| Error("invalid HTTP publication receipt".into()))?;
        require(
            record.format == 1
                && record.route == "default"
                && record.id == id
                && record.universe == vm.universe_id().to_string(),
            "invalid/unsupported HTTP publication receipt binding",
        )?;
        pinned_method(vm, &record)?;
        records.push(record);
    }
    records.sort_by_key(|record| record.sequence);
    for (index, record) in records.iter().enumerate() {
        require(
            record.sequence as usize == index + 1,
            "invalid HTTP publication sequence",
        )?;
    }
    let expected = records
        .last()
        .map(|record| roots[&format!("{RECEIPTS}{}", record.id)]);
    require(
        roots.get(CURRENT).copied() == expected,
        "invalid current HTTP publication root",
    )?;
    Ok(records)
}
pub(super) fn published_method(vm: &Vm<'_>) -> Result<Option<MethodId>> {
    journal(vm)?
        .last()
        .map(|record| pinned_method(vm, record))
        .transpose()
}

/// Read-only projection of the saved route and every immutable publication.
pub fn inspect_publications(store: &mut CheckpointStore) -> Result<Json> {
    let saved = store.load()?;
    let vm = saved.snapshot.restore()?;
    let records = journal(&vm)?;
    Ok(json!({"format":1,"universe":vm.universe_id().to_string(),
        "checkpoint":store.generation(),"route":"default",
        "current":records.last(),"publications":records}))
}
/// A lookup never changes the current publication or advances the checkpoint.
pub fn find_publication(
    store: &mut CheckpointStore,
    id: &str,
) -> Result<Option<PublicationRecord>> {
    validate_id(id)?;
    let saved = store.load()?;
    let vm = saved.snapshot.restore()?;
    Ok(journal(&vm)?.into_iter().find(|record| record.id == id))
}
/// Publish installed CIL without running guest instructions or changing method
/// dispatch. A duplicate ID returns its original receipt, never republishing it.
pub fn publish_handler(
    store: &mut CheckpointStore,
    id: &str,
    selector: &str,
) -> Result<PublicationRecord> {
    publish_handler_with_observer(store, id, selector, |_| Ok(()))
}
pub fn publish_handler_with_observer(
    store: &mut CheckpointStore,
    id: &str,
    selector: &str,
    observer: impl FnMut(CommitStage) -> Result<()>,
) -> Result<PublicationRecord> {
    validate_id(id)?;
    let saved = store.load()?;
    let mut vm = saved.snapshot.restore()?;
    let records = journal(&vm)?;
    let entry = handler(vm.program, selector)?;
    let binding = vm.program.method(entry)?.binding;
    if let Some(record) = records.iter().find(|record| record.id == id) {
        require(
            record.method == binding.method.to_string() && record.code == binding.code.to_string(),
            "publication ID is already bound to a different handler",
        )?;
        return Ok(record.clone());
    }
    require(
        records.len() < MAX_PUBLICATIONS,
        "HTTP publication limit exceeded",
    )?;
    let record = PublicationRecord {
        format: 1,
        universe: vm.universe_id().to_string(),
        id: id.into(),
        route: "default".into(),
        sequence: records.len() as u32 + 1,
        method: binding.method.to_string(),
        code: binding.code.to_string(),
    };
    let text = serde_json::to_string(&record).map_err(|e| Error(e.to_string()))?;
    let object = vm.new_string(text.encode_utf16().collect())?;
    vm.set_root(format!("{RECEIPTS}{id}"), object)?;
    vm.set_root(CURRENT, object)?;
    // This host metadata transition is safe at any guest instruction boundary,
    // including a paused handler. The original frames/heap/code commit with it.
    store.save_with_observer(&vm, observer)?;
    Ok(record)
}
