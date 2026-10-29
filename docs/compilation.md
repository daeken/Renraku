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

The manifest records compilation inputs; the current VM CLI still takes the two
DLL paths directly and does not validate the build manifest. Use the core copy
inside the bundle. `Program::load` checks its metadata, bindings, and supported
instruction profile. Roslyn can emit CIL that Renraku explicitly rejects, such
as division or generics; successful compilation alone does not imply execution
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
source and DLL files. The build manifest and copied source remain external
build provenance; they are not automatically stored in the guest checkpoint.

Five integration tests cover deterministic compilation from different source
directories, multi-file input, exact hashes/options, structured errors, absent
host BCL references, invalid inputs and limits, preserved existing outputs,
loader rejection of unsupported emitted CIL, and nested-frame persistence.
The recovery test deletes the source and bundle after checkpointing, then
resumes and invokes the same code again with retained static state. It also
reopens the checkpoint using the standalone native CLI with an empty PATH.

## Runtime compilation and installation

The tool supplies the external compiler seam. It is not yet a managed
`Compile` intrinsic, a durable compiler request, or code installation into a
running universe. Current loading creates a fresh Program and logical bindings.

The next compilation capability should capture source bytes, exact reference
images, compiler identity/options, and a stable request ID, then return a
versioned artifact or diagnostics. Its guest call can suspend through portable
frames while the host compiler runs. Compiler process memory stays transient.

Installing a returned DLL is a separate validated transaction. It must preserve
logical identities where required, publish new implementation versions and
dispatch/route bindings atomically with code and managed state, and retain old
code for active frames. Type-shape changes and active-frame migration need their
own policy; a new DLL must not silently reinterpret existing objects or stacks.
The loader is not a full CIL verifier or a hostile-code isolation boundary.

## Website workload

A small website is a useful next integration workload after asynchronous host
delivery and scheduler work. The host owns HTTP sockets and bounded parsing;
managed CIL handlers own routes, application state, and response values. The
initial service should bind locally. No ASP.NET compatibility is required for
that proof, and HTTP APIs are not implemented yet.

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
