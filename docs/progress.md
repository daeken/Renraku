# Renraku progress and next work

As of October 28, 2026, the core process crash-and-resume demonstration, managed
strings, and persistent local model harness work. CIL is the execution foundation for all
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
- CIL `Renraku.Model.Complete` with portable AwaitEffect frames, waiting status,
  and atomic response + result stack + runnable continuation checkpoints.
  New model calls default to disabled on every fresh VM and restore; use
  `set_model_calls_enabled(true)` or trailing CLI `--allow-model-calls`.
- Linked C# conversation with two model calls and four messages. It returns 417
  after 158 guest instructions, matching the CoreCLR echo reference shim.
- Bounded `Vm::drive_model` and `drive-local` CLI coordinate checkpointed execution,
  prepared dispatch, and uncertain receipt lookup. Explicit per-invocation
  instruction and provider-operation budgets include zero budgets. Missing
  receipts stop without retry; unrelated host outbox work is left alone.
- Structured `Model.Chat` accepts mutable linked CoreLib ChatMessage objects and
  a ModelBinding, capturing immutable capability/model names, roles, and content
  references at the call boundary. Limits are 128 messages / one million total
  UTF-16 units, with a final user message.
- Provider model support is checked before external calls. Local echo-v1 receipts
  bind the entire versioned request body. Ledger schema 2 migrates schema 1
  receipts in place, retaining their original bytes and submission counts.
- Payload format 5 with backward reading of formats 1 through 4. Preserve all
  checked-in legacy fixtures; they were captured from the earlier writers.

## Validation

Run `cargo test --workspace`, `cargo fmt --all -- --check`, and
`cargo clippy --workspace --all-targets -- -D warnings`. The full suite includes
CoreCLR comparisons for twenty guest cases, per-instruction checkpoint tests,
actual process kills, format upgrades, exact UTF-16 recovery, and stale writers.
All 86 tests pass. Eighteen effect tests cover intent, dispatch, and response/wakeup
crash windows with live CIL frames, per-instruction harness recovery, missing
receipt uncertainty, failed response rollback, malformed waiting continuations,
capability checks, legacy outbox upgrade, and linked-history retention after GC.
Twelve driver tests cover budget accounting, automatic delivery, first/second
submission and completion SIGKILL recovery, missing receipts, stale writers,
fault persistence, and CLI authorization and recovery. The full CLI harness
also runs end to end with `drive-local`.
Eight structured-model tests cover frozen histories/bindings, malformed chains,
full-body receipts, model rejection, limits, per-instruction and SIGKILL recovery,
and upgrade of a legacy waiting call with its accepted receipt. A golden canonical
request encoding preserves exact UTF-16. ChatHarnessScore returns 11 like CoreCLR.

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

The user selected **OpenAI Responses API through their ChatGPT subscription**,
specifically requiring OAuth. Structured requests and durable model bindings are
complete. The next milestone is host-side OAuth/networking with local HTTP
fixtures, then the subscription-backed Responses adapter. Keep subscription
access as the intended path, rather than requiring an API billing account/key.

Current official docs verified October 28, 2026:

- [Registration and sign-in](https://developers.openai.com/siwc/token-sharing-open-source/sign-in):
  dynamic registration, stable host ID, issued client ID, loopback callback,
  PKCE/state/nonce, verified ID token, and granted plan-usage scopes.
- [Models and inference](https://developers.openai.com/siwc/token-sharing-open-source/models-and-inference):
  account-specific model discovery and public Responses endpoint with OAuth,
  `store: false` and `stream: true`. Only `response.completed` proves success.
- [Official integration cookbook](https://developers.openai.com/cookbook/articles/sign-in-with-chatgpt):
  local/open-source apps can use eligible users' ChatGPT plans.

Implement bounded loopback callback handling, token exchange/refresh, and
host-only protected credential profiles. Keep credentials out of guest objects,
checkpoints, logs, and Git. Use the app's own registration; no sign-in has been
started and no remote inference call has been made.
Bind capabilities to the verified OAuth profile so account switching cannot
silently retarget pending effects.

The subscription flow's `store: false` requirement changes the earlier generic
background-polling idea. Do not assume GET retrieval or remote idempotency can
reconcile interrupted subscription streams. Preserve uncertainty unless a local
durable completion receipt proves the terminal result; no automatic resubmission.
Test partial SSE, missing terminal events, failures, and crashes against a local
server before an interactive sign-in. Preserve exact managed UTF-16; reject a
transport request explicitly if its encoding cannot represent it.
Transport streaming can remain internal to the host adapter; guest code still
receives one completed string, with no native network stack to checkpoint.

No model has been chosen. Discover models through the selected account and
capture the explicit choice. The OpenAI Docs skill applies. The current
`drive_model` preflight is local.echo-only; extend host provider authorization
deliberately when introducing the remote adapter. Host opt-in and limits remain
independent of saved guest state.
Preserve atomic heap/code/frame/effect checkpoints, logical identities, and old
readers as representations expand. See [the current effect contract](effects.md).
