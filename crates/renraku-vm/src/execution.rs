//! Inspectable VM records. The interpreter must validate instruction boundaries,
//! signatures, frame ordering, and stack shapes before executing these records.

use crate::{
    identity::{CodeVersionId, ExecutionId, MethodId, ObjectId},
    value::Value,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MethodVersion {
    pub method: MethodId,
    pub code: CodeVersionId,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CallResult {
    /// Push the callee's result, if its signature has one.
    CalleeReturn,
    /// `newobj` calls a void constructor, then produces the allocated object.
    ConstructedObject(ObjectId),
}

/// CIL offsets are unsigned byte offsets within the pinned method body.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Continuation {
    /// The next instruction has not yet executed.
    Execute { next: u32 },
    /// The call's arguments have moved to the callee; no result is present yet.
    /// Retain the call site for future exception handling, and the next position
    /// for successful return. Returning and pushing a result must be one VM step.
    AwaitReturn {
        call_site: u32,
        next: u32,
        result: CallResult,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Frame {
    pub method: MethodVersion,
    pub continuation: Continuation,
    pub arguments: Vec<Value>,
    /// None is uninitialized, which is different from a guest null reference.
    pub locals: Vec<Option<Value>>,
    /// Bottom to top, with the top value at the end.
    pub evaluation_stack: Vec<Value>,
}

impl Frame {
    pub fn references(&self) -> impl Iterator<Item = ObjectId> + '_ {
        let constructed_object = match self.continuation {
            Continuation::AwaitReturn {
                result: CallResult::ConstructedObject(id),
                ..
            } => Some(id),
            _ => None,
        };
        self.arguments
            .iter()
            .chain(self.locals.iter().filter_map(Option::as_ref))
            .chain(self.evaluation_stack.iter())
            .filter_map(Value::referenced_object)
            .chain(constructed_object)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ExecutionStatus {
    Runnable,
    Paused,
    Completed {
        result: Option<Value>,
    },
    /// A VM diagnostic, not a guest exception or resumable exception state.
    Faulted {
        message: String,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExecutionContext {
    pub id: ExecutionId,
    pub status: ExecutionStatus,
    /// Oldest caller first; the final frame is the active callee.
    pub frames: Vec<Frame>,
}

impl ExecutionContext {
    /// Completed contexts retain their result until the context is released.
    pub fn references(&self) -> impl Iterator<Item = ObjectId> + '_ {
        let result = match &self.status {
            ExecutionStatus::Completed { result } => result.as_ref(),
            _ => None,
        };
        self.frames
            .iter()
            .flat_map(Frame::references)
            .chain(result.and_then(Value::referenced_object))
    }
}
