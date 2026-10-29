//! A trusted, fieldless local contract handle for one exported remote method.
//! Contract IDs remain local; the authoritative owner/type/object/code are explicit.
use crate::{
    checkpoint::CheckpointStore,
    error::{Error, Result, require},
    execution::{Continuation, ExecutionStatus, Frame, MethodVersion},
    heap::AllocationDomain,
    identity::{CodeVersionId, GrantId, MethodId, NodeId, ObjectId, TypeId, UniverseId},
    owner::{OwnerCall, OwnerReply, ReplyState},
    peer::{self, CallReport, Endpoint, PeerKey},
    placement::{self, Argument},
    program::{Method, SlotType},
    runtime::Vm,
    value::Value,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
const NS: &str = "renraku.remote-v1/";
const MAX_IMPORTS: usize = 64;
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Import {
    format: u32,
    handle: String,
    local_type: String,
    local_method: String,
    local_code: String,
    owner_type: String,
    owner: String,
    node: String,
    epoch: u64,
    address: String,
    grant: String,
    object: String,
    method: String,
    code: String,
    parameters: Vec<String>,
    result: Option<String>,
}
fn oid(s: &str) -> Result<ObjectId> {
    Ok(ObjectId::from_bytes(placement::id(s)?))
}
fn tid(s: &str) -> Result<TypeId> {
    Ok(TypeId::from_bytes(placement::id(s)?))
}
fn method(m: &str, c: &str) -> Result<MethodVersion> {
    Ok(MethodVersion {
        method: MethodId::from_bytes(placement::id(m)?),
        code: CodeVersionId::from_bytes(placement::id(c)?),
    })
}
fn json<T: Serialize>(v: &T) -> Result<Vec<u8>> {
    let b = serde_json::to_vec(v).map_err(|_| Error("invalid remote metadata".into()))?;
    require(b.len() <= peer::MAX_FRAME, "remote metadata exceeds 64 KiB")?;
    Ok(b)
}
fn profile(vm: &Vm<'_>, slot: SlotType) -> Result<String> {
    Ok(match slot {
        SlotType::I32 => "i32",
        SlotType::I64 => "i64",
        SlotType::Boolean => "bool",
        SlotType::Char => "char",
        SlotType::Reference(t) if t == vm.program.string_type()? => "string",
        _ => {
            return Err(Error(
                "remote contract supports only primitive/string slots".into(),
            ));
        }
    }
    .into())
}
fn signature(vm: &Vm<'_>, m: &Method) -> Result<(Vec<String>, Option<String>)> {
    require(
        m.signature.instance
            && m.intrinsic.is_none()
            && !m.name.starts_with('.')
            && m.signature.parameters.len() <= placement::MAX_ARGUMENTS,
        "remote contract needs an ordinary bounded instance method",
    )?;
    Ok((
        m.signature
            .parameters
            .iter()
            .map(|s| profile(vm, *s))
            .collect::<Result<_>>()?,
        m.signature.result.map(|s| profile(vm, s)).transpose()?,
    ))
}
fn local<'a>(vm: &'a Vm<'_>, i: &Import) -> Result<&'a Method> {
    let binding = method(&i.local_method, &i.local_code)?;
    let m = vm
        .program
        .methods
        .get(&binding.code)
        .ok_or_else(|| Error("remote contract code missing".into()))?;
    require(
        m.binding == binding && m.owner == tid(&i.local_type)?,
        "remote contract binding mismatch",
    )?;
    let ty = &vm.program.types[&m.owner];
    require(
        ty.fields.is_empty()
            && ty.flags & 0x100 != 0
            && ty.base == Some(vm.program.find_type("System.Object")?),
        "remote contract must be sealed, fieldless and directly extend Object",
    )?;
    require(
        vm.program
            .methods
            .values()
            .filter(|other| other.owner == m.owner)
            .all(|other| other.binding == binding || other.name == ".ctor"),
        "remote contract exposes additional methods",
    )?;
    require(
        signature(vm, m)? == (i.parameters.clone(), i.result.clone()),
        "remote contract signature changed",
    )?;
    Ok(m)
}
impl Import {
    fn endpoint(&self) -> Result<Endpoint> {
        let address = self
            .address
            .parse()
            .map_err(|_| Error("invalid imported endpoint".into()))?;
        let e = Endpoint {
            owner: UniverseId::from_bytes(placement::id(&self.owner)?),
            node: NodeId::from_bytes(placement::id(&self.node)?),
            epoch: self.epoch,
            address,
        };
        require(
            e.address.ip().is_loopback()
                && e.address.port() != 0
                && e.address.to_string() == self.address
                && e.epoch > 0,
            "invalid imported endpoint",
        )?;
        Ok(e)
    }
    fn call(&self, operation: String, arguments: Vec<Argument>) -> Result<OwnerCall> {
        Ok(OwnerCall {
            operation,
            owner: self.endpoint()?.owner,
            epoch: self.epoch,
            grant: GrantId::from_bytes(placement::id(&self.grant)?),
            object: oid(&self.object)?,
            method: method(&self.method, &self.code)?,
            arguments,
        })
    }
    fn matches(&self, c: &OwnerCall) -> Result<bool> {
        Ok(self.call(c.operation.clone(), c.arguments.clone())? == *c)
    }
}
fn imports(vm: &Vm<'_>) -> Result<Vec<Import>> {
    let mut out = Vec::new();
    let mut expected = BTreeSet::new();
    let mut handles = BTreeSet::new();
    let mut identities = BTreeSet::new();
    let mut types = BTreeSet::new();
    for (name, id) in vm
        .heap
        .roots()
        .iter()
        .filter(|(n, _)| n.starts_with(&format!("{NS}import/")))
    {
        require(out.len() < MAX_IMPORTS, "remote import limit exceeded")?;
        let units = vm
            .heap
            .object(*id)?
            .string_units()
            .ok_or_else(|| Error("remote import must be a string".into()))?;
        require(
            units.len() <= peer::MAX_FRAME,
            "remote import exceeds bound",
        )?;
        let bytes = String::from_utf16(units)
            .map_err(|_| Error("invalid remote import UTF16".into()))?
            .into_bytes();
        let i: Import = serde_json::from_slice(&bytes)
            .map_err(|_| Error("invalid remote import JSON".into()))?;
        require(
            i.format == 1 && json(&i)? == bytes && *name == format!("{NS}import/{}", i.handle),
            "remote import canonical/root mismatch",
        )?;
        let handle = oid(&i.handle)?;
        let m = local(vm, &i)?;
        let e = i.endpoint()?;
        placement::id(&i.owner_type)?;
        i.call("validate".into(), vec![])?;
        require(
            e.owner != vm.universe_id()
                && handles.insert(handle)
                && identities.insert((i.owner.clone(), i.object.clone()))
                && types.insert(m.owner),
            "remote import identity/type alias conflict",
        )?;
        let object = vm.heap.object(handle)?;
        require(
            object.type_id() == m.owner
                && object.domain() == AllocationDomain::Managed
                && object.fields().is_empty()
                && object.string_units().is_none(),
            "remote handle shape/domain mismatch",
        )?;
        let root = format!("{NS}object/{}", i.handle);
        require(
            vm.heap.roots().get(&root) == Some(&handle),
            "remote handle retention root missing",
        )?;
        expected.insert(name.clone());
        expected.insert(root);
        out.push(i);
    }
    require(
        vm.heap
            .roots()
            .keys()
            .filter(|n| n.starts_with(NS))
            .cloned()
            .collect::<BTreeSet<_>>()
            == expected,
        "unknown/orphan remote import roots",
    )?;
    Ok(out)
}
/// Trusted import reads authoritative saved owner schema and the original grant.
/// The local declaration is a separate contract, never a recreated owner type.
pub fn import_method(
    owner: &mut CheckpointStore,
    caller: &mut CheckpointStore,
    endpoint: &Endpoint,
    grant: GrantId,
    contract: MethodVersion,
) -> Result<ObjectId> {
    let saved = owner.load()?;
    let ovm = saved.snapshot.restore()?;
    let cs = caller.load()?;
    let mut vm = cs.snapshot.restore()?;
    require(
        matches!(
            vm.context.status,
            ExecutionStatus::Completed { .. } | ExecutionStatus::Faulted { .. }
        ),
        "caller universe is busy",
    )?;
    let registry =
        placement::read(&ovm)?.ok_or_else(|| Error("owner has no placement registry".into()))?;
    require(
        registry.owner().universe == endpoint.owner
            && registry.owner().node == endpoint.node
            && registry.owner().epoch == endpoint.epoch,
        "import owner scope mismatch",
    )?;
    let g = registry
        .grants()
        .find(|g| g.id == grant)
        .ok_or_else(|| Error("import grant missing".into()))?;
    require(
        !g.revoked && g.recipient == vm.universe_id(),
        "import grant revoked or wrong principal",
    )?;
    let om = ovm.entry_method(g.method)?;
    let cm = vm.entry_method(contract)?;
    let (parameters, result) = signature(&ovm, om)?;
    require(
        signature(&vm, cm)? == (parameters.clone(), result.clone()) && cm.name == om.name,
        "import method signature/name mismatch",
    )?;
    let i = Import {
        format: 1,
        handle: ObjectId::fresh().to_string(),
        local_type: cm.owner.to_string(),
        local_method: contract.method.to_string(),
        local_code: contract.code.to_string(),
        owner_type: ovm.heap.object(g.object)?.type_id().to_string(),
        owner: endpoint.owner.to_string(),
        node: endpoint.node.to_string(),
        epoch: endpoint.epoch,
        address: endpoint.address.to_string(),
        grant: grant.to_string(),
        object: g.object.to_string(),
        method: g.method.method.to_string(),
        code: g.method.code.to_string(),
        parameters,
        result,
    };
    local(&vm, &i)?;
    i.endpoint()?;
    let old = imports(&vm)?;
    if let Some(old) = old
        .iter()
        .find(|o| o.owner == i.owner && o.object == i.object)
    {
        let mut retry = i.clone();
        retry.handle = old.handle.clone();
        require(
            json(&retry)? == json(old)?,
            "remote identity is already bound to another contract/grant",
        )?;
        return oid(&old.handle);
    }
    require(
        old.len() < MAX_IMPORTS && !old.iter().any(|o| o.local_type == i.local_type),
        "remote import limit/type binding conflict",
    )?;
    require(
        !vm.heap.objects().any(|o| o.type_id() == cm.owner),
        "contract already has local instances",
    )?;
    let handle = oid(&i.handle)?;
    vm.heap
        .allocate_initialized(handle, cm.owner, BTreeMap::new())?;
    vm.set_root(format!("{NS}object/{handle}"), handle)?;
    let record = vm.new_string(
        String::from_utf8(json(&i)?)
            .unwrap()
            .encode_utf16()
            .collect(),
    )?;
    vm.set_root(format!("{NS}import/{handle}"), record)?;
    validate(&vm)?;
    caller.save(&vm)?;
    Ok(handle)
}
pub(crate) fn contract_type(vm: &Vm<'_>, ty: TypeId) -> Result<bool> {
    Ok(imports(vm)?.iter().any(|i| i.local_type == ty.to_string()))
}
pub(crate) struct Prepared {
    pub request: ObjectId,
    pub peer: peer::GuestIntent,
}
pub(crate) fn prepare(
    vm: &Vm<'_>,
    callee: &Method,
    args: &[Value],
    constructing: bool,
) -> Result<Option<Prepared>> {
    let imports = imports(vm)?;
    if !imports
        .iter()
        .any(|i| i.local_type == callee.owner.to_string())
    {
        require(
            !args
                .first()
                .filter(|_| callee.signature.instance)
                .is_some_and(|v| {
                    imports.iter().any(|i| {
                        v.referenced_object()
                            .is_some_and(|id| i.handle == id.to_string())
                    })
                }),
            "remote handle does not grant this method",
        )?;
        return Ok(None);
    }
    require(!constructing, "remote contract construction is unsupported")?;
    let i = imports
        .iter()
        .find(|i| {
            args.first()
                .and_then(Value::referenced_object)
                .is_some_and(|id| i.handle == id.to_string())
        })
        .ok_or_else(|| Error("remote call requires an imported non-null handle".into()))?;
    require(
        callee.binding == method(&i.local_method, &i.local_code)?,
        "remote method is not granted",
    )?;
    require(
        args.len() == callee.signature.parameters.len() + 1,
        "remote argument count mismatch",
    )?;
    let mut arguments = Vec::new();
    for (v, slot) in args[1..].iter().zip(&callee.signature.parameters) {
        vm.check_value(*v, *slot)?;
        arguments.push(match v {
            Value::I32(n) => Argument::I32(*n),
            Value::I64(n) => Argument::I64(*n),
            Value::Null => Argument::Null,
            Value::Reference(id) => Argument::Text(
                vm.heap
                    .object(*id)?
                    .string_units()
                    .ok_or_else(|| {
                        Error("remote arguments cannot contain object references".into())
                    })?
                    .to_vec(),
            ),
        });
    }
    require(
        vm.instructions_executed.checked_add(1).is_some(),
        "remote instruction identity space exhausted",
    )?;
    let operation = format!("g.{}.{}", vm.context.id, vm.instructions_executed);
    let call = i.call(operation, arguments)?;
    placement::check_arguments(vm, callee, &call.arguments)?;
    let prepared = peer::prepare_guest(vm, &i.endpoint()?, &call)?;
    Ok(Some(Prepared {
        request: prepared.id,
        peer: prepared,
    }))
}
pub(crate) fn park(vm: &mut Vm<'_>, frame: Frame, p: Prepared) -> Result<()> {
    peer::publish_guest(vm, p.peer)?;
    *vm.context.frames.last_mut().unwrap() = frame;
    vm.context.status = ExecutionStatus::WaitingRemote { request: p.request };
    Ok(())
}
pub(crate) fn validate_wait(vm: &Vm<'_>, request: ObjectId, callee: &Method) -> Result<()> {
    let (e, c) = peer::guest_call(vm, request)?;
    let i = imports(vm)?
        .into_iter()
        .find(|i| i.matches(&c).unwrap_or(false))
        .ok_or_else(|| Error("remote waiter has no matching import".into()))?;
    require(
        local(vm, &i)?.binding == callee.binding && i.endpoint()? == e,
        "remote waiter contract/endpoint mismatch",
    )?;
    placement::check_arguments(vm, callee, &c.arguments)?;
    let reply = peer::guest_reply(vm, &c.operation)?;
    let valid = match (&vm.context.status, reply.as_ref().map(|r| &r.state)) {
        (ExecutionStatus::WaitingRemote { .. }, None) => true,
        (
            ExecutionStatus::Faulted { message },
            Some(ReplyState::Faulted { diagnostic, .. } | ReplyState::InvalidResult { diagnostic }),
        ) => *message == format!("remote owner fault: {diagnostic}"),
        _ => false,
    };
    require(valid, "remote wait/terminal reply disagreement")?;
    Ok(())
}
pub(crate) fn validate(vm: &Vm<'_>) -> Result<()> {
    let imports = imports(vm)?;
    for frame in &vm.context.frames {
        let owner = vm.current_method(frame)?.owner;
        require(
            !imports.iter().any(|i| i.local_type == owner.to_string()),
            "remote contract code cannot have local frames",
        )?;
    }
    if let ExecutionStatus::WaitingRemote { request } = vm.context.status {
        require(
            vm.context.frames.last().is_some_and(
                |f| matches!(f.continuation,Continuation::AwaitRemote{request:r,..} if r==request),
            ),
            "remote wait has no matching continuation",
        )?;
    }
    Ok(())
}
pub(crate) fn owns(vm: &Vm<'_>, operation: &str) -> Result<bool> {
    let ExecutionStatus::WaitingRemote { request } = vm.context.status else {
        return Ok(false);
    };
    let (_, c) = peer::guest_call(vm, request)?;
    Ok(c.operation == operation)
}
pub(crate) fn complete(vm: &mut Vm<'_>, reply: &OwnerReply) -> Result<()> {
    let ExecutionStatus::WaitingRemote { request } = vm.context.status else {
        return Ok(());
    };
    let (e, c) = peer::guest_call(vm, request)?;
    require(
        c.method == reply.method && c.object == reply.object,
        "remote completion binding mismatch",
    )?;
    let i = imports(vm)?
        .into_iter()
        .find(|i| i.matches(&c).unwrap_or(false))
        .ok_or_else(|| Error("remote completion import missing".into()))?;
    require(i.endpoint()? == e, "remote completion endpoint mismatch")?;
    let callee = local(vm, &i)?;
    let slot = callee.signature.result;
    let value = match &reply.state {
        ReplyState::Pending => return Ok(()),
        ReplyState::Faulted { diagnostic, .. } | ReplyState::InvalidResult { diagnostic } => {
            vm.context.status = ExecutionStatus::Faulted {
                message: format!("remote owner fault: {diagnostic}"),
            };
            vm.finish_invocation();
            return Ok(());
        }
        ReplyState::Completed(value) => {
            require(
                slot.is_some() == value.is_some(),
                "remote result/void mismatch",
            )?;
            match value {
                None => None,
                Some(Argument::I32(n)) => Some(Value::I32(*n)),
                Some(Argument::I64(n)) => Some(Value::I64(*n)),
                Some(Argument::Null) => Some(Value::Null),
                Some(Argument::Text(units)) => {
                    require(
                        slot == Some(SlotType::Reference(vm.program.string_type()?)),
                        "remote text result type mismatch",
                    )?;
                    Some(Value::Reference(vm.new_string(units.clone())?))
                }
            }
        }
    };
    if let (Some(v), Some(s)) = (value, slot) {
        vm.check_value(v, s)?;
    }
    let frame = vm.context.frames.last_mut().unwrap();
    let Continuation::AwaitRemote { next, .. } = frame.continuation else {
        return Err(Error("missing remote return continuation".into()));
    };
    require(
        value.is_none()
            || frame.evaluation_stack.len() < vm.program.methods[&frame.method.code].max_stack,
        "remote return exceeds maxstack",
    )?;
    if let Some(v) = value {
        frame.evaluation_stack.push(v);
    }
    frame.continuation = Continuation::Execute { next };
    vm.context.status = ExecutionStatus::Runnable;
    Ok(())
}
/// Deliver/reconcile one saved guest call. No credentials means a durable wait.
/// Networking is blocking host work; it never lives on a guest stack.
pub fn drive(store: &mut CheckpointStore, key: Option<&PeerKey>) -> Result<CallReport> {
    let saved = store.load()?;
    let vm = saved.snapshot.restore()?;
    let ExecutionStatus::WaitingRemote { request } = vm.context.status else {
        return Err(Error("guest is not waiting for a remote call".into()));
    };
    let (e, c) = peer::guest_call(&vm, request)?;
    let Some(key) = key else {
        return Ok(CallReport::Unresolved);
    };
    peer::call_guest(store, &e, &c, key)
}

pub(crate) fn has_imports(vm: &Vm<'_>) -> bool {
    vm.heap.roots().keys().any(|n| n.starts_with(NS))
}
