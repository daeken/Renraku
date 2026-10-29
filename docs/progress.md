# Renraku progress and next work

As of October 28, 2026, the core process crash-and-resume demonstration, managed
strings, persistent local model harness, and subscription Responses adapter work.
CIL is the execution foundation for all
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
- Separate host-only `renraku-openai` crate and CLI: subscription OAuth login,
  profile metadata and selection, refresh, and logout. Uses production discovery,
  bounded loopback callbacks, PKCE, state/nonce and JWT validation, and explicit
  granted plan permission. Credentials never enter guest state.
- Private Unix credential files outside Git, atomic commits and process locks.
  Pending refresh markers prevent reuse of rotating tokens after uncertain
  outcomes or SIGKILL. Logout clears local credentials before remote revocation.
- Profile-pinned subscription model discovery and streamed Responses text
  inference. Explicit model choice, full histories, developer-role mapping,
  transport UTF-16 preflight, scope checks, and expiry/refresh of the exact profile.
- Private SQLite completion ledger, with one durable reservation before each
  POST and a terminal receipt before VM response/wakeup commit. Reconciliation
  is offline; partial streams and missing receipts stay uncertain without retry.
- Generalized bounded host driver accepts an explicitly supplied provider and
  still checks each saved capability/model. A non-inference validation hook
  rejects unsupported requests before marking InFlight. Public canonical request
  bytes let separate host crates bind receipts without changing checkpoint format.
- `renraku-openai models`, `demo`, and `drive` CLI commands. OpenAIHarness builds
  ordinary linked CIL objects, receives a completed string, and retains its
  prompt/reply. Real inference requires explicit host opt-in; see the
  [try-it guide](openai-responses.md).
- Live subscription inference verified after user-completed login. GPT-6 Luna
  returned `Persistent CIL is alive.` through 49 CIL instructions and one
  provider operation. A fresh process reopened the saved result with zero
  instructions and provider operations. Headerless SSE is accepted through the
  same bounded terminal parser; explicitly different transport types fail.
- Reusable external Roslyn compiler in `tools/Compiler`: arbitrary UTF-8 source
  files compile against an exact supplied Renraku core, without inheriting the
  host BCL. New output bundles capture source/reference/DLL bytes, Roslyn and
  driver identities/hashes, deterministic options, and structured diagnostics.
  The compiler never executes the emitted guest code. See
  [compilation and installation boundaries](compilation.md).
- Executor-independent asynchronous provider/dispatch/reconciliation/driver APIs.
  Blocking and async paths share checkpoint transitions and driver accounting.
  Independent universes progress on one executor thread while delivery waits;
  async guest quanta yield after at most 1024 instructions. Dropped delivery
  futures recover via receipts, without automatic resubmission.
- AsyncOpenAIProvider uses asynchronous HTTP/SSE and offloads profile/refresh and
  receipt I/O to the host blocking pool. OpenAI demo/drive now use it by default
  on a current-thread executor with at most four blocking workers. Request,
  catalog, transport-diagnostic, SSE, and receipt policies are shared with the
  blocking compatibility adapter. Checkpoint format and ledger schema are unchanged.

## Validation

Run `cargo test --workspace`, `cargo fmt --all -- --check`, and
`cargo clippy --workspace --all-targets -- -D warnings`. The full suite includes
CoreCLR comparisons for twenty guest cases, per-instruction checkpoint tests,
actual process kills, format upgrades, exact UTF-16 recovery, and stale writers.
All 129 tests pass. Eighteen effect tests cover intent, dispatch, and response/wakeup
crash windows with live CIL frames, per-instruction harness recovery, missing
receipt uncertainty, failed response rollback, malformed waiting continuations,
capability checks, legacy outbox upgrade, and linked-history retention after GC.
Thirteen driver tests cover budget accounting, automatic delivery, first/second
submission and completion SIGKILL recovery, missing receipts, stale writers,
fault persistence, and CLI authorization and recovery. The full CLI harness
also runs end to end with `drive-local`.
Six asynchronous-driver tests additionally verify independent-universe progress,
CPU yielding, dropped futures before/after acceptance and during preflight,
receipt wakeup of original frames, precise bounded accounting across restores,
and stale-writer fencing. See [asynchronous host delivery](async-delivery.md).
Eight structured-model tests cover frozen histories/bindings, malformed chains,
full-body receipts, model rejection, limits, per-instruction and SIGKILL recovery,
and upgrade of a legacy waiting call with its accepted receipt. A golden canonical
request encoding preserves exact UTF-16. ChatHarnessScore returns 11 like CoreCLR.
Sixteen auth tests cover protocol validation, invalid identities, scopes,
registration reuse, profiles, redaction, storage, sign-out, and actual refresh
SIGKILL recovery, and refreshing a profile independently of active selection.
Fourteen inference tests cover SSE framing/fragmentation, terminal validation and
bounds, full wire requests, fixed profiles, offline and mismatched receipts,
preflight rejections, uncertain errors, receipt commit failure, concurrent
reservations, foreign ledgers, live CIL delivery, and actual SIGKILL before and
after completion receipt commit, missing MIME headers, and non-SSE diagnostics.
A CLI integration test verifies opt-in,
durable preflight rejection, zero budgets, and credential absence from snapshots.
Three ignored subprocess probes are launched by their parent crash tests. The
Responses parent runs both adapters at partial-stream and post-receipt/pre-VM
commit SIGKILL boundaries. Async HTTP tests also prove independent guest progress
on one executor, abandoned-future uncertainty, profile pinning, offline receipt
interoperability, changed-request rejection, and shared preflight/error handling.
Five compiler tests verify deterministic multi-file output and exact manifests,
structured diagnostics, no implicit host framework references, preserved outputs,
input limits, and explicit loader rejection of unsupported emitted CIL. A nested
frame checkpoint resumes after deleting its source/bundle, retains static state
on a second execution, and reopens with the native CLI under an empty PATH.
The external Hello example returned 36 in 11 instructions; its CLI checkpoint
paused at instruction 10 and resumed to the same result.
Production discovery and CLI status were smoke-tested in the auth milestone.
The user completed interactive login. Plan permission, account model discovery,
live inference, and a zero-operation restore have now been verified. Initial
header-rejection probes remain uncertain and were not resubmitted. Read
[the auth guide](openai-auth.md) and [Responses guide](openai-responses.md).

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
specifically requiring OAuth. Sign-in, discovery, a live one-prompt CIL demo,
and zero-operation restore are verified. GPT-6 Luna was selected explicitly for
the smoke test, not made a default; continue selecting models from account
discovery. Keep subscription access as the intended path, with no API-key billing
fallback. Do not start login or more real inference in an unattended heartbeat.
Existing local fixtures allow independent progress.

The latest user direction includes website hosting and runtime compilation, with
Roslyn explicitly allowed to remain external and return a DLL. The reusable host
compiler seam is implemented; it does not add a guest Compile intrinsic, durable
compiler requests, or installation into an already running Program. Those need
separate versioned code-registry transactions and active-frame retention. Keep
the original architecture's self-hosting/non-arbitrary-.NET limits; do not turn
running Roslyn inside Renraku into a prerequisite for runtime compilation.

For website work, use a host HTTP edge with managed handlers/state and a durable
request inbox, bounded parsing, and local binding first. Persist accepted work
before executing it and response/state before sending. A crashed socket is not
recoverable, so define client request IDs/retry deduplication explicitly. The
first proof should retain page state and avoid duplicate mutations across
request recovery. HTTP guest APIs are not implemented yet; see the
[workload direction](compilation.md#website-workload).

The user raised async-by-default as an early replacement-BCL direction.
[Suspension by default](suspension.md) now records the proposed contract and
implementation order: ordinary CIL calls can park logical contexts, explicit
managed futures/scopes express concurrency, suspension points remain observable,
and no-suspend is separate from transaction isolation. Cancellation never proves
remote non-acceptance. This is design direction, not newly implemented scheduling,
Task<T>, C# async/await, or cancellation support.

Asynchronous host delivery is now implemented and is the OpenAI CLI default.
Keep native futures out of snapshots and retain the blocking API for existing
embedders. Auth/receipt operations use blocking workers; universe checkpoint I/O
still blocks a bounded poll. Each VM still has only one context. Multi-context
scheduling must atomically checkpoint all contexts, wait relationships, shared
heap, and code together, with readers for old snapshots. Do not confuse progress
of independent universes with concurrency inside one universe.

The latest user clarification is runtime-generated metadata reference DLLs for
Roslyn when live classes are not defined by conventional implementation DLLs.
The [reference-assembly contract](compilation.md#reference-assemblies-for-live-runtime-types)
is recorded. A local /refonly counter facade successfully compiled a caller
using its field/method against only our core, but no live-registry exporter or
ID-aware installer exists yet. Make the runtime schema authoritative; export
deterministic per-logical-module views and pin their hashes plus symbol-to-ID
bindings. Separate metadata/API revisions from heap generations and method-body
versions. Facade tokens must never allocate replacement runtime type identities.
Compiling a member body on an existing type needs explicit owning-type context
and generated-definition-to-ID mappings; ordinary partial classes cannot extend
a referenced compiled type. Current compiler inputs remain one core plus source
files; additional reference inputs and durable schema authoring are next work.

The next useful milestone is a persistent interactive conversation CLI: append
ordinary linked guest messages, submit the complete captured history, and resume
the same universe between turns. Keep conversation work in CIL and treat host
input as an explicit boundary. Test restart/GC between turns and multiple calls
with changing account UI selection. Avoid persisting native socket state or
making the host's own Rust conversation vector the guest's source of truth.
Tool calls need a separate effect/recovery policy before model tools are enabled.

Current official docs verified October 28, 2026:

- [Registration and sign-in](https://developers.openai.com/siwc/token-sharing-open-source/sign-in):
  dynamic registration, stable host ID, issued client ID, loopback callback,
  PKCE/state/nonce, verified ID token, and granted plan-usage scopes.
- [Models and inference](https://developers.openai.com/siwc/token-sharing-open-source/models-and-inference):
  account-specific model discovery and public Responses endpoint with OAuth,
  `store: false` and `stream: true`. Only `response.completed` proves success.
- [Preview limitations](https://developers.openai.com/siwc/token-sharing-open-source/preview-limitations):
  full input history, developer messages, no background/retrieval assumptions,
  and omitted unsupported request fields.
- [Official integration cookbook](https://developers.openai.com/cookbook/articles/sign-in-with-chatgpt):
  local/open-source apps can use eligible users' ChatGPT plans.

Implementation is in `crates/renraku-openai`, including `inference.rs`,
`receipts.rs`, and `stream.rs`. Production endpoints are fixed; HTTP test
overrides are private to unit tests. Profile-bound capabilities are
`openai.chatgpt.<profile-uuid>`. A ledger also binds issuer, subject, issued
client, model, and canonical full request bytes. The new receipt ledger has
application ID 0x52454f41 / schema 1; the old local.echo ledger remains schema 2.
Keep both backends and all original format 1–4 fixtures intact.

Public metadata contains no tokens. Preserve the pending-refresh marker,
private Unix storage, profile registration/identity mapping, and lock invariants.
Windows/keychain storage remains future work. Signed-out metadata plus a local
receipt can reconcile offline; a removed profile cannot be guessed from a
different active account. Only completed final text is returned; refusals,
tool calls, and unsupported output fail explicitly. Lone surrogates remain valid
managed strings but are rejected by this text transport before dispatch.

Do not assume GET retrieval or remote idempotency can reconcile interrupted
subscription streams. Preserve uncertainty without a durable completion receipt.
There is no automatic resubmission, cancellation, or manual resolution command.
Model/refusal failure diagnostics are conservative and may still represent usage.
Host opt-in and per-invocation budgets remain independent of saved guest state.
The OpenAI Docs skill applies to further integration changes.
Preserve atomic heap/code/frame/effect checkpoints, logical identities, and old
readers as representations expand. See [the current effect contract](effects.md).
