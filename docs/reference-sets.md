# Frozen reference sets across modules

`Program::reference_set` exports compiler metadata for acyclic declaration
dependencies spanning installed modules. A page with a `StringMap` field can now
be referenced by a compiled caller without loading implementation DLLs as
compiler references or making facade tokens into live types.

## Wire and identity contract

`export-reference` accepts 1–32 logical type IDs or unambiguous names, including
roots from different modules. It merges roots per module and follows supported
field, parameter, result, and base-type declarations. Ordinary recursion within
one module works. Cross-module cycles reject before export; no partial set is
published. References used only inside method bodies are not declaration
dependencies and do not enter the facade closure.

An isolated view keeps its exact original format-1 envelope and schema hashing.
The existing Rust `reference_view` method remains the single-module API and still
rejects foreign non-core declarations; use `reference_set` for those declarations.
The original core image remains the bootstrap compiler reference.

A complete set has `format: 1`, `kind: "referenceSet"`, `setHash`, and `views`.
Each member remains scoped to its original logical module. Views without foreign
declarations stay format 1. A view with foreign declarations uses format 2 and
adds `dependencies` to `schemaJson`. Each dependency binds its module ID/name,
exact schema hash, and required logical type IDs. Its classes are declared in
the dependency's facade, never copied into the referring assembly.

Each schema hash is SHA-256 of its exact UTF-8 schemaJson string. The set hash is
SHA-256 of the compact serialized array of view envelopes, using their established
field order, in canonical dependency-first order. Ordering uses a depth-first
walk of module names/IDs in ordinal order, visiting dependencies first. Input
root permutations and duplicates produce the same set. Parsers normalize outer
JSON formatting and view order before checking the set hash; schemaJson itself
remains exact hashed content. Heap changes and checkpoint generations do not
change metadata hashes. Logical identities from another universe cannot
substitute, even when its original DLL bytes match.

Sets contain at most 16 modules, each with at most 32 roots, 256 closure types,
8192 bindings, and 1 MiB declaration text. The set envelope is bounded to 4 MiB.
Unknown/missing modules, changed dependency hashes, missing type bindings,
duplicate module identities/names, and ambiguous cross-module type names reject.
Extern aliases for ambiguous names and cyclic facade generation remain future
work. Other original exporter shape limits still apply: classes and supported
primitive signatures, ASCII identifiers, unsigned version-zero modules, no
properties/events, literal/default fields, custom attributes, or generic/value
type roots. Unsupported contracts are not erased to make compilation succeed.

Original independent format-1 view files and captured results remain usable.
They keep their established validation behavior; a new set applies the stricter
combined dependency/name checks. This distinction preserves earlier captures,
including isolated views that were supplied independently to raw installation.

## Compile, install, and recover

The existing `--reference-view` option accepts either one legacy view or a whole
set. Multiple options may supply distinct modules, with at most 16 expanded
views. Export shared/overlapping roots together into one set; duplicate module
inputs reject rather than silently rewriting frozen dependencies.

The external compiler verifies core/schema/set hashes and emits metadata-only
dependency facades before their dependents. A facade references the core and its
declared dependency closure. The caller receives the full facade set. No
implementation image is substituted for an unrepresentable declaration.
Facade images remain marked reference assemblies and cannot execute in Renraku.

Compilation capture retains all exact views, sources, core, options, and pinned
compiler identity before Roslyn runs. Terminal artifacts retain every facade
image/view and their hashes. Installation validates the complete set against
the live registry before appending code. Missing or retargeted bindings and
failed checkpoint commits leave the saved code/heap/frames unchanged. Published
compilations replay without the SDK; installation retains their provenance, and
invoke-once receipts still resume or return the original execution/result.
Checkpoint format stays 7. A reference set freezes declarations, not static
values or arbitrary object graphs.

For an existing grouped-visit universe and built compiler driver:

```sh
target/debug/renraku-vm export-reference target/site.db GroupedWebsite > target/groups.set.json
target/debug/renraku-vm compile-once target/site.db target/site-compiler.db \
  groups/read/build target/compiler/Renraku.Compiler.dll Generated.ExistingGroups \
  --reference-view target/groups.set.json Caller.cs
target/debug/renraku-vm install-compilation target/site.db target/site-compiler.db \
  groups/read/build groups/read/install
target/debug/renraku-vm invoke-once target/site.db groups/read/invoke ExistingGroups::Read 10000
```

`Caller.cs` can use the page's existing map directly:

```csharp
public static class ExistingGroups
{
    public static string Read()
    {
        var result = GroupedWebsite.Counts.Get("red");
        return result.Success ? result.Value : "missing";
    }
}
```

The agent's compile action now accepts selected roots across modules within its
existing eight-root/source bounds and captures their complete dependency set.
Describe retains the original primary schemaHash/output shape and adds
referenceSetHash/dependencies when needed. The full result remains under the
existing tool-output bound. There are no new model actions or host capabilities.

## Evidence

Tests use actual C# metadata for mutually dependent modules and ambiguous names.
They cover root merging, deterministic ordering/hashes, legacy wire compatibility,
wrong-universe/missing/changed dependencies, direct SDK set inputs including
reordered/pretty JSON, reversed facade input files, and bounded agent
describe/compile/install/invoke/finish across modules. These are deterministic
fixtures, not autonomous-model evidence.

The grouped-website fixture exports its saved Counts declaration, compiles a
caller that gets and updates the existing map, checks failed-install rollback,
and replays published compilation with a missing compiler path. Fresh SDK-free
invocation processes retain the same execution/result and original map/type/field
IDs after GC. There is still exactly one map object; prior HTTP responses and
global visit state remain intact after the caller's managed mutation.
