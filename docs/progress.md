# Renraku progress and next work

As of October 28, 2026, the core process crash-and-resume demonstration works
and managed strings are implemented. CIL is the execution foundation for all
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
- Payload format 2 with backward reading of format 1. Preserve the checked-in
  legacy checkpoint fixture; it was captured before the string implementation.

## Validation

Run `cargo test --workspace`, `cargo fmt --all -- --check`, and
`cargo clippy --workspace --all-targets -- -D warnings`. The full suite includes
CoreCLR comparisons for eighteen guest cases, per-instruction checkpoint tests,
actual process kills, format upgrades, exact UTF-16 recovery, and stale writers.

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

Build the first persistent LLM harness around a deterministic local provider.
Conversations can initially be linked managed objects, so general arrays,
generics, and standard collections need not block the effect recovery proof.

Define durable request intent, in-flight and uncertain outcomes, committed
responses, and the waiting guest continuation. Commit intent before dispatch,
then commit a response together with making the computation runnable. Recovery
must not resend completed requests. An in-flight request with no saved response
must require reconciliation or an explicit retry policy.

Use scripted acceptance and response failures to test the crash windows before
selecting or connecting a real paid provider. Keep credentials outside managed
objects. Preserve atomic heap/code/frame checkpoints and existing format readers
as the representation expands. General arrays and broader BCL support remain
independent follow-on work.
