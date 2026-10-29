use crate::{
    cil::{Comparison, Op},
    effects::EffectState,
    error::{Error, Result, require},
    execution::{CallResult, Continuation, ExecutionStatus},
    identity::{FieldId, TypeId},
    model::{DEFAULT_MODEL, MessageRole},
    program::{Intrinsic, Method, Program, SlotType},
    runtime::Vm,
    value::Value,
};
use std::collections::{BTreeMap, BTreeSet, VecDeque};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Kind {
    I32,
    I64,
    Reference(TypeId),
    Null,
}
fn kind(ty: SlotType) -> Kind {
    match ty {
        SlotType::I32 | SlotType::Boolean | SlotType::Char => Kind::I32,
        SlotType::I64 => Kind::I64,
        SlotType::Reference(id) => Kind::Reference(id),
    }
}
fn constant_kind(value: Value) -> Result<Kind> {
    match value {
        Value::I32(_) => Ok(Kind::I32),
        Value::I64(_) => Ok(Kind::I64),
        Value::Null => Ok(Kind::Null),
        Value::Reference(_) => Err(Error("object constants are unsupported".into())),
    }
}
fn fields(program: &Program, id: TypeId) -> Result<BTreeSet<FieldId>> {
    let mut fields = BTreeSet::new();
    let mut current = Some(id);
    while let Some(id) = current {
        let ty = program
            .types
            .get(&id)
            .ok_or_else(|| Error("unknown object type in checkpoint".into()))?;
        fields.extend(
            ty.fields
                .iter()
                .filter(|id| !program.fields[id].is_static)
                .copied(),
        );
        current = ty.base;
    }
    Ok(fields)
}
pub(super) fn validate(vm: &Vm<'_>) -> Result<()> {
    vm.validate_invocations()?;
    vm.validate_installations()?;
    let retained = vm.retained_objects()?;
    let program = vm.program;
    for id in retained {
        let object = vm.heap.object(id)?;
        let expected = fields(program, object.type_id())?;
        if let Some(units) = object.string_units() {
            require(
                object.type_id() == program.string_type()?
                    && expected.is_empty()
                    && units.len() <= crate::heap::MAX_STRING_UNITS,
                "invalid string payload or schema",
            )?;
        } else {
            program.allocatable(object.type_id())?;
        }
        require(
            object.schema_version() == 1,
            "unsupported saved object schema",
        )?;
        require(
            object.fields().keys().copied().collect::<BTreeSet<_>>() == expected,
            "saved object fields do not match its schema",
        )?;
        for (field, value) in object.fields() {
            vm.check_value(*value, program.fields[field].value_type)?;
        }
    }
    let expected: BTreeSet<_> = program
        .fields
        .values()
        .filter(|f| f.is_static && !f.literal)
        .map(|f| f.id)
        .collect();
    require(
        vm.statics.keys().copied().collect::<BTreeSet<_>>() == expected,
        "saved static storage does not match its schema",
    )?;
    for (field, value) in &vm.statics {
        vm.check_value(*value, program.fields[field].value_type)?;
    }
    for (units, id) in &vm.interned_strings {
        let object = vm.heap.object(*id)?;
        require(
            object.type_id() == program.string_type()?
                && object.string_units() == Some(units.as_slice()),
            "interned string does not match its managed payload",
        )?;
        require(
            units.is_empty()
                || program
                    .string_literals
                    .values()
                    .any(|literal| literal == units),
            "interned string is not a retained literal",
        )?;
    }
    for (id, effect) in &vm.effects {
        require(
            !effect.capability.is_empty() && effect.capability.len() <= 128,
            "invalid effect capability name",
        )?;
        require(*id == effect.id, "effect record identity mismatch")?;
        vm.materialize_request(*id, &effect.capability, &effect.model, &effect.messages)?
            .validate()?;
        require(
            effect
                .messages
                .last()
                .is_some_and(|m| m.content == effect.request),
            "model request does not match its final message",
        )?;
        for object in effect.references() {
            let object = vm.heap.object(object)?;
            require(
                object.type_id() == program.string_type()? && object.string_units().is_some(),
                "effect request or response is not a managed string",
            )?;
        }
    }
    require(
        vm.max_frames > 0 && vm.context.frames.len() <= vm.max_frames,
        "saved frame limit is inconsistent",
    )?;
    if let Some(entry) = vm.entry {
        let method = program
            .methods
            .get(&entry.code)
            .ok_or_else(|| Error("missing entry code version".into()))?;
        require(
            method.binding == entry && !method.signature.instance,
            "invalid saved entry binding",
        )?;
        if let Some(first) = vm.context.frames.first() {
            require(
                first.method == entry,
                "saved stack has a different entry method",
            )?;
        }
        if let ExecutionStatus::Completed { result } = vm.context.status {
            match (result, method.signature.result) {
                (Some(value), Some(ty)) => vm.check_value(value, ty)?,
                (None, None) => {}
                _ => {
                    return Err(Error(
                        "saved completion result does not match entry signature".into(),
                    ));
                }
            }
        }
    } else {
        require(
            vm.context.frames.is_empty()
                && vm.context.status == ExecutionStatus::Completed { result: None },
            "saved execution has no entry method",
        )?;
    }
    match vm.context.status {
        ExecutionStatus::Completed { .. } => require(
            vm.context.frames.is_empty(),
            "completed context still has frames",
        )?,
        _ => require(
            !vm.context.frames.is_empty(),
            "active context has no frames",
        )?,
    }
    let mut shapes = BTreeMap::new();
    for (index, frame) in vm.context.frames.iter().enumerate() {
        let method = vm.current_method(frame)?;
        require(
            method.intrinsic.is_none(),
            "intrinsic method cannot have a suspended frame",
        )?;
        require(
            frame.arguments.len()
                == method.signature.parameters.len() + usize::from(method.signature.instance),
            "saved argument count does not match method signature",
        )?;
        if method.signature.instance {
            vm.check_value(frame.arguments[0], SlotType::Reference(method.owner))?;
        }
        for (value, ty) in frame
            .arguments
            .iter()
            .skip(usize::from(method.signature.instance))
            .zip(&method.signature.parameters)
        {
            vm.check_value(*value, *ty)?;
        }
        require(
            frame.locals.len() == method.locals.len(),
            "saved local count does not match method body",
        )?;
        for (value, ty) in frame.locals.iter().zip(&method.locals) {
            match value {
                Some(value) => vm.check_value(*value, *ty)?,
                None => require(!method.initialize_locals, "initialized local is missing")?,
            }
        }
        if let std::collections::btree_map::Entry::Vacant(entry) = shapes.entry(method.binding.code)
        {
            entry.insert(stack_shapes(program, method)?);
        }
        let positions = &shapes[&method.binding.code];
        let expected = match frame.continuation {
            Continuation::Execute { next } => {
                require(
                    !matches!(vm.context.status, ExecutionStatus::Waiting { .. }),
                    "waiting context has an executable active frame",
                )?;
                require(
                    index + 1 == vm.context.frames.len(),
                    "non-active caller is not awaiting return",
                )?;
                positions
                    .get(&next)
                    .cloned()
                    .ok_or_else(|| Error("saved offset is not reachable in this method".into()))?
            }
            Continuation::AwaitEffect {
                call_site,
                next,
                effect,
                request,
            } => {
                require(
                    index + 1 == vm.context.frames.len(),
                    "effect waiter must be the active frame",
                )?;
                require(
                    vm.context.status == (ExecutionStatus::Waiting { effect }),
                    "waiting status and effect continuation disagree",
                )?;
                let record = vm.effect(effect)?;
                require(
                    record.request == request,
                    "waiting effect request or capability mismatch",
                )?;
                require(
                    !matches!(record.state, EffectState::Completed { .. }),
                    "waiting effect already has a completed response",
                )?;
                let instruction = method
                    .instructions
                    .get(&call_site)
                    .ok_or_else(|| Error("saved effect call site is invalid".into()))?;
                require(
                    instruction.next == next,
                    "saved effect return offset does not follow the call",
                )?;
                let Op::Call { token, .. } = instruction.op else {
                    return Err(Error(
                        "saved effect continuation does not point at a call".into(),
                    ));
                };
                let callee = program.method(program.method_tokens[&(method.assembly, token)])?;
                require(
                    matches!(
                        callee.intrinsic,
                        Some(Intrinsic::ModelComplete | Intrinsic::ModelChat)
                    ),
                    "saved effect call is not the model capability",
                )?;
                if callee.intrinsic == Some(Intrinsic::ModelComplete) {
                    require(
                        record.capability == "local.echo"
                            && record.model == DEFAULT_MODEL
                            && record.messages.len() == 1
                            && record.messages[0].role == MessageRole::User,
                        "simple model call has a different saved binding or message sequence",
                    )?;
                }
                let mut shape = positions
                    .get(&call_site)
                    .cloned()
                    .ok_or_else(|| Error("saved effect call site is unreachable".into()))?;
                require(
                    shape.len() >= callee.signature.parameters.len(),
                    "saved effect call stack underflow",
                )?;
                shape.truncate(shape.len() - callee.signature.parameters.len());
                require(
                    shape.len() < method.max_stack,
                    "saved effect result exceeds maxstack",
                )?;
                shape
            }
            Continuation::AwaitReturn {
                call_site,
                next,
                result,
            } => {
                let child = vm
                    .context
                    .frames
                    .get(index + 1)
                    .ok_or_else(|| Error("saved caller has no callee".into()))?;
                let instruction = method
                    .instructions
                    .get(&call_site)
                    .ok_or_else(|| Error("saved call site is invalid".into()))?;
                require(
                    instruction.next == next,
                    "saved return offset does not follow the call",
                )?;
                let (token, constructing) = match instruction.op {
                    Op::Call { token, .. } => (token, false),
                    Op::NewObject(token) => (token, true),
                    _ => {
                        return Err(Error(
                            "saved caller continuation does not point at a call".into(),
                        ));
                    }
                };
                let callee = program.method(program.method_tokens[&(method.assembly, token)])?;
                require(
                    child.method == callee.binding,
                    "saved child is not the called method version",
                )?;
                match (result, constructing) {
                    (CallResult::CalleeReturn, false) => {}
                    (CallResult::ConstructedObject(id), true) => require(
                        vm.heap.object(id)?.type_id() == callee.owner,
                        "pending constructor result has the wrong type",
                    )?,
                    _ => {
                        return Err(Error(
                            "saved constructor continuation is inconsistent".into(),
                        ));
                    }
                }
                let mut shape = positions
                    .get(&call_site)
                    .cloned()
                    .ok_or_else(|| Error("saved call site is unreachable".into()))?;
                let count = callee.signature.parameters.len()
                    + usize::from(callee.signature.instance && !constructing);
                require(shape.len() >= count, "saved call stack underflow")?;
                shape.truncate(shape.len() - count);
                shape
            }
        };
        require(
            frame.evaluation_stack.len() <= method.max_stack
                && frame.evaluation_stack.len() == expected.len(),
            "saved evaluation stack does not match its instruction position",
        )?;
        for (value, expected) in frame.evaluation_stack.iter().zip(expected) {
            let result = match expected {
                Kind::I32 => vm.check_value(*value, SlotType::I32),
                Kind::I64 => vm.check_value(*value, SlotType::I64),
                Kind::Reference(id) => vm.check_value(*value, SlotType::Reference(id)),
                Kind::Null => require(*value == Value::Null, "expected a null stack value"),
            };
            result.map_err(|e| {
                Error(format!(
                    "saved evaluation stack has an incompatible value: {e}"
                ))
            })?;
        }
    }
    Ok(())
}

fn pop(stack: &mut Vec<Kind>) -> Result<Kind> {
    stack
        .pop()
        .ok_or_else(|| Error("invalid code: abstract stack underflow".into()))
}
fn reference(kind: Kind) -> bool {
    matches!(kind, Kind::Reference(_) | Kind::Null)
}
fn compatible(program: &Program, actual: Kind, expected: Kind) -> bool {
    actual == expected
        || match (actual, expected) {
            (Kind::Null, Kind::Reference(_)) => true,
            (Kind::Reference(actual), Kind::Reference(expected)) => {
                program.is_assignable(actual, expected)
            }
            _ => false,
        }
}
fn take(program: &Program, stack: &mut Vec<Kind>, expected: Kind) -> Result<()> {
    require(
        compatible(program, pop(stack)?, expected),
        "invalid code: incompatible abstract stack type",
    )
}
fn pair(stack: &mut Vec<Kind>, integer: bool) -> Result<Kind> {
    let right = pop(stack)?;
    let left = pop(stack)?;
    require(
        if integer {
            left == right && !reference(left)
        } else {
            left == right || reference(left) && reference(right)
        },
        "invalid code: incompatible operand types",
    )?;
    Ok(left)
}
fn argument(method: &Method, index: usize) -> Result<Kind> {
    if method.signature.instance && index == 0 {
        return Ok(Kind::Reference(method.owner));
    }
    method
        .signature
        .parameters
        .get(index - usize::from(method.signature.instance))
        .copied()
        .map(kind)
        .ok_or_else(|| Error("invalid argument index".into()))
}
fn stack_shapes(program: &Program, method: &Method) -> Result<BTreeMap<u32, Vec<Kind>>> {
    let mut states = BTreeMap::from([(0, Vec::new())]);
    let mut pending = VecDeque::from([0]);
    while let Some(offset) = pending.pop_front() {
        let instruction = &method.instructions[&offset];
        let mut stack = states[&offset].clone();
        let mut targets = vec![instruction.next];
        match instruction.op {
            Op::LoadString(_) => stack.push(Kind::Reference(program.string_type()?)),
            Op::ReferenceCast { token, .. } => {
                require(
                    reference(pop(&mut stack)?),
                    "invalid reference cast operand",
                )?;
                stack.push(Kind::Reference(
                    program.reference_cast_target(method.assembly, token)?,
                ));
            }
            Op::Nop => {}
            Op::Constant(value) => stack.push(constant_kind(value)?),
            Op::LoadArgument(index) => stack.push(argument(method, index)?),
            Op::StoreArgument(index) => take(program, &mut stack, argument(method, index)?)?,
            Op::LoadLocal(index) => stack.push(kind(method.locals[index])),
            Op::StoreLocal(index) => take(program, &mut stack, kind(method.locals[index]))?,
            Op::Dup => {
                let top = *stack
                    .last()
                    .ok_or_else(|| Error("abstract dup underflow".into()))?;
                stack.push(top);
            }
            Op::Pop => {
                pop(&mut stack)?;
            }
            Op::Add
            | Op::Subtract
            | Op::Multiply
            | Op::AddChecked { .. }
            | Op::SubtractChecked { .. }
            | Op::MultiplyChecked { .. }
            | Op::Divide { .. }
            | Op::Remainder { .. }
            | Op::BitAnd
            | Op::BitOr
            | Op::BitXor => {
                let ty = pair(&mut stack, true)?;
                stack.push(ty);
            }
            Op::BitNot => {
                let ty = pop(&mut stack)?;
                require(!reference(ty), "invalid not operand")?;
                stack.push(ty);
            }
            Op::Negate => {
                let ty = pop(&mut stack)?;
                require(!reference(ty), "invalid neg operand")?;
                stack.push(ty);
            }
            Op::ShiftLeft | Op::ShiftRight { .. } => {
                require(pop(&mut stack)? == Kind::I32, "invalid shift count type")?;
                let value = pop(&mut stack)?;
                require(!reference(value), "invalid shift value type")?;
                stack.push(value);
            }
            Op::ConvertI8 | Op::ConvertU8 | Op::ConvertI16 | Op::ConvertU16 => {
                require(
                    !reference(pop(&mut stack)?),
                    "invalid small-width conversion operand",
                )?;
                stack.push(Kind::I32);
            }
            Op::ConvertI32 => {
                require(
                    !reference(pop(&mut stack)?),
                    "invalid conv.i4/conv.u4 operand",
                )?;
                stack.push(Kind::I32);
            }
            Op::ConvertChecked { wide, .. } => {
                require(
                    !reference(pop(&mut stack)?),
                    "invalid checked conversion operand",
                )?;
                stack.push(if wide { Kind::I64 } else { Kind::I32 });
            }
            Op::ConvertSmallChecked { .. } => {
                require(
                    !reference(pop(&mut stack)?),
                    "invalid small checked conversion operand",
                )?;
                stack.push(Kind::I32);
            }
            Op::ConvertI64 | Op::ConvertU64 => {
                require(
                    !reference(pop(&mut stack)?),
                    "invalid conv.i8/conv.u8 operand",
                )?;
                stack.push(Kind::I64);
            }
            Op::Branch(target) => targets = vec![target],
            Op::Switch(ref branches) => {
                require(pop(&mut stack)? == Kind::I32, "invalid switch index type")?;
                targets.extend(branches);
            }
            Op::BranchIf { target, .. } => {
                pop(&mut stack)?;
                targets.push(target);
            }
            Op::Compare {
                comparison,
                unsigned,
            } => {
                let ty = pair(&mut stack, false)?;
                require(
                    !reference(ty)
                        || matches!(comparison, Comparison::Equal)
                        || matches!(comparison, Comparison::Greater) && unsigned,
                    "invalid reference comparison",
                )?;
                stack.push(Kind::I32);
            }
            Op::BranchCompare {
                target, comparison, ..
            } => {
                let ty = pair(&mut stack, false)?;
                require(
                    !reference(ty)
                        || matches!(comparison, Comparison::Equal | Comparison::NotEqual),
                    "invalid reference ordering branch",
                )?;
                targets.push(target);
            }
            Op::LoadField { token, is_static } => {
                let field = &program.fields[&program.field_tokens[&(method.assembly, token)]];
                if !is_static {
                    take(program, &mut stack, Kind::Reference(field.owner))?;
                }
                stack.push(kind(field.value_type));
            }
            Op::StoreField { token, is_static } => {
                let field = &program.fields[&program.field_tokens[&(method.assembly, token)]];
                take(program, &mut stack, kind(field.value_type))?;
                if !is_static {
                    take(program, &mut stack, Kind::Reference(field.owner))?;
                }
            }
            Op::Call { token, .. } | Op::NewObject(token) => {
                let callee = program.method(program.method_tokens[&(method.assembly, token)])?;
                for parameter in callee.signature.parameters.iter().rev() {
                    take(program, &mut stack, kind(*parameter))?;
                }
                if matches!(instruction.op, Op::NewObject(_)) {
                    stack.push(Kind::Reference(callee.owner));
                } else {
                    if callee.signature.instance {
                        take(program, &mut stack, Kind::Reference(callee.owner))?;
                    }
                    if let Some(result) = callee.signature.result {
                        stack.push(kind(result));
                    }
                }
            }
            Op::Return => {
                if let Some(result) = method.signature.result {
                    take(program, &mut stack, kind(result))?;
                }
                require(stack.is_empty(), "invalid abstract return stack")?;
                targets.clear();
            }
        }
        require(
            stack.len() <= method.max_stack,
            "abstract stack exceeds maxstack",
        )?;
        for target in targets {
            if let Some(existing) = states.get_mut(&target) {
                require(
                    existing.len() == stack.len(),
                    "inconsistent stack heights at control flow merge",
                )?;
                let mut changed = false;
                for (slot, incoming) in existing.iter_mut().zip(&stack) {
                    let joined = join(program, *slot, *incoming)?;
                    changed |= joined != *slot;
                    *slot = joined;
                }
                if changed {
                    pending.push_back(target);
                }
            } else {
                states.insert(target, stack.clone());
                pending.push_back(target);
            }
        }
    }
    Ok(states)
}

fn join(program: &Program, left: Kind, right: Kind) -> Result<Kind> {
    if compatible(program, left, right) {
        return Ok(right);
    }
    if compatible(program, right, left) {
        return Ok(left);
    }
    if let (Kind::Reference(mut candidate), Kind::Reference(other)) = (left, right) {
        while let Some(base) = program.types[&candidate].base {
            if program.is_assignable(other, base) {
                return Ok(Kind::Reference(base));
            }
            candidate = base;
        }
    }
    Err(Error(
        "inconsistent stack types at control flow merge".into(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        cil::decode,
        execution::MethodVersion,
        identity::{AssemblyId, CodeVersionId, MethodId},
        program::Signature,
    };
    fn method(body: &[u8]) -> Method {
        Method {
            binding: MethodVersion {
                method: MethodId::fresh(),
                code: CodeVersionId::fresh(),
            },
            owner: TypeId::fresh(),
            name: "Switch".into(),
            assembly: AssemblyId::fresh(),
            signature: Signature {
                instance: false,
                parameters: vec![],
                result: Some(SlotType::I32),
            },
            locals: vec![],
            initialize_locals: false,
            max_stack: 4,
            intrinsic: None,
            instructions: decode(body).unwrap(),
        }
    }
    #[test]
    fn switch_abstract_validation_visits_all_targets_and_fallthrough_with_the_index_removed() {
        // Both paths must see an empty stack after selection, and then return I32.
        let valid = [0x16, 0x45, 1, 0, 0, 0, 2, 0, 0, 0, 0x16, 0x2a, 0x17, 0x2a];
        let program = Program::default();
        let shapes = stack_shapes(&program, &method(&valid)).unwrap();
        assert!(shapes[&10].is_empty() && shapes[&12].is_empty());
        for bad in [10, 12] {
            let mut body = valid.to_vec();
            body[bad] = 0x14; // bad return only on one successor
            assert!(stack_shapes(&program, &method(&body)).is_err());
        }
        let mut wide = vec![0x21];
        wide.extend(0i64.to_le_bytes());
        wide.extend([0x45, 0, 0, 0, 0, 0x16, 0x2a]);
        assert!(
            stack_shapes(&program, &method(&wide))
                .unwrap_err()
                .0
                .contains("switch index")
        );
        let invalid = [0x14, 0x45, 0, 0, 0, 0, 0x16, 0x2a];
        assert!(
            stack_shapes(&program, &method(&invalid))
                .unwrap_err()
                .0
                .contains("switch index")
        );
        // A branch and fallthrough merge with incompatible stack heights.
        let merge = [0x16, 0x45, 1, 0, 0, 0, 1, 0, 0, 0, 0x17, 0x16, 0x2a];
        assert!(stack_shapes(&program, &method(&merge)).is_err());
    }
}
