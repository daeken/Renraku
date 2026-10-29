# Renraku initial CIL execution profile

The interpreter runs the custom CoreLib and Roslyn-generated
simulation in this repository. Compatibility with arbitrary third-party
assemblies is outside its scope.

## Compilation and loading

The guest compiler uses Roslyn from the .NET 10 SDK with C# 12 syntax, release
optimization, and deterministic output. CoreLib compiles with no assembly
references; the guest compiles with only that CoreLib as a reference. The same
simulation also compiles against the host BCL for differential tests under
CoreCLR.

`tools/Compiler` also compiles arbitrary source files against an explicitly
supplied Renraku core and emits a captured build bundle. It runs on the host CLR;
the resulting DLL still passes this loader's supported-profile checks. It does
not install new assemblies into a running universe. See [external compilation](compilation.md).

The Rust loader reads PE32 and PE32+ containers, the CLI header, compressed
metadata tables, string and blob heaps, signatures, and tiny or fat method
bodies. It requires managed IL-only assemblies. References resolve among the
explicitly supplied assemblies by assembly name, namespace, type name, member
name, and complete supported method signature. Duplicate assembly names and
ambiguous member bindings are rejected. Signed assembly identity and binding
redirects are not implemented.

Each load creates fresh logical identities and immutable method implementation
records. Assembly tokens resolve through mappings to logical type, field, and
method IDs. Active frames pin code version IDs. Checkpoints save these mappings
with the assembly bytes and reparse using the saved identities during recovery.
A fresh load outside recovery creates new identities.

Loading validates supported signatures and opcodes, branch boundaries, operand
indices, member bindings, and static versus instance accesses. Execution checks
stack limits, slot and receiver types, return shapes, and instruction budgets.
This is not a complete static CIL verifier or an isolation boundary for hostile
code.

## Values and objects

Executable storage types are Int32, UInt32, Int64, UInt64, Boolean, Char, reference
types, and enums with Int32 storage. Signed and unsigned integers share bit
representations. Boolean stores and returns accept zero and one. Null is
distinct from every object identity. Char slots use Int32 stack values restricted
to unsigned 16-bit units.

Ordinary classes support inheritance, constructors, declared fields, and
assignment of derived references to base slots. Allocation initializes instance
fields, including inherited fields, to zero or null. Abstract types, value types,
arrays, and reference classes with explicit or sequential physical
layouts cannot be allocated. Primitive CoreLib struct and enum metadata can be
loaded without enabling arbitrary value type execution.

Static fields have VM-owned storage initialized to zero or null and retain object
references across successive executions and checkpoint recovery. Explicit type
initializers are rejected until their resumable state is implemented.

Floating point, small integer storage other than Boolean and Char, native integers,
arbitrary structs, boxing, arrays, pointers, byrefs, generics, interfaces,
virtual definitions, exception handling, and arbitrary external capabilities
are unsupported. The explicit local model-call capability is documented in
[the effect contract](effects.md).

## Supported instructions

IL byte offsets remain byte offsets after decoding.

| Instructions | Behavior |
| --- | --- |
| `nop`, `ldnull` | Advance or push null |
| `ldstr` | Load and intern an exact UTF-16 literal from the assembly's user string heap |
| `ldc.i4.m1`, `ldc.i4.0` through `ldc.i4.8`, `ldc.i4.s`, `ldc.i4`, `ldc.i8` | Push exact integer bits |
| `ldarg.0` through `ldarg.3`, `ldarg.s`, `ldarg`; `starg.s`, `starg` | Read or replace a typed argument |
| `ldloc.0` through `ldloc.3`, `ldloc.s`, `ldloc`; `stloc.0` through `stloc.3`, `stloc.s`, `stloc` | Read or replace a typed local |
| `dup`, `pop` | Duplicate or discard the top value |
| `add`, `sub`, `mul` | Wrapping arithmetic on matching integer stack types |
| `conv.i8` | Sign extend Int32 or preserve Int64 |
| `br`, `br.s` | Branch to an instruction boundary |
| `brfalse`, `brfalse.s`, `brtrue`, `brtrue.s` | Branch on an integer or reference |
| `beq`, `bne.un`, and their short forms | Compare matching integers or references and branch |
| `bge`, `bgt`, `ble`, `blt`, their unsigned forms, and all short forms | Compare matching integers and branch |
| `ceq` | Compare matching integers or references |
| `cgt`, `clt`, `cgt.un`, `clt.un` | Compare matching integers; `cgt.un` also supports reference comparisons involving null |
| `call` | Invoke a managed method |
| `callvirt` | Null check and invoke a nonvirtual instance method |
| `newobj` | Allocate an object and invoke its constructor |
| `ldfld`, `stfld` | Read or replace an instance field after receiver and type checks |
| `ldsfld`, `stsfld` | Read or replace a static field |
| `ret` | Complete a call and produce its result |

Two non-null references have no exposed ordering; ordering comparisons between
them are rejected. The supported `callvirt` behavior handles calls Roslyn emits
for ordinary nonvirtual instance methods.

Other opcodes fail during loading. Incompatible operand types fail during
execution with a method name and CIL offset. These are VM diagnostics; catchable
guest exceptions are future work. A failed instruction preserves its original
frame operands and continuation.

## Execution and validation

One VM represents one universe with one active execution context.
The [suspension-first BCL direction](suspension.md) describes future asynchronous
host delivery and multi-context scheduling. Those features and C# async/await
remain unsupported in this execution profile.
Completed or faulted contexts can be replaced by starting another execution
while retaining the heap and static storage. The default managed frame limit
is 1024 and is adjustable.

Every successful instruction ends at an inspectable safe point. Calls,
constructor allocation, result production, and returns publish complete
transitions. Budget exhaustion pauses the context; another `Vm::run` call resumes
it. A fault retains the failing stack. Native Rust frames never represent
suspended guest calls.

The effectful intrinsics are `Renraku.Model.Complete(string) -> string` and
`Renraku.Model.Chat(ModelBinding, ChatMessage) -> string` in Renraku.CoreLib.
Complete captures a single user prompt with the local default binding; Chat
captures a capability/model binding and an ordinary linked sequence of
system/user/assistant messages. Calls require host opt-in, remove their arguments
into immutable outbox data, and install an explicit `AwaitEffect` continuation. Execution returns
`RunOutcome::Waiting` until delivery commits both a response on the evaluation
stack and a runnable continuation. This transition creates no callee frame.
Restoration disables new model calls until the host opts in again; it can still
inspect a waiting context without dispatching anything.

## Managed strings

Strings are immutable heap objects with ordinary object identities and exact
UTF-16 payloads. Length and indexing count UTF-16 units, following
[C# string semantics](https://learn.microsoft.com/en-us/dotnet/csharp/fundamentals/strings/).
Embedded NULs, surrogate pairs, and unpaired surrogates are retained without
conversion to Rust strings.

The custom CoreLib explicitly declares Length, the Char indexer, two-argument
Concat, and equality/inequality operators as InternalCall methods. The loader
accepts only their exact names and signatures in Renraku.CoreLib, alongside the
explicit model capability above. Other InternalCall methods are rejected.
These bounded, pure string operations
complete in a single guest call instruction; they leave no native frame to
checkpoint. Creating strings through ordinary constructors is still unsupported.

Concat treats null arguments as empty. Equality compares exact units, with null
equal only to null. Negative or out-of-range indices, null receivers, and strings
larger than one million units produce explicit VM faults. Failed calls preserve
their original operands and do not publish partial allocations.

Literal interning is scoped to the universe. Its registry is a durable root, so
literal identity survives recovery and later executions. Unreachable temporary
concatenations can be reclaimed by the ordinary tracing collector.

The CLI lists type and method metadata, runs a parameterless static method, and
prints a paused context with retained objects and static storage. Its budget is
an instruction count. Persistent runs checkpoint automatically, and `resume`
restores the complete universe without external assembly files. See
[checkpoint storage and recovery](checkpoints.md).

The supported semantics follow [ECMA-335, sixth edition](https://www.ecma-international.org/wp-content/uploads/ECMA-335_6th_edition_june_2012.pdf).
Focused instruction tests and Roslyn/CoreCLR differential cases cover loops,
signed and unsigned comparisons, overflow, 64-bit precision, aliasing and cycles,
constructors, and recursion. Corruption tests exercise malformed instruction
streams and mutations of emitted assemblies.
