# Persistent coding agent

The coding loop connects model-selected search, code inspection, C# compilation,
installation, invocation, and completion. Its conversation and action journal are
ordinary linked CIL objects inside the universe. A deterministic provider exercises
the whole sequence and recovery. A [live GPT-6.1 Sol acceptance run](acceptance/live-agent-2026-10-29.md)
also completed discovery, inspection, compilation, installation, and invocation
against the existing counter, changing 19 to 26 across process restarts.

## Try it in the foreground

Build the current guest, including `AgentHarness`, and the external compiler:

```sh
dotnet run --project tools/GuestCompiler -- . target/guest
dotnet build tools/Compiler --output target/compiler
cargo build --workspace
cargo run -p renraku-openai -- status
```

Use a signed-in profile with ChatGPT plan permission. Choose a model explicitly
from `renraku-openai models <profile-id>`, as described in the
[Responses guide](openai-responses.md). Set these two values to that profile and
model; they are not defaults:

```sh
RENRAKU_PROFILE="<profile-id>"
RENRAKU_MODEL="<model-slug>"
RENRAKU_RECEIPTS="$HOME/.local/share/renraku/agent-demo/receipts"
cargo run -p renraku-openai -- models "$RENRAKU_PROFILE"
```

Use a private receipt directory outside Git checkouts, as shown above, and an
unused universe path. This command authorizes model calls and
managed compilation/execution within the stated budgets:

```sh
cargo run -p renraku-openai -- agent target/agent-demo.db "$RENRAKU_RECEIPTS" target/agent-compiler.db target/compiler/Renraku.Compiler.dll target/guest/Renraku.CoreLib.dll target/guest/Renraku.Simulation.dll "$RENRAKU_PROFILE" "$RENRAKU_MODEL" "Discover Counter and inspect Counter.Add. Compile a new static Generated.Entry.Run(Counter counter) method that adds 7 and returns the result. Install it, invoke it on Renraku.Guest.Simulation::Saved, then finish with the result." 10000 8 16 --allow-model-calls
```

The initial saved Counter has value 19. The scripted acceptance sequence reaches
26; a real model chooses its own actions and may need corrective turns. The
report identifies completion or a budget stop. Inspect the saved conversation
without credentials or model calls:

```sh
cargo run -p renraku-vm -- agent-inspect target/agent-demo.db
cargo run -p renraku-vm -- inspect-checkpoint target/agent-demo.db
```

Continue the same session with fresh per-invocation budgets:

```sh
cargo run -p renraku-openai -- agent-drive target/agent-demo.db "$RENRAKU_RECEIPTS" target/agent-compiler.db target/compiler/Renraku.Compiler.dll "$RENRAKU_PROFILE" "$RENRAKU_MODEL" 10000 8 16 --allow-model-calls
```

Keep the same universe, model receipt directory, compiler ledger, and profile
identity across recovery. Published compilation results and completed managed
calls need no compiler process to replay. A prepared compiler request still needs
its exact pinned toolchain. Finishing parks the current goal until explicit new
input is accepted; it never starts another goal automatically.
The CLI requires explicit `--allow-model-calls` on both commands. The user has
authorized bounded live subscription inference, including recurring development.
Local fixtures remain the regression suite; use live runs for meaningful model
acceptance checks and retain their transcripts and receipts.

## Add a follow-up goal

For universes created with the current guest, a finished goal can accept another
goal without replacing its objects, installed code, conversation, or turn IDs:

```sh
RENRAKU_FOLLOWUP="Inspect the installed caller, invoke it once again on the same saved Counter, and finish with the observed result."
cargo run -p renraku-vm -- agent-input target/agent-demo.db goal-2 "$RENRAKU_FOLLOWUP" 1000
cargo run -p renraku-openai -- agent-drive target/agent-demo.db "$RENRAKU_RECEIPTS" target/agent-compiler.db target/compiler/Renraku.Compiler.dll "$RENRAKU_PROFILE" "$RENRAKU_MODEL" 10000 4 16 --allow-model-calls
```

`agent-input` only appends managed user input; it performs no inference and needs
no credentials. Its instruction budget defaults to 1,000 and permits zero. A
`pending` reply means the private CIL append helper has saved frames; repeat the
same input command with a budget or let `agent-drive` resume it. `accepted` means
the append committed, not that the model has completed the new goal. A faulted
helper retains its diagnostic and receipt rather than starting a second append.

Input IDs contain 1..48 ASCII letters, digits, dots, underscores, or hyphens.
Goals contain at most 4,096 UTF-8 bytes and cannot be blank. Retrying the same ID
with exactly the same goal resumes or returns its original receipt, even after
later goals finish. A different goal under that ID rejects without appending.
Use a new ID for each new goal; receipt IDs are
`agent/<universe>/input/<input-ID>`. The receipt pins the helper's method/code,
original string references, the preceding turn number, and one execution.

The append runs in ordinary CIL and links an `AgentInput` and `ChatMessage` into
the session. Input intent and its arguments commit before any helper instruction;
frames and receipts retain them through GC. Partial appends finish before the
driver reads the stable journal or submits another model call. The transcript
validator ties each extra user message to its input node, a completed finish
turn, and the exact completed input receipt. `agent-inspect` adds
`followupSupported` and an `inputs` array containing IDs, content, and `afterTurn`.
The existing profile/model binding stays in effect.

New inputs require a finished goal and an idle execution. The lifetime 32-turn
and transcript limits continue across goals; accepting input cannot reset them
or reuse old compile/install/invoke IDs. A known input ID can still be queried
by replay after a limit is reached. This adds no native checkpoint fields:
format 7 and its earlier readers remain unchanged.

Older saved agent schemas lack the input list and private helper. They remain
readable, inspectable, and resumable, with `followupSupported: false`; new input
rejects explicitly. There is no automatic schema migration or method-body
replacement. Recompiling guest DLLs on disk does not update types already saved
inside an older universe.

## Progress and recovery output

`agent` and `agent-drive` print short progress lines on stderr as managed actions
and results become available:

```text
turn 4 saved action [compile]: compile Example (145 source bytes)
turn 4 saved error [compile]: CS1503 Generated.cs:1: Argument 1: cannot convert from 'string' to 'int'
turn 5 saved action [compile]: compile Example (139 source bytes)
turn 5 saved result [compile]: compiled artifact saved
turn 7 saved result [invoke]: returned {"kind":"i32","value":26}
```

An action line means the assistant action is saved. A result line means its
managed tool result has committed with the linked transcript. Partial journal
updates resume their CIL frames before publishing a result. Summaries contain at
most 512 UTF-8 bytes, escape control characters, and omit compilation source.
Use `agent-inspect` for full saved messages or `read-code` for installed source.

Stdout remains one JSON report, with the existing format-1 result/counters and
an additive `recovery` object. It identifies `finished`, `resume`, `inspect`, or
`reconcile_only`, gives guidance, and includes `inspectArgs`. Budget stops also
include `resumeArgs`, an argument array preserving the same universe, receipt
directory, compiler ledger/driver, auth directory, profile, and model. Stderr
prints the corresponding shell-quoted commands. Suggested resume budgets retain
larger current values and otherwise use at least 10,000 instructions, one model
operation, and 16 tool attempts. Running the suggested command is an explicit new
invocation; it is never run automatically.

Instruction/provider/tool budgets can be resumed. Lifetime turn/transcript caps
require inspection and do not suggest increasing a per-run budget. An uncertain
model outcome uses `reconcile_only`: the original completion ledger must yield a
receipt before continuation. A missing receipt stops again without resubmission.
Infrastructure and preflight failures still return an error and print inspection
and saved-frame commands; a partially built journal may require the latter.

Embedders can use `AgentHost::drive_with_progress` and `AgentEvent::json()` for
structured events. IDs are `agent/<universe>/<turn>/action` or `/result`, with the
observed checkpoint generation. Events are disposable projections of the latest
saved turn, not a delivery ledger. They may repeat on resume; deduplicate display
by ID if needed. They do not replace `agent-inspect`'s complete journal. A callback
error can stop presentation after a commit; the next drive recovers from managed
state and receipts without repeating a committed mutation. Progress adds no
checkpoint fields or new model operations.

## Text action protocol

The prototype reuses the adapter's final-text Responses transport. Each assistant
reply must be one JSON object, without Markdown. Tool results become user JSON
messages in the managed history. Native Responses function calls remain
unsupported by this adapter; they are not interpreted as action text. Full
history, `store: false`, and streamed final text follow the subscription route's
[HTTP requirements](https://developers.openai.com/siwc/token-sharing-open-source/preview-limitations).

| Action | JSON example |
| --- | --- |
| Search live symbols | `{"op":"search","query":"Counter","limit":20}` |
| Read an exact implementation | `{"op":"read_code","method":"Renraku.Guest.Counter::Add"}` |
| Describe a compiler contract | `{"op":"describe","type":"Renraku.Guest.Counter"}` |
| Inspect a stored static | `{"op":"inspect_static","field":"Renraku.Guest.Simulation::Saved","stringUnits":64}` |
| Inspect one object | `{"op":"inspect_object","id":"LOGICAL_OBJECT_ID","maxFields":16,"stringUnits":64}` |
| Compile a caller | `{"op":"compile","assembly":"Generated.Example","source":"C# source","referenceTypes":["Renraku.Guest.Counter"]}` |
| Install an earlier build | `{"op":"install","compilationTurn":3}` |
| Invoke a static entry | `{"op":"invoke","method":"Generated.Entry::Run","arguments":[{"kind":"static","field":"Renraku.Guest.Simulation::Saved"}]}` |
| Inspect website publication and authority | `{"op":"web_status"}` |
| Preview the published handler locally | `{"op":"preview_web","method":"GET","target":"/page","body":""}` |
| Publish an installed HTTP handler | `{"op":"publish_web","method":"Generated.Website::Handle"}` |
| Finish | `{"op":"finish","message":"The saved counter is now 26."}` |

Use logical IDs returned by search when names are ambiguous. `read_code` also
accepts a code-version ID and freezes that exact implementation. It returns saved
CIL with resolved targets, plus original assembly-scoped source for captured
installations. Raw DLL loads have no attached source. Source maps and
decompilation are not implemented.

Invocation arguments also accept `{"kind":"i32","value":7}`,
`{"kind":"i64","value":"9007199254740993"}`,
`{"kind":"object","id":"<logical-object-id>"}`, and `{"kind":"null"}`.
Static selectors freeze to literal values before execution. Unknown operations,
unknown properties, and model-supplied host/compiler paths are rejected. Direct
invocation of the coordinator's `AgentHarness` entries is reserved.

Compilation emits one `Generated.cs` against the runtime core and optional frozen
reference types across modules with acyclic declaration dependencies. The
complete [frozen reference set](reference-sets.md) is captured before compilation;
describe adds dependency schemas/set hash within the existing output bound.
Runtime schema and logical bindings stay
authoritative. The model cannot redefine a referenced live type by compiling a
facade. Installation adds a new assembly; replacing bodies on existing types is
not implemented. Compilation success does not imply supported CIL: installation
performs a separate loader check. See [compilation](compilation.md),
[workbench](workbench.md), and [compiler service](compiler-service.md).

The [state inspector](inspection.md) reads object/static storage without running
guest code. Views carry stable identities, declaring owners, runtime slots, exact
scalar values/reference IDs, and source checkpoint generations. Object Freeze
retains its target through GC using typed managed roots. UTF-16 previews preserve
lone surrogates as units; all fields/text/output have explicit bounds/truncation.
Lost unappended reads may observe a newer generation; committed results remain in
the managed transcript. No host paths, getters, compiler, or new authority are used.

For a large method, use `{"op":"read_code","method":"Type::Method","sourceOnly":true}`
to read retained assembly-scoped source without the instruction listing or build
manifest. The result keeps the exact method/code IDs, source hashes and artifact
identity, marks representation `source_provenance`, and retains sourceAvailable
and sourceScope. This projection runs no code and grants no file access. Default
read_code output remains authoritative saved CIL plus provenance. Source is not
an executable replacement, and the same 16 KiB output limit still applies.
An unavailable/oversized source remains explicit; this is not source pagination.
The option defaults false and is omitted from canonical old frozen inputs, so
older pending read actions and format-7 agent fixtures retain their wire shape.

## Author and publish a website

`web_status` reports the current default handler, publication count/limit, current
host publication permission, and HTTP-v1 ABI. It returns the current receipt rather
than the entire journal, keeping the model-visible result bounded even at 128
publications. It performs no HTTP requests or mutations. `web-inspect` remains the
full host inspection command.

`publish_web` selects an already installed static
`string Handle(string id, string method, string target, string body)` for new HTTP
request IDs. The model may optionally include `codeVersion` to assert a particular
installed body. Freeze resolves the method and exact code ID into the managed
turn before execution; incompatible ABI or code IDs reject as saved tool errors.
Compilation and installation do not publish by themselves. Existing accepted
HTTP requests retain their original code and response. See the
[website contract](website.md#compile-install-and-publish-a-handler).

Publication authority is a separate, transient host policy, **disabled by default**.
Embedders must set `AgentHost::allow_web_publication = true`. The foreground
`agent` and `agent-drive` commands require an additional trailing
`--allow-web-publication` flag; `--allow-model-calls` remains independently required.
Both flag orders are accepted. Resume argument arrays preserve an existing grant
and do not introduce one when absent. Permission is not saved in guest state and
cannot be acquired by installing or invoking code. A denied publication becomes
a durable model-visible tool error.

For example, create a new universe with this goal and opt-in:

```sh
RENRAKU_WEB_GOAL="Discover Website, inspect its handler and compiler contract, then compile a new four-string/string-result handler that adds an HTML heading and calls the existing Website.Handle. Install and publish it. Preserve all visit state; never call Initialize. Finish after publication."
cargo run -p renraku-openai -- agent target/agent-website.db "$RENRAKU_RECEIPTS" target/agent-web-compiler.db target/compiler/Renraku.Compiler.dll target/guest/Renraku.CoreLib.dll target/guest/Renraku.Simulation.dll "$RENRAKU_PROFILE" "$RENRAKU_MODEL" "$RENRAKU_WEB_GOAL" 20000 4 16 --allow-web-publication --allow-model-calls
```

Resume budget stops with the returned arguments. After the goal finishes, serve
the saved handler with `renraku-vm web-serve ... @published ...`. Host listeners
and sockets stay external. One guest execution remains the limit: new HTTP work
receives 503 while the coordinator has active frames/waits. Use one controller
per universe and finish accepted HTTP work before submitting another agent goal.
Guest handlers inherit no model or publication capability.

Publication IDs are `agent.<universe>.<turn>.publish`, within the HTTP journal's
64-character ASCII alphabet. A committed result is reconciled by read-only lookup
before checking current permission, so losing the controller after publication
does not require granting authority again. It returns the original receipt,
including when an operator has since published a newer route. The older action
never republishes itself over that route. If no receipt committed and permission
has been removed, recovery returns a denied result without publishing. Storage
failures leave the turn pending; they do not become a success or a new action ID.
The actual frozen inputs and publication results stay in linked managed history.
This adds no native checkpoint fields or HTTP wire changes.

## Preview and correct a published page

`preview_web` executes the current universe's published handler under the same
HTTP-v1 argument and response contract as the localhost host. It opens no socket,
accepts no URL, and grants no external network authority. Its result explicitly
identifies `transport: "managed_handler"` and `socketTested: false`. Independently
verify the HTTP edge with its TCP fixtures or a browser.

The action accepts only `method` (GET or POST), `target` (an ASCII origin-form
target of at most 1,024 bytes), and optional UTF-8 `body` (default empty, at most
8,192 bytes). Request IDs and publication selection are host-owned; extra fields,
URLs, or a model-supplied `publication` reject. Preview needs an existing saved
publication. It requires no new host grant beyond the model's existing authority
to run managed code; publication itself still requires its separate explicit opt-in.

Preview can mutate managed state, including when the handler chooses to mutate
on GET. Each saved turn derives client ID `agent.<universe>.<turn>.preview` and
inbox ID `http-v1/<universe>/<client-ID>`. Freeze retains the exact method/target/body
and full selected publication in the managed turn before acceptance. If that
publication changes before an inbox receipt commits, the action returns a saved
conflict error and a new turn must select again. After acceptance, retries resume
or return the original pinned code, argument references, execution, and immutable
response, even after another handler is published.

The response projection is:

```json
{"ok":true,"requestId":"http-v1/...","preview":{"transport":"managed_handler","socketTested":false,"status":200,"state":"completed","body":"<h1>Page</h1>","bodyBytes":13,"truncated":false,"execution":"...","publicationId":"...","method":"...","codeVersion":"..."}}
```

`bodyBytes` measures the full original UTF-8 body. The displayed body contains at
most 8,192 bytes of JSON-escaped content (excluding its quotes); it is cut only at
a complete Unicode scalar. This keeps control characters and quotes within the
existing 16 KiB tool-result limit. `truncated` is explicit. The complete immutable
string remains rooted by the invocation receipt. A terminal fault or invalid
handler response returns the existing HTTP-v1 500/state mapping with `ok: false`.
Storage failures keep the frozen action pending instead of appending an unsaved
result.

A preview uses the existing instruction/tool budgets and at most 1,024 guest
instructions per attempt. A paused preview has one durable execution and receives
more quanta on explicit drive resumption; no second context or extra model call
is started to retry it. Instructions include the handler's nested calls. All
handler model capabilities are disabled, including for the public Rust
`web::preview_handler` API when given a previously enabled VM. The managed journal
and receipt arguments retain request data through GC and process recovery.

Models can inspect the returned body, compile a corrected caller, install and
publish it, then preview the new page. Keep new mutation IDs for genuinely new
requests. Replaying a committed preview never repeats its mutation. This adds no
native checkpoint fields or HTTP wire changes.

## Durable ownership and recovery

`AgentHarness.Saved` roots an `AgentSession` with linked `ChatMessage` and
`AgentTurn` objects. Each turn saves the exact assistant action, canonical frozen
input, result, and next pointer. Typed `AgentArgument` nodes retain frozen object
arguments through GC even if their previous static root changes before the first
invocation. The host reconstructs projections from these objects; it does not
maintain an authoritative native conversation vector.

The session phases are 0 (model call), 1 (pending action), 2 (next model turn), and
3 (finished). Portable frames can pause halfway through constructing or appending
a turn. Recovery resumes an owned Initialize/Ask/Freeze/Apply frame before reading
the stable journal, rather than starting that helper again. During a partial
helper update, `agent-inspect` may reject the incomplete graph; use
`inspect-checkpoint` to inspect the saved frames and resume with `agent-drive`.

Compile, install, and invoke request IDs are
`agent/<universe-id>/<assistant-turn>/<operation>`. Before a host action, the CIL
Freeze call commits resolved type/method/code IDs and literal arguments. Recovery
validates this saved input, reconciles its receipt, and appends the result to that
same turn. A lost install/invoke reply cannot repeat a committed mutation. A
prepared pure compiler job can rerun under the compiler service's explicit pinned
recovery policy; a published artifact or diagnostics cannot change.

The managed transcript, heap, code, frames, and invocation/installation receipts
share the universe checkpoint. Compilation and inference have separate durable
host ledgers. Neither ledger replaces managed conversation state. Checkpoint
format remains 7, with readers for formats 1 through 6.

The initial model submission error is surfaced with its diagnostic. A subsequent
resume only looks up a local receipt. A missing receipt leaves OutcomeUnknown and
stops; it never authorizes a second inference submission. Malformed actions and
ordinary tool input/compiler/CIL errors become saved tool-error messages for the
next turn. Infrastructure and checkpoint failures leave the action pending for
recovery. A faulted managed invocation has a terminal receipt; mutations made
before the guest fault are ordinary committed execution state, not a whole-call
transaction rollback.

Host tools run on disposable worker threads, including compiler process work, so
they yield the async executor to independent universes. Dropping a controller
future can leave a worker finishing its durable job; recovery uses its receipts.
Native futures, threads, and sockets are never persisted. Use one active
controller per universe. Concurrency within one universe is still future work.

## Bounds and authority

Each explicit drive invocation has separate instruction, provider-operation, and
tool-attempt budgets. Zero budgets are supported. Provider operations count both
submissions and receipt lookups. Replaying a tool receipt consumes a tool attempt;
a running invocation can consume several attempts as it advances in chunks of at
most 1,024 guest instructions. Instruction budgets also include managed journal
updates. Per-invocation policy caps instructions at one million and tool attempts
at 32. These are operation counts, not token quotas or provider latency limits.

The session has a hard limit of 32 assistant turns. Before another model call,
the controller stops if the saved transcript reaches 500,000 UTF-16 units; model
input retains its separate 128-message/one-million-unit bound. Other limits are
64 KiB per action, 32 KiB source, eight reference roots, eight invocation arguments,
4,096 bytes each for the goal and finish message, and 16 KiB per tool result.
Oversized outputs return an explicit truncation error. Compiler diagnostic
projections contain at most eight messages of 512 bytes, marked when truncated;
the compiler ledger retains the original bounded report.

The model controls trusted managed computation in this universe. It is not a
security sandbox for hostile generated code. No shell, file, or general network
tools are exposed. Generated invocations run with model capabilities disabled;
only the coordinator's owned model turn enables submission. Existing CIL limits
still apply, including no arrays, generics, exception handling, checked arithmetic, arbitrary
host BCL, or C# async/await. Cancellation and in-place conversation editing are not
implemented. A budget-paused invocation remains pending until explicitly resumed.
Int32/UInt32/Int64/UInt64 division and remainder are supported. Zero divisors and
signed minimum / or % -1 produce terminal VM faults; see the [CIL profile](cil-profile.md).

Fixed-width bitwise `&`, `|`, `^` and `~` preserve all 32/64 bits for signed or
unsigned declarations. Binary operands must have matching widths. They execute as
ordinary CIL, with no capability grant or host wait. Fixed-width `<<`, `>>` and `>>>` accept I32 counts. Raw CIL counts use their
low 5/6 bits for I32/I64 values, including negative/high counts; this is a
portable Renraku policy where ECMA leaves results unspecified. Signed right shift
preserves sign; unsigned right shift inserts zeros. Native/checked operations
remain unsupported.

Unary integer `-value` is supported: unchecked CIL negation preserves width, and
signed minimum remains unchanged. C# uint unary minus first widens to Int64.
This differs from the terminal signed-minimum division/remainder fault policy.

After an operator installs the [managed integer text library](integer-text.md),
search/describe `Renraku.Text.IntegerText` before compiling callers against its
frozen type view. `FormatInt32`/`FormatInt64` return canonical decimal strings;
`ParseInt32`/`ParseInt64` return objects with Success/Value/Error/Position. Parsing
accepts an optional ASCII sign and digits within 64 UTF-16 units. Old universes
keep their saved core and do not automatically gain this separate module.

The separate [unsigned decimal module](unsigned-text.md) similarly exposes
`Renraku.Text.UnsignedIntegerText` with FormatUInt32/64 and ParseUInt32/64.
Its 64-unit input syntax allows an optional ASCII plus and digits; all minus
signs reject, including `-0`. Select its own frozen type root after installation.
Raw invoke/inspection I32/I64 values keep their storage bits; unsigned decimal
meaning comes from the managed API, including a stored I64 -1 for UInt64 maximum.
Existing signed-library and CoreLib bodies are unchanged.
The separately installed [hex module](hex-text.md) exposes
`Renraku.Text.HexIntegerText` FormatUInt32/64 and ParseUInt32/64. Output is lowercase
without a prefix; input permits ASCII hex digits and an optional 0x/0X prefix
within 64 units, with no signs or whitespace. It returns the existing unsigned
result types. Select HexIntegerText as a frozen root; the result-module dependency
is included automatically. Stored I32/I64 bits still carry unsigned values.

The separately installed [StringMap module](string-map.md) exposes bounded
ordinal UTF-16 string Get/Set/Remove/Count with data results. Search/describe
`Renraku.Collections.StringMap` and select its frozen view before compiling calls.
Its methods require a non-null receiver; missing/null stored values are distinct.
Map mutators have intermediate instruction safe points, so resume their original
invocation rather than starting a new call after a lost result.

## Validation

```sh
cargo test -p renraku-vm --test agent
cargo test -p renraku-openai --test cli
cargo test -p renraku-openai --lib inference::tests::agent
```

Twenty-five agent tests cover the selected search/read/compile/install/invoke/finish
sequence, lost results around every tool, SDK-free published replay, frozen
static arguments and pre-invocation GC roots, uncertain inference with and without
receipts, strict inputs and budgets, generated-code capability denial, compiler
worker yielding, turn/transcript caps, and restore during partial journal updates
with seven-instruction budgets. The fixture selects actions from actual request
history and tool results; its determinism does not establish autonomous model
intelligence. Foreground CLI tests enforce opt-in and budget preflight. Existing
component tests additionally exercise real process kills around compiler,
installation, invocation, and inference commits. These automated fixtures use no
live inference. The separately recorded live acceptance used ten real model turns.

Eight additional local HTTP integration tests run the actual asynchronous
subscription adapter through the coding loop. The fixture chooses nine actions
from the received full history: search, read CIL, describe a contract, compile an
intentional type error, correct it using the saved CS1503 diagnostic, install,
invoke, read retained source, and finish. Every HTTP history is compared with the
corresponding prefix of the managed transcript, including NUL and Unicode text.
Reopening the adapter, changing active account selection, and losing published
compile/install/invoke results retain the original profile and action identities.
Published replay works with a missing compiler path, and finished recovery works
offline after sign-out with zero operations.

A complete action contained only in provisional stream deltas is never executed.
Unsupported native function-call output also stays uncertain. Both cases preserve
the completed installation, leave the counter unchanged, and perform only offline
receipt lookups on resume. Conversely, a committed HTTP completion receipt whose
reply is lost recovers the original assistant action while signed out, then
executes its managed mutation once without another POST. A subprocess test kills
the controller after the invocation checkpoint and before appending its result;
recovery preserves its exact request/method/code/execution/argument receipt,
returns 26 without another mutation, and finishes with the remaining two model
calls. Its ignored child probe is launched by the parent test. Test endpoint
overrides remain private to unit tests; none of these cases uses a real account.

Follow-up tests recover the input append at every CIL instruction with GC between
restores, resume pending input through the general driver, retry native CLI input
IDs, reject conflicts/active goals/invalid inputs/lifetime overflow, and restore
the original agent schema from a fixture captured before this addition. A second
goal reads and invokes the installed caller to reach 33 without a compiler or a
second installation; its original invocation receipt remains unchanged. The
subscription HTTP fixture sends the full managed history plus the new user goal,
preserving monotonic turns and the same object across both goals.

Website tests separately enforce default denial, strict inspection/publication
schemas, exact frozen code binding, invalid ABI/path rejection, and revoked
authority before and after commit. A local subscription fixture selects a full
search/read/describe/inspect/compile/install/publish sequence, loses the publication
result, and reconciles it offline without authority after a newer operator
publication. A corrective model turn deliberately republishes with a fresh ID.
A second goal inspects retained source and republishes the installed handler
without a compiler. Real TCP visits share the original managed history; old
request replay is byte-for-byte stable. Every HTTP model history matches the
linked CIL transcript across both goals. These fixtures are scripted evidence,
not proof of autonomous model intelligence.

A [live two-goal website acceptance](acceptance/live-website-2026-10-29.md) used
15 real GPT-6.1 Sol turns to author and publish two versions across fresh processes.
HTTP visits retained their managed state; an older request kept its original page
after the new publication. Finished recovery used zero operations. Publication
authority was explicitly enabled for that run.

Preview tests freeze before acceptance, change the publication, and recover
accepted calls at zero/small instruction budgets with GC between restores. They
retain the same execution and one visit, reject new selection conflicts and strict
input violations, disable handler model calls, preserve fault/invalid-response
receipts, and bound Unicode/NUL/control-character output while retaining the full
string. A failed terminal checkpoint leaves the original inbox/action pending.
The socket host can replay a preview-created receipt under the same client ID.

A twelve-turn subscription HTTP fixture previews a draft page, selects a correction
from the actual response, publishes it, and previews the corrected page with the
original visit intact. Lost preview and compiler results reconcile offline after
restores; published replay needs no SDK. Every model history equals the linked CIL
transcript prefix. Separate TCP delivery confirms the corrected handler, and an
old preview still returns the draft response without repeating its mutation.

A [live preview/correction run](acceptance/live-preview-2026-10-29.md) used ten
real GPT-6.1 Sol turns to observe the existing page, write its own correction,
publish it, and verify the returned body and retained visit. The first preview
resumed from paused portable frames in fresh seven-instruction processes.
Independent TCP delivery matched the corrected preview; old-preview replay kept
its original page/execution. Finished recovery used zero operations.
