# Renraku

Renraku is a universal orthogonally persistent runtime under development. Rust
implements the substrate; CIL is the execution foundation and C# is the first
guest language. The goal is a durable universe of objects, code, and active
computations that survives the lifetime of its host process.

The foundational crash-and-resume milestone is working: a C# computation with
shared mutable objects and nested calls continues after its Rust host is forcibly
terminated. A basic model harness also suspends nested C# calls around durable
requests, reconciles uncertain outcomes with a deterministic local provider,
and resumes a linked conversation after process crashes.

## Current implementation

The initial CIL interpreter runs real Roslyn-compiled C# against a custom minimal
CoreLib. It supports integer arithmetic, locals and arguments, conditional
branches and loops, object allocation and constructors, reference fields,
instance and static fields, nested calls, returns, and immutable UTF-16 strings.
Signed/unsigned 32-bit and 64-bit division and remainder retain their operands
at checkpoint safe points; zero-divisor and overflow faults are durable receipts.
String literals, length, character indexing, concatenation, and ordinal equality
survive checkpoints. The sample simulation
returns **48** after **155 guest instructions**.

- Distinct, opaque identities for objects, types, fields, methods, code versions,
  executions, effects, and transient arenas.
- A persistent heap with named roots, checked reference lifetimes, and tracing GC.
- Graph tracing that includes references held by retained execution contexts.
- Explicit frame records with pinned method versions, CIL instruction offsets,
  arguments, locals, evaluation stacks, and pending call results.
- A strict, versioned primitive encoding for integer values and references.
- A durable outbox with prepared, in-flight, uncertain, and completed requests.
  Local provider receipts support reconciliation after a process crash.
- Portable CIL effect continuations with atomic response storage and guest wakeup.

Atomic SQLite checkpoints save reachable objects, roots, statics, original
assemblies, logical identity bindings, execution status, and active frames.
Recovery validates this state and resumes without needing the original assembly
files or the .NET SDK. Checkpoint intervals and execution budgets are measured
in guest instructions. The CLI can inspect both live and saved state.

Live code replacement, type initializers, and multiple simultaneous contexts
remain to be implemented. Subscription-backed model inference is available.
The [CIL profile](docs/cil-profile.md)
and [checkpoint contract](docs/checkpoints.md) specify the current limits.

A [persistent localhost website](docs/website.md) is also working: CIL handlers
own page content and visit state, while the host owns sockets. Stable request IDs
resume or replay saved responses after a crash without repeating mutations.
New handlers can be compiled externally, installed, and durably published while
accepted requests retain their original code and response. Publication retries
preserve a newer handler selection.
The coding agent can publish under a separate explicit opt-in. A
[live two-goal run](docs/acceptance/live-website-2026-10-29.md) authored and
published two website versions while preserving saved visits and old responses.
Its bounded local preview can inspect handler output and drive a correction;
[live acceptance](docs/acceptance/live-preview-2026-10-29.md) verified this loop
with paused-frame recovery and independent TCP delivery.
The [state inspector](docs/inspection.md) now exposes bounded read-only object and
static storage views. A [live model run](docs/acceptance/live-inspection-2026-10-29.md)
observed 19 and 26 on the same Counter around one existing mutation, without a
compiler or getter.

The first [managed text library](docs/integer-text.md) formats and parses bounded
signed decimal integers in ordinary checkpointable CIL. It installs alongside an
existing core and powers a numbered-visit page with a decimal count precondition.
A [managed string map](docs/string-map.md) supplies bounded ordinal Get/Set/Remove
over checkpointed linked objects. A grouped-visit handler combines both libraries
and resumes a request between updates without repeating a committed visit.
[Frozen reference sets](docs/reference-sets.md) let compiled callers use fields
and APIs across installed modules while preserving their existing identities.
The separate [unsigned decimal module](docs/unsigned-text.md) formats and parses
the full UInt32/UInt64 range in ordinary resumable CIL, alongside the signed API.
Fixed-width bitwise AND/OR/XOR/NOT also run in CIL with portable operand stacks
and durable invocation replay. Fixed-width shifts use an explicit portable count
policy, with sign-preserving and zero-filling right shifts.
The [managed hex module](docs/hex-text.md) uses those operations and reuses the
existing unsigned parse-result types through frozen cross-module references.
The [ordinal search module](docs/ordinal-text.md) supplies bounded IndexOf,
StartsWith and EndsWith in pure CIL, including exact surrogate-unit recovery.
A [live multi-module model run](docs/acceptance/live-multimodule-2026-10-29.md)
used those libraries to update the original map once and publish/preview its own
read-only page, retaining old HTTP responses across fresh processes.

The [managed object map](docs/object-map.md) holds nullable object references
with identity-based updates; its frozen caller and website reuse the same saved
Counter through native recovery. The next development phase is
[networked objects, placement and universe boundaries](docs/distributed-objects.md).
[Fixed-owner placement and method-grant preflight](docs/placement.md) now persists
explicit policies and denies ungranted access under its bounded host API.
The [owner backend](docs/owner-calls.md) now executes pinned instance calls with
atomic grant acceptance and durable reply reconciliation after revocation.
[Authenticated loopback peers](docs/peer-transport.md) now connect native owner
and caller universes with durable caller intent and lost-reply recovery.
[Imported contract handles](docs/remote-calls.md) now route ordinary C# instance
calls through durable guest waits and atomic reply/wakeup recovery. General
foreign references and copying/mobility remain ahead; the initial contract allows
one object/method per fieldless local type. [Closed child endowments](docs/child-endowments.md)
now enforce explicit code/class/object/field/effect rights and a filtered child
tool surface. [Scoped compiler bootstrap](docs/scoped-bootstrap.md) now compiles
captured C# against selected declarations before sealing, including opaque
remote handles, without exposing private APIs or opening the sealed registry.
Dynamic child compilation, delegation and a general hostile-agent
sandbox remain future work.

## Run the demonstration

Install Rust 1.89 or later and the .NET 10 SDK. From the repository root:

```sh
dotnet run --project tools/GuestCompiler -- . target/guest
cargo run -p renraku-vm -- run target/guest/Renraku.CoreLib.dll target/guest/Renraku.Simulation.dll Renraku.Guest.Simulation::Run
```

Expected output:

```text
result: 48
instructions: 155
```

Inspect loaded types and methods, or stop after 30 instructions to inspect an
active call stack:

```sh
cargo run -p renraku-vm -- inspect target/guest/Renraku.CoreLib.dll target/guest/Renraku.Simulation.dll
cargo run -p renraku-vm -- run target/guest/Renraku.CoreLib.dll target/guest/Renraku.Simulation.dll Renraku.Guest.Simulation::Run 30
```

The budgeted CLI prints the paused state and exits; it does not save that state.
Embedding applications can continue execution by calling `Vm::run` again on the
same VM.

## Compile your own guest

The [external compiler](docs/compilation.md) runs Roslyn on the host and emits a
bundle against the supplied Renraku core. Choose a new output directory:

```sh
dotnet run --project tools/Compiler -- --core target/guest/Renraku.CoreLib.dll --output target/compiled-hello --name Example.Hello guest/examples/Hello.cs
cargo run -p renraku-vm -- run target/compiled-hello/Renraku.CoreLib.dll target/compiled-hello/Example.Hello.dll Example.Hello::Run
```

The result is **36**. The bundle captures exact sources, reference bytes, compiler
identities/hashes, options, and emitted CIL. Compilation and runtime acceptance
remain separate; unsupported CIL is rejected. Saved universes retain code and
resume after the original compiler, source, and DLL files are gone.

The [runtime workbench](docs/workbench.md) can search saved symbols, inspect exact
CIL implementations, export compiler reference views, and install new callers
while preserving existing objects and paused frames. Its acceptance fixture
compiles a durable model response and executes it against the same persistent
Counter across restarts. Existing-body replacement and native model tool calling
remain future work. `invoke-once` adds durable request IDs: retrying a call after
a lost reply resumes the original execution or returns its saved result without
repeating the mutation. Receipts commit with heap/code/frames and survive GC.
`install-once` likewise commits a captured compiler bundle and installation
receipt with the code. `read-code` can show its original source after the bundle
is deleted; old/raw DLL loads still expose saved CIL.
The [compiler service](docs/compiler-service.md) also saves pinned compilation
requests before execution and retains completed artifacts or diagnostics. Its
CLI can resume after deleting source files and install a published result without
the SDK. The [persistent coding agent](docs/agent.md) now connects these tools:
assistant actions and tool results live in linked CIL objects, and stable turn
IDs reconcile compilation, installation, and invocation after a lost result.
A scripted provider discovers Counter, inspects its code, writes and installs a
caller, then changes the same persistent object from 19 to 26. A
[live GPT-6.1 Sol run](docs/acceptance/live-agent-2026-10-29.md) now completes the
same acceptance target using its own selected actions and generated source,
retaining the conversation across process restarts. A finished restore returns
the saved result with zero model operations. Local HTTP fixtures exercise the actual
subscription adapter through compiler-error correction, full-history turns,
lost results, and controller SIGKILL recovery without repeating the mutation.
The foreground commands show bounded saved-turn progress on stderr and keep stdout
as one JSON report with stop guidance and matching resume/inspection arguments.
`agent-input` now accepts a follow-up goal after completion, retaining managed
history, installed code, and existing objects. Input IDs make lost replies safe
to retry, and turn-derived mutation IDs keep increasing across goals. Older saved
agent schemas remain resumable and explicitly reject the new input operation.
A [live two-goal run](docs/acceptance/live-followup-2026-10-29.md) reused the
installed caller after restart, changing the same counter from 26 to 23 with
delta -3 and no new compilation.

## Save and resume a universe

After compiling the guest as above, choose an unused database path. This command
checkpoints every 25 instructions and stops after a budget of 100:

```sh
cargo run -p renraku-vm -- run-persistent target/demo-universe.db target/guest/Renraku.CoreLib.dll target/guest/Renraku.Simulation.dll Renraku.Guest.Simulation::DurableRun 100 25
cargo run -p renraku-vm -- inspect-checkpoint target/demo-universe.db
cargo run -p renraku-vm -- resume target/demo-universe.db
```

Resumption retains the printed universe and execution IDs and eventually returns
**5526**. The guest builds a cyclic graph and repeatedly mutates it through nested
calls. It contains no persistence, serialization, or restart code.

Use `Renraku.Guest.Simulation::Strings` as the entry method to try a persistent
conversation object containing embedded NULs, an emoji, and an unpaired
surrogate. The result is **167861**. Checkpoints preserve its exact UTF-16 units.
Existing format 1 through 6 universes remain readable and upgrade to format 7 on their next commit.

Unexpected process termination restores the most recent committed checkpoint.
Uncommitted instructions and their heap mutations are rolled back together.
The recovery tests use actual process kills at controlled commit boundaries;
see [checkpoint validation](docs/checkpoints.md#validation).

## Run a persistent C# model harness

`Renraku.Model.Complete(string)` waits for a host-delivered response. The sample
retains four linked messages across two calls, including a response reused as
the next prompt. After compiling the guest, choose unused database paths. Save
the entry state with a zero instruction budget, then run the bounded driver:

```sh
cargo run -p renraku-vm -- run-persistent target/harness.db target/guest/Renraku.CoreLib.dll target/guest/Renraku.Simulation.dll Renraku.Guest.Simulation::ModelHarness 0
cargo run -p renraku-vm -- drive-local target/harness.db target/harness-provider.db 1000 2 --allow-model-calls
```

The driver executes and checkpoints CIL, dispatches prepared waiting calls,
and reconciles recovered uncertain calls. Delivery commits the response and
runnable frame together. The final result is **417** after **158 guest
instructions** and **2 provider operations**, matching CoreCLR with the
deterministic echo substitute.

The two explicit budgets limit guest instructions and provider operations for
that invocation. Both submissions and receipt lookups count as operations.
Use a provider budget of 1 to stop at the second waiting call, then invoke
`drive-local` again to finish. A missing receipt stops with an uncertain outcome
and never triggers another submission. Budgets reset on each explicit invocation;
they bound counts, rather than provider latency or lifetime spending.

Model calls require explicit host opt-in, which is disabled on every fresh VM
and restore. `Vm::set_model_calls_enabled(true)` is the embedding equivalent of
the trailing CLI flag. Running a waiting VM returns its effect ID immediately
without submitting or advancing guest instructions. Manual `dispatch-model`,
`reconcile-model`, and `resume` commands remain available for individual steps.

For full conversation input, use `Renraku.Model.Chat(binding, messages)`.
`ModelBinding` holds a capability and model identifier; `ChatMessage` is an
ordinary linked object with role, content, and next fields. The outbox captures
the binding and message sequence at the call boundary. Later edits to guest
history cannot change a saved request or its provider receipt.

Run `Renraku.Guest.Simulation::ChatHarnessScore` as the entry method to try two
structured calls with system, user, and assistant messages. It returns **11**,
matching CoreCLR, while preserving embedded NULs and a lone surrogate. The local
binding is `local.echo` / `echo-v1`; the echo substitute uses the final user text.
Receipts verify the entire history and model identity.

## Deliver and reconcile requests

Host code can also queue requests independently in an existing universe:

```sh
cargo run -p renraku-vm -- queue-model target/demo-universe.db "hello"
cargo run -p renraku-vm -- dispatch-model target/demo-universe.db target/demo-provider.db
cargo run -p renraku-vm -- inspect-checkpoint target/demo-universe.db
```

The local provider returns `echo: hello` and commits a receipt in its separate
database. Renraku commits the response as an ordinary managed string. A completed
request is never submitted again. If the host dies after marking a request in
flight, use:

```sh
cargo run -p renraku-vm -- reconcile-model target/demo-universe.db target/demo-provider.db
```

Reconciliation looks up the same effect ID. A missing receipt leaves the outcome
unknown and does not trigger another submission. Keep the provider database to
retain its receipts.

For a waiting harness, `drive-local` can reconcile and resume within its budgets.
Alternatively, reconcile using its universe and provider paths, then resume
with `--allow-model-calls`. This demonstration uses a local echo
substitute. A [subscription-backed Responses harness](docs/openai-responses.md)
is also available, including the coding agent's bounded JSON action protocol.
Native Responses function calls and explicit inference retry policies remain
future work. See the
[effect contract](docs/effects.md).

## Development and validation

```sh
cargo test --workspace
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
```

The C# integration tests require the .NET 10 SDK. They build the custom CoreLib,
compile the same guest source for both Renraku and CoreCLR, and compare twenty
results. Tests also single step through nested constructors and ordinary calls,
check static storage across successive executions, enforce instruction and frame
budgets, and reject unsupported or malformed assemblies. Focused CIL tests cover
the supported integer, stack, local, argument, and branch encoding forms.

Persistence tests cover every instruction of the first demonstration, inherited
references, rollback, checksum and format errors, invalid frames and references,
stale writers, and repeated process kill/recovery cycles. The test suite also
checks that readers see the previous checkpoint during an uncommitted write.
Effect tests kill a process before and after provider acceptance, reconcile
receipts without resubmission, reject stale dispatchers, and recover from a
failed response checkpoint. They also restore the harness at every instruction,
kill hosts with live waiting CIL frames and after response/wakeup commit, and
verify that linked conversation history survives outbox collection.
Driver tests enforce instruction and provider-operation budgets, recover actual
process kills during automatic delivery, stop on missing receipts, and retain
fault diagnostics and stale-writer protection.
Structured request tests cover frozen histories, roles and model identities,
malformed linked messages, request limits, full-history receipt matching, and
upgrade of a waiting format 4 call with its original accepted provider receipt.

The guest compiler uses Roslyn shipped with the SDK, with no NuGet package
dependencies. Rust uses `uuid` for identities, `sha2` for checkpoint checksums,
and `rusqlite` with bundled SQLite for storage. Building SQLite requires a C
compiler.

## Sign in with your ChatGPT subscription

The host OAuth CLI is ready for macOS and Linux:

```sh
cargo run -p renraku-openai -- login
cargo run -p renraku-openai -- status
```

Login opens Continue with ChatGPT in your browser, verifies the returned
identity, and saves a private host profile. Refresh, account selection, and
logout are also implemented. See [subscription authentication](docs/openai-auth.md)
for commands and storage details.

The [Responses harness](docs/openai-responses.md) discovers account models and
runs a real prompt through persistent CIL with explicit opt-in. It saves local
completion receipts before waking the guest and never resends an uncertain
request. Credentials stay outside the guest universe. Login and status do not
submit inference.
The auth crate uses HTTPS and JWT libraries; its TLS backend also needs CMake
and a C build toolchain.

## Design and next steps

[The supplied architecture draft](docs/architecture-draft-v0.1.md) is preserved
unchanged. [The initial semantic contract](docs/semantics.md) records the working
implementation choices, acceptance criteria, and the LLM harness recovery model.

The local model harness now demonstrates strings, linked conversation history,
durable requests, waiting CIL callers, and bounded automatic execution and
delivery. Structured model requests and saved provider bindings are implemented.
The selected first real provider is the OpenAI Responses API using the user's
ChatGPT subscription through OAuth. Sign-in, account-specific discovery, streamed
inference, and local receipt recovery are implemented. The coding agent now
selects workbench tools through a strict text/JSON protocol with a durable managed
transcript. Native model function calls, general interactive conversation, and
existing-body replacement remain on the architecture roadmap.

The replacement BCL's direction is [suspension by default](docs/suspension.md):
waiting operations park portable guest contexts, with explicit concurrency and
durable operation ownership. [Asynchronous host delivery](docs/async-delivery.md)
is implemented and used by the OpenAI demo/driver. Each universe still has one
guest context, and universe checkpoint I/O remains synchronous.

[Progress and handoff](docs/progress.md) records the current state and the next
implementation targets for recurring development.
