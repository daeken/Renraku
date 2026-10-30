//! Trusted pre-seal compiler bootstrap. Facade possession is never authority.
use crate::{
    checkpoint::CheckpointStore,
    endowment::Endowment,
    error::{Error, Result, require},
    execution::ExecutionStatus,
    heap::AllocationDomain,
    identity::ObjectId,
    program::Program,
    runtime::Vm,
    workbench::{ApiFilter, ReferenceSet, ReferenceView, ScopeBinding},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
const NS: &str = "renraku.bootstrap-v1/";
const ROOT: &str = "renraku.bootstrap-v1/approval";
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Approval {
    format: u32,
    universe: String,
    filter: ApiFilter,
    modules: BTreeMap<String, String>,
    imports: String,
    context: String,
    instructions: u64,
    objects: BTreeSet<String>,
    effects: BTreeSet<(String, String)>,
}
fn hash(b: &[u8]) -> String {
    Sha256::digest(b)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}
fn bytes(a: &Approval) -> Result<Vec<u8>> {
    let b = serde_json::to_vec(a).map_err(|e| Error(e.to_string()))?;
    require(b.len() <= 256 * 1024, "bootstrap approval bound exceeded")?;
    Ok(b)
}
fn modules(p: &Program) -> BTreeMap<String, String> {
    p.bindings()
        .into_iter()
        .map(|b| {
            let mut h = Sha256::new();
            h.update(b"renraku.bootstrap.module.v1\0");
            h.update(&b.bytes);
            for (t, id) in b.types {
                h.update(t.to_le_bytes());
                h.update(id.as_bytes());
            }
            for (t, id) in b.fields {
                h.update(t.to_le_bytes());
                h.update(id.as_bytes());
            }
            for (t, id) in b.methods {
                h.update(t.to_le_bytes());
                h.update(id.method.as_bytes());
                h.update(id.code.as_bytes());
            }
            (
                b.id.to_string(),
                h.finalize().iter().map(|b| format!("{b:02x}")).collect(),
            )
        })
        .collect()
}
fn scope(a: &Approval) -> Result<ScopeBinding> {
    Ok(ScopeBinding {
        format: 1,
        universe: a.universe.clone(),
        approval: hash(&bytes(a)?),
        filter: a.filter.clone(),
    })
}
fn read(vm: &Vm<'_>) -> Result<Option<Approval>> {
    let Some(id) = vm.heap.roots().get(ROOT) else {
        require(
            !vm.heap.roots().keys().any(|n| n.starts_with(NS)),
            "orphan bootstrap roots",
        )?;
        return Ok(None);
    };
    let o = vm.heap.object(*id)?;
    require(
        o.domain() == AllocationDomain::Managed,
        "invalid bootstrap metadata domain",
    )?;
    let units = o
        .string_units()
        .ok_or_else(|| Error("invalid bootstrap metadata".into()))?;
    require(
        units.len() <= 256 * 1024,
        "bootstrap metadata bound exceeded",
    )?;
    let text = String::from_utf16(units).map_err(|_| Error("invalid bootstrap text".into()))?;
    let a: Approval =
        serde_json::from_str(&text).map_err(|_| Error("invalid bootstrap approval".into()))?;
    require(
        bytes(&a)? == text.as_bytes()
            && a.format == 1
            && a.universe == vm.universe_id().to_string(),
        "bootstrap canonical scope mismatch",
    )?;
    a.filter.validate_against(vm.program)?;
    require(
        !a.modules.is_empty()
            && a.modules.len() <= 16
            && a.objects.len() <= 64
            && a.effects.len() <= 16,
        "bootstrap list bounds exceeded",
    )?;
    crate::placement::id(&a.context)?;
    let actual = modules(vm.program);
    require(
        a.modules.iter().all(|(m, h)| actual.get(m) == Some(h))
            && a.imports == crate::endowment::imports_hash(vm)?,
        "bootstrap registry/import binding changed",
    )?;
    let expected: BTreeSet<_> = std::iter::once(ROOT.to_owned())
        .chain(a.objects.iter().map(|id| format!("{NS}object/{id}")))
        .collect();
    require(
        vm.heap
            .roots()
            .keys()
            .filter(|n| n.starts_with(NS))
            .cloned()
            .collect::<BTreeSet<_>>()
            == expected,
        "bootstrap root set mismatch",
    )?;
    for id in &a.objects {
        let id = ObjectId::from_bytes(crate::placement::id(id)?);
        let o = vm.heap.object(id)?;
        require(
            vm.heap.roots().get(&format!("{NS}object/{id}")) == Some(&id)
                && o.domain() == AllocationDomain::Managed
                && a.filter.types.contains(&o.type_id().to_string()),
            "bootstrap object binding mismatch",
        )?;
    }
    for (c, m) in &a.effects {
        crate::model::validate_identifier(c)?;
        crate::model::validate_identifier(m)?;
    }
    Ok(Some(a))
}
/// Capture trusted authority while the dedicated child is idle, then publish only
/// selected declarations. This does not seal or execute the child.
pub fn prepare(store: &mut CheckpointStore, e: &Endowment) -> Result<ReferenceSet> {
    let saved = store.load()?;
    let mut vm = saved.snapshot.restore()?;
    crate::endowment::require_unsealed(&vm)?;
    require(
        matches!(vm.context.status, ExecutionStatus::Completed { .. })
            && vm.invocations.is_empty()
            && vm.effects.is_empty()
            && crate::placement::read(&vm)?.is_none()
            && !vm
                .heap
                .roots()
                .keys()
                .any(|n| n.starts_with("renraku.peer-v1/")),
        "bootstrap requires a dedicated idle child",
    )?;
    let a = Approval {
        format: 1,
        universe: vm.universe_id().to_string(),
        filter: ApiFilter::from_endowment(e),
        modules: modules(vm.program),
        imports: crate::endowment::imports_hash(&vm)?,
        context: vm.context.id.to_string(),
        instructions: vm.instructions_executed,
        objects: e.objects.iter().map(ToString::to_string).collect(),
        effects: e.effects.clone(),
    };
    if let Some(old) = read(&vm)? {
        require(
            bytes(&old)? == bytes(&a)?,
            "bootstrap approval cannot be rebound",
        )?;
        return vm.program.scoped_references(&scope(&old)?);
    }
    require(
        vm.installations.is_empty(),
        "prepare bootstrap before installations",
    )?;
    let set = vm.program.scoped_references(&scope(&a)?)?;
    for id in &e.objects {
        let o = vm.heap.object(*id)?;
        require(
            o.domain() == AllocationDomain::Managed && e.types.contains(&o.type_id()),
            "bootstrap object outside scope",
        )?;
        vm.set_root(format!("{NS}object/{id}"), *id)?;
    }
    let id = ObjectId::fresh();
    let units = std::str::from_utf8(&bytes(&a)?)
        .unwrap()
        .encode_utf16()
        .collect();
    vm.heap
        .allocate_string(id, vm.program.string_type()?, units)?;
    vm.set_root(ROOT, id)?;
    validate(&vm)?;
    store.save(&vm)?;
    Ok(set)
}
pub fn references(store: &mut CheckpointStore) -> Result<ReferenceSet> {
    let s = store.load()?;
    let vm = s.snapshot.restore()?;
    let a = read(&vm)?.ok_or_else(|| Error("bootstrap approval missing".into()))?;
    vm.program.scoped_references(&scope(&a)?)
}
pub(crate) fn authorize_views(vm: &Vm<'_>, views: &[ReferenceView]) -> Result<()> {
    let scoped = views
        .iter()
        .find_map(|v| v.scope().transpose())
        .transpose()?;
    if let Some(given) = scoped {
        let a = read(vm)?.ok_or_else(|| Error("trusted bootstrap approval missing".into()))?;
        require(
            given == scope(&a)?
                && views
                    .iter()
                    .all(|v| v.scope().is_ok_and(|s| s.as_ref() == Some(&given))),
            "compiler view is outside trusted bootstrap approval",
        )?;
    } else {
        require(
            read(vm)?.is_none(),
            "bootstrap installation requires scoped views",
        )?;
    }
    Ok(())
}
pub(crate) fn validate(vm: &Vm<'_>) -> Result<()> {
    if let Some(a) = read(vm)? {
        let actual = modules(vm.program);
        for m in actual.keys().filter(|m| !a.modules.contains_key(*m)) {
            let record = vm
                .installations
                .values()
                .find(|r| r.assembly.to_string() == *m)
                .ok_or_else(|| Error("unapproved bootstrap module".into()))?;
            authorize_views(vm, &record.artifact.views()?)?;
        }
    }
    Ok(())
}
pub(crate) fn admit_seal(vm: &Vm<'_>, e: &Endowment) -> Result<()> {
    let Some(a) = read(vm)? else { return Ok(()) };
    validate(vm)?;
    require(
        vm.context.id.to_string() == a.context && vm.instructions_executed == a.instructions,
        "bootstrap executed before sealing",
    )?;
    let mut allowed = a.filter.clone();
    for b in vm
        .program
        .bindings()
        .into_iter()
        .filter(|b| !a.modules.contains_key(&b.id.to_string()))
    {
        for (_, id) in b.types {
            allowed.types.insert(id.to_string());
            allowed.construct.insert(id.to_string());
        }
        for (_, id) in b.fields {
            allowed.read.insert(id.to_string());
            allowed.write.insert(id.to_string());
        }
        for (_, id) in b.methods {
            allowed.methods.insert(id.code.to_string());
        }
    }
    let f = ApiFilter::from_endowment(e);
    require(
        f.types.is_subset(&allowed.types)
            && f.methods.is_subset(&allowed.methods)
            && f.construct.is_subset(&allowed.construct)
            && f.read.is_subset(&allowed.read)
            && f.write.is_subset(&allowed.write)
            && e.objects
                .iter()
                .all(|id| a.objects.contains(&id.to_string()))
            && e.effects.is_subset(&a.effects),
        "child seal widens bootstrap authority",
    )
}
