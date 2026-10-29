//! Preparatory host-owned placement policies and method export grants.
//!
//! These records authorize preflight only. They do not implement transport,
//! remote CIL references, graph copying, migration, or a guest security sandbox.
use crate::{
    checkpoint::CheckpointStore,
    error::{Error, Result, require},
    execution::MethodVersion,
    heap::AllocationDomain,
    identity::{CodeVersionId, GrantId, MethodId, NodeId, ObjectId, TypeId, UniverseId},
    program::{Method, SlotType},
    runtime::Vm,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

const NAMESPACE: &str = "renraku.placement-v1/";
const REGISTRY: &str = "renraku.placement-v1/registry";
const OBJECTS: &str = "renraku.placement-v1/object/";
pub const MAX_OBJECTS: usize = 256;
pub const MAX_GRANTS: usize = 256;
pub const MAX_NODES: usize = 16;
pub const MAX_RESOURCES: usize = 16;
pub const MAX_REGISTRY_BYTES: usize = 256 * 1024;
pub const MAX_ARGUMENTS: usize = 16;
pub const MAX_TEXT_UNITS: usize = 4096;
pub const MAX_CALL_TEXT_UNITS: usize = 8192;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Owner {
    pub universe: UniverseId,
    pub node: NodeId,
    pub epoch: u64,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PlacementPolicy {
    pub allowed_nodes: BTreeSet<NodeId>,
    pub required_resources: BTreeSet<String>,
    pub movable: bool,
    pub copyable: bool,
}
impl PlacementPolicy {
    /// Pin to one node; copying is independently denied until explicitly allowed.
    pub fn pinned(node: NodeId) -> Self {
        Self {
            allowed_nodes: BTreeSet::from([node]),
            required_resources: BTreeSet::new(),
            movable: false,
            copyable: false,
        }
    }
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ObjectPolicy {
    pub object: ObjectId,
    pub type_id: TypeId,
    pub placement: PlacementPolicy,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MethodGrant {
    pub id: GrantId,
    pub recipient: UniverseId,
    pub object: ObjectId,
    pub method: MethodVersion,
    pub revoked: bool,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Registry {
    owner: Owner,
    objects: BTreeMap<ObjectId, ObjectPolicy>,
    grants: BTreeMap<GrantId, MethodGrant>,
}
impl Registry {
    pub fn owner(&self) -> &Owner {
        &self.owner
    }
    pub fn objects(&self) -> impl Iterator<Item = &ObjectPolicy> {
        self.objects.values()
    }
    pub fn grants(&self) -> impl Iterator<Item = &MethodGrant> {
        self.grants.values()
    }
    pub fn hash(&self) -> Result<String> {
        Ok(Sha256::digest(encode(self)?.as_bytes())
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect())
    }
}

/// Trusted embedding context, never evidence supplied by a network request.
/// Resource labels attest availability only as far as the embedding is trusted.
#[derive(Clone, Debug)]
pub struct OwnerContext {
    pub node: NodeId,
    pub epoch: u64,
    pub resources: BTreeSet<String>,
}
/// Bounded value profile for future call transport. References cannot be smuggled
/// as owner-local ObjectIds, and scalar values are never implicitly boxed.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    content = "value",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum Argument {
    I32(i32),
    I64(i64),
    Text(Vec<u16>),
    Null,
}
#[derive(Clone, Debug)]
pub enum Access {
    Invoke {
        owner: UniverseId,
        epoch: u64,
        object: ObjectId,
        method: MethodVersion,
        arguments: Vec<Argument>,
    },
    InspectObject(ObjectId),
    InspectType(TypeId),
    ReadCode(CodeVersionId),
    ReadStatic(crate::identity::FieldId),
    ReadField(ObjectId, crate::identity::FieldId),
    WriteField(ObjectId, crate::identity::FieldId),
    Discover,
    CopyGraph(ObjectId),
    MoveObject(ObjectId),
}
/// A preflight decision, not an executable or transferable capability token.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AuthorizedCall {
    pub observed_generation: u64,
    pub registry_hash: String,
    pub owner: Owner,
    pub grant: GrantId,
    pub recipient: UniverseId,
    pub object: ObjectId,
    pub method: MethodVersion,
    pub arguments: Vec<Argument>,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct WireRegistry {
    format: u32,
    universe: String,
    node: String,
    epoch: u64,
    objects: Vec<WireObject>,
    grants: Vec<WireGrant>,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct WireObject {
    object: String,
    type_id: String,
    allowed_nodes: Vec<String>,
    required_resources: Vec<String>,
    movable: bool,
    copyable: bool,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct WireGrant {
    id: String,
    recipient: String,
    object: String,
    method: String,
    code: String,
    revoked: bool,
}
pub(crate) fn id(text: &str) -> Result<[u8; 16]> {
    require(
        text.len() == 32
            && text
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
        "placement identity must be 32 lowercase hex digits",
    )?;
    Ok(*uuid::Uuid::parse_str(text)
        .map_err(|_| Error("invalid placement identity".into()))?
        .as_bytes())
}
fn resource(name: &str) -> Result<()> {
    require(
        !name.is_empty()
            && name.len() <= 64
            && name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"._-:".contains(&b)),
        "resource label requires 1..64 ASCII identifier characters",
    )
}
fn policy(policy: &PlacementPolicy, node: NodeId) -> Result<()> {
    require(
        !policy.allowed_nodes.is_empty()
            && policy.allowed_nodes.len() <= MAX_NODES
            && policy.allowed_nodes.contains(&node),
        "object placement must explicitly permit the current owner node (at most 16 nodes)",
    )?;
    require(
        policy.required_resources.len() <= MAX_RESOURCES,
        "too many required resources",
    )?;
    for name in &policy.required_resources {
        resource(name)?;
    }
    Ok(())
}
fn encode(registry: &Registry) -> Result<String> {
    let wire = WireRegistry {
        format: 1,
        universe: registry.owner.universe.to_string(),
        node: registry.owner.node.to_string(),
        epoch: registry.owner.epoch,
        objects: registry
            .objects
            .values()
            .map(|p| WireObject {
                object: p.object.to_string(),
                type_id: p.type_id.to_string(),
                allowed_nodes: p
                    .placement
                    .allowed_nodes
                    .iter()
                    .map(ToString::to_string)
                    .collect(),
                required_resources: p.placement.required_resources.iter().cloned().collect(),
                movable: p.placement.movable,
                copyable: p.placement.copyable,
            })
            .collect(),
        grants: registry
            .grants
            .values()
            .map(|g| WireGrant {
                id: g.id.to_string(),
                recipient: g.recipient.to_string(),
                object: g.object.to_string(),
                method: g.method.method.to_string(),
                code: g.method.code.to_string(),
                revoked: g.revoked,
            })
            .collect(),
    };
    let text = serde_json::to_string(&wire).map_err(|e| Error(e.to_string()))?;
    require(
        text.len() <= MAX_REGISTRY_BYTES,
        "placement registry exceeds 256 KiB",
    )?;
    Ok(text)
}
fn decode(text: &str) -> Result<Registry> {
    require(
        text.len() <= MAX_REGISTRY_BYTES,
        "placement registry exceeds 256 KiB",
    )?;
    let wire: WireRegistry =
        serde_json::from_str(text).map_err(|_| Error("invalid placement registry JSON".into()))?;
    require(
        wire.format == 1 && wire.epoch > 0,
        "unsupported placement format or zero epoch",
    )?;
    require(
        wire.objects.len() <= MAX_OBJECTS && wire.grants.len() <= MAX_GRANTS,
        "placement record limit exceeded",
    )?;
    let mut registry = Registry {
        owner: Owner {
            universe: UniverseId::from_bytes(id(&wire.universe)?),
            node: NodeId::from_bytes(id(&wire.node)?),
            epoch: wire.epoch,
        },
        objects: BTreeMap::new(),
        grants: BTreeMap::new(),
    };
    for p in wire.objects {
        require(
            p.allowed_nodes.len() <= MAX_NODES && p.required_resources.len() <= MAX_RESOURCES,
            "placement list limit exceeded",
        )?;
        let placement = PlacementPolicy {
            allowed_nodes: p
                .allowed_nodes
                .iter()
                .map(|n| id(n).map(NodeId::from_bytes))
                .collect::<Result<_>>()?,
            required_resources: p.required_resources.iter().cloned().collect(),
            movable: p.movable,
            copyable: p.copyable,
        };
        require(
            placement.allowed_nodes.len() == p.allowed_nodes.len()
                && placement.required_resources.len() == p.required_resources.len(),
            "duplicate placement constraint",
        )?;
        policy(&placement, registry.owner.node)?;
        let record = ObjectPolicy {
            object: ObjectId::from_bytes(id(&p.object)?),
            type_id: TypeId::from_bytes(id(&p.type_id)?),
            placement,
        };
        require(
            registry.objects.insert(record.object, record).is_none(),
            "duplicate placement object",
        )?;
    }
    for g in wire.grants {
        let grant = MethodGrant {
            id: GrantId::from_bytes(id(&g.id)?),
            recipient: UniverseId::from_bytes(id(&g.recipient)?),
            object: ObjectId::from_bytes(id(&g.object)?),
            method: MethodVersion {
                method: MethodId::from_bytes(id(&g.method)?),
                code: CodeVersionId::from_bytes(id(&g.code)?),
            },
            revoked: g.revoked,
        };
        require(
            registry.grants.insert(grant.id, grant).is_none(),
            "duplicate method grant",
        )?;
    }
    Ok(registry)
}
fn grant_method<'a>(
    vm: &'a Vm<'_>,
    grant: &MethodGrant,
    target: &ObjectPolicy,
) -> Result<&'a Method> {
    let method = vm
        .program
        .methods
        .get(&grant.method.code)
        .ok_or_else(|| Error("method grant has missing code".into()))?;
    require(
        method.binding == grant.method
            && method.signature.instance
            && method.intrinsic.is_none()
            && !method.name.starts_with('.'),
        "grant requires a pinned ordinary instance method",
    )?;
    require(
        vm.program.is_assignable(target.type_id, method.owner),
        "grant method is incompatible with target type",
    )?;
    require(
        grant.recipient != vm.universe_id(),
        "export recipient must be another universe",
    )?;
    let string = vm.program.string_type()?;
    let supported = |slot| !matches!(slot, SlotType::Reference(id) if id != string);
    require(
        method.signature.parameters.len() <= MAX_ARGUMENTS
            && method.signature.parameters.iter().copied().all(supported)
            && method.signature.result.is_none_or(supported),
        "grant signature requires bounded integer/string/null values",
    )?;
    Ok(method)
}

/// Read and validate host metadata against live logical identities and typed
/// retention roots. No registry means no policy/grant, never implicit authority.
pub fn read(vm: &Vm<'_>) -> Result<Option<Registry>> {
    let roots = vm.heap().roots();
    let Some(metadata) = roots.get(REGISTRY) else {
        require(
            !roots.keys().any(|k| k.starts_with(NAMESPACE)),
            "placement roots exist without a registry",
        )?;
        return Ok(None);
    };
    let units = vm
        .heap()
        .object(*metadata)?
        .string_units()
        .ok_or_else(|| Error("placement registry root must be a string".into()))?;
    require(
        units.len() <= MAX_REGISTRY_BYTES,
        "placement registry exceeds its unit bound",
    )?;
    let text =
        String::from_utf16(units).map_err(|_| Error("invalid placement registry text".into()))?;
    let registry = decode(&text)?;
    require(
        registry.owner.universe == vm.universe_id(),
        "placement registry belongs to another universe",
    )?;
    let expected: BTreeSet<_> = registry
        .objects
        .keys()
        .map(|id| format!("{OBJECTS}{id}"))
        .chain([REGISTRY.into()])
        .collect();
    require(
        roots
            .keys()
            .filter(|k| k.starts_with(NAMESPACE))
            .cloned()
            .collect::<BTreeSet<_>>()
            == expected,
        "placement roots differ from registered targets",
    )?;
    for record in registry.objects.values() {
        require(
            roots[&format!("{OBJECTS}{}", record.object)] == record.object,
            "placement target root changed",
        )?;
        let object = vm.heap().object(record.object)?;
        require(
            object.domain() == AllocationDomain::Managed
                && object.type_id() == record.type_id
                && vm.program.types.contains_key(&record.type_id),
            "placement target type/domain changed",
        )?;
    }
    for grant in registry.grants.values() {
        let target = registry
            .objects
            .get(&grant.object)
            .ok_or_else(|| Error("grant target has no placement policy".into()))?;
        grant_method(vm, grant, target)?;
    }
    require(
        encode(&registry)? == text,
        "placement registry is not canonical",
    )?;
    Ok(Some(registry))
}
fn required(vm: &Vm<'_>) -> Result<Registry> {
    read(vm)?.ok_or_else(|| Error("no placement registry; access denied".into()))
}
fn persist(store: &mut CheckpointStore, vm: &mut Vm<'_>, registry: &Registry) -> Result<()> {
    let text = encode(registry)?;
    let object = vm.new_string(text.encode_utf16().collect())?;
    vm.set_root(REGISTRY, object)?;
    for target in registry.objects.keys() {
        vm.set_root(format!("{OBJECTS}{target}"), *target)?;
    }
    // Metadata and typed retention roots commit with the entire existing VM.
    // On error this scratch VM is discarded; reload before any retry.
    store.save(vm)?;
    Ok(())
}
/// Trusted operator API, not a guest operation or a transport authentication API.
/// This fixed-owner milestone deliberately provides no epoch/node transfer.
pub fn initialize(store: &mut CheckpointStore, node: NodeId, epoch: u64) -> Result<Owner> {
    require(epoch > 0, "owner epoch must be positive")?;
    let saved = store.load()?;
    let mut vm = saved.snapshot.restore()?;
    let owner = Owner {
        universe: vm.universe_id(),
        node,
        epoch,
    };
    if let Some(registry) = read(&vm)? {
        require(
            registry.owner == owner,
            "owner identity/node/epoch is already fixed",
        )?;
        return Ok(owner);
    }
    persist(
        store,
        &mut vm,
        &Registry {
            owner: owner.clone(),
            objects: BTreeMap::new(),
            grants: BTreeMap::new(),
        },
    )?;
    Ok(owner)
}
pub fn register_object(
    store: &mut CheckpointStore,
    object: ObjectId,
    placement: PlacementPolicy,
) -> Result<ObjectPolicy> {
    let saved = store.load()?;
    let mut vm = saved.snapshot.restore()?;
    let mut registry = required(&vm)?;
    policy(&placement, registry.owner.node)?;
    let target = vm.heap().object(object)?;
    require(
        target.domain() == AllocationDomain::Managed,
        "only managed objects have durable placement",
    )?;
    let record = ObjectPolicy {
        object,
        type_id: target.type_id(),
        placement,
    };
    if let Some(old) = registry.objects.get(&object) {
        require(old == &record, "object policy is already fixed")?;
        return Ok(old.clone());
    }
    require(
        registry.objects.len() < MAX_OBJECTS,
        "placement object limit reached",
    )?;
    registry.objects.insert(object, record.clone());
    persist(store, &mut vm, &registry)?;
    Ok(record)
}
pub fn export_method(
    store: &mut CheckpointStore,
    id: GrantId,
    recipient: UniverseId,
    object: ObjectId,
    method: MethodVersion,
) -> Result<MethodGrant> {
    let saved = store.load()?;
    let mut vm = saved.snapshot.restore()?;
    let mut registry = required(&vm)?;
    let target = registry
        .objects
        .get(&object)
        .ok_or_else(|| Error("export target has no placement policy".into()))?;
    let grant = MethodGrant {
        id,
        recipient,
        object,
        method,
        revoked: false,
    };
    grant_method(&vm, &grant, target)?;
    if let Some(old) = registry.grants.get(&id) {
        let mut expected = grant;
        expected.revoked = old.revoked;
        require(
            old == &expected,
            "grant ID is already bound to another export",
        )?;
        return Ok(old.clone()); // A retry can never reactivate a revoked grant.
    }
    require(
        registry.grants.len() < MAX_GRANTS,
        "method grant limit reached",
    )?;
    registry.grants.insert(id, grant.clone());
    persist(store, &mut vm, &registry)?;
    Ok(grant)
}
pub fn revoke(store: &mut CheckpointStore, id: GrantId) -> Result<MethodGrant> {
    let saved = store.load()?;
    let mut vm = saved.snapshot.restore()?;
    let mut registry = required(&vm)?;
    let grant = registry
        .grants
        .get_mut(&id)
        .ok_or_else(|| Error("unknown method grant".into()))?;
    if grant.revoked {
        return Ok(grant.clone());
    }
    grant.revoked = true;
    let record = grant.clone();
    persist(store, &mut vm, &registry)?;
    Ok(record)
}
/// The embedding must supply the actual authenticated caller and owner context;
/// a claimed UUID from an untrusted message is insufficient. This only checks
/// preflight of a fresh checkpoint: it does not accept work, execute CIL, or
/// create a remote reference. Later admission must fence the observed generation;
/// a decision is not reusable authority after another checkpoint/revocation.
pub fn authorize(
    store: &mut CheckpointStore,
    context: &OwnerContext,
    authenticated_caller: UniverseId,
    id: GrantId,
    access: &Access,
) -> Result<AuthorizedCall> {
    let saved = store.load()?;
    let vm = saved.snapshot.restore()?;
    authorize_snapshot(&vm, context, authenticated_caller, id, access)
}
pub(crate) fn authorize_snapshot(
    vm: &Vm<'_>,
    context: &OwnerContext,
    authenticated_caller: UniverseId,
    id: GrantId,
    access: &Access,
) -> Result<AuthorizedCall> {
    let Access::Invoke {
        owner,
        epoch,
        object,
        method,
        arguments,
    } = access
    else {
        return Err(Error(
            "method grants confer no inspection/static/field/copy/move authority".into(),
        ));
    };
    let registry = required(vm)?;
    require(
        *owner == registry.owner.universe
            && *epoch == registry.owner.epoch
            && context.epoch == registry.owner.epoch
            && context.node == registry.owner.node,
        "owner universe/node/epoch mismatch",
    )?;
    let grant = registry
        .grants
        .get(&id)
        .ok_or_else(|| Error("unknown method grant; access denied".into()))?;
    require(
        !grant.revoked
            && grant.recipient == authenticated_caller
            && grant.object == *object
            && grant.method == *method,
        "method grant does not authorize this caller/target/code",
    )?;
    let target = &registry.objects[object];
    require(
        context.resources.len() <= MAX_RESOURCES,
        "owner resource context exceeds its bound",
    )?;
    for name in &context.resources {
        resource(name)?;
    }
    require(
        target.placement.allowed_nodes.contains(&context.node)
            && target
                .placement
                .required_resources
                .is_subset(&context.resources),
        "owner placement/resource constraint is unsatisfied",
    )?;
    let declaration = grant_method(vm, grant, target)?;
    check_arguments(vm, declaration, arguments)?;
    Ok(AuthorizedCall {
        observed_generation: vm.checkpoint_generation.get(),
        registry_hash: registry.hash()?,
        owner: registry.owner,
        grant: id,
        recipient: authenticated_caller,
        object: *object,
        method: *method,
        arguments: arguments.clone(),
    })
}

pub(crate) fn check_arguments(vm: &Vm<'_>, method: &Method, arguments: &[Argument]) -> Result<()> {
    require(
        arguments.len() <= MAX_ARGUMENTS && arguments.len() == method.signature.parameters.len(),
        "method argument count mismatch",
    )?;
    let mut text_units = 0;
    let string = vm.program.string_type()?;
    for (argument, slot) in arguments.iter().zip(&method.signature.parameters) {
        let matches = match (argument, slot) {
            (Argument::I32(_), SlotType::I32) | (Argument::I64(_), SlotType::I64) => true,
            (Argument::I32(n), SlotType::Boolean) => matches!(n, 0 | 1),
            (Argument::I32(n), SlotType::Char) => (0..=65535).contains(n),
            (Argument::Null, SlotType::Reference(id)) => *id == string,
            (Argument::Text(units), SlotType::Reference(id)) if *id == string => {
                require(
                    units.len() <= MAX_TEXT_UNITS,
                    "call string exceeds 4096 UTF-16 units",
                )?;
                text_units += units.len();
                true
            }
            _ => false,
        };
        require(
            matches,
            "method argument type mismatch; boxing/reference coercion is unsupported",
        )?;
    }
    require(
        text_units <= MAX_CALL_TEXT_UNITS,
        "call text exceeds 8192 UTF-16 units",
    )?;
    Ok(())
}
pub(crate) fn check_owner_context(
    vm: &Vm<'_>,
    context: &OwnerContext,
    owner: UniverseId,
    epoch: u64,
    object: ObjectId,
) -> Result<()> {
    let registry = required(vm)?;
    require(
        owner == registry.owner.universe
            && epoch == registry.owner.epoch
            && context.epoch == registry.owner.epoch
            && context.node == registry.owner.node,
        "owner universe/node/epoch mismatch",
    )?;
    let target = registry
        .objects
        .get(&object)
        .ok_or_else(|| Error("owner target has no placement policy".into()))?;
    require(
        context.resources.len() <= MAX_RESOURCES,
        "owner resource context exceeds its bound",
    )?;
    for name in &context.resources {
        resource(name)?;
    }
    require(
        target.placement.allowed_nodes.contains(&context.node)
            && target
                .placement
                .required_resources
                .is_subset(&context.resources),
        "owner placement/resource constraint is unsatisfied",
    )
}
