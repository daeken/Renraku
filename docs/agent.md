# Persistent coding agent

The coding loop connects model-selected search, code inspection, C# compilation,
installation, invocation, and completion. Its conversation and action journal are
ordinary linked CIL objects inside the universe. A deterministic provider exercises
the whole sequence and recovery. The foreground subscription command is ready;
actual autonomous model acceptance has not yet been live-tested.

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
cargo run -p renraku-openai -- models "$RENRAKU_PROFILE"
```

Start with an unused universe path. This command authorizes model calls and
managed compilation/execution within the stated budgets:

```sh
cargo run -p renraku-openai -- agent target/agent-demo.db target/agent-receipts target/agent-compiler.db target/compiler/Renraku.Compiler.dll target/guest/Renraku.CoreLib.dll target/guest/Renraku.Simulation.dll "$RENRAKU_PROFILE" "$RENRAKU_MODEL" "Discover Counter and inspect Counter.Add. Compile a new static Generated.Entry.Run(Counter counter) method that adds 7 and returns the result. Install it, invoke it on Renraku.Guest.Simulation::Saved, then finish with the result." 10000 8 16 --allow-model-calls
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
cargo run -p renraku-openai -- agent-drive target/agent-demo.db target/agent-receipts target/agent-compiler.db target/compiler/Renraku.Compiler.dll "$RENRAKU_PROFILE" "$RENRAKU_MODEL" 10000 8 16 --allow-model-calls
```

Keep the same universe, model receipt directory, compiler ledger, and profile
identity across recovery. Published compilation results and completed managed
calls need no compiler process to replay. A prepared compiler request still needs
its exact pinned toolchain. Finishing the session permits no additional turns.
The CLI requires explicit `--allow-model-calls` on both commands. Recurring
development uses local fixtures and does not initiate subscription inference.

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
| Compile a caller | `{"op":"compile","assembly":"Generated.Example","source":"C# source","referenceTypes":["Renraku.Guest.Counter"]}` |
| Install an earlier build | `{"op":"install","compilationTurn":3}` |
| Invoke a static entry | `{"op":"invoke","method":"Generated.Entry::Run","arguments":[{"kind":"static","field":"Renraku.Guest.Simulation::Saved"}]}` |
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
reference types from one logical module. Runtime schema and logical bindings stay
authoritative. The model cannot redefine a referenced live type by compiling a
facade. Installation adds a new assembly; replacing bodies on existing types is
not implemented. Compilation success does not imply supported CIL: installation
performs a separate loader check. See [compilation](compilation.md),
[workbench](workbench.md), and [compiler service](compiler-service.md).

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
still apply, including no arrays, generics, exception handling, division, arbitrary
host BCL, or C# async/await. Cancellation and interactive session editing are not
implemented. A budget-paused invocation remains pending until explicitly resumed.

## Validation

```sh
cargo test -p renraku-vm --test agent
cargo test -p renraku-openai --test cli
cargo test -p renraku-openai --lib inference::tests::agent
```

Ten agent tests cover the selected search/read/compile/install/invoke/finish
sequence, lost results around every tool, SDK-free published replay, frozen
static arguments and pre-invocation GC roots, uncertain inference with and without
receipts, strict inputs and budgets, generated-code capability denial, compiler
worker yielding, turn/transcript caps, and restore during partial journal updates
with seven-instruction budgets. The fixture selects actions from actual request
history and tool results; its determinism does not establish autonomous model
intelligence. Foreground CLI tests enforce opt-in and budget preflight. Existing
component tests additionally exercise real process kills around compiler,
installation, invocation, and inference commits. No new live inference was run
for this milestone.

Four additional local HTTP integration tests run the actual asynchronous
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
