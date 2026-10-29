//! The Renraku CIL interpreter and persistent execution model.
//!
//! Guest call state lives in inspectable managed frames. Atomic checkpoints
//! retain code, identity bindings, reachable objects, statics, and active frames.
//! See docs/checkpoints.md for recovery and docs/cil-profile.md for compatibility.

pub mod agent;
pub mod artifact;
pub mod checkpoint;
pub mod cil;
pub mod compiler;
pub mod driver;
pub mod effects;
pub mod error;
pub mod execution;
pub mod heap;
pub mod identity;
mod image;
pub mod invocation;
pub mod model;
pub mod program;
pub mod runtime;
pub mod value;
pub mod web;
pub mod workbench;
