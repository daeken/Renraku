# External C# compilation

Roslyn runs in a host .NET process and emits a DLL against Renraku's core
reference assembly. Renraku consumes that DLL as CIL. The compiler's own CLR,
collections, threads, and internal state do not need to run inside the guest.
This follows the existing test compiler and is now available for arbitrary
source files through `tools/Compiler`.

## Compile and run a guest

The .NET 10 SDK builds and runs the compiler tool. It uses the Roslyn assemblies
shipped with that SDK, with no NuGet compiler package. First build the sample
core reference if it is not already available:

```sh
dotnet run --project tools/GuestCompiler -- . target/guest
```

Compile your own UTF-8 source files into a new bundle directory:

```sh
dotnet run --project tools/Compiler -- \
  --core target/guest/Renraku.CoreLib.dll \
  --output target/compiled-hello --name Example.Hello \
  guest/examples/Hello.cs
cargo run -p renraku-vm -- run \
  target/compiled-hello/Renraku.CoreLib.dll \
  target/compiled-hello/Example.Hello.dll Example.Hello::Run
```

The result is **36** after **11 guest instructions**. The compiler reports
structured JSON with success, output directory, and diagnostics. Give multiple
source filenames after the options to compile them into one assembly. These
must have distinct filenames, including case. Assembly names are dot-separated
ASCII identifiers; `Renraku.CoreLib` is reserved. Existing outputs are never
replaced; choose another directory for each build.

The output bundle contains:

- The emitted guest DLL and the exact copied `Renraku.CoreLib.dll` reference.
- Exact input bytes in `sources/`, with logical filenames independent of the
  source's original directory.
- `renraku-build.json`, format 1, recording Roslyn/driver assembly identities and
  hashes, language
  and compilation options, input order, filenames, byte lengths, and SHA-256
  hashes for sources, core, and guest assembly.

The guest is compiled as a C# 12 release DLL, with deterministic output, checked
arithmetic disabled, unsafe code disabled, and runtime metadata version
`v4.0.30319`. It references only the supplied Renraku core, without inheriting
the host framework assemblies. It does not evaluate guest projects, restore guest
packages, run analyzers/source generators, or execute the emitted code
inside the compiler. Diagnostics include code, severity, logical file, and
one-based line/column. Reports bound diagnostic count and message length.

Bounds are 32 sources, 1 MiB per source, 4 MiB total source bytes, and 16 MiB
each for the core reference and emitted DLL. These are input/output bounds, not
a general resource-isolation guarantee. Publication writes a sibling staging
directory and renames it into place after compilation; normal errors remove
staging. Process termination can leave an unpublished staging directory.

The manifest records compilation inputs. Raw `run` and `run-persistent` still take
DLL paths directly; use the core copy inside the bundle. The workbench's
`install-once` captures and validates the manifest and every declared file, then
retains that build with its installation receipt. `Program::load` checks metadata, bindings, and supported
instruction profile. Roslyn can emit CIL that Renraku explicitly rejects, such
as checked conversions or generics; successful compilation alone does not imply execution
support. See [the current CIL profile](cil-profile.md).

## Persistence after loading

Choose an unused checkpoint path:

```sh
cargo run -p renraku-vm -- run-persistent \
  target/compiled-hello-universe.db \
  target/compiled-hello/Renraku.CoreLib.dll \
  target/compiled-hello/Example.Hello.dll Example.Hello::Run 10 5
cargo run -p renraku-vm -- resume target/compiled-hello-universe.db
```

The first command pauses after ten instructions; the second returns 36. The
checkpoint captures the original assembly bytes, code/metadata identities,
frames, statics, and heap together. Restore needs neither Roslyn/.NET nor the
source and DLL files. Raw DLL loading does not attach source provenance;
`install-once` stores the captured build and exposes original source inputs through
`read-code`, even after the bundle is deleted. See the [workbench](workbench.md).

Five integration tests cover deterministic compilation from different source
directories, multi-file input, exact hashes/options, structured errors, absent
host BCL references, invalid inputs and limits, preserved existing outputs,
loader rejection of unsupported emitted CIL, and nested-frame persistence.
The recovery test deletes the source and bundle after checkpointing, then
resumes and invokes the same code again with retained static state. It also
reopens the checkpoint using the standalone native CLI with an empty PATH.

## Runtime compilation and installation

The tool supplies the external compiler seam. The
[durable compiler service](compiler-service.md) now captures pinned requests and
publishes artifacts or diagnostics with recovery. It is not yet a managed
`Compile` intrinsic or a guest suspension operation. The
[runtime workbench](workbench.md) now supports reference views and append-only
code installation into an existing universe, preserving its logical bindings.

The service captures source, core/reference views, compiler identity/options,
and a stable request ID before execution; the terminal artifact captures emitted
reference-image bytes as well. A future guest call can suspend through portable
frames while the host compiler runs. Compiler process memory stays transient.

Installing a returned DLL is a separate validated transaction. It must preserve
logical identities where required, publish new implementation versions and
dispatch/route bindings atomically with code and managed state, and retain old
code for active frames. Type-shape changes and active-frame migration need their
own policy; a new DLL must not silently reinterpret existing objects or stacks.
The loader is not a full CIL verifier or a hostile-code isolation boundary.

## Reference assemblies for live runtime types

The user identified the next compiler bridge: runtime-defined classes need not
have conventional implementation DLLs, while Roslyn still needs metadata for
their shapes. Export frozen reference-assembly images from the runtime's type
and member registry. They describe the compilation contract; they do not become
the authority for live types or supply their runtime implementations.

The target flow is:

```text
frozen runtime schema -> reference images + binding manifest
source + those images -> external Roslyn -> compiled CIL artifact
artifact + pinned bindings -> validated runtime installation
```

Preserve logical module/assembly scope so identically named types from different
modules remain distinct. Each exported symbol must map back to its existing
TypeId, FieldId, or MethodId. Emitted metadata tokens are image-local locators,
not logical identities. The initial installer verifies the view against the
current registry and resolves compiled TypeRef/MemberRef scopes and supported
signatures through that registry's existing bindings. The native host supplies
the view alongside the implementation; a durable compile/artifact protocol must
also bind those exact reference bytes to its request. The installer must not
import the facade's TypeDefs as newly allocated runtime classes.

The durable schema must retain compiler-facing metadata as well as execution
tables: accessibility, properties/indexers and their accessors, constants,
attributes, and other supported signature relationships. A list of executable
fields and methods alone cannot reconstruct that contract. Export the transitive
signature/base-type closure and reject unsupported shapes explicitly.

Capture deterministic reference bytes, content hashes, schema/API revision,
and the symbol-to-logical-ID manifest with each compilation request/artifact.
Installation must check compatibility with that pinned contract before changing
code or bindings. Cache views by metadata content, independently of ordinary
heap checkpoint generations and implementation-body versions. Heap writes and
body-only edits should not invalidate unchanged reference metadata. Visibility
and logical assembly boundaries must remain deliberate compiler policy.

Reference images can be supplied in memory with MetadataReference.CreateFromImage;
disk DLLs are a transport/artifact choice. Roslyn's reference emission uses
EmitMetadataOnly=true and IncludePrivateMembers=false. Bootstrap generation may
use valid C# declaration scaffolding, with direct metadata emission available
for shapes that C# source cannot represent. Treat the resulting artifacts as
compile-only and reject them as executable implementation inputs.

Compiling callers of existing types is distinct from compiling a new body *on*
an existing type. A reference does not let a normal C# partial declaration extend
an already compiled class. Member-body compilation needs an explicit owning-type
context (including needed nonpublic members), visibility policy, and installation manifest that maps generated
definitions back to the intended existing IDs. Shadow declarations must never
silently create duplicate types or replace unrelated members.

Today the registry is reconstructed from saved implementation images/bindings.
Live type authoring will also require durable schema definitions independent of
those original images. The initial exporter, compiler view inputs, and append-only
installer are implemented for a deliberately bounded declaration subset; see the
[workbench guide](workbench.md) for commands and exact limits.

A local SDK experiment emitted a /refonly Live.Counter contract and compiled a
consumer that accesses its field and method, referencing only that facade and
Renraku.CoreLib. This proves the compiler boundary, not live-registry export or
installation. The tested SDK required well-formed C# stub declarations; a bare
non-abstract method declaration produced CS0501, so an exporter should not rely
on bodyless C# declarations being accepted.

See Microsoft's [reference assembly contract](https://learn.microsoft.com/en-us/dotnet/standard/assembly/reference-assemblies)
and Roslyn's [reference output design](https://github.com/dotnet/roslyn/blob/main/docs/features/refout.md).
The [partial-type rules](https://learn.microsoft.com/en-us/dotnet/csharp/programming-guide/classes-and-structs/partial-classes-and-methods)
describe the compilation boundary for ordinary C# partial declarations.

## Website workload

The first [persistent localhost website](website.md) is implemented. The host owns
HTTP sockets and bounded parsing; managed CIL handlers own application routing,
state, and immutable response strings. The prototype uses a dedicated serial host
and the existing invocation inbox, preserving the one-context limit. General
guest networking APIs and ASP.NET compatibility remain future work.

Ingress needs its own durable inbox: request identity and accepted work must
commit before handler execution, and application updates plus response data
must commit before sending the response. A connection cannot survive process
death. Client retries need stable request IDs and an explicit deduplication
policy; an HTTP response write cannot by itself prove delivery to the browser.
The first proof should be a page backed by persistent state, recovery during a
request, and a defined retry that does not duplicate its state mutation.

Later, external compilation can produce a new handler version. Publishing a
route update should affect new calls while existing frames retain their pinned
implementation. This composes the compiler seam with the
[suspension and scheduling direction](suspension.md), without requiring Roslyn
to be hosted inside Renraku first.

Roslyn's official [compiler model](https://learn.microsoft.com/en-us/dotnet/csharp/roslyn-sdk/compiler-api-model)
and [Emit API](https://learn.microsoft.com/en-us/dotnet/api/microsoft.codeanalysis.compilation.emit)
describe the parse/bind/emit boundary used by the host tool.
