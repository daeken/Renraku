//! Closed child execution with immutable, explicit authority. Raw Vm/Program and
//! operator tools remain trusted; child-facing tools below never expose them.
use crate::{
    checkpoint::CheckpointStore,
    cil::Op,
    error::{Error, Result, require},
    execution::{Continuation, ExecutionStatus},
    heap::AllocationDomain,
    identity::{CodeVersionId, FieldId, ObjectId, TypeId},
    program::{Method, Program, SlotType},
    runtime::Vm,
    value::Value,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value as Json, json};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
const NS: &str = "renraku.child-v1/";
const POLICY: &str = "renraku.child-v1/policy";
const OWNED: &str = "renraku.child-v1/owned";
const MAX_BYTES: usize = 256 * 1024;
pub const MAX_ALLOCATIONS: usize = 256;
#[derive(Clone, Debug, Default)]
pub struct Endowment {
    pub types: BTreeSet<TypeId>,
    pub methods: BTreeSet<CodeVersionId>,
    pub construct: BTreeSet<TypeId>,
    pub read: BTreeSet<FieldId>,
    pub write: BTreeSet<FieldId>,
    pub objects: BTreeSet<ObjectId>,
    pub effects: BTreeSet<(String, String)>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Policy {
    format: u32,
    universe: String,
    registry: String,
    imports: String,
    types: Vec<String>,
    methods: Vec<String>,
    construct: Vec<String>,
    read: Vec<String>,
    write: Vec<String>,
    objects: Vec<String>,
    effects: Vec<(String, String)>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Owned {
    format: u32,
    policy: String,
    allocations: Vec<(String, String)>,
}
fn deny(ok: bool) -> Result<()> {
    require(ok, "child authority denied")
}
fn json_bytes<T: Serialize>(v: &T) -> Result<Vec<u8>> {
    let b = serde_json::to_vec(v).map_err(|_| Error("invalid child metadata".into()))?;
    require(b.len() <= MAX_BYTES, "child metadata bound exceeded")?;
    Ok(b)
}
fn hash(b: &[u8]) -> String {
    Sha256::digest(b)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}
fn tid(s: &str) -> Result<TypeId> {
    Ok(TypeId::from_bytes(crate::placement::id(s)?))
}
fn oid(s: &str) -> Result<ObjectId> {
    Ok(ObjectId::from_bytes(crate::placement::id(s)?))
}
fn cid(s: &str) -> Result<CodeVersionId> {
    Ok(CodeVersionId::from_bytes(crate::placement::id(s)?))
}
fn fid(s: &str) -> Result<FieldId> {
    Ok(FieldId::from_bytes(crate::placement::id(s)?))
}
fn registry(program: &Program) -> String {
    let mut h = Sha256::new();
    h.update(b"renraku.child.registry.v1\0");
    for b in program.bindings() {
        h.update(b.id.as_bytes());
        h.update(Sha256::digest(&b.bytes));
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
    }
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}
fn imports_hash(vm: &Vm<'_>) -> Result<String> {
    let mut h = Sha256::new();
    h.update(b"renraku.child.imports.v1\0");
    for (name, id) in vm
        .heap
        .roots()
        .iter()
        .filter(|(n, _)| n.starts_with("renraku.remote-v1/"))
    {
        h.update((name.len() as u64).to_le_bytes());
        h.update(name.as_bytes());
        h.update(id.as_bytes());
        let o = vm.heap.object(*id)?;
        h.update(o.type_id().as_bytes());
        if let Some(units) = o.string_units() {
            for unit in units {
                h.update(unit.to_le_bytes());
            }
        }
    }
    Ok(h.finalize().iter().map(|b| format!("{b:02x}")).collect())
}
fn get<T: serde::de::DeserializeOwned + Serialize>(vm: &Vm<'_>, name: &str) -> Result<T> {
    let id = vm
        .heap
        .roots()
        .get(name)
        .ok_or_else(|| Error("child metadata root missing".into()))?;
    let object = vm.heap.object(*id)?;
    require(
        object.domain() == AllocationDomain::Managed,
        "transient child metadata",
    )?;
    let units = object
        .string_units()
        .ok_or_else(|| Error("child metadata must be a string".into()))?;
    require(units.len() <= MAX_BYTES, "child metadata bound exceeded")?;
    let bytes = String::from_utf16(units)
        .map_err(|_| Error("invalid child metadata UTF16".into()))?
        .into_bytes();
    require(bytes.len() <= MAX_BYTES, "child metadata bound exceeded")?;
    let doc: T =
        serde_json::from_slice(&bytes).map_err(|_| Error("invalid child metadata JSON".into()))?;
    require(json_bytes(&doc)? == bytes, "noncanonical child metadata")?;
    Ok(doc)
}
fn put(vm: &mut Vm<'_>, name: &str, bytes: &[u8]) -> Result<()> {
    let mut id = ObjectId::fresh();
    while vm.heap.object(id).is_ok() {
        id = ObjectId::fresh();
    }
    let units = std::str::from_utf8(bytes)
        .map_err(|_| Error("invalid child metadata bytes".into()))?
        .encode_utf16()
        .collect();
    vm.heap
        .allocate_string(id, vm.program.string_type()?, units)?;
    vm.set_root(name, id)
}
fn sorted(ids: &[String], max: usize) -> Result<()> {
    require(
        ids.len() <= max && ids.windows(2).all(|w| w[0] < w[1]),
        "child list bound/order violation",
    )?;
    for id in ids {
        crate::placement::id(id)?;
    }
    Ok(())
}
fn read(vm: &Vm<'_>) -> Result<Option<Policy>> {
    if !vm.heap.roots().contains_key(POLICY) {
        require(
            !vm.heap.roots().keys().any(|n| n.starts_with(NS)),
            "orphan child metadata",
        )?;
        return Ok(None);
    }
    let p: Policy = get(vm, POLICY)?;
    require(
        p.format == 1 && p.universe == vm.universe_id().to_string(),
        "child policy universe/version mismatch",
    )?;
    sorted(&p.types, 128)?;
    sorted(&p.methods, 256)?;
    sorted(&p.construct, 128)?;
    sorted(&p.read, 256)?;
    sorted(&p.write, 256)?;
    sorted(&p.objects, 64)?;
    require(
        p.effects.len() <= 16 && p.effects.windows(2).all(|w| w[0] < w[1]),
        "child effect list bound/order violation",
    )?;
    for (c, m) in &p.effects {
        crate::model::validate_identifier(c)?;
        crate::model::validate_identifier(m)?;
    }
    Ok(Some(p))
}
fn contains(ids: &[String], id: impl ToString) -> bool {
    ids.binary_search(&id.to_string()).is_ok()
}
fn owned(vm: &Vm<'_>, p: &Policy) -> Result<Owned> {
    let o: Owned = get(vm, OWNED)?;
    require(
        o.format == 1
            && o.policy == hash(&json_bytes(p)?)
            && o.allocations.len() <= MAX_ALLOCATIONS
            && o.allocations.windows(2).all(|w| w[0].0 < w[1].0),
        "child allocation ledger mismatch",
    )?;
    for (id, t) in &o.allocations {
        oid(id)?;
        tid(t)?;
    }
    Ok(o)
}
fn value_with(vm: &Vm<'_>, p: &Policy, value: Value) -> Result<()> {
    if let Value::Reference(id) = value {
        let known = contains(&p.objects, id)
            || owned(vm, p)?
                .allocations
                .iter()
                .any(|(key, _)| *key == id.to_string());
        deny(known)?;
        let o = vm.heap.object(id)?;
        deny(o.domain() == AllocationDomain::Managed && contains(&p.types, o.type_id()))?;
        if let Some(units) = o.string_units() {
            deny(units.len() <= 4096)?;
        }
        if !contains(&p.objects, id) {
            deny(
                owned(vm, p)?
                    .allocations
                    .iter()
                    .any(|(key, ty)| *key == id.to_string() && *ty == o.type_id().to_string()),
            )?;
        }
    }
    Ok(())
}
pub(crate) fn value(vm: &Vm<'_>, v: Value) -> Result<()> {
    if let Some(p) = read(vm)? {
        value_with(vm, &p, v)?;
    }
    Ok(())
}
pub(crate) fn ty(vm: &Vm<'_>, t: TypeId) -> Result<()> {
    if let Some(p) = read(vm)? {
        deny(contains(&p.types, t))?;
    }
    Ok(())
}
pub(crate) fn call(vm: &Vm<'_>, m: &Method, args: &[Value], constructing: bool) -> Result<()> {
    if let Some(p) = read(vm)? {
        deny(contains(&p.methods, m.binding.code))?;
        if constructing {
            deny(contains(&p.construct, m.owner))?;
        }
        for v in args {
            value_with(vm, &p, *v)?;
        }
    }
    Ok(())
}
pub(crate) fn field(vm: &Vm<'_>, f: FieldId, write: bool) -> Result<()> {
    if let Some(p) = read(vm)? {
        deny(contains(if write { &p.write } else { &p.read }, f))?;
    }
    Ok(())
}
pub(crate) fn effect(vm: &Vm<'_>, capability: &str, model: &str) -> Result<()> {
    if let Some(p) = read(vm)? {
        deny(p.effects.iter().any(|(c, m)| c == capability && m == model))?;
    }
    Ok(())
}
pub(crate) fn execution(vm: &Vm<'_>, m: &Method) -> Result<()> {
    if let Some(p) = read(vm)? {
        deny(contains(&p.methods, m.binding.code))?;
    }
    Ok(())
}
pub(crate) fn unchanged(vm: &Vm<'_>, program: &Program) -> Result<()> {
    if let Some(p) = read(vm)? {
        deny(p.registry == registry(program))?;
    }
    Ok(())
}
pub(crate) fn require_unsealed(vm: &Vm<'_>) -> Result<()> {
    deny(read(vm)?.is_none())
}
fn slot(p: &Policy, s: SlotType) -> Result<()> {
    if let SlotType::Reference(t) = s {
        deny(contains(&p.types, t))?;
    }
    Ok(())
}
fn admission(vm: &Vm<'_>, p: &Policy) -> Result<()> {
    require(
        p.registry == registry(vm.program) && p.imports == imports_hash(vm)?,
        "child registry/import binding changed",
    )?;
    for t in &p.types {
        let t = vm
            .program
            .types
            .get(&tid(t)?)
            .ok_or_else(|| Error("child type missing".into()))?;
        require(t.name.len() <= 512, "child type name exceeds bound")?;
        if let Some(b) = t.base {
            deny(contains(&p.types, b))?;
        }
    }
    for t in &p.construct {
        let id = tid(t)?;
        deny(contains(&p.types, id))?;
        vm.program.allocatable(id)?;
        let mut current = Some(id);
        let mut fields = 0usize;
        while let Some(t) = current {
            let ty = &vm.program.types[&t];
            fields += ty
                .fields
                .iter()
                .filter(|id| !vm.program.fields[id].is_static)
                .count();
            current = ty.base;
        }
        require(fields <= 64, "child allocation layout exceeds 64 fields")?;
    }
    for id in p.read.iter().chain(&p.write) {
        let f = vm
            .program
            .fields
            .get(&fid(id)?)
            .ok_or_else(|| Error("child field missing".into()))?;
        require(f.name.len() <= 256, "child field name exceeds bound")?;
        deny(!f.literal)?;
        deny(contains(&p.types, f.owner))?;
        slot(p, f.value_type)?;
    }
    let mut instructions = 0usize;
    for id in &p.methods {
        let m = vm
            .program
            .methods
            .get(&cid(id)?)
            .ok_or_else(|| Error("child method missing".into()))?;
        require(m.name.len() <= 256, "child method name exceeds bound")?;
        require(
            m.locals.len() <= 256 && m.max_stack <= 256,
            "child frame layout exceeds 256 slots",
        )?;
        deny(contains(&p.types, m.owner))?;
        require(
            m.signature.parameters.len() <= 16,
            "child method exceeds 16 arguments",
        )?;
        for s in m
            .signature
            .parameters
            .iter()
            .chain(m.signature.result.iter())
            .chain(m.locals.iter())
        {
            slot(p, *s)?;
        }
        instructions += m.instructions.len();
        require(
            instructions <= 16384,
            "child code exceeds 16384 instructions",
        )?;
        for i in m.instructions.values() {
            match i.op {
                Op::Call { token, .. } | Op::NewObject(token) => {
                    let c = vm
                        .program
                        .method(vm.program.method_tokens[&(m.assembly, token)])?;
                    deny(contains(&p.methods, c.binding.code))?;
                    if matches!(i.op, Op::NewObject(_)) {
                        deny(contains(&p.construct, c.owner))?;
                    }
                }
                Op::LoadField { token, .. } | Op::StoreField { token, .. } => {
                    let f = vm.program.field_tokens[&(m.assembly, token)];
                    deny(contains(
                        if matches!(i.op, Op::StoreField { .. }) {
                            &p.write
                        } else {
                            &p.read
                        },
                        f,
                    ))?;
                }
                Op::ReferenceCast { token, .. } => deny(contains(
                    &p.types,
                    vm.program.reference_cast_target(m.assembly, token)?,
                ))?,
                Op::LoadString(token) => {
                    deny(contains(&p.types, vm.program.string_type()?))?;
                    deny(vm.program.string_literals[&(m.assembly, token)].len() <= 4096)?;
                }
                _ => {}
            }
        }
    }
    Ok(())
}
/// Trusted seal of a dedicated idle child. Immutable thereafter; no policy update,
/// append-only install, or import expansion is available in this closed profile.
pub fn seal(store: &mut CheckpointStore, endowment: &Endowment) -> Result<()> {
    let saved = store.load()?;
    let mut vm = saved.snapshot.restore()?;
    require_unsealed(&vm)?;
    deny(
        matches!(vm.context.status, ExecutionStatus::Completed { .. })
            && vm.invocations.is_empty()
            && vm.effects.is_empty()
            && crate::placement::read(&vm)?.is_none()
            && !vm
                .heap
                .roots()
                .keys()
                .any(|n| n.starts_with("renraku.peer-v1/")),
    )?;
    let p = Policy {
        format: 1,
        universe: vm.universe_id().to_string(),
        registry: registry(vm.program),
        imports: imports_hash(&vm)?,
        types: endowment.types.iter().map(ToString::to_string).collect(),
        methods: endowment.methods.iter().map(ToString::to_string).collect(),
        construct: endowment
            .construct
            .iter()
            .map(ToString::to_string)
            .collect(),
        read: endowment.read.iter().map(ToString::to_string).collect(),
        write: endowment.write.iter().map(ToString::to_string).collect(),
        objects: endowment.objects.iter().map(ToString::to_string).collect(),
        effects: endowment.effects.iter().cloned().collect(),
    };
    admission(&vm, &p)?;
    for id in &p.objects {
        let object = vm.heap.object(oid(id)?)?;
        deny(object.domain() == AllocationDomain::Managed && contains(&p.types, object.type_id()))?;
        vm.set_root(format!("{NS}object/{id}"), object.id())?;
    }
    let o = Owned {
        format: 1,
        policy: hash(&json_bytes(&p)?),
        allocations: vec![],
    };
    put(&mut vm, POLICY, &json_bytes(&p)?)?;
    put(&mut vm, OWNED, &json_bytes(&o)?)?;
    // Sealing never resumes a pre-seal continuation or exports its prior result.
    vm.context.frames.clear();
    vm.context.status = ExecutionStatus::Completed { result: None };
    vm.entry = None;
    vm.set_frame_limit(64)?;
    validate(&vm)?;
    store.save(&vm)?;
    Ok(())
}
/// Preflight metadata before allocation; commit only after the checked heap step.
pub(crate) struct Allocation {
    bytes: Vec<u8>,
}
pub(crate) fn allocation(vm: &Vm<'_>, id: ObjectId, t: TypeId) -> Result<Option<Allocation>> {
    let Some(p) = read(vm)? else { return Ok(None) };
    deny(contains(&p.types, t) && (t == vm.program.string_type()? || contains(&p.construct, t)))?;
    let mut o = owned(vm, &p)?;
    if let Some((_, recorded)) = o.allocations.iter().find(|(key, _)| *key == id.to_string()) {
        deny(*recorded == t.to_string() && vm.heap.object(id).is_ok())?;
        return Ok(None);
    }
    if contains(&p.objects, id) {
        deny(vm.heap.object(id)?.type_id() == t)?;
        return Ok(None);
    }
    deny(o.allocations.len() < MAX_ALLOCATIONS)?;
    o.allocations.push((id.to_string(), t.to_string()));
    o.allocations.sort();
    Ok(Some(Allocation {
        bytes: json_bytes(&o)?,
    }))
}
pub(crate) fn allocated(vm: &mut Vm<'_>, a: Option<Allocation>) -> Result<()> {
    if let Some(a) = a {
        put(vm, OWNED, &a.bytes)?;
    }
    Ok(())
}
pub(crate) fn validate(vm: &Vm<'_>) -> Result<()> {
    let Some(p) = read(vm)? else { return Ok(()) };
    admission(vm, &p)?;
    require(vm.max_frames <= 64, "child frame depth limit exceeds 64")?;
    let o = owned(vm, &p)?;
    let mut expected = BTreeSet::from([POLICY.to_owned(), OWNED.to_owned()]);
    for id in &p.objects {
        let id = oid(id)?;
        deny(vm.heap.roots().get(&format!("{NS}object/{id}")) == Some(&id))?;
        value_with(vm, &p, Value::Reference(id))?;
        expected.insert(format!("{NS}object/{id}"));
    }
    deny(
        vm.heap
            .roots()
            .keys()
            .filter(|n| n.starts_with(NS))
            .cloned()
            .collect::<BTreeSet<_>>()
            == expected,
    )?;
    for (id, t) in o.allocations {
        let id = oid(&id)?;
        let ty = tid(&t)?;
        deny(
            !contains(&p.objects, id)
                && contains(&p.types, ty)
                && (ty == vm.program.string_type()? || contains(&p.construct, ty)),
        )?;
        if let Ok(object) = vm.heap.object(id) {
            deny(object.type_id() == ty && object.domain() == AllocationDomain::Managed)?;
        }
    }
    for f in &vm.context.frames {
        execution(vm, vm.current_method(f)?)?;
        for v in f
            .arguments
            .iter()
            .chain(f.locals.iter().flatten())
            .chain(&f.evaluation_stack)
        {
            value_with(vm, &p, *v)?;
        }
        if let Continuation::AwaitReturn {
            result: crate::execution::CallResult::ConstructedObject(id),
            ..
        } = f.continuation
        {
            value_with(vm, &p, Value::Reference(id))?;
        }
    }
    if let ExecutionStatus::Completed { result: Some(v) } = vm.context.status {
        value_with(vm, &p, v)?;
    }
    for e in vm.effects.values() {
        effect(vm, &e.capability, &e.model)?;
        for id in e.references() {
            value_with(vm, &p, Value::Reference(id))?;
        }
    }
    for r in vm.invocations.values() {
        let m = vm
            .program
            .methods
            .get(&r.method.code)
            .ok_or_else(|| Error("child receipt code missing".into()))?;
        execution(vm, m)?;
        for v in &r.arguments {
            value_with(vm, &p, *v)?;
        }
        if let crate::invocation::InvocationState::Completed { result: Some(v) } = r.state {
            value_with(vm, &p, v)?;
        }
    }
    Ok(())
}
/// Child-facing metadata only. No raw Vm/Program, source, manifests, root names,
/// compiler paths, module discovery or inherited inspector is exposed.
pub fn search(store: &mut CheckpointStore, query: &str, limit: usize) -> Result<Json> {
    require(
        query.len() <= 256 && (1..=100).contains(&limit),
        "invalid child search bounds",
    )?;
    let saved = store.load()?;
    let vm = saved.snapshot.restore()?;
    let p = read(&vm)?.ok_or_else(|| Error("child policy missing".into()))?;
    let mut rows = Vec::new();
    for t in &p.types {
        let ty = &vm.program.types[&tid(t)?];
        rows.push((
            ty.name.clone(),
            json!({"kind":"type","id":t,"name":ty.name}),
        ));
    }
    for c in &p.methods {
        let m = &vm.program.methods[&cid(c)?];
        let name = format!("{}::{}", vm.program.types[&m.owner].name, m.name);
        rows.push((
            name.clone(),
            json!({"kind":"method","id":m.binding.method.to_string(),"codeVersion":c,"name":name}),
        ));
    }
    for f in p.read.iter().chain(&p.write).collect::<BTreeSet<_>>() {
        let field = &vm.program.fields[&fid(f)?];
        let name = format!("{}::{}", vm.program.types[&field.owner].name, field.name);
        rows.push((name.clone(),json!({"kind":"field","id":f,"name":name,"static":field.is_static,"read":contains(&p.read,field.id),"write":contains(&p.write,field.id)})));
    }
    rows.sort_by(|a, b| a.0.cmp(&b.0));
    let rows: Vec<_> = rows
        .into_iter()
        .filter(|(n, _)| n.to_lowercase().contains(&query.to_lowercase()))
        .take(limit)
        .map(|(_, v)| v)
        .collect();
    bounded(json!({"format":1,"symbols":rows}))
}
fn projection(vm: &Vm<'_>, p: &Policy, v: Value) -> Result<Json> {
    value_with(vm, p, v)?;
    Ok(match v {
        Value::I32(n) => json!({"kind":"i32","value":n}),
        Value::I64(n) => json!({"kind":"i64","value":n.to_string()}),
        Value::Null => json!({"kind":"null"}),
        Value::Reference(id) => {
            let object = vm.heap.object(id)?;
            let mut j =
                json!({"kind":"object","id":id.to_string(),"typeId":object.type_id().to_string()});
            if let Some(s) = object.string_units() {
                j["utf16"] = json!(s.iter().take(64).copied().collect::<Vec<_>>());
                j["truncated"] = json!(s.len() > 64);
            }
            j
        }
    })
}
pub fn inspect_object(store: &mut CheckpointStore, id: ObjectId) -> Result<Json> {
    let saved = store.load()?;
    let vm = saved.snapshot.restore()?;
    let p = read(&vm)?.ok_or_else(|| Error("child policy missing".into()))?;
    value_with(&vm, &p, Value::Reference(id))?;
    let object = vm.heap.object(id)?;
    let mut fields = Vec::new();
    for (id, v) in object
        .fields()
        .iter()
        .filter(|(id, _)| contains(&p.read, **id))
        .take(32)
    {
        let f = &vm.program.fields[id];
        let value = projection(&vm, &p, *v).unwrap_or_else(|_| json!({"kind":"denied"}));
        fields.push(json!({"id":id.to_string(),"name":f.name,"value":value}));
    }
    bounded(json!({"format":1,"object":projection(&vm,&p,Value::Reference(id))?,"fields":fields}))
}
pub fn inspect_static(store: &mut CheckpointStore, id: FieldId) -> Result<Json> {
    let saved = store.load()?;
    let vm = saved.snapshot.restore()?;
    let p = read(&vm)?.ok_or_else(|| Error("child policy missing".into()))?;
    deny(contains(&p.read, id))?;
    let f = vm
        .program
        .fields
        .get(&id)
        .ok_or_else(|| Error("child authority denied".into()))?;
    deny(f.is_static && !f.literal)?;
    let value = *vm
        .statics
        .get(&id)
        .ok_or_else(|| Error("child authority denied".into()))?;
    bounded(json!({"format":1,"field":id.to_string(),"value":projection(&vm,&p,value)?}))
}
pub fn read_source_or_code(_store: &mut CheckpointStore, _selector: &str) -> Result<Json> {
    Err(Error(
        "child source/CIL access is not granted by this profile".into(),
    ))
}
pub(crate) fn text(vm: &Vm<'_>, len: usize) -> Result<()> {
    if read(vm)?.is_some() {
        deny(len <= 4096)?;
    }
    Ok(())
}
fn bounded(v: Json) -> Result<Json> {
    require(
        serde_json::to_vec(&v)
            .map_err(|_| Error("invalid child projection".into()))?
            .len()
            <= 12288,
        "child projection exceeds 12288 bytes",
    )?;
    Ok(v)
}
pub fn describe_type(store: &mut CheckpointStore, id: TypeId) -> Result<Json> {
    let saved = store.load()?;
    let vm = saved.snapshot.restore()?;
    let p = read(&vm)?.ok_or_else(|| Error("child policy missing".into()))?;
    deny(contains(&p.types, id))?;
    let ty = &vm.program.types[&id];
    let slot = |s: SlotType| match s {
        SlotType::Reference(t) => json!({"kind":"reference","typeId":t.to_string()}),
        _ => json!({"kind":format!("{s:?}")}),
    };
    let methods:Vec<_>=p.methods.iter().filter_map(|c|vm.program.methods.get(&cid(c).ok()?)).filter(|m|m.owner==id).map(|m|json!({"id":m.binding.method.to_string(),"codeVersion":m.binding.code.to_string(),"name":m.name,"instance":m.signature.instance,"parameters":m.signature.parameters.iter().copied().map(slot).collect::<Vec<_>>(),"result":m.signature.result.map(slot)})).collect();
    bounded(
        json!({"format":1,"typeId":id.to_string(),"name":ty.name,"base":ty.base.map(|id|id.to_string()),"construct":contains(&p.construct,id),"methods":methods,"sourceGranted":false,"compilerReferencesGranted":false}),
    )
}
/// Closed child invocation; results and stop data contain no operator inspection.
pub fn invoke_once(
    store: &mut CheckpointStore,
    operation: &str,
    method: CodeVersionId,
    arguments: Vec<Value>,
    budget: u64,
) -> Result<Json> {
    require(budget <= 150000, "child instruction budget exceeds 150000")?;
    let saved = store.load()?;
    let mut vm = saved.snapshot.restore()?;
    let p = read(&vm)?.ok_or_else(|| Error("child policy missing".into()))?;
    deny(contains(&p.methods, method))?;
    let binding = vm.program.methods[&method].binding;
    let receipt = vm.invoke_once(store, operation, binding.method, arguments, budget, 1024)?;
    let mut report =
        json!({"format":1,"operation":operation,"execution":receipt.execution.to_string()});
    match receipt.state {
        crate::invocation::InvocationState::Running => {
            report["state"] = json!("running");
            report["waitingRemote"] = json!(matches!(
                vm.context.status,
                ExecutionStatus::WaitingRemote { .. }
            ));
            report["waitingEffect"] =
                json!(matches!(vm.context.status, ExecutionStatus::Waiting { .. }));
        }
        crate::invocation::InvocationState::Completed { result } => {
            report["state"] = json!("completed");
            report["result"] = result
                .map(|v| projection(&vm, &p, v))
                .transpose()?
                .unwrap_or(Json::Null);
        }
        crate::invocation::InvocationState::Faulted { .. } => {
            report["state"] = json!("faulted");
            report["diagnostic"] =
                json!("child execution fault; trusted operator retains full receipt");
        }
    }
    bounded(report)
}
pub const MAX_TOOL_BYTES: usize = 16384;
#[derive(Deserialize)]
#[serde(
    tag = "kind",
    content = "value",
    rename_all = "snake_case",
    deny_unknown_fields
)]
enum ToolArgument {
    I32(i32),
    I64(String),
    Null,
    Object(String),
}
impl ToolArgument {
    fn value(self) -> Result<Value> {
        Ok(match self {
            Self::I32(n) => Value::I32(n),
            Self::I64(s) => {
                let n: i64 = s.parse().map_err(|_| Error("invalid child I64".into()))?;
                require(n.to_string() == s, "noncanonical child I64")?;
                Value::I64(n)
            }
            Self::Null => Value::Null,
            Self::Object(s) => Value::Reference(oid(&s)?),
        })
    }
}
#[derive(Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
enum Tool {
    Search {
        query: String,
        limit: usize,
    },
    Type {
        #[serde(rename = "typeId")]
        type_id: String,
    },
    Object {
        #[serde(rename = "objectId")]
        object_id: String,
    },
    Static {
        #[serde(rename = "fieldId")]
        field_id: String,
    },
    Invoke {
        operation: String,
        code: String,
        arguments: Vec<ToolArgument>,
        budget: u64,
    },
}
/// One fixed child protocol. Host/compiler paths, root enumeration, source,
/// registry images, policy mutation, installation and generic effects are absent.
pub fn tool(store: &mut CheckpointStore, request: &[u8]) -> Result<Json> {
    require(
        request.len() <= MAX_TOOL_BYTES,
        "child tool input exceeds bound",
    )?;
    let request: Tool = serde_json::from_slice(request)
        .map_err(|_| Error("unsupported child tool request".into()))?;
    match request {
        Tool::Search { query, limit } => search(store, &query, limit),
        Tool::Type { type_id } => describe_type(store, tid(&type_id)?),
        Tool::Object { object_id } => inspect_object(store, oid(&object_id)?),
        Tool::Static { field_id } => inspect_static(store, fid(&field_id)?),
        Tool::Invoke {
            operation,
            code,
            arguments,
            budget,
        } => {
            require(arguments.len() <= 16, "child tool argument bound exceeded")?;
            invoke_once(
                store,
                &operation,
                cid(&code)?,
                arguments
                    .into_iter()
                    .map(ToolArgument::value)
                    .collect::<Result<_>>()?,
                budget,
            )
        }
    }
}
