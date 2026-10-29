# Renraku progress and next work

As of October 29, 2026, the core process crash-and-resume demonstration, managed
strings, persistent model harness, subscription Responses adapter, and bounded
coding-agent loop work. The loop has deterministic acceptance/recovery coverage
and live GPT-6.1 Sol acceptance across process restarts: model-selected discovery,
inspection, generated C#, compilation, installation, and one invocation reach 26.
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
- Payload format 7 with backward reading of formats 1 through 6. Preserve all
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
- Runtime workbench: bounded JSON symbol search, exact saved CIL inspection by
  logical method/code ID, frozen compiler views with schema/core hashes and
  symbol-to-ID bindings, and external Roslyn metadata reference generation.
  The first export subset preserves primitive signedness, named parameters,
  visibility, and readonly fields; unsupported declaration shapes fail explicitly.
- Append-only `Program::link_modules` and atomic `Vm::install_program`, preserving
  existing heap/type/field/method/code identities, paused frames, and effects.
  Native CLI search/read-code/export-reference/install/invoke tools work against
  a saved universe. Compiler reference images are explicitly rejected as code.
- Deterministic code-generation acceptance fixture: a CIL model call receives
  search/CIL output, returns source, and its saved response compiles against a
  runtime view. Installed code mutates the same Counter 19 -> 26 -> 33 across
  native CLI processes after deleting the build bundle. This tests the pipeline,
  not autonomous model tool selection or live inference. See [workbench](workbench.md).
- Durable `Vm::invoke_once` / CLI `invoke-once` binds a request ID to a pinned
  method, exact arguments, and one execution. Intent commits before instructions;
  completion/fault receipts commit with heap and frames. Retry resumes or returns
  the saved result without a second mutation. Read receipts with `invocation`.
  Receipt arguments/object results remain GC roots across unrelated executions.
  Requests are generation-fenced and capped at 10,000, with no automatic eviction.
- Captured `BuildArtifact` inputs validate manifest formats 1/2, compiler options,
  all file hashes/lengths, source encoding, exact runtime core, marked reference
  images, and live logical-ID views. Bundle paths/symlinks are restricted, and
  captured files are bounded to 64 MiB. Artifact IDs hash exact captured contents.
- `install-once` / `artifact::install_once` commits new code, captured build
  provenance, and a stable request receipt atomically. Replays preserve all IDs
  and do not advance the generation. Query `installation` without the bundle.
  Invocation/installation IDs cannot collide. Paused frames, receipts, effects,
  and newer concurrent heap state remain coherent. Limits are 128 installations
  and the existing global 128 MiB checkpoint size, without automatic eviction.
- `Vm::read_code` and native `read-code` now include original UTF-8 sources,
  hashes, artifact ID, and build manifest for captured modules after deleting
  the external bundle. CIL stays authoritative; source scope is explicitly the
  entire assembly, without fabricated per-method source locations or attestation.
- Durable host compiler service with universe-scoped request IDs, exact captured
  core/source/view bytes, C# options, and pinned compiler/package/host/runtime
  profiles. Preparation commits before guest compilation; terminal artifacts or
  diagnostics commit atomically in a separate WAL/FULL service ledger. Changed
  inputs/toolchains reject under an existing ID; published jobs never rerun.
  Prepared pure compilation may be explicitly resumed after failure or SIGKILL.
  The universe checkpoint stays format 7. See [compiler service](compiler-service.md).
- CLI `compile-once`, `compile-prepare`, `compile-resume`, `compilation`, and
  `install-compilation`. Captured inputs work after deleting source/view files;
  terminal recovery/install need neither the SDK nor compiler files. Installation
  then retains all required build bytes in the universe without its service ledger.
- Bounded model-selected search/read-code/describe/compile/install/invoke/finish
  loop using strict JSON final-text actions. Linked CIL AgentSession, AgentTurn,
  ChatMessage, and AgentArgument objects retain exact actions, frozen inputs,
  results, and object roots. Partial journal updates resume their portable frames.
  Stable universe/turn request IDs reconcile compiler/install/invoke receipts
  before appending a lost result. Frozen static selectors cannot retarget on replay.
- Disposable tool workers keep Roslyn off the async executor. Explicit per-run
  budgets and session turn/transcript/source/output caps bound the loop. Generated
  invocations have no model authority. Unknown actions/fields and model-supplied
  host paths reject explicitly. Checkpoint format remains 7.
- Foreground subscription CLI `agent` and `agent-drive`, plus credential-free
  `agent-inspect`. The scripted sequence changes Counter 19 -> 26 in six assistant
  turns, including discovery and exact code inspection. This fixture demonstrates
  orchestration and persistence, not autonomous intelligence. No live inference
  ran in this milestone. See the [coding-agent guide](agent.md).
- Subscription-adapter coding-loop HTTP acceptance fixture chooses actions from
  actual wire history, recovers an intentional C# type error using saved CS1503
  diagnostics, installs/invokes the corrected caller, reads its retained source,
  and finishes in nine assistant turns. Each HTTP history matches the managed
  transcript prefix. Lost compile/install/invoke results, changed UI account
  selection, SDK-free published replay, signed-out offline receipt recovery, and
  whole-controller SIGKILL retain their original action/execution identities.
  Provisional delta text and native function calls cannot become agent actions.
  Production wire/protocol/checkpoint/ledger behavior is unchanged; no live calls.
- Live coding-agent acceptance with user-authorized subscription inference:
  GPT-6.1 Sol chose ten actions, compiled its own caller successfully on the first
  attempt, installed it, advanced the existing Counter 19 -> 26 once, and inspected
  its saved CIL/source. Provider budgets stopped after turns 3/6/9; fresh processes
  resumed the same universe. Finished recovery used zero instructions/operations;
  invoke-once replay retained its execution ID and returned 26 again. See the
  [live report](acceptance/live-agent-2026-10-29.md). Corrected the agent guide's
  receipt path to a private directory outside Git after a pre-inference rejection.
  The user explicitly permits bounded live inference in future development.
- Bounded `drive_with_progress` events project saved managed actions/results,
  with stable universe/turn/event IDs and observed checkpoint generations. Partial
  helper frames finish before result publication; waiting model calls can show
  the previous saved turn. Events may replay after restart and never own actions.
- Foreground agent progress/error summaries go to stderr; stdout remains one
  format-1 JSON report with additive recovery guidance. Budget stops print quoted
  resume commands preserving paths/profile/model; lifetime caps require inspection,
  and uncertain inference permits receipt reconciliation only. Source is omitted
  from progress, summaries cap at 512 UTF-8 bytes, and controls are escaped.
  Finished live-session replay verified this output with zero operations.
- Durable `agent-input` / `agent::submit_input` accepts a follow-up after a finished
  goal. A pinned private CIL helper appends linked AgentInput/ChatMessage objects;
  invoke-once commits intent and exact arguments before any helper instruction.
  Replays resume or return the original receipt; conflicting content rejects.
  The driver owns pending append frames, counts stay monotonic, and all lifetime
  limits remain in effect. Input adds no native snapshot fields or OAuth calls.
- Agent transcript validation merges input nodes at completed finish boundaries
  and verifies their exact messages/receipts. Inspection exposes input IDs/content,
  `afterTurn`, and followupSupported. Older agent schemas remain readable and
  resumable while explicitly rejecting follow-up; a pre-change format-7 agent
  fixture preserves that coverage alongside the unchanged format 1–6 fixtures.
- Live two-goal acceptance: GPT-6.1 Sol compiled a reusable caller, advanced the
  saved Counter 19 -> 26, then handled a durable follow-up in a fresh process by
  reusing the same caller with -3 to reach 23. The second goal used two model
  turns and no compiler/installation. Repeated pending and completed input IDs
  preserved one append/execution. See the [live follow-up report](acceptance/live-followup-2026-10-29.md).

## Validation

Run `cargo test --workspace`, `cargo fmt --all -- --check`, and
`cargo clippy --workspace --all-targets -- -D warnings`. The full suite includes
CoreCLR comparisons for twenty guest cases, per-instruction checkpoint tests,
actual process kills, format upgrades, exact UTF-16 recovery, and stale writers.
All 182 tests pass. Eighteen effect tests cover intent, dispatch, and response/wakeup
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
Four ignored subprocess probes are launched by their parent crash tests. The
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
Five further compiler/workbench tests verify identity-preserving installation
with live frames, the scripted model/source/compiler/CLI acceptance path,
rollback and stale installation rejection, exact code selection/search bounds,
and compiler-visible metadata fidelity. Unsigned/named/readonly contracts work;
properties, attributes, constants, and default parameters reject explicitly.
Nine invocation tests cover per-instruction replay, cached completion/fault
receipts, retained object results/arguments after GC and unrelated calls,
conflicting IDs, stale callers, failed intent/completion saves, model waits and
driver completion, checksum-repaired corruption, and actual SIGKILL around commit.
An original format 5 structured-chat snapshot, captured before the writer change,
upgrades with an empty invocation registry and finishes with result 11.
The compiler/CLI acceptance fixture now retries the same generated call and
receives 26 again with the same execution ID, then uses a new ID to reach 33.
Seven additional artifact/installation tests cover manifest/content validation,
traversal/symlink and encoding rejection, exact original source retention,
receipt replay and conflicting IDs, paused invocation preservation, failed writes
and lost post-commit replies, SIGKILL at commit boundaries, checksum-repaired
source/receipt corruption, wrong-core/CIL rejection, cross-universe binding
rejection, and concurrent-writer fencing that preserves the newer heap.
A genuine format 6 paused invocation, captured before the writer change, upgrades
with an empty installation registry and resumes/replays to 4 instead of 8.
Seven compiler-service tests cover CLI preparation/resume/install after source
deletion, SDK-free terminal recovery, retained syntax/input diagnostics, immutable
inputs and compiler identity, failed preparation/publication, lost post-commit
replies, foreign/corrupt ledgers, successful compilation with unsupported CIL,
and actual SIGKILL at prepared, child-started, captured, written, and committed
boundaries. Terminal replay is tested with an observer that fails if invoked.
Fifteen coding-agent tests cover full action selection from actual saved request
history, lost results after all tools, SDK-free terminal replay, frozen static
arguments, pre-invocation GC roots, uncertain inference without resubmission,
strict inputs and all budgets, generated-code capability denial, compiler worker
yielding to another universe, and partial journal recovery with seven-instruction
budgets. A foreground CLI test rejects missing opt-in and oversized budgets before
touching universe/auth state. Existing field-name test lookups now include owners,
so valid same-named fields in different managed classes cannot make tests flaky.
Five further agent tests exercise the actual subscription adapter with local
HTTP: the nine-turn correction/installation/source-inspection sequence, interrupted
and unsupported native-call streams without resubmission, signed-out receipt
recovery into a single action/mutation, and actual controller SIGKILL after the
invocation commit but before its result append. Recovery preserves the exact
invocation receipt and finishes with only the two remaining model POSTs. Fixtures
select actions from received histories and do not prove real-model autonomy.
Progress tests reopen the checkpoint from callbacks to verify result visibility
after commit, including seven-instruction partial helper updates. Losing console
output after the invocation result commits replays its event ID and original
receipt without repeating the mutation. The HTTP correction/recovery fixture
projects all 18 saved action/result events with zero-operation replay, including
CS1503 diagnostics. Two console unit tests cover terminal-vs-budget/uncertain
guidance and shell quoting of spaces, quotes, substitutions, and newlines. Another
CLI integration test keeps progress on stderr and stdout parseable JSON, verifies
bounded/control-escaped signed-out finished recovery without a new checkpoint,
and retains error diagnostics plus frame-inspection guidance on preflight failure.
Follow-up coverage restores every CIL append instruction with GC between saves,
resumes pending input through the ordinary driver, retries native CLI requests,
rejects conflicting/invalid/early/over-limit input without changing the checkpoint,
and completes the original pre-input-schema agent under a local provider. A second
goal reuses installed source/code without an SDK and preserves the first mutation
receipt. The HTTP fixture sends both complete histories plus the new user message
and reaches Counter 33 using only three new model requests and one installation.
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
fallback. The user's latest instruction explicitly authorizes bounded live
subscription inference, including recurring development. Use local fixtures for
routine regression, and real calls when they advance model acceptance. Preserve
transcripts/receipts and uncertainty; do not repeat a completed live demo without
a new reason. Use the stored sign-in and surface any required reauthentication.

The latest user direction includes website hosting and runtime compilation, with
Roslyn explicitly allowed to remain external and return a DLL. The reusable host
compiler seam, durable compilation service, append-only installation, and coding
loop are implemented. A guest Compile intrinsic and replacement bodies on
existing types remain future work. Keep
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
is recorded, and the first exporter/reference compiler/append-only installer now
work. Make the runtime schema authoritative; export
deterministic per-logical-module views and pin their hashes plus symbol-to-ID
bindings. Separate metadata/API revisions from heap generations and method-body
versions. Facade tokens must never allocate replacement runtime type identities.
Compiling a member body on an existing type needs explicit owning-type context
and generated-definition-to-ID mappings; ordinary partial classes cannot extend
a referenced compiled type. Compiler inputs now accept up to 16 frozen reference
views. Today schema is still derived from retained implementation metadata;
independent durable schema authoring and broader reference shapes are future work.

**The next acceptance target, selected by the user, is an LLM discovering APIs,
reading existing code, writing C#, pulling it in, and executing it.** The
[workbench](workbench.md) supplies the host tools, and the [coding agent](agent.md)
now connects model-selected search -> read code -> compile -> install -> invoke
-> finish. Managed linked objects retain the actual assistant/tool transcript,
canonical input, and typed reference roots. Compiler/install/invoke IDs derive
from saved turns; lost results reconcile before continuing. Keep these objects
authoritative and preserve partial helper-frame recovery. The strict text/JSON
prototype reuses final-text transport; native Responses function calls still need
an explicit wire/result representation and receipt compatibility work.

The subscription-adapter loop now has local HTTP coverage for full-history JSON
actions, corrective compiler diagnostics, uncertain streams, durable receipts,
and a whole-controller crash. A live GPT-6.1 Sol run now completes the user's
acceptance target in ten model-selected turns, including caller source inspection
and zero-operation recovery. Distinguish this evidence from the deterministic
fixtures, which do not prove real-model autonomy. The user authorizes bounded
live inference; use it when it resolves a meaningful acceptance question.

Foreground feedback and recovery guidance now work, backed by the managed journal
and tested error/correction/restart paths. Keep machine-readable inspection/report
output and stable replayable progress IDs; do not confuse presentation events with
execution ownership or an exactly-once event-delivery protocol.

Follow-up input now works across goals in one universe, with durable IDs, linked
managed history, monotonic turns/receipts, local recovery tests, and a live reuse
demonstration. Keep the current limits and explicit legacy-schema boundary.

**Next implement the first persistent localhost website milestone.** Use a host
HTTP edge with persistent CIL handlers/state and a durable request inbox. Start
with bounded parsing and one accepted request at a time, with an explicit client
request ID bound to exact request data and a pinned handler. Save accepted work
before executing; commit response and state before writing to the socket. A
recovered retry must resume or return its receipt without repeating mutations.
Demonstrate retained page/counter state and request deduplication across process
recovery using local HTTP fixtures. Keep sockets outside the universe and preserve
the one-context limit. The existing invocation/input protocols provide useful
patterns; do not treat raw implementation DLLs or native futures as guest state.
The compiler service ledger is outside the guest and must not replace managed
conversation/action state. It serializes one bounded worker per ledger and may
rerun explicitly resumed prepared pure jobs; inference uncertainty remains a
separate receipt-only policy. Preserve terminal results and matching toolchain
requirements. The service captures metadata views before execution and emitted
reference images in the result; it does not capture a whole portable CLR or add
a guest Compile intrinsic.
Current `read-code` returns authoritative saved CIL with resolved logical targets;
captured installations also return exact original source inputs and manifests,
even after deleting their bundles. Older/raw loads explicitly have no attached
source. Method-level PDB/source maps and decompilation remain future work.

The input/drive/inspect CLI now supports a persistent conversation across finished
goals. A general interactive UI and input cancellation/editing remain future work.
Keep conversation work in CIL and treat host input as an explicit durable boundary.
Avoid persisting native socket state or making a host vector authoritative.
The coding agent already supplies one bounded tool policy; broader remote tools
need explicit operation identities and uncertainty rules of their own.

Current official subscription HTTP requirements rechecked October 29, 2026:

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
Keep both backends and all original format 1–6 fixtures intact.

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
