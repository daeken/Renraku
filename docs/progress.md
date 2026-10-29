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
- Persistent localhost website with `WebHost`, `WebServer`, and `web-serve` CLI.
  Static four-string CIL handlers own routing/state/immutable HTML responses;
  host sockets and a bounded HTTP profile stay outside snapshots. Invocation
  receipts form a durable inbox with pinned code and exact method/target/body.
  Intent commits before instructions; terminal response/state commits before
  socket writes. Conflicting IDs reject, pending calls exclude new executions,
  and older completed responses remain immutable across later requests and GC.
- A browser bootstrap retains one pending request ID in sessionStorage and uses
  the CIL visit demo. Manual browser verification recorded a visit, restarted the
  host on the same port, and reloaded the same visit ID. The current demo is at
  http://127.0.0.1:56903/ with database `target/web-demo-20261029.db`; its host may
  need manual restart after this session. See [website guide](website.md).
  Checkpoint format remains 7; handlers receive no implicit model capability.
- Durable default-handler publication with `web-publish`, `web-inspect`, and
  `web-serve ... @published`. New request IDs reload the saved binding; older
  accepted requests keep their pinned method/code and response. Separate compile,
  install, and publication IDs preserve the external compiler boundary. Immutable
  publication receipts and the current alias are managed strings/named roots,
  committed with the same heap/code/frames. Old publication retries return their
  original receipt without overwriting a newer selection. Publication runs no CIL
  and may commit while an older request is paused. Reserved journal validation,
  optimistic fencing, a 128-publication cap, and no eviction are explicit.
  `guest/examples/WebsiteV2.cs` compiles against a frozen Website view and calls
  its original handler against the same persistent visit state. Format 7 and
  HTTP-v1 wire/receipt semantics are unchanged. See [publication guide](website.md#compile-install-and-publish-a-handler).
- Coding-agent `web_status` and `publish_web` actions, with exact method/code
  frozen in the managed turn. Inspection exposes the bounded current publication,
  handler ABI, limits, and current host permission. Publication authority defaults
  off and requires `AgentHost::allow_web_publication` or CLI
  `--allow-web-publication`, independently of model opt-in. Resume commands preserve
  that explicit policy. IDs `agent.<universe>.<turn>.publish` reconcile committed
  receipts read-only even after authority is removed or a newer route is published.
  An uncommitted action without authority returns a saved tool error. Unknown
  fields, unsupported ABI/code bindings, and host paths reject. No guest/native
  schema or HTTP wire changes were required.
- Live website acceptance: GPT-6.1 Sol chose discovery/inspection, wrote and
  published a handler, then inspected its retained source and authored/published a
  second version after a real HTTP visit. Across five fresh foreground processes,
  15 model turns and terminal receipts retained both visit IDs and the original
  old-request response/execution. Finished recovery without publication permission
  used zero operations. A separate browser demo is at http://127.0.0.1:59177/
  against `target/live-website-20261029-01/universe.db`; preserve its state and
  the original website demo. See [live report](acceptance/live-website-2026-10-29.md).
- Bounded `preview_web` agent action, restricted to the current universe's saved
  handler with no URLs/sockets/network authority. Freeze records exact HTTP data
  and selected publication in the managed turn; before-acceptance route changes
  reject, while accepted calls keep their original code. Turn-derived HTTP-v1
  inbox IDs own string arguments, frames, full response, and execution through
  GC/recovery. Pauses resume under the same guest/tool budgets, with no handler
  model authority. Results project status/body/execution and explicit truncation
  at 8,192 JSON-escaped body bytes, under the 16 KiB tool limit. The public Rust
  `preview_handler` shares the socket host's core inbox and clears transient
  model permission on its supplied VM. No snapshot or wire fields changed.
- Live preview/correction acceptance: GPT-6.1 Sol recorded and observed a visit,
  wrote/compiled/installed/published a handler with the requested heading, and
  previewed the corrected page while retaining that visit. Ten turns and terminal
  model receipts span seven-instruction/fresh-process recovery of the first
  paused preview and subsequent model-budget resumes. Independent TCP GET matched
  the corrected preview; POST replay of its original request retained the original
  baseline response/execution after publication. Finished recovery used zero
  operations. See [live report](acceptance/live-preview-2026-10-29.md) and
  `target/live-preview-20261029-01/`; both prior demo universes remain unchanged.
- Read-only `inspect_object`/`inspect_static` agent actions and native
  `inspect-object`/`inspect-static` CLI commands. Bounded views show logical
  object/type/field identities, declaring owners, runtime storage slots and exact
  scalar bits/reference IDs, with source checkpoint generations. Object Freeze
  passes the target into the existing typed managed roots before host execution.
  Reads execute no guest code and perform no heap/checkpoint writes. Limits are
  1..32 fields, 0..256 UTF-16 preview units, 128-byte label prefixes, and 12 KiB
  JSON per view, with explicit truncation. Lone surrogates remain exact units and
  produce null decoded preview text. Lost unappended reads may reobserve a newer
  generation; historical result IDs are not unlimited retention promises. Source
  signedness/enum formatting stays separate from the runtime's raw I32/I64 slots.
  Format 7 and all legacy bindings remain unchanged. See [inspection](inspection.md).
- Live state inspection acceptance: GPT-6.1 Sol observed stored Counter.Value 19,
  found/read the existing Advance method, invoked it once with 7, and inspected
  stored Value 26 on the same logical object at checkpoints 18/61. Seven turns,
  seven terminal receipts, and three fresh foreground processes used no compiler,
  installation, getter, or publication grant. Native SDK-free reads preserved
  checkpoint generation/digest, invocation replay retained the original execution
  and 26, and finished recovery used zero operations. See
  [live report](acceptance/live-inspection-2026-10-29.md) and
  `target/live-inspection-20261029-01/`; all prior demo universes remain unchanged.
- Fixed-width integer division/remainder: `div`, `div.un`, `rem`, and `rem.un`
  execute on matching I32/I64 operands and participate in checkpoint stack-shape
  validation. Signed division truncates toward zero; signed remainder follows
  the dividend's sign. Zero divisors fault. Signed minimum / or % -1 deliberately
  faults on every host; ECMA permits this remainder policy and CoreCLR .NET 10 on
  this arm64 host agrees. No Rust arithmetic panic or host overflow setting owns
  guest semantics. The failed instruction retains its operands/continuation;
  faults remain durable invocation receipts, without catchable guest EH.
  `guest/IntegerArithmetic.cs` compiles unchanged against both CoreLibs, with a
  2,312-case differential matrix and nested Euclidean/decimal-digit workload.
  Six integration tests plus one VM unit test cover actual emitted opcodes,
  bounds, matching widths, every instruction cut, GC, stable identities, cyclic
  state, fault-after-mutation receipt replay after root/context replacement,
  failed terminal checkpoint recovery, and SDK-free native process resume.
  Negative compiler/install tests now use unsupported XOR; floating/native
  arithmetic still rejects. Format 7 and every legacy fixture are unchanged.
  See [CIL profile](cil-profile.md). No live inference was needed for this step.
- Bounded managed decimal text in the separately compiled `Renraku.Library`:
  `Renraku.Text.IntegerText` formats signed Int32/Int64 and parses optional ASCII
  signs/digits within 64 UTF-16 units. Readonly result objects retain Success,
  Value, Error, and Position; null/empty/length/digit/overflow failures are data.
  Negative accumulation preserves signed minima without overflowing, and guards
  precede multiplication/subtraction. This is ordinary CIL, with no new opcodes,
  InternalCalls, capabilities, or checkpoint fields; CoreLib bytes are unchanged.
  The fixture compiler emits the library separately and compiles identical source
  against CoreCLR. Three library tests cover 286 boundary/seeded formats, 1,188
  parses with a BigInteger prefix oracle, every-instruction checkpoint/GC across
  six representative operations, and SDK-free native process recovery.
  A fourth website test compiles/installs the library into an existing universe,
  exports both library and Website reference views, and compiles/publishes
  `guest/examples/IntegerWebsite.cs`. The page renders existing visit state and
  accepts an optional decimal expected-count precondition on POST /visit. It
  resumes seven-instruction quanta with GC/reopens, preserves old responses and
  one-context exclusion, rejects stale/invalid input without a visit, and serves
  native TCP requests/replays without the compiler ledger or SDK. Model guidance
  advertises the module conditionally; old universes do not gain it implicitly.
  Format 7, all legacy fixtures and demo universes remain unchanged. See
  [managed integer text](integer-text.md). No live inference was needed.
- Bounded managed ordinal StringMap in the separate `Renraku.Collections` module:
  Get/Set/Remove/Count operate on ordinary linked objects with 64 entries,
  128-unit keys, and 4096-unit nullable values. Empty keys, NUL, and exact lone
  surrogate units work; data results distinguish missing/invalid/full outcomes
  from stored null values. Equal-content Set preserves the existing node/key/value
  identities; removal reuses capacity. No new opcodes, intrinsics, capabilities,
  or checkpoint fields were added. CoreLib and Renraku.Library remain unchanged.
  Four tests compare 441 stateful operations with CoreCLR's ordinal dictionary,
  verify shared string/no-op identities and removed-result receipt roots, recover
  eight mutations at every instruction with GC and pinned IDs, and resume in a
  native SDK-free process. A typed argument/receipt retains an old map after its
  static root is replaced. The test harness is a separate fixture module.
  A fifth website test compiles/installs both libraries, exports their own-module
  closures, and publishes `guest/examples/GroupedWebsite.cs` against existing
  global visit state. POST bodies select group keys with decimal counters. The
  test pauses after a group link changes but before the original visit, then
  resumes the same execution over native TCP without compiler ledger or SDK.
  Old responses, multiple/default groups, invalid keys, conflict detection, and
  one-context exclusion survive. Mutators have intermediate fields; whole methods
  are not atomic transactions. The page's cross-module Counts field currently
  makes exporting a facade for GroupedWebsite itself reject explicitly, as tested.
  Source/CIL and raw state remain inspectable. See [managed string map](string-map.md).
  All legacy fixtures, format 7, and existing demos are unchanged; no live inference.
- Dependency-aware frozen reference sets now span acyclic non-core declaration
  graphs. `Program::reference_set` merges roots per original module, records
  foreign module/type IDs and exact dependency schema hashes, and emits canonical
  dependency-first sets. Isolated format-1 view bytes and the module-scoped
  `reference_view` API remain intact; foreign declarations use view format 2
  inside a hashed format-1 referenceSet envelope. No live facade types are minted
  and implementation DLLs are not compiler-reference substitutes. Native export,
  compile/install inputs, agent describe, and mixed-module compile roots use sets
  under existing bounds. The external compiler emits metadata-only dependencies
  before dependents; captured requests/artifacts and installation validate the
  complete graph. Checkpoint format stays 7.
  Two reference tests cover real cyclic/ambiguous C# metadata, merged/permuted
  roots, canonical hashes, old wire/independent-view behavior, missing/changed
  dependencies and wrong universes. A compiler test covers pretty/reordered set
  envelopes, reversed view-file inputs, deterministic output and missing-input
  rejection. A five-turn managed-agent fixture describes a cross-module owner,
  compiles mixed-module roots, installs/invokes and finishes with zero-operation
  finished replay. These are deterministic fixtures, not live-model evidence.
  The grouped-website test now exports its Counts dependency set and compiles a
  caller that gets/updates the original map. A failed install rolls back code;
  published compilation replays with a missing compiler path, native SDK-free
  invocation resumes/replays once, and GC preserves original map/type/field IDs,
  one map object, page state, and old HTTP receipts. See
  [reference sets](reference-sets.md). Cycles, ambiguous dependency names and other
  unsupported declaration shapes still reject explicitly. Legacy fixtures and
  both existing demos remain unchanged; no live inference was used.
- The subscription adapter now has an 18-turn multi-module local HTTP fixture:
  discover/inspect a saved grouped-site map, describe its dependency set, read
  retained library/page source, correct CS1503 diagnostics, compile/install/invoke
  a caller against the original map, inspect its changed value, then compile,
  install, publish and preview a read-only page using the decimal library.
  Exact wire histories match managed transcript prefixes. Lost source, rejected/
  successful compiler, install, invoke and publication results recover after GC
  without provider operations or the SDK; an operator's newer publication is not
  rolled back when authority is revoked. All 18 requests have terminal receipts,
  original map/type/field/node identities and old HTTP responses survive, and
  independent TCP/replay and zero-operation finished recovery pass. This fixture
  is scripted rather than autonomous-model evidence.
  It exposed the full StringMap.Set listing exceeding the existing 16 KiB tool
  output limit. Optional read_code sourceOnly:true now returns pinned method/code
  IDs and exact retained assembly source/hash/artifact provenance, omitting CIL
  instructions and buildManifest and marking source_provenance. Default/old frozen
  input shapes remain unchanged, and the same output bound/capability policy
  applies. See [agent source inspection](agent.md).
- Live multi-module GPT-6.1 Sol acceptance now completes the new workflow: inspect
  the original map/node and red=2, read page/library source and dependency
  contracts, compile a 1,212-byte two-method assembly against four frozen facades
  on the first attempt, install, invoke the guarded update exactly once, inspect
  red=3, publish/preview its own read-only page, and recheck map/visit state.
  Twenty main-goal model operations/tool actions and 3,126 guest instructions ran
  across seven fresh processes with three-operation budget stops. One bootstrap
  finish allowed ordinary operator setup and a durable zero-budget follow-up;
  overall 21 turns/45 messages have 21 terminal private model receipts. Native
  verification retained the exact map/type/field/node IDs and one map object,
  global visits=2, original POST responses, invoke execution/result replay, and
  read-only inspection generation/digest. Finished recovery without publication
  authority/compiler/tools used zero operations. Evidence is
  [the live report](acceptance/live-multimodule-2026-10-29.md) and
  target/live-multimodule-20261029-01/. The bounded edge exited; both older demos
  remain unchanged. This evidence is separate from scripted HTTP fault fixtures.

## Validation

Run `cargo test --workspace`, `cargo fmt --all -- --check`, and
`cargo clippy --workspace --all-targets -- -D warnings`. The full suite includes
CoreCLR comparisons for twenty guest cases, per-instruction checkpoint tests,
actual process kills, format upgrades, exact UTF-16 recovery, and stale writers.
All 236 tests pass. Eighteen effect tests cover intent, dispatch, and response/wakeup
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
Twenty-five coding-agent tests cover full action selection from actual saved request
history, lost results after all tools, SDK-free terminal replay, frozen static
arguments, pre-invocation GC roots, uncertain inference without resubmission,
strict inputs and all budgets, generated-code capability denial, compiler worker
yielding to another universe, and partial journal recovery with seven-instruction
budgets. A foreground CLI test rejects missing opt-in and oversized budgets before
touching universe/auth state. Existing field-name test lookups now include owners,
so valid same-named fields in different managed classes cannot make tests flaky.
Eight further agent tests exercise the actual subscription adapter with local
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
CS1503 diagnostics. Three console unit tests cover terminal-vs-budget/uncertain
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
Thirteen website tests cover actual TCP/CLI serving, SDK-free CLI delivery, exact
old-response replay after newer mutations and handler selection, all handler
instruction cuts with GC/recovery, paused-call exclusion, denied model authority,
fault/null/surrogate/oversized terminal responses, malformed framing and bounds,
failed response checkpoints, and fragmented UTF-8/NUL data. Actual SIGKILL tests
stop before execution, after a partial mutation, and after response commit before
socket delivery; the same port rebinds and retry retains one original execution.
The publication acceptance compiles the checked-in new handler through the pinned
compiler service, installs while an old request is partially mutated, and publishes
without disturbing that execution. The old request resumes its original code; new
TCP requests use the installed caller and the same visit history. Publication ID
conflicts, replay after newer publication, caps, invalid ABI/IDs, corrupt roots,
failed commits, stale writers, GC, and native SDK-free serving are covered. Actual
publication SIGKILL at Prepared/Written/Committed restores a coherent old or new
route, journal, and paused context. No live inference was needed for this milestone.
The agent publication tests additionally cover default denial, strict zero-argument
inspection, frozen code IDs, revoked authority before/after commit, and read-only
recovery after a newer operator publication. The HTTP fixture completes 13 selected
turns across two goals, comparing every full request history to the linked CIL
transcript, replaying a lost publication result offline without permission, and
using the same Website history through real TCP. The second goal needs no compiler.
Native CLI tests accept both opt-in flag orders, preserve the flag in resume
arguments, and reject unrelated/duplicate grants or missing model authorization.
Four additional VM preview tests reject invalid inputs and model-owned selection,
recover after pre/post-acceptance route changes and zero/small budgets with GC,
reconcile lost results, preserve model-denied/null/surrogate/oversized receipts,
bound escaped Unicode/NUL output while retaining full strings, and recover a failed
response save into the original execution. A socket/preview interoperability test
also proves explicit model permission removal on a supplied enabled VM. The new
twelve-turn subscription fixture observes draft output, compiles/publishes a
correction, and verifies both the new body and retained visit. Lost preview/compiler
results reconcile offline, every model history matches the managed transcript,
and independent TCP confirms new output plus immutable old-preview replay.
Three native inspection unit tests cover shared cycles, hidden inherited field
owners, label/field/JSON bounds, large integers, exact UTF-16 and split surrogates,
invalid/ambiguous/static/literal selectors, transients, and unchanged runtime state.
Two real-CIL integration tests retain cyclic/static/string objects through GC and
restore, check inherited owner identities, and run the read-only CLI without SDK
tools. Three agent tests additionally prove typed target retention after static
replacement and lost read recovery, explicit newer static re-observation, strict
inputs, and one existing mutation with no helper compilation. An eight-turn local
subscription fixture observes 19/26 on the same object, loses read/invoke replies,
compares every full history with managed state, and finishes signed out offline.
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

The first website proof now uses a host HTTP edge with managed handlers/state and
a durable invocation inbox, bounded parsing, and local binding. Accepted work is
saved before executing it and response/state before sending. A crashed socket is
not recoverable; client IDs and receipt replay prevent duplicate mutations.
General guest networking APIs remain future work; see the
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

The first persistent localhost website milestone is implemented and verified.
Its supported HTTP-v1 profile fixes successful status/content type and returns an
immutable managed string. Keep these receipt semantics stable; richer response
envelopes and general network APIs require explicit contracts.

Durable website publication now connects external compilation and append-only
installation to live selection for new request IDs. Retain its narrow default
route contract, immutable publication IDs, original accepted-call bindings, and
strict journal validation; method dispatch replacement remains unsupported.

The coding agent now authors and publishes website handlers under explicit host
authority, with durable results and local/live two-goal acceptance. Preserve its
default denial and read-only committed-receipt recovery after authority changes.

The coding agent can now preview and correct a published page, with local recovery
fixtures and real-model acceptance. Preserve the distinction between direct managed
preview and transport verification, exact frozen selection, and the same HTTP-v1
inbox/response semantics across both paths.

Bounded state inspection now completes the immediate object/static observation
milestone, including local and real-model evidence. Preserve raw storage semantics,
typed target roots, shallow views, immutable committed history, and explicit newer
observations after an unappended read is lost. More host tools are not required
before the next core-runtime/library step.

**Next extend fixed-width CIL conversion and unsigned decimal library support.**
Start with the integer conversions actually emitted for UInt32/UInt64 decimal
formatting/parsing (for example conv.u8, conv.i4/conv.u4 where required), preserving
raw I32/I64 storage bits. Consult the primary ECMA-335 specification for truncation,
signed/unsigned extension and stack/result types; compare actual emitted C# and
CoreCLR boundary results rather than assuming Rust casts match every opcode.
Keep unsupported floating/native integers and checked conversions explicit.
Then add bounded pure managed unsigned formatting/parsing as a new append-only
module instead of changing Renraku.Library/CoreLib bodies. Reuse the signed text
API's explicit lexical/bounds/result conventions where appropriate, with correct
UInt32/UInt64 minima/maxima/high bits and overflow guards. No host formatter or
new generic/native collection is needed. Test every new opcode with exact storage
bits, per-instruction/GC/recovery, stable IDs and old readers; compare the actual
library against CoreCLR under its declared syntax. Demonstrate compilation and
use through frozen reference sets and SDK-free replay. Preserve default authority
denial, all receipts/transcripts, demo universes and sourceOnly projection bounds.
Do not repeat completed live model runs without a new acceptance question. Shifts,
bitwise operations, general formatting/culture, broad BCL compatibility, cyclic
facades, replacement bodies and same-universe concurrency remain future work.

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
