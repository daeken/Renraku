# Renraku progress and next work

As of October 28, 2026, the core process crash-and-resume demonstration, managed
strings, and host model-request outbox work. CIL is the execution foundation for all
persistent guest computation. External programs will require explicit capabilities
and do not inherit persistence.

## Implemented

- Rust CIL loader and interpreter, custom Roslyn CoreLib, explicit portable frames,
  object identity, fields, inheritance, loops, nested calls, and static storage.
- Atomic SQLite checkpoints, code and identity binding recovery, schema and stack
  validation, optimistic generation checks, named roots, and tracing collection.
- Process kill tests with a cyclic object graph and nested frames. The durable
  simulation returns 5526 and the first demo returns 48 in 155 instructions.
- UTF-16 literals, immutable string objects, Char slots, length, indexing,
  concatenation, equality, and persistent literal interning. String intrinsics
  are whitelisted InternalCall methods and are atomic guest steps.
- Host effect outbox with stable IDs, prepared/in-flight/uncertain/completed
  states, GC retention, and atomic intent/dispatch and response checkpoints.
- Deterministic local echo provider with a separate durable receipt ledger.
  Reconciliation never falls back to submission; completed work is not resent.
- Queue, dispatch, reconcile, and inspect CLI commands. A CIL method consumes a
  stored response as an ordinary managed string in the integration suite.
- Payload format 3 with backward reading of formats 1 and 2. Preserve both
  checked-in legacy fixtures; they were captured from the earlier writers.

## Validation

Run `cargo test --workspace`, `cargo fmt --all -- --check`, and
`cargo clippy --workspace --all-targets -- -D warnings`. The full suite includes
CoreCLR comparisons for eighteen guest cases, per-instruction checkpoint tests,
actual process kills, format upgrades, exact UTF-16 recovery, and stale writers.
All 56 tests pass. Nine effect tests include SIGKILL before and after provider
acceptance, missing receipt uncertainty, failed response commits, capability
checks, malformed outbox recovery, and idempotency-key consistency.

On this host the shell may require `/Users/daeken/.cargo/bin/cargo` explicitly.
The .NET 10 SDK is needed for compiler fixtures. Restore itself does not need it.
Preserve unrelated working tree changes when continuing.

## Continuation and publishing

The current chat has an active follow-up every five minutes during initial
development. Each productive run should finish a coherent, verified change,
commit it, and push to origin. Push completed milestones frequently instead of
accumulating local changes. Check the current branch and remote state, do not
force-push, and preserve local commits if the remote is temporarily unavailable.

## Next implementation target

Connect the host outbox to a portable waiting CIL continuation to build the first
persistent LLM harness. See [the current effect contract](effects.md).
Conversations can initially be linked managed objects, so general arrays,
generics, and standard collections need not block the effect recovery proof.

Intent, dispatch, uncertain outcomes, managed responses, and local receipt
lookup are implemented. Guest suspension and wakeup are not. Define a
guest-visible capability call that records request intent and a waiting
continuation together, then commit a response with making that continuation
runnable. Extend recovery validation to check this relationship and the call's
saved operands and result type. Keep every waiting frame portable.

The next demonstration should run a linked conversation through multiple local
requests and crash with a live waiting CIL frame. Cover termination before intent
commit, after response commit but before guest execution, and repeated recovery.
Continue using the deterministic local provider before selecting a real paid
provider. Keep credentials outside managed objects. Preserve atomic heap/code/
frame/effect checkpoints and existing format readers as the representation
expands. General arrays and broader BCL support remain independent follow-on work.
