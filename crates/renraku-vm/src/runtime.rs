use crate::{
    cil::{Comparison, Op},
    effects::EffectRecord,
    error::{Error, Result, require},
    execution::{CallResult, Continuation, ExecutionContext, ExecutionStatus, Frame},
    heap::{Heap, MAX_STRING_UNITS},
    identity::{EffectId, ExecutionId, FieldId, MethodId, ObjectId, TypeId, UniverseId},
    program::{Intrinsic, Method, Program, SlotType, StringIntrinsic},
    value::Value,
};
use std::{
    cell::Cell,
    collections::{BTreeMap, BTreeSet},
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RunOutcome {
    Paused,
    Waiting(EffectId),
    Completed(Option<Value>),
}

/// One universe with one cooperative execution context. All guest call
/// state is in the context; a Rust call never remains live across a guest step.
pub struct Vm<'a> {
    pub(crate) program: &'a Program,
    pub(crate) heap: Heap,
    pub(crate) statics: BTreeMap<FieldId, Value>,
    pub(crate) context: ExecutionContext,
    pub(crate) instructions_executed: u64,
    pub(crate) max_frames: usize,
    pub(crate) universe: UniverseId,
    pub(crate) entry: Option<crate::execution::MethodVersion>,
    pub(crate) checkpoint_generation: Cell<u64>,
    pub(crate) interned_strings: BTreeMap<Vec<u16>, ObjectId>,
    pub(crate) effects: BTreeMap<crate::identity::EffectId, EffectRecord>,
    pub(crate) invocations: BTreeMap<String, crate::invocation::InvocationRecord>,
    pub(crate) installations: BTreeMap<String, crate::artifact::InstallationRecord>,
    // Host authorization is deliberately not persisted by guest checkpoints.
    pub(crate) model_calls_enabled: bool,
}
enum Mutation {
    String {
        id: ObjectId,
        type_id: TypeId,
        units: Vec<u16>,
        intern: bool,
    },
    Allocate {
        id: ObjectId,
        type_id: TypeId,
        fields: BTreeMap<FieldId, Value>,
    },
    Field {
        owner: ObjectId,
        field: FieldId,
        value: Value,
    },
    Static {
        field: FieldId,
        value: Value,
    },
}
enum Transition {
    Continue(Frame),
    Wait {
        frame: Frame,
        effect: EffectRecord,
    },
    Call {
        caller: Frame,
        callee: Frame,
    },
    Return {
        caller: Option<Frame>,
        result: Option<Value>,
    },
}

impl<'a> Vm<'a> {
    /// Atomically checkpoint an append-only registry with the existing heap and
    /// frames. On error reload the prior checkpoint; no partial registry is saved.
    pub fn install_program<'b>(
        self,
        store: &mut crate::checkpoint::CheckpointStore,
        program: &'b Program,
    ) -> Result<Vm<'b>> {
        let vm = self.rebind_program(program)?;
        store.save(&vm)?;
        Ok(vm)
    }
    pub(crate) fn rebind_program<'b>(self, program: &'b Program) -> Result<Vm<'b>> {
        program.extends(self.program)?;
        let mut statics = self.statics;
        for field in program
            .fields
            .values()
            .filter(|f| f.is_static && !f.literal)
        {
            statics
                .entry(field.id)
                .or_insert_with(|| field.value_type.default_value());
        }
        let vm = Vm {
            program,
            statics,
            heap: self.heap,
            context: self.context,
            instructions_executed: self.instructions_executed,
            max_frames: self.max_frames,
            universe: self.universe,
            entry: self.entry,
            checkpoint_generation: self.checkpoint_generation,
            interned_strings: self.interned_strings,
            effects: self.effects,
            invocations: self.invocations,
            installations: self.installations,
            model_calls_enabled: self.model_calls_enabled,
        };
        Ok(vm)
    }
    pub fn new(program: &'a Program) -> Self {
        let statics = program
            .fields
            .values()
            .filter(|f| f.is_static && !f.literal)
            .map(|f| (f.id, f.value_type.default_value()))
            .collect();
        Self {
            program,
            interned_strings: BTreeMap::new(),
            effects: BTreeMap::new(),
            invocations: BTreeMap::new(),
            installations: BTreeMap::new(),
            model_calls_enabled: false,
            universe: UniverseId::fresh(),
            entry: None,
            checkpoint_generation: Cell::new(0),
            heap: Heap::default(),
            statics,
            context: ExecutionContext {
                id: ExecutionId::fresh(),
                status: ExecutionStatus::Completed { result: None },
                frames: Vec::new(),
            },
            instructions_executed: 0,
            max_frames: 1024,
        }
    }
    pub fn heap(&self) -> &Heap {
        &self.heap
    }
    pub fn universe_id(&self) -> UniverseId {
        self.universe
    }
    pub fn set_root(&mut self, name: impl Into<String>, id: ObjectId) -> Result<()> {
        Ok(self.heap.set_root(name, id)?)
    }
    pub fn remove_root(&mut self, name: &str) -> Option<ObjectId> {
        self.heap.remove_root(name)
    }
    pub fn collect_garbage(&mut self) -> Result<usize> {
        let retained = self.retained_objects()?;
        let before = self.heap.objects().count();
        self.heap = self.heap.retained_subset(&retained);
        Ok(before - self.heap.objects().count())
    }
    pub fn context(&self) -> &ExecutionContext {
        &self.context
    }
    pub fn set_model_calls_enabled(&mut self, enabled: bool) {
        self.model_calls_enabled = enabled;
    }
    pub fn static_fields(&self) -> &BTreeMap<FieldId, Value> {
        &self.statics
    }
    pub fn instructions_executed(&self) -> u64 {
        self.instructions_executed
    }
    pub fn retained_objects(&self) -> Result<BTreeSet<ObjectId>> {
        Ok(self.heap.retained_objects(
            self.context
                .references()
                .chain(self.statics.values().filter_map(Value::referenced_object))
                .chain(self.interned_strings.values().copied())
                .chain(self.effects.values().flat_map(EffectRecord::references))
                .chain(
                    self.invocations
                        .values()
                        .flat_map(crate::invocation::InvocationRecord::references),
                ),
        )?)
    }
    pub fn set_frame_limit(&mut self, limit: usize) -> Result<()> {
        require(
            limit > 0 && limit >= self.context.frames.len(),
            "invalid frame limit",
        )?;
        self.max_frames = limit;
        Ok(())
    }
    pub fn start(&mut self, entry: MethodId, arguments: Vec<Value>) -> Result<()> {
        require(
            matches!(
                self.context.status,
                ExecutionStatus::Completed { .. } | ExecutionStatus::Faulted { .. }
            ),
            "an execution is already active",
        )?;
        let method = self.program.method(entry)?;
        require(
            method.intrinsic.is_none(),
            "intrinsics must be invoked from CIL",
        )?;
        require(!method.signature.instance, "entry method must be static")?;
        self.check_parameters(method, &arguments)?;
        self.entry = Some(method.binding);
        self.context = ExecutionContext {
            id: ExecutionId::fresh(),
            status: ExecutionStatus::Runnable,
            frames: vec![new_frame(method, arguments)],
        };
        Ok(())
    }
    pub fn run(&mut self, budget: u64) -> Result<RunOutcome> {
        match &self.context.status {
            ExecutionStatus::Completed { result } => return Ok(RunOutcome::Completed(*result)),
            ExecutionStatus::Faulted { message } => return Err(Error(message.clone())),
            ExecutionStatus::Waiting { effect } => return Ok(RunOutcome::Waiting(*effect)),
            _ => {}
        }
        self.context.status = ExecutionStatus::Runnable;
        for _ in 0..budget {
            if let Err(error) = self.step() {
                self.context.status = ExecutionStatus::Faulted {
                    message: error.to_string(),
                };
                self.finish_invocation();
                return Err(error);
            }
            self.instructions_executed = self.instructions_executed.saturating_add(1);
            if let ExecutionStatus::Completed { result } = self.context.status {
                self.finish_invocation();
                return Ok(RunOutcome::Completed(result));
            }
            if let ExecutionStatus::Waiting { effect } = self.context.status {
                return Ok(RunOutcome::Waiting(effect));
            }
        }
        self.context.status = ExecutionStatus::Paused;
        Ok(RunOutcome::Paused)
    }
    pub(crate) fn check_value(&self, value: Value, expected: SlotType) -> Result<()> {
        let valid = match (value, expected) {
            (Value::I32(_), SlotType::I32) | (Value::I64(_), SlotType::I64) => true,
            (Value::I32(0 | 1), SlotType::Boolean) => true,
            (Value::I32(0..=65535), SlotType::Char) => true,
            (Value::Null, SlotType::Reference(_)) => true,
            (Value::Reference(id), SlotType::Reference(expected)) => self
                .program
                .is_assignable(self.heap.object(id)?.type_id(), expected),
            _ => false,
        };
        require(valid, "value does not match the declared slot type")
    }
    fn check_parameters(&self, method: &Method, arguments: &[Value]) -> Result<()> {
        require(
            arguments.len() == method.signature.parameters.len(),
            "argument count does not match signature",
        )?;
        for (&value, &expected) in arguments.iter().zip(&method.signature.parameters) {
            self.check_value(value, expected)?;
        }
        Ok(())
    }
    fn receiver(&self, value: Value, owner: TypeId) -> Result<ObjectId> {
        match value {
            Value::Reference(id) => {
                require(
                    self.program
                        .is_assignable(self.heap.object(id)?.type_id(), owner),
                    "receiver type does not declare this member",
                )?;
                Ok(id)
            }
            Value::Null => Err(Error("null receiver".into())),
            _ => Err(Error("receiver is not an object reference".into())),
        }
    }
    pub(crate) fn current_method(&self, frame: &Frame) -> Result<&Method> {
        let method = self
            .program
            .methods
            .get(&frame.method.code)
            .ok_or_else(|| Error("missing pinned code version".into()))?;
        require(
            method.binding == frame.method,
            "pinned code version has a different logical method",
        )?;
        Ok(method)
    }
    fn step(&mut self) -> Result<()> {
        let active = self
            .context
            .frames
            .last()
            .ok_or_else(|| Error("runnable context has no frame".into()))?;
        let offset = match active.continuation {
            Continuation::Execute { next } => next,
            _ => return Err(Error("active frame is waiting for a missing callee".into())),
        };
        let method = self.current_method(active)?;
        let name = format!(
            "{}::{}",
            self.program.types[&method.owner].name, method.name
        );
        let prepared = self
            .prepare_step(active, method, offset)
            .map_err(|error| Error(format!("{name} IL_{offset:04x}: {error}")))?;
        let (transition, mutation) = prepared;
        // Every fallible guest operation has been checked against a cloned frame.
        // Publish heap effects and the complete continuation transition together
        // before allowing the next safe point.
        if let Some(mutation) = mutation {
            match mutation {
                Mutation::String {
                    id,
                    type_id,
                    units,
                    intern,
                } => {
                    if intern {
                        self.heap.allocate_string(id, type_id, units.clone())?;
                        self.interned_strings.insert(units, id);
                    } else {
                        self.heap.allocate_string(id, type_id, units)?;
                    }
                }
                Mutation::Allocate {
                    id,
                    type_id,
                    fields,
                } => self.heap.allocate_initialized(id, type_id, fields)?,
                Mutation::Field {
                    owner,
                    field,
                    value,
                } => self.heap.set_field(owner, field, value)?,
                Mutation::Static { field, value } => {
                    self.statics.insert(field, value);
                }
            }
        }
        match transition {
            Transition::Continue(frame) => {
                *self.context.frames.last_mut().unwrap() = frame;
            }
            Transition::Wait { frame, effect } => {
                self.context.status = ExecutionStatus::Waiting { effect: effect.id };
                self.effects.insert(effect.id, effect);
                *self.context.frames.last_mut().unwrap() = frame;
            }
            Transition::Call { caller, callee } => {
                *self.context.frames.last_mut().unwrap() = caller;
                self.context.frames.push(callee);
            }
            Transition::Return { caller, result } => {
                self.context.frames.pop();
                if let Some(caller) = caller {
                    *self.context.frames.last_mut().unwrap() = caller;
                } else {
                    self.context.status = ExecutionStatus::Completed { result };
                }
            }
        }
        Ok(())
    }
    fn prepare_step(
        &self,
        active: &Frame,
        method: &Method,
        offset: u32,
    ) -> Result<(Transition, Option<Mutation>)> {
        let instruction = method
            .instructions
            .get(&offset)
            .ok_or_else(|| Error("instruction offset is invalid".into()))?;
        let mut frame = active.clone();
        frame.continuation = Continuation::Execute {
            next: instruction.next,
        };
        let mut mutation = None;
        match instruction.op {
            Op::LoadString(token) => {
                let units = &self.program.string_literals[&(method.assembly, token)];
                let (value, effect) = self.prepare_string(units.clone(), true)?;
                frame.evaluation_stack.push(value);
                mutation = effect;
            }
            Op::Nop => {}
            Op::Constant(value) => frame.evaluation_stack.push(value),
            Op::LoadArgument(index) => frame.evaluation_stack.push(
                *frame
                    .arguments
                    .get(index)
                    .ok_or_else(|| Error("argument index out of range".into()))?,
            ),
            Op::StoreArgument(index) => {
                let value = pop(&mut frame)?;
                let expected = if method.signature.instance && index == 0 {
                    SlotType::Reference(method.owner)
                } else {
                    *method
                        .signature
                        .parameters
                        .get(index - usize::from(method.signature.instance))
                        .ok_or_else(|| Error("argument index out of range".into()))?
                };
                self.check_value(value, expected)?;
                *frame
                    .arguments
                    .get_mut(index)
                    .ok_or_else(|| Error("argument index out of range".into()))? = value;
            }
            Op::LoadLocal(index) => frame.evaluation_stack.push(
                frame
                    .locals
                    .get(index)
                    .copied()
                    .flatten()
                    .ok_or_else(|| Error("local is uninitialized or out of range".into()))?,
            ),
            Op::StoreLocal(index) => {
                let value = pop(&mut frame)?;
                self.check_value(
                    value,
                    *method
                        .locals
                        .get(index)
                        .ok_or_else(|| Error("local index out of range".into()))?,
                )?;
                frame.locals[index] = Some(value);
            }
            Op::Dup => {
                let value = *frame
                    .evaluation_stack
                    .last()
                    .ok_or_else(|| Error("evaluation stack underflow".into()))?;
                frame.evaluation_stack.push(value);
            }
            Op::Pop => {
                pop(&mut frame)?;
            }
            Op::ConvertI32 => {
                let value = match pop(&mut frame)? {
                    Value::I32(value) => Value::I32(value),
                    Value::I64(value) => Value::I32(value as i32),
                    _ => return Err(Error("conv.i4/conv.u4 requires an integer".into())),
                };
                frame.evaluation_stack.push(value);
            }
            Op::ConvertI64 | Op::ConvertU64 => {
                let value = match pop(&mut frame)? {
                    Value::I32(value) if matches!(instruction.op, Op::ConvertU64) => {
                        Value::I64(i64::from(value as u32))
                    }
                    Value::I32(value) => Value::I64(i64::from(value)),
                    Value::I64(value) => Value::I64(value),
                    _ => {
                        return Err(Error(
                            if matches!(instruction.op, Op::ConvertU64) {
                                "conv.u8 requires an integer"
                            } else {
                                "conv.i8 requires an integer"
                            }
                            .into(),
                        ));
                    }
                };
                frame.evaluation_stack.push(value);
            }
            Op::Add | Op::Subtract | Op::Multiply | Op::Divide { .. } | Op::Remainder { .. } => {
                let right = pop(&mut frame)?;
                let left = pop(&mut frame)?;
                let value = arithmetic(left, right, &instruction.op)?;
                frame.evaluation_stack.push(value);
            }
            Op::Branch(target) => frame.continuation = Continuation::Execute { next: target },
            Op::BranchIf { target, when_true } => {
                if truth(pop(&mut frame)?) == when_true {
                    frame.continuation = Continuation::Execute { next: target };
                }
            }
            Op::Compare {
                comparison,
                unsigned,
            } => {
                let right = pop(&mut frame)?;
                let left = pop(&mut frame)?;
                frame.evaluation_stack.push(Value::I32(i32::from(compare(
                    left,
                    right,
                    comparison,
                    unsigned,
                    matches!(comparison, Comparison::Greater) && unsigned,
                )?)));
            }
            Op::BranchCompare {
                target,
                comparison,
                unsigned,
            } => {
                let right = pop(&mut frame)?;
                let left = pop(&mut frame)?;
                if compare(left, right, comparison, unsigned, false)? {
                    frame.continuation = Continuation::Execute { next: target };
                }
            }
            Op::LoadField { token, is_static } => {
                let field =
                    &self.program.fields[&self.program.field_tokens[&(method.assembly, token)]];
                let value = if is_static {
                    self.statics[&field.id]
                } else {
                    let receiver = self.receiver(pop(&mut frame)?, field.owner)?;
                    *self
                        .heap
                        .object(receiver)?
                        .fields()
                        .get(&field.id)
                        .ok_or_else(|| Error("object field is missing".into()))?
                };
                frame.evaluation_stack.push(value);
            }
            Op::StoreField { token, is_static } => {
                let field =
                    &self.program.fields[&self.program.field_tokens[&(method.assembly, token)]];
                let value = pop(&mut frame)?;
                self.check_value(value, field.value_type)?;
                mutation = Some(if is_static {
                    Mutation::Static {
                        field: field.id,
                        value,
                    }
                } else {
                    let owner = self.receiver(pop(&mut frame)?, field.owner)?;
                    Mutation::Field {
                        owner,
                        field: field.id,
                        value,
                    }
                });
            }
            Op::Call { token, .. } | Op::NewObject(token) => {
                let callee = self
                    .program
                    .method(self.program.method_tokens[&(method.assembly, token)])?;
                let constructing = matches!(instruction.op, Op::NewObject(_));
                let count = callee.signature.parameters.len()
                    + usize::from(callee.signature.instance && !constructing);
                require(
                    frame.evaluation_stack.len() >= count,
                    "evaluation stack underflow at call",
                )?;
                let mut arguments = frame
                    .evaluation_stack
                    .split_off(frame.evaluation_stack.len() - count);
                let result = if constructing {
                    self.check_parameters(callee, &arguments)?;
                    let mut id = ObjectId::fresh();
                    while self.heap.object(id).is_ok() {
                        id = ObjectId::fresh();
                    }
                    let mut fields = BTreeMap::new();
                    let mut current = Some(callee.owner);
                    while let Some(owner) = current {
                        let ty = &self.program.types[&owner];
                        for id in &ty.fields {
                            let field = &self.program.fields[id];
                            if !field.is_static {
                                fields.insert(*id, field.value_type.default_value());
                            }
                        }
                        current = ty.base;
                    }
                    mutation = Some(Mutation::Allocate {
                        id,
                        type_id: callee.owner,
                        fields,
                    });
                    arguments.insert(0, Value::Reference(id));
                    CallResult::ConstructedObject(id)
                } else {
                    let parameter_start = usize::from(callee.signature.instance);
                    self.check_parameters(callee, &arguments[parameter_start..])?;
                    if callee.signature.instance {
                        let receiver = arguments[0];
                        self.check_value(receiver, SlotType::Reference(callee.owner))?;
                        if matches!(
                            instruction.op,
                            Op::Call {
                                null_check: true,
                                ..
                            }
                        ) {
                            self.receiver(receiver, callee.owner)?;
                        }
                    }
                    if let Some(intrinsic) = callee.intrinsic {
                        let Intrinsic::String(intrinsic) = intrinsic else {
                            require(
                                self.model_calls_enabled,
                                "model capability calls are disabled by the host",
                            )?;
                            let effect = match intrinsic {
                                Intrinsic::ModelComplete => {
                                    let Value::Reference(request) = arguments[0] else {
                                        return Err(Error(
                                            "model prompt must be a non-null string".into(),
                                        ));
                                    };
                                    self.prepare_model_request(request)?
                                }
                                Intrinsic::ModelChat => {
                                    let [Value::Reference(binding), Value::Reference(head)] =
                                        arguments[..]
                                    else {
                                        return Err(Error(
                                            "model binding and message head must be non-null"
                                                .into(),
                                        ));
                                    };
                                    let [
                                        SlotType::Reference(binding_type),
                                        SlotType::Reference(message_type),
                                    ] = callee.signature.parameters[..]
                                    else {
                                        return Err(Error(
                                            "invalid chat intrinsic parameters".into(),
                                        ));
                                    };
                                    self.prepare_guest_chat(
                                        binding,
                                        head,
                                        binding_type,
                                        message_type,
                                    )?
                                }
                                Intrinsic::String(_) => unreachable!(),
                            };
                            let request = effect.request;
                            frame.continuation = Continuation::AwaitEffect {
                                call_site: offset,
                                next: instruction.next,
                                effect: effect.id,
                                request,
                            };
                            require(
                                frame.evaluation_stack.len() < method.max_stack,
                                "maxstack exceeded by model result",
                            )?;
                            return Ok((Transition::Wait { frame, effect }, None));
                        };
                        let (value, effect) = self.prepare_intrinsic(intrinsic, &arguments)?;
                        frame.evaluation_stack.push(value);
                        require(
                            frame.evaluation_stack.len() <= method.max_stack,
                            "maxstack exceeded",
                        )?;
                        return Ok((Transition::Continue(frame), effect));
                    }
                    CallResult::CalleeReturn
                };
                require(
                    self.context.frames.len() < self.max_frames,
                    "guest frame limit exceeded",
                )?;
                frame.continuation = Continuation::AwaitReturn {
                    call_site: offset,
                    next: instruction.next,
                    result,
                };
                require(
                    frame.evaluation_stack.len() <= method.max_stack,
                    "maxstack exceeded",
                )?;
                return Ok((
                    Transition::Call {
                        caller: frame,
                        callee: new_frame(callee, arguments),
                    },
                    mutation,
                ));
            }
            Op::Return => {
                let result = if let Some(expected) = method.signature.result {
                    let value = pop(&mut frame)?;
                    self.check_value(value, expected)?;
                    Some(value)
                } else {
                    None
                };
                require(
                    frame.evaluation_stack.is_empty(),
                    "return has extra evaluation stack values",
                )?;
                let caller = if self.context.frames.len() > 1 {
                    let mut caller = self.context.frames[self.context.frames.len() - 2].clone();
                    let Continuation::AwaitReturn {
                        next,
                        result: call_result,
                        ..
                    } = caller.continuation
                    else {
                        return Err(Error("caller is not awaiting a return".into()));
                    };
                    let produced = match call_result {
                        CallResult::CalleeReturn => result,
                        CallResult::ConstructedObject(id) => Some(Value::Reference(id)),
                    };
                    if let Some(value) = produced {
                        caller.evaluation_stack.push(value);
                    }
                    caller.continuation = Continuation::Execute { next };
                    require(
                        caller.evaluation_stack.len() <= self.current_method(&caller)?.max_stack,
                        "caller maxstack exceeded",
                    )?;
                    Some(caller)
                } else {
                    None
                };
                return Ok((Transition::Return { caller, result }, None));
            }
        }
        require(
            frame.evaluation_stack.len() <= method.max_stack,
            "maxstack exceeded",
        )?;
        Ok((Transition::Continue(frame), mutation))
    }
}

impl Vm<'_> {
    fn prepare_string(&self, units: Vec<u16>, intern: bool) -> Result<(Value, Option<Mutation>)> {
        require(
            units.len() <= MAX_STRING_UNITS,
            "string exceeds the UTF-16 unit limit",
        )?;
        if intern && let Some(id) = self.interned_strings.get(&units) {
            return Ok((Value::Reference(*id), None));
        }
        let mut id = ObjectId::fresh();
        while self.heap.object(id).is_ok() {
            id = ObjectId::fresh();
        }
        Ok((
            Value::Reference(id),
            Some(Mutation::String {
                id,
                type_id: self.program.string_type()?,
                units,
                intern,
            }),
        ))
    }
    fn string_value(&self, value: Value) -> Result<Option<&[u16]>> {
        match value {
            Value::Null => Ok(None),
            Value::Reference(id) => self
                .heap
                .object(id)?
                .string_units()
                .map(Some)
                .ok_or_else(|| Error("object is not a managed string".into())),
            _ => Err(Error("value is not a string reference".into())),
        }
    }
    fn prepare_intrinsic(
        &self,
        intrinsic: StringIntrinsic,
        args: &[Value],
    ) -> Result<(Value, Option<Mutation>)> {
        let value = match intrinsic {
            StringIntrinsic::Length => {
                let units = self
                    .string_value(args[0])?
                    .ok_or_else(|| Error("null receiver".into()))?;
                Value::I32(units.len() as i32)
            }
            StringIntrinsic::CharAt => {
                let units = self
                    .string_value(args[0])?
                    .ok_or_else(|| Error("null receiver".into()))?;
                let Value::I32(index) = args[1] else {
                    return Err(Error("string index must be Int32".into()));
                };
                let index = usize::try_from(index)
                    .map_err(|_| Error("string index out of range".into()))?;
                Value::I32(
                    *units
                        .get(index)
                        .ok_or_else(|| Error("string index out of range".into()))?
                        as i32,
                )
            }
            StringIntrinsic::Concat => {
                let left = self.string_value(args[0])?.unwrap_or(&[]);
                let right = self.string_value(args[1])?.unwrap_or(&[]);
                if right.is_empty() && !left.is_empty() {
                    return Ok((args[0], None));
                }
                if left.is_empty() && !right.is_empty() {
                    return Ok((args[1], None));
                }
                require(
                    left.len() + right.len() <= MAX_STRING_UNITS,
                    "string exceeds the UTF-16 unit limit",
                )?;
                let mut units = Vec::with_capacity(left.len() + right.len());
                units.extend(left);
                units.extend(right);
                let intern = units.is_empty();
                return self.prepare_string(units, intern);
            }
            StringIntrinsic::Equal | StringIntrinsic::NotEqual => {
                let equal = self.string_value(args[0])? == self.string_value(args[1])?;
                Value::I32(i32::from(if intrinsic == StringIntrinsic::NotEqual {
                    !equal
                } else {
                    equal
                }))
            }
        };
        Ok((value, None))
    }
}

fn new_frame(method: &Method, arguments: Vec<Value>) -> Frame {
    Frame {
        method: method.binding,
        continuation: Continuation::Execute { next: 0 },
        arguments,
        locals: method
            .locals
            .iter()
            .map(|ty| {
                if method.initialize_locals {
                    Some(ty.default_value())
                } else {
                    None
                }
            })
            .collect(),
        evaluation_stack: Vec::new(),
    }
}
fn pop(frame: &mut Frame) -> Result<Value> {
    frame
        .evaluation_stack
        .pop()
        .ok_or_else(|| Error("evaluation stack underflow".into()))
}
fn truth(value: Value) -> bool {
    match value {
        Value::I32(v) => v != 0,
        Value::I64(v) => v != 0,
        Value::Null => false,
        Value::Reference(_) => true,
    }
}
fn arithmetic(left: Value, right: Value, op: &Op) -> Result<Value> {
    let quotient_or_remainder = matches!(op, Op::Divide { .. } | Op::Remainder { .. });
    if quotient_or_remainder && matches!(right, Value::I32(0) | Value::I64(0)) {
        // Check types first: a malformed mixed-width operation is not division.
        require(
            matches!(
                (left, right),
                (Value::I32(_), Value::I32(_)) | (Value::I64(_), Value::I64(_))
            ),
            "arithmetic requires matching integer stack types",
        )?;
        return Err(Error("integer division or remainder by zero".into()));
    }
    // ECMA-335 III.3.55 permits a fault for min % -1. Renraku deliberately
    // faults for both signed operators, independent of the host CPU or Rust
    // overflow settings, matching the CoreCLR differential fixture.
    if matches!(
        op,
        Op::Divide { unsigned: false } | Op::Remainder { unsigned: false }
    ) && matches!(
        (left, right),
        (Value::I32(i32::MIN), Value::I32(-1)) | (Value::I64(i64::MIN), Value::I64(-1))
    ) {
        return Err(Error(
            "signed integer division or remainder overflow".into(),
        ));
    }
    Ok(match (left, right) {
        (Value::I32(a), Value::I32(b)) => Value::I32(match op {
            Op::Add => a.wrapping_add(b),
            Op::Subtract => a.wrapping_sub(b),
            Op::Multiply => a.wrapping_mul(b),
            Op::Divide { unsigned: false } => a / b,
            Op::Divide { unsigned: true } => ((a as u32) / (b as u32)) as i32,
            Op::Remainder { unsigned: false } => a % b,
            Op::Remainder { unsigned: true } => ((a as u32) % (b as u32)) as i32,
            _ => unreachable!(),
        }),
        (Value::I64(a), Value::I64(b)) => Value::I64(match op {
            Op::Add => a.wrapping_add(b),
            Op::Subtract => a.wrapping_sub(b),
            Op::Multiply => a.wrapping_mul(b),
            Op::Divide { unsigned: false } => a / b,
            Op::Divide { unsigned: true } => ((a as u64) / (b as u64)) as i64,
            Op::Remainder { unsigned: false } => a % b,
            Op::Remainder { unsigned: true } => ((a as u64) % (b as u64)) as i64,
            _ => unreachable!(),
        }),
        _ => {
            return Err(Error(
                "arithmetic requires matching integer stack types".into(),
            ));
        }
    })
}
fn compare(
    left: Value,
    right: Value,
    comparison: Comparison,
    unsigned: bool,
    object_null_test: bool,
) -> Result<bool> {
    use std::cmp::Ordering;
    let ordering = match (left, right) {
        (Value::I32(a), Value::I32(b)) => {
            if unsigned {
                (a as u32).cmp(&(b as u32))
            } else {
                a.cmp(&b)
            }
        }
        (Value::I64(a), Value::I64(b)) => {
            if unsigned {
                (a as u64).cmp(&(b as u64))
            } else {
                a.cmp(&b)
            }
        }
        (Value::Null, Value::Null)
            if object_null_test
                || matches!(comparison, Comparison::Equal | Comparison::NotEqual) =>
        {
            Ordering::Equal
        }
        (Value::Reference(a), Value::Reference(b))
            if matches!(comparison, Comparison::Equal | Comparison::NotEqual) =>
        {
            if a == b {
                Ordering::Equal
            } else {
                Ordering::Greater
            }
        }
        (Value::Reference(_), Value::Null)
            if object_null_test
                || matches!(comparison, Comparison::Equal | Comparison::NotEqual) =>
        {
            Ordering::Greater
        }
        (Value::Null, Value::Reference(_))
            if object_null_test
                || matches!(comparison, Comparison::Equal | Comparison::NotEqual) =>
        {
            Ordering::Less
        }
        _ => return Err(Error("comparison requires compatible stack types".into())),
    };
    Ok(match comparison {
        Comparison::Equal => ordering.is_eq(),
        Comparison::NotEqual => !ordering.is_eq(),
        Comparison::Greater => ordering.is_gt(),
        Comparison::GreaterEqual => !ordering.is_lt(),
        Comparison::Less => ordering.is_lt(),
        Comparison::LessEqual => !ordering.is_gt(),
    })
}

#[cfg(test)]
mod tests;
