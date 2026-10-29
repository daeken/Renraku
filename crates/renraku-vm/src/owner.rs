//! Durable, grant-checked owner-side acceptance. Transport/authentication and
//! caller-side remote references/waits are intentionally separate future work.
use crate::{
    checkpoint::CheckpointStore,
    cil::Op,
    error::{Error, Result, require},
    execution::{ExecutionStatus, MethodVersion},
    identity::{CodeVersionId, ExecutionId, GrantId, ObjectId, UniverseId},
    invocation::{InstanceCall, InvocationRecord, InvocationState},
    placement::{self, Access, Argument, OwnerContext},
    program::{Intrinsic, Program},
    runtime::Vm,
    value::Value,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

const NAMESPACE: &str = "renraku.owner-v1/";
pub const MAX_CALLS: usize = 128;
pub const MAX_RECORD_BYTES: usize = 64 * 1024;
pub const MAX_METHODS: usize = 256;
pub const MAX_INSTRUCTIONS: usize = 16_384;
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OwnerCall {
    pub operation: String,
    pub owner: UniverseId,
    pub epoch: u64,
    pub grant: GrantId,
    pub object: ObjectId,
    pub method: MethodVersion,
    pub arguments: Vec<Argument>,
}
impl OwnerCall {
    fn access(&self) -> Access {
        Access::Invoke {
            owner: self.owner,
            epoch: self.epoch,
            object: self.object,
            method: self.method,
            arguments: self.arguments.clone(),
        }
    }
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ReplyState {
    Pending,
    Completed(Option<Argument>),
    Faulted { diagnostic: String, truncated: bool },
    InvalidResult { diagnostic: String },
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OwnerReply {
    pub execution: ExecutionId,
    pub method: MethodVersion,
    pub object: ObjectId,
    pub state: ReplyState,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Accepted {
    format: u32,
    caller: String,
    operation: String,
    owner: String,
    epoch: u64,
    grant: String,
    object: String,
    method: String,
    code: String,
    arguments: Vec<Argument>,
    registry_hash: String,
    prepared_generation: u64,
}
fn operation(id: &str) -> Result<()> {
    require(
        !id.is_empty()
            && id.len() <= 64
            && id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b)),
        "owner operation ID requires 1..64 ASCII letters/digits/dots/underscores/hyphens",
    )
}
fn key(caller: UniverseId, id: &str) -> Result<String> {
    operation(id)?;
    Ok(format!("{NAMESPACE}{caller}/{id}"))
}
fn text(record: &Accepted) -> Result<String> {
    let text = serde_json::to_string(record).map_err(|e| Error(e.to_string()))?;
    require(
        text.len() <= MAX_RECORD_BYTES,
        "owner acceptance metadata exceeds 64 KiB",
    )?;
    Ok(text)
}
fn request(record: &Accepted) -> Result<(UniverseId, OwnerCall)> {
    require(
        record.format == 1
            && record.epoch > 0
            && record.registry_hash.len() == 64
            && record
                .registry_hash
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
        "invalid owner acceptance format/epoch/hash",
    )?;
    operation(&record.operation)?;
    Ok((
        UniverseId::from_bytes(placement::id(&record.caller)?),
        OwnerCall {
            operation: record.operation.clone(),
            owner: UniverseId::from_bytes(placement::id(&record.owner)?),
            epoch: record.epoch,
            grant: GrantId::from_bytes(placement::id(&record.grant)?),
            object: ObjectId::from_bytes(placement::id(&record.object)?),
            method: MethodVersion {
                method: crate::identity::MethodId::from_bytes(placement::id(&record.method)?),
                code: CodeVersionId::from_bytes(placement::id(&record.code)?),
            },
            arguments: record.arguments.clone(),
        },
    ))
}
// Explicit conservative closure check. Calls/newobj can reach only ordinary CIL
// or existing pure String intrinsics. Unreachable effectful helpers reject too.
fn code_graph(
    program: &Program,
    root: MethodVersion,
    seen: &mut BTreeSet<CodeVersionId>,
    count: &mut usize,
) -> Result<()> {
    let mut pending = vec![root];
    while let Some(binding) = pending.pop() {
        if !seen.insert(binding.code) {
            continue;
        }
        require(
            seen.len() <= MAX_METHODS,
            "owner code closure exceeds 256 methods",
        )?;
        let method = program
            .methods
            .get(&binding.code)
            .ok_or_else(|| Error("owner code closure has missing code".into()))?;
        require(
            method.binding == binding,
            "owner code closure binding mismatch",
        )?;
        match method.intrinsic {
            Some(Intrinsic::String(_)) => continue,
            Some(_) => {
                return Err(Error(
                    "owner calls cannot reach external capabilities".into(),
                ));
            }
            None => {}
        }
        *count += method.instructions.len();
        require(
            *count <= MAX_INSTRUCTIONS,
            "owner code closure exceeds 16384 instructions",
        )?;
        for instruction in method.instructions.values() {
            if let Op::Call { token, .. } | Op::NewObject(token) = instruction.op {
                let id = program
                    .method_tokens
                    .get(&(method.assembly, token))
                    .ok_or_else(|| Error("owner code closure has unresolved call".into()))?;
                pending.push(program.method(*id)?.binding);
            }
        }
    }
    Ok(())
}
fn matches_argument(vm: &Vm<'_>, value: Value, argument: &Argument) -> Result<bool> {
    Ok(match (value, argument) {
        (Value::I32(a), Argument::I32(b)) => a == *b,
        (Value::I64(a), Argument::I64(b)) => a == *b,
        (Value::Null, Argument::Null) => true,
        (Value::Reference(id), Argument::Text(units)) => {
            vm.heap().object(id)?.string_units() == Some(units.as_slice())
        }
        _ => false,
    })
}
fn journal(vm: &Vm<'_>) -> Result<BTreeMap<String, Accepted>> {
    let mut records = BTreeMap::new();
    if !vm
        .heap()
        .roots()
        .keys()
        .any(|name| name.starts_with(NAMESPACE))
    {
        return Ok(records);
    }
    require(
        !crate::remote::has_imports(vm),
        "owner universes cannot import remote contracts in this profile",
    )?;
    let registry = placement::read(vm)?
        .ok_or_else(|| Error("owner acceptance has no placement registry".into()))?;
    let mut seen = BTreeSet::new();
    let mut count = 0;
    for (root, object) in vm
        .heap()
        .roots()
        .iter()
        .filter(|(name, _)| name.starts_with(NAMESPACE))
    {
        require(records.len() < MAX_CALLS, "owner acceptance limit exceeded")?;
        let units = vm
            .heap()
            .object(*object)?
            .string_units()
            .ok_or_else(|| Error("owner acceptance root must be a string".into()))?;
        require(
            units.len() <= MAX_RECORD_BYTES,
            "owner acceptance text exceeds its bound",
        )?;
        let body =
            String::from_utf16(units).map_err(|_| Error("invalid owner acceptance text".into()))?;
        require(
            body.len() <= MAX_RECORD_BYTES,
            "owner acceptance metadata exceeds 64 KiB",
        )?;
        let record: Accepted = serde_json::from_str(&body)
            .map_err(|_| Error("invalid owner acceptance JSON".into()))?;
        let (caller, call) = request(&record)?;
        require(
            key(caller, &call.operation)? == *root && text(&record)? == body,
            "owner acceptance root/canonical text mismatch",
        )?;
        require(
            registry.owner().universe == call.owner && registry.owner().epoch == call.epoch,
            "owner acceptance universe/epoch mismatch",
        )?;
        let grant = registry
            .grants()
            .find(|g| g.id == call.grant)
            .ok_or_else(|| Error("owner acceptance has missing grant".into()))?;
        require(
            grant.recipient == caller && grant.object == call.object && grant.method == call.method,
            "owner acceptance grant scope mismatch",
        )?; // Revocation does not erase accepted work.
        let method = vm.entry_method(call.method)?;
        placement::check_arguments(vm, method, &call.arguments)?;
        code_graph(vm.program, call.method, &mut seen, &mut count)?;
        let invocation = vm
            .invocation(root)
            .ok_or_else(|| Error("owner acceptance has no invocation receipt".into()))?;
        require(
            invocation.method == call.method
                && invocation.arguments.len() == call.arguments.len() + 1
                && invocation.arguments[0] == Value::Reference(call.object),
            "owner invocation receiver/binding mismatch",
        )?;
        for (value, expected) in invocation.arguments[1..].iter().zip(&call.arguments) {
            require(
                matches_argument(vm, *value, expected)?,
                "owner invocation argument differs from accepted input",
            )?;
        }
        require(
            invocation.state != InvocationState::Running
                || !matches!(vm.context().status, ExecutionStatus::Waiting { .. }),
            "owner invocation cannot await an external capability",
        )?;
        records.insert(root.clone(), record);
    }
    Ok(records)
}
pub(crate) fn validate(vm: &Vm<'_>) -> Result<()> {
    journal(vm).map(|_| ())
}
fn project(vm: &Vm<'_>, call: &OwnerCall, receipt: &InvocationRecord) -> Result<OwnerReply> {
    let state = match &receipt.state {
        InvocationState::Running => ReplyState::Pending,
        InvocationState::Faulted { message } => {
            let mut end = message.len().min(4096);
            while !message.is_char_boundary(end) {
                end -= 1;
            }
            ReplyState::Faulted {
                diagnostic: message[..end].into(),
                truncated: end != message.len(),
            }
        }
        InvocationState::Completed { result } => {
            let value = match result {
                None => None,
                Some(Value::Null) => Some(Argument::Null),
                Some(Value::I32(n)) => Some(Argument::I32(*n)),
                Some(Value::I64(n)) => Some(Argument::I64(*n)),
                Some(Value::Reference(id)) => {
                    let units = vm
                        .heap()
                        .object(*id)?
                        .string_units()
                        .ok_or_else(|| Error("owner result is not a string".into()))?;
                    if units.len() > placement::MAX_TEXT_UNITS {
                        return Ok(OwnerReply {
                            execution: receipt.execution,
                            method: receipt.method,
                            object: call.object,
                            state: ReplyState::InvalidResult {
                                diagnostic: "owner result exceeds 4096 UTF-16 units".into(),
                            },
                        });
                    }
                    Some(Argument::Text(units.to_vec()))
                }
            };
            ReplyState::Completed(value)
        }
    };
    Ok(OwnerReply {
        execution: receipt.execution,
        method: receipt.method,
        object: call.object,
        state,
    })
}
/// Trusted host boundary: caller identity and owner context must be authenticated
/// externally. Acceptance commits exact authority/input and instance intent in
/// one checkpoint before any guest instruction. Reload after every save error.
pub fn accept(
    store: &mut CheckpointStore,
    context: &OwnerContext,
    authenticated_caller: UniverseId,
    call: &OwnerCall,
    budget: u64,
) -> Result<OwnerReply> {
    drive(
        store,
        context,
        authenticated_caller,
        call,
        budget,
        true,
        None,
    )
}
/// Receipt-only reconciliation: a missing accepted operation never starts work.
pub fn resume(
    store: &mut CheckpointStore,
    context: &OwnerContext,
    authenticated_caller: UniverseId,
    call: &OwnerCall,
    budget: u64,
    expected: Option<ExecutionId>,
) -> Result<OwnerReply> {
    drive(
        store,
        context,
        authenticated_caller,
        call,
        budget,
        false,
        expected,
    )
}
#[allow(clippy::too_many_arguments)]
fn drive(
    store: &mut CheckpointStore,
    context: &OwnerContext,
    authenticated_caller: UniverseId,
    call: &OwnerCall,
    budget: u64,
    allow_new: bool,
    expected: Option<ExecutionId>,
) -> Result<OwnerReply> {
    let id = key(authenticated_caller, &call.operation)?;
    let saved = store.load()?;
    let mut vm = saved.snapshot.restore()?;
    require(
        !crate::remote::has_imports(&vm),
        "owner universes cannot import remote contracts in this profile",
    )?;
    let records = journal(&vm)?;
    placement::check_owner_context(&vm, context, call.owner, call.epoch, call.object)?;
    let arguments = if let Some(record) = records.get(&id) {
        require(
            request(record)? == (authenticated_caller, call.clone()),
            "owner operation ID is already bound to another call",
        )?;
        let receipt = vm.invocation(&id).unwrap();
        require(
            expected.is_none_or(|id| id == receipt.execution),
            "owner execution differs from caller receipt",
        )?;
        receipt.arguments[1..].to_vec()
    } else {
        require(
            allow_new && expected.is_none(),
            "owner accepted receipt is missing; outcome remains uncertain",
        )?;
        require(records.len() < MAX_CALLS, "owner acceptance limit reached")?;
        require(
            vm.invocation(&id).is_none(),
            "owner operation ID collides with an existing invocation",
        )?;
        require(
            matches!(
                vm.context().status,
                ExecutionStatus::Completed { .. } | ExecutionStatus::Faulted { .. }
            ),
            "owner is busy with another execution",
        )?;
        let decision = placement::authorize_snapshot(
            &vm,
            context,
            authenticated_caller,
            call.grant,
            &call.access(),
        )?;
        code_graph(vm.program, call.method, &mut BTreeSet::new(), &mut 0)?;
        let mut values = Vec::new();
        for argument in &call.arguments {
            values.push(match argument {
                Argument::I32(n) => Value::I32(*n),
                Argument::I64(n) => Value::I64(*n),
                Argument::Null => Value::Null,
                Argument::Text(units) => Value::Reference(vm.new_string(units.clone())?),
            });
        }
        let record = Accepted {
            format: 1,
            caller: authenticated_caller.to_string(),
            operation: call.operation.clone(),
            owner: call.owner.to_string(),
            epoch: call.epoch,
            grant: call.grant.to_string(),
            object: call.object.to_string(),
            method: call.method.method.to_string(),
            code: call.method.code.to_string(),
            arguments: call.arguments.clone(),
            registry_hash: decision.registry_hash,
            prepared_generation: decision.observed_generation,
        };
        let object = vm.new_string(text(&record)?.encode_utf16().collect())?;
        vm.set_root(id.clone(), object)?;
        values
    };
    vm.set_model_calls_enabled(false);
    let receipt = vm.invoke_instance_once(
        store,
        &id,
        InstanceCall {
            method: call.method,
            receiver: call.object,
            arguments,
        },
        budget,
        1024,
    )?;
    project(&vm, call, &receipt)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        cil::decode,
        identity::{AssemblyId, MethodId, TypeId},
        program::{Method, Signature},
    };
    fn add(program: &mut Program, bytes: &[u8]) -> MethodVersion {
        let binding = MethodVersion {
            method: MethodId::fresh(),
            code: CodeVersionId::fresh(),
        };
        program.dispatch.insert(binding.method, binding.code);
        program.methods.insert(
            binding.code,
            Method {
                intrinsic: None,
                binding,
                owner: TypeId::fresh(),
                name: "Helper".into(),
                signature: Signature {
                    instance: false,
                    parameters: vec![],
                    result: None,
                },
                locals: vec![],
                initialize_locals: false,
                max_stack: 0,
                instructions: decode(bytes).unwrap(),
                assembly: AssemblyId::fresh(),
            },
        );
        binding
    }
    #[test]
    fn code_closure_limits_are_aggregate_and_cycles_do_not_repeat_work() {
        let mut program = Program::default();
        let root = add(&mut program, &[0x28, 1, 0, 0, 6, 0x2a]);
        let assembly = program.methods[&root.code].assembly;
        program
            .method_tokens
            .insert((assembly, 0x06000001), root.method);
        let mut count = 0;
        code_graph(&program, root, &mut BTreeSet::new(), &mut count).unwrap();
        assert_eq!(count, 2);
        let mut previous = root;
        for _ in 0..MAX_METHODS {
            let next = add(&mut program, &[0x28, 1, 0, 0, 6, 0x2a]);
            let assembly = program.methods[&previous.code].assembly;
            program
                .method_tokens
                .insert((assembly, 0x06000001), next.method);
            let assembly = program.methods[&next.code].assembly;
            program
                .method_tokens
                .insert((assembly, 0x06000001), next.method);
            previous = next;
        }
        assert!(
            code_graph(&program, root, &mut BTreeSet::new(), &mut 0)
                .unwrap_err()
                .0
                .contains("256 methods")
        );
        let mut program = Program::default();
        let mut body = vec![0; MAX_INSTRUCTIONS - 1];
        body.push(0x2a);
        let exact = add(&mut program, &body);
        let mut count = 0;
        let mut seen = BTreeSet::new();
        code_graph(&program, exact, &mut seen, &mut count).unwrap();
        assert_eq!(count, MAX_INSTRUCTIONS);
        let extra = add(&mut program, &[0x2a]);
        assert!(
            code_graph(&program, extra, &mut seen, &mut count)
                .unwrap_err()
                .0
                .contains("16384 instructions")
        );
    }
}
