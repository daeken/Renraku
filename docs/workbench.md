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
target/debug/renraku-vm install target/codegen.db \
  target/generated-counter/Generated.Counter.dll target/counter.view.json
target/debug/renraku-vm invoke target/codegen.db Generated.Entry::Run 1000 \
  static:Renraku.Guest.Simulation::Saved
```

`SaveObject` creates a Counter containing 19. The generated caller returns **26**.
Invoke it again in another process and it returns **33**, using the same object
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
deleted. It reports `sourceAvailable: false`: C# source is captured in compiler
bundles, but not yet attached to the runtime code registry. The tool does not
pretend decompiled text is original source.

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

`invoke` starts a new call only when the current context permits it, then runs
within the explicit instruction budget. Arguments are `i32:N`, `i64:N`, `null`,
`object:ID`, or `static:Type::Field`. Object/static references are taken from the
current universe, and VM argument types are checked. Model calls are disabled
for this command; installed code does not acquire network permission. Resume a
budget-paused call with the ordinary `resume` command.

The acceptance fixture invokes `OpenAIHarness` with a deterministic provider
that receives actual symbol-search and CIL-inspection output. The completed
source response survives checkpoint/restore, compiles against the exported view,
installs through the native CLI, and mutates the same Counter across separate
processes. Live model inference is deliberately absent from recurring tests.
Run it with `cargo test -p renraku-vm --test compiler scripted_codegen`.

The remaining model integration should expose bounded read tools first, retain
the model's source and tool transcript, and give compile/install/invoke actions
explicit request identities and recovery semantics. An interrupted invocation
must resume its existing execution rather than start the mutation again. Unknown
external outcomes must continue to reconcile receipts without automatic retries.
