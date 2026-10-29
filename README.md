# Renraku

Renraku is a universal orthogonally persistent runtime under development. Rust
implements the substrate; CIL is the execution foundation and C# is the first
guest language. The goal is a durable universe of objects, code, and active
computations that survives the lifetime of its host process.

The foundational crash-and-resume milestone is working: a C# computation with
shared mutable objects and nested calls continues after its Rust host is forcibly
terminated. A durable model-request outbox now tests uncertain external outcomes
with a deterministic local provider. Connecting it to suspended CIL calls is the
next step toward a basic LLM harness.

## Current implementation

The initial CIL interpreter runs real Roslyn-compiled C# against a custom minimal
CoreLib. It supports integer arithmetic, locals and arguments, conditional
branches and loops, object allocation and constructors, reference fields,
instance and static fields, nested calls, returns, and immutable UTF-16 strings.
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

Atomic SQLite checkpoints save reachable objects, roots, statics, original
assemblies, logical identity bindings, execution status, and active frames.
Recovery validates this state and resumes without needing the original assembly
files or the .NET SDK. Checkpoint intervals and execution budgets are measured
in guest instructions. The CLI can inspect both live and saved state.

Live code replacement, type initializers, multiple simultaneous contexts, and
guest calls to external capabilities remain to be implemented. The [CIL profile](docs/cil-profile.md)
and [checkpoint contract](docs/checkpoints.md) specify the current limits.

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
Existing format 1 and 2 universes remain readable and upgrade on their next commit.

Unexpected process termination restores the most recent committed checkpoint.
Uncommitted instructions and their heap mutations are rolled back together.
The recovery tests use actual process kills at controlled commit boundaries;
see [checkpoint validation](docs/checkpoints.md#validation).

## Try durable model delivery

Use an existing universe, such as the one above:

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

This is a host API and CLI demonstration using a local echo substitute. Guest
code can consume the stored managed response, but cannot yet suspend a CIL call
waiting for one. Real model providers, streaming, tools, and explicit retry
policies are later work. See the [effect contract](docs/effects.md).

## Development and validation

```sh
cargo test --workspace
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
```

The C# integration tests require the .NET 10 SDK. They build the custom CoreLib,
compile the same guest source for both Renraku and CoreCLR, and compare eighteen
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
failed response checkpoint.

The guest compiler uses Roslyn shipped with the SDK, with no NuGet package
dependencies. Rust uses `uuid` for identities, `sha2` for checkpoint checksums,
and `rusqlite` with bundled SQLite for storage. Building SQLite requires a C
compiler.

## Design and next steps

[The supplied architecture draft](docs/architecture-draft-v0.1.md) is preserved
unchanged. [The initial semantic contract](docs/semantics.md) records the working
implementation choices, acceptance criteria, and the LLM harness recovery model.

The next workload is an LLM harness. Strings and the host outbox now work;
conversation collections can initially use ordinary linked managed objects.
The next milestone is a portable waiting continuation for CIL calls, committing
the response together with making the guest runnable. Broader introspection and
live code evolution remain on the architecture roadmap.

[Progress and handoff](docs/progress.md) records the current state and the next
implementation targets for recurring development.
