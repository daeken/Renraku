# Durable external compilation

The compiler service saves a request before running Roslyn and publishes either
a captured artifact or terminal diagnostics afterward. It is a host capability
with its own durable ledger, like the separate model provider receipt service.
Compilation does not mutate the guest universe. `install-compilation` imports a
published artifact using the existing atomic installation protocol.

The universe checkpoint remains format 7. This milestone adds no guest Compile
intrinsic, suspended CLR state, scheduler, or native process handles to it. A
future model tool loop must retain its action ID and tool transcript in managed
state before using the service; a compiler ledger is not the conversation's
source of truth.

## Use the service

Build the compiler driver once against the installed .NET 10 SDK:

```sh
dotnet build tools/Compiler --output target/compiler
```

With a saved universe and a runtime reference view, choose a new request ID:

```sh
target/debug/renraku-vm compile-prepare target/codegen.db target/compiler.db \
  turn-1/compile target/compiler/Renraku.Compiler.dll Generated.Counter \
  --reference-view target/counter.view.json guest/examples/CounterCall.cs
target/debug/renraku-vm compile-resume target/codegen.db target/compiler.db \
  turn-1/compile target/compiler/Renraku.Compiler.dll
target/debug/renraku-vm compilation target/codegen.db target/compiler.db turn-1/compile
target/debug/renraku-vm install-compilation target/codegen.db target/compiler.db \
  turn-1/compile turn-1/install
target/debug/renraku-vm invoke-once target/codegen.db turn-1/invoke \
  Generated.Entry::Run 1000 static:Renraku.Guest.Simulation::Saved
```

`compile-once` accepts the same arguments as `compile-prepare` and performs both
preparation and execution. Source files follow all options. `--reference-view`
can repeat up to 16 times; a compilation without views needs no such option.
The core is captured from the saved runtime registry, not from a separate path.
The sample assumes Counter.Value is 19 and returns 26 after invocation.

After preparation, the original source and view files can be deleted: resume
uses the captured bytes. Once a terminal result is published, `compilation`,
`compile-resume`, and `install-compilation` need neither the SDK nor the compiler
files. A terminal resume returns its receipt before inspecting the supplied
compiler path. After installation, the universe no longer needs the compiler
ledger either: code, build inputs, and installation receipt are checkpointed,
and `read-code` returns the retained original source.

The Rust embedding APIs are `CompilerToolchain::open`,
`CompilationRequest::capture`, and `CompilationStore::{prepare,drive,get}`.
`Compilation::summary` returns the CLI's result shape; `artifact` contains a
validated `BuildArtifact` on success. Artifact/request byte encodings are framed
and bounded, and can be captured without filesystem paths.

## Pinned inputs and results

Each request captures the universe ID, assembly name, ordered source names and
exact UTF-8 bytes, core bytes, reference-view bytes, language version, compiler
options, and compiler identity. The input ID hashes this captured representation.
Request IDs follow the workbench's 1–128 byte ASCII identifier rules and are
scoped by universe ID in the compiler ledger. Reusing an ID with different
source, reference, options, or compiler identity is an error. Compile/install/
invoke IDs remain distinct actions; prefixing them with a turn ID is convenient.

Compiler identity includes SHA-256 for the configured dotnet executable and the
five driver package files: Renraku.Compiler.dll, its deps/runtimeconfig JSON,
Microsoft.CodeAnalysis.dll, and Microsoft.CodeAnalysis.CSharp.dll. The driver's
`--identity` probe also captures assembly names/hashes and the actual framework,
process architecture, and host CoreLib hash. The probe runs trusted host code
without compiling or executing guest source. Files/profile are checked before
execution, and actual compiler/runtime identity is checked in the final report.
Package hashes are checked again after execution. Relocating identical compiler
files is allowed; changing them requires a new request rather than retargeting
an existing one. This records a configured toolchain profile, not a portable copy
or attestation of the full CLR/native dependency closure or host environment.

A successful result must match the pinned assembly name, compiler identity,
options, source order/bytes, core bytes, and reference views. Its manifest and
all captured file hashes are verified before publication. Roslyn deterministically
emits the metadata reference images from the pinned views; those image bytes are
captured in the terminal artifact. C# errors and deterministic input rejections
publish `rejected` results with diagnostics or an input-error report. Success
publishes `compiled`; successful C# compilation still does not imply acceptance
of its CIL by Renraku. Unsupported division, for example, compiles but fails
installation without changing the universe.

Reports retain the actual compiler identity and bounded diagnostics; transient
output-directory paths are removed. Input and terminal result digests detect
damaged ledger blobs. These hashes are integrity checks, not authentication of
a hostile compiler or database. The configured driver is trusted host code;
guest code still cannot add projects, NuGet dependencies, analyzers, generators,
or inherited host BCL references through this interface.

## Recovery policy

`prepare` commits exact inputs before any guest compilation. The service states
are `prepared`, `compiled`, and `rejected`. Terminal results are immutable: a
retry/query returns the published artifact or diagnostics, without launching
Roslyn again. Publication saves report, captured artifact, and result digest
in one SQLite transaction, before the caller receives a successful reply.

An explicitly resumed prepared request may rerun the same pinned pure compiler
operation. A crash during execution, a timeout, an infrastructure failure, or a
failed result commit leaves it prepared. This differs deliberately from model
inference: compilation has no intended external side effects beyond its private
workspace, so recovery may repeat it. Uncertain model requests still reconcile
receipts and must never automatically resubmit. No guest code is executed by
compilation, and installation is always a separate action.

A bounded worker holds a SQLite writer transaction while verifying, compiling,
and publishing. This serializes workers for one ledger; another writer waits up
to five seconds and then fails without launching another job. WAL readers can
observe the prior prepared state. Process death releases the lock. The child
compiler can briefly outlive a killed host, but it has no access to publication;
only the ledger owner can commit a result. Recovery uses a fresh workspace.

Workspaces are private directories with fresh IDs under the ledger's parent.
Normal completion/failure removes them; SIGKILL can leave an orphan directory
containing captured inputs/output. It is never treated as a published receipt
or reused on resume. Cleanup of abandoned workspaces is currently manual.
The API is synchronous and is intended for a host worker; it must not be called
on an async executor worker that needs to keep unrelated universes progressing.

The ledger has application ID `0x52454343`, schema 1, WAL, and `synchronous=FULL`.
It rejects foreign databases and in-memory storage. Initial limits are 1,000
requests per ledger, 32 MiB of captured core/source/view bytes per request,
32 sources / 1 MiB each / 4 MiB total source, 16 views, 128 KiB input manifest,
64 MiB terminal artifact, 512 KiB per output stream/report, and a 30-second child
process deadline. Compiler package files and the dotnet executable are individually
limited to 32 MiB. There is no automatic receipt eviction or distributed worker
ownership protocol. Keep the ledger until its published artifacts have been
installed or otherwise retained.

Seven service tests exercise the native CLI path, source deletion, SDK-free
terminal recovery/install, compiler errors and input errors, input/toolchain
rebinding rejection, write failures, lost post-commit replies, foreign/corrupt
ledgers, CIL rejection, and real process kills before/during execution and around
publication. Completed jobs are tested with an observer that fails if launched.
