# Runtime workbench and code generation

The next acceptance target is a model discovering the available API, reading
current code, generating a caller, compiling it, and executing it against existing
persistent objects. The host tools now provide that path. A deterministic model
fixture exercises the complete path through the durable CIL model call; native
Responses function calling and an autonomous model tool loop remain next work.

## Try the compiler and installation path

From the repository root, build the fixtures and native CLI, then choose unused
database and output paths:

```sh
dotnet run --project tools/GuestCompiler -- . target/guest
cargo build -p renraku-vm
target/debug/renraku-vm run-persistent target/codegen.db \
  target/guest/Renraku.CoreLib.dll target/guest/Renraku.Simulation.dll \
  Renraku.Guest.Simulation::SaveObject
target/debug/renraku-vm search target/codegen.db counter 20
target/debug/renraku-vm read-code target/codegen.db Renraku.Guest.Counter::Add
target/debug/renraku-vm export-reference target/codegen.db Renraku.Guest.Counter \
  > target/counter.view.json
dotnet run --project tools/Compiler -- \
  --core target/guest/Renraku.CoreLib.dll --output target/generated-counter \
  --name Generated.Counter --reference-view target/counter.view.json \
  guest/examples/CounterCall.cs
target/debug/renraku-vm install-once target/codegen.db counter-install-1 \
  target/generated-counter
target/debug/renraku-vm invoke-once target/codegen.db counter-add-1 Generated.Entry::Run 1000 \
  static:Renraku.Guest.Simulation::Saved
```

`SaveObject` creates a Counter containing 19. The generated caller returns **26**.
Repeat that command in another process and its JSON receipt still reports **26**,
with the same execution ID and no repeated mutation. Use request ID
`counter-add-2` for another call and it returns **33**, using the same object
identity and the same original method. Restart requires neither compiler nor
build bundle. Installation can also commit while an existing computation is
paused: its portable frames and code-version bindings survive unchanged.

## Read tools

`search` returns JSON with symbol kinds, names, logical IDs, execution signatures,
and code-version IDs. Matching is case-insensitive and requires all query words;
an empty query lists symbols. Queries are at most 256 bytes / 16 words, with a
result limit of 1–100. Results report total count and truncation.

`read-code` accepts a qualified `Type::Method`, MethodId, or CodeVersionId. An
ambiguous overload requires an ID. It returns the saved, decoded CIL for that
exact implementation, including offsets and resolved method/field logical IDs.
String operands retain exact UTF-16. This works after the original files are
deleted. For modules installed from captured bundles, it also returns the exact
UTF-8 source inputs, their hashes, artifact ID, and build manifest. The source
scope is explicitly `assembly`: PDB mappings from individual methods to source
locations remain future work. Legacy/raw DLL loads report `sourceAvailable: false`.
CIL remains the authoritative executable representation; captured source is build
provenance supplied by the trusted compiler boundary, not decompiled text or
cryptographic proof that a particular compiler emitted those instructions.

`export-reference` accepts 1–32 type names/IDs from one logical module. Its JSON
envelope includes `schemaHash`, `coreSha256`, and `schemaJson`. Parsing the latter
gives C# declaration scaffolding and type/field/method bindings to existing IDs.
The hash is SHA-256 of the exact UTF-8 `schemaJson` string, without an additional
newline; the core hash pins the bootstrap reference. Heap mutations and checkpoint
generations do not affect the view. Another universe loaded from the same DLLs
has different logical IDs and cannot substitute its view.

The initial exporter supports unsigned, version-zero modules with nongeneric,
top-level reference classes; it closes over same-module signature/base types and
uses the original core for core dependencies. It preserves visibility, static
and readonly fields, primitive signedness, and parameter names. Properties,
events, declaration/parameter custom attributes, literal/default fields,
optional/default parameters, value-type roots, non-ASCII identifiers, and
dependencies on another non-core module are explicit errors. It does not silently
erase those contracts. Independent live schema authoring is still future work;
today the authoritative registry is reconstructed from saved implementation bytes
and their logical bindings.

## Compile, install, invoke

`--reference-view` is repeatable up to 16 times. The external compiler verifies
view/core hashes, emits deterministic metadata-only reference DLLs with
`ReferenceAssemblyAttribute`, and references those images when compiling callers.
Reference builds use `includePrivateMembers: false`. Bundles using views have
manifest format 2 and capture reference image bytes/hashes and the exact views;
builds without views retain format 1. The current bootstrap core defines the
marker attribute; older cores without it cannot emit these facades.

`install` accepts one implementation DLL and up to 16 view files. The host
validates views against the current registry, validates the added CIL, resolves
references against existing logical bindings, and appends new definitions.
Existing implementation images and type/field/method/code IDs are retained.
New static fields receive their normal default values. Publication saves the
new registry with heap, frames, statics, and effects in one generation-checked
checkpoint transaction. Rejected DLLs, mismatched views, stale writers, and failed
commits leave the previous checkpoint intact. Reference DLLs are explicitly
rejected as executable inputs.

This is append-only installation, not replacement of a body on an existing
type, type-shape migration, or active-frame rewriting. A duplicate assembly name
is an error. The CLI takes exact DLL/view paths rather than interpreting or
authenticating a build manifest. These are trusted host operations, not a hostile
code sandbox or a complete CIL verifier. Runtime checkpoints retain CIL and its
logical bindings; external source/reference provenance remains in the bundle.

## Captured builds and durable installation

`install-once <database> <request-ID> <bundle-directory>` captures the compiler
bundle, verifies its manifest, and atomically publishes its code, provenance,
and installation receipt. `artifact::install_once` is the embedding API;
`BuildArtifact::from_files` accepts an in-memory manifest/file map, and
`BuildArtifact::from_bundle` captures a host directory. These are host operations;
the compiler process and its CLR remain external.

The reader accepts the compiler's manifest formats 1 and 2, pins the supported
compiler options and compiler identity/hash records, and verifies every declared
file's exact byte length and SHA-256. It captures the original manifest bytes,
core, implementation, sources, reference images, and reference views. Manifest
paths must use the expected core/assembly/source/reference locations; duplicate
names, path traversal, symlinks, missing files, invalid UTF-8 sources, and unknown
options are rejected. Unlisted directory files are not captured. The core must
match an exact retained runtime image; reference views must match the live schema,
facades must be marked compiler references, and implementation assembly scopes
must have captured compiler references. Normal CIL acceptance still applies.

The artifact ID hashes a versioned domain marker, exact manifest bytes, and sorted
filename/file-byte pairs with unsigned 64-bit little-endian length prefixes.
Moving a bundle does not change its ID. Changing source, reference bytes, options,
or the exact manifest changes the artifact ID, even if the emitted CIL is identical.
The metadata view's logical-ID bindings remain part of those inputs.

Installation request IDs follow the same rules as invocation IDs. A repeated ID
with the same artifact returns the committed receipt with the same assembly and
member identities, without relinking or advancing the checkpoint generation.
Changed artifacts or IDs already used for an invocation are rejected. A new ID
does not bypass duplicate assembly-name checks. `installation <database> <ID>`
retrieves a receipt without the original bundle, so a lost install reply can be
resolved after its files are deleted. Installation takes a fresh checkpoint and
uses the normal generation check to reject a concurrent write before publication.

Publication retains paused frames, invocation receipts, effects, heap, and old
code. The new registry, default storage for new statics, captured build, and
receipt commit in one snapshot. Before commit, an error leaves the old snapshot;
after commit, a lost reply is resolved by the receipt. The build bytes survive
restart and GC and are exposed by `read-code` for that immutable module. Plain
`install` remains available for trusted raw-DLL loading without this provenance
or request replay contract.

Bounds are 128 KiB for a manifest, 64 MiB aggregate captured file bytes, 66 files,
and 128 installation receipts, in addition to compiler source/reference limits
and the universe's 128 MiB checkpoint limit. Receipts/builds are not evicted
automatically. Large or accumulated artifacts can therefore hit the checkpoint
limit earlier. This is append-only module installation; body replacement and
type migrations need separate code-version/source association policies.

The [durable compiler service](compiler-service.md) now supplies preparation,
terminal diagnostics/artifacts, and recovery through `compile-once`,
`compile-prepare`, `compile-resume`, and `compilation`. `install-compilation`
imports a published artifact directly from its service ledger. Compiler process
memory is not a portable guest continuation; the next model loop must retain
its action IDs and transcript in managed state before using these capabilities.

`invoke` starts a new call only when the current context permits it, then runs
within the explicit instruction budget. Arguments are `i32:N`, `i64:N`, `null`,
`object:ID`, or `static:Type::Field`. Object/static references are taken from the
current universe, and VM argument types are checked. Model calls are disabled
for this command; installed code does not acquire network permission. Resume a
budget-paused call with the ordinary `resume` command.

## Durable invocation requests

Use `invoke-once <database> <request-ID> <method-name-or-ID> <budget> [arguments]`
for tool-driven mutations. `Vm::invoke_once` is the embedding equivalent. Request
IDs are scoped to the universe and contain 1–128 ASCII letters, digits, or
`._-/:`. Capture the request ID and exact arguments before issuing the call.

Before any guest instruction, the runtime commits a receipt binding that ID to
the exact method/code version, argument values, and one execution ID. A retry
with the same call resumes its saved frames, or returns its saved completion or
fault. Reusing the ID with different arguments or a different method is an error.
Budgets apply to each attempt and may differ on retry. A zero budget commits the
intent without executing any instructions. `running` receipts cover budget-paused
and effect-waiting calls; `waitingEffect` identifies the latter.

Terminal receipts commit with the same heap changes and execution state. Native
CLI output happens afterward. A lost reply or host crash can therefore be
resolved with `invocation <database> <request-ID>`, a read-only query, or by
repeating `invoke-once`. Replay is generation-fenced and may write an unchanged
checkpoint, but does not restart guest instructions. Faults are terminal receipts
as well. A new request ID explicitly starts a new call once the current context
permits it. `invoke` remains the simpler command that starts a fresh call each time.

Receipt JSON includes method/code IDs, execution ID, checkpoint generation,
state, result, diagnostic, and optional waiting effect. Results distinguish a
void result (`null`) from a guest null (`{"kind":"null"}`), and encode Int64
values as decimal strings. Passing a method ID obtained from symbol search avoids
ambiguous names. `static:` arguments are resolved against current storage on each
CLI call; if a static is reassigned, replay must use the originally captured
`object:ID` argument rather than retargeting the request.

Receipts, argument references, and returned object references are part of the
universe checkpoint and GC roots. They survive unrelated later executions and
append-only code installation. The initial limit is 10,000 receipts; automatic
eviction is not implemented, because forgetting an ID could permit a duplicate
mutation. Native futures, credentials, and sockets remain outside the checkpoint.
Storage errors require reloading the latest committed checkpoint. Stale callers
are rejected before executing, and an unsaved in-memory result cannot be returned
as a committed receipt after a failed save.

These receipts cover managed invocation within one universe, not distributed
execution ownership or exactly-once remote effects. Model delivery still uses
the separate effect/receipt protocol and never resubmits an uncertain request.
Compilation requests use their own universe-scoped service identities.
Installation uses the captured artifact and receipt protocol above.

The acceptance fixture invokes `OpenAIHarness` with a deterministic provider
that receives actual symbol-search and CIL-inspection output. The completed
source response survives checkpoint/restore, compiles against the exported view,
installs through the native CLI, and mutates the same Counter across separate
processes. A repeated invocation request returns the same result/execution before
a new ID performs the next mutation. Live model inference is deliberately absent
from recurring tests.
Run it with `cargo test -p renraku-vm --test compiler scripted_codegen`.

The [persistent coding agent](agent.md) now exposes these tools through a bounded
text/JSON protocol, retaining actual assistant actions, frozen inputs, tool
results, and reference roots in linked managed objects. Stable turn IDs connect
compiler, installation, and invocation receipts. Its deterministic provider tests
model-selected actions and lost-result recovery; actual autonomous model
acceptance still needs a foreground subscription run. Unknown inference outcomes
continue to reconcile receipts without automatic resubmission.
