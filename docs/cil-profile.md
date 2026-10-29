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
| `and`, `or`, `xor` | Bitwise operations on matching I32 or I64 stacks; preserve operand width |
| `not` | Bitwise complement of one I32 or I64; preserve operand width |
| `neg` | Two's-complement negation of one I32 or I64; signed minimum remains unchanged |
| `shl`, `shr`, `shr.un` | Shift an I32/I64 value using an I32 count; retain value width |
| `div`, `div.un`, `rem`, `rem.un` | Signed or unsigned division/remainder on matching Int32 or Int64 stack types |
| `conv.i8` | Sign extend Int32 or preserve Int64 |
| `conv.u8` | Zero extend Int32 or preserve Int64 bits |
| `conv.i4`, `conv.u4` | Preserve Int32 or truncate Int64 to its low 32 bits, producing Int32 storage |
| `conv.i1`, `conv.i2` | Keep low 8/16 bits and sign-extend to I32 |
| `conv.u1`, `conv.u2` | Keep low 8/16 bits and zero-extend to I32 |
| `br`, `br.s` | Branch to an instruction boundary |
| `switch` | Consume an I32 index and select a validated jump-table target, or fall through |
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

Integer `switch` consumes one I32 index, compares its raw unsigned value with
the table length, and selects that zero-based entry. Negative values and indices
at/above the length fall through to the next instruction. A zero-entry table
still consumes the index and falls through. Each signed 32-bit target delta is
relative to the end of the complete encoded table, rather than its own entry.
Duplicate targets and backward jumps are supported. Targets must be instruction
boundaries inside the same method, and fallthrough must exist. See
[ECMA-335 III.3.66](https://www.ecma-international.org/wp-content/uploads/ECMA-335_6th_edition_june_2012.pdf#page=418).

Renraku bounds each table to 4,096 entries. The decoder checks the declared count
against remaining method bytes before allocation, uses checked relative-offset
arithmetic, and rejects truncated/oversized tables and invalid targets. The table
is immutable code metadata reconstructed from the saved assembly; it adds no
guest array, value tag or checkpoint field. Execution requires an I32 index;
I64/null/object values fault with their original frame/stack intact. Checkpoint
validation removes the index and follows every target and fallthrough, including
stack merges. Dispatch is one ordinary pure CIL step, not a host wait.

`guest/IntegerSwitch.cs` compiles unchanged against both cores. A 1,432-case
CoreCLR comparison covers dense signed/unsigned/offset/enum dispatch, I64 return
values, seeded/boundary indices, plus independent DynamicMethods for naked
zero-entry/duplicate/backward tables. A test-only PE variant replaces the reserved
raw-method bodies, retaining metadata and method-body lengths. Thirty-two cases restore
every instruction with GC and original bindings; a nested dispatch mutates the
same Counter 19 to 30, then 7, while the first receipt keeps 30 without repeating
a mutation. Seven fresh SDK-free native resumes retain saved switch indices and
receipt identities/results, including the original Counter mutation. String-switch
lowering and general pattern matching have no supported contract in this milestone.

Two non-null references have no exposed ordering; ordering comparisons between
them are rejected. The supported `callvirt` behavior handles calls Roslyn emits
for ordinary nonvirtual instance methods.

Other opcodes fail during loading. Incompatible operand types fail during
execution with a method name and CIL offset. These are VM diagnostics; catchable
guest exceptions are future work. A failed instruction preserves its original
frame operands and continuation.

Integer division truncates toward zero. Signed remainder is zero or has the
dividend's sign. The unsigned forms interpret all 32 or 64 operand bits as
unsigned and return the same-width storage bits. Every zero divisor faults.
Signed minimum divided by -1 faults; Renraku also deliberately faults for signed
minimum remainder by -1. ECMA permits the latter fault, and CoreCLR on the test
host produces it for both widths. This policy is independent of host CPU and
Rust overflow settings. See ECMA-335 III.3.31–32 and III.3.55–56 in the
[primary specification](https://www.ecma-international.org/wp-content/uploads/ECMA-335_6th_edition_june_2012.pdf#page=382).

Arithmetic faults follow the existing durable VM diagnostic contract, including
method/offset and the original operand stack. They are terminal invocation
receipts, rather than catchable guest exceptions. Retrying a saved fault does
not execute the call again or repeat a preceding mutation. Pure arithmetic uses
ordinary instruction safe points and does not suspend for host work. Floating
point, native integers, checked arithmetic opcodes, and
exception handling remain unsupported.

Bitwise instructions operate on all 32 or 64 storage bits. Signed and unsigned
source declarations use the same instructions and produce identical bits.
Binary operands must have the same width; `not` returns its operand width.
There is no arithmetic overflow or host wait. References and mixed-width binary
operands fault without consuming their original stack or continuation. This
implements the fixed-width subset of ECMA-335 III.3.3, III.3.52–53 and III.3.67,
[Table 5 and instruction definitions](https://www.ecma-international.org/wp-content/uploads/ECMA-335_6th_edition_june_2012.pdf#page=352).
Native integer operations remain unsupported.

Unchecked `neg` preserves the operand width and negates its integer bits modulo
2^32 or 2^64. Zero stays zero; signed minimum stays signed minimum. It does not
take the signed division/remainder overflow-fault path. References fault with
the original operand and continuation intact; floating/native operands and
checked arithmetic remain unsupported. The implementation uses explicit wrapping
negation independently of Rust debug/release overflow checks. See ECMA-335
III.3.50/Table 3 in the [primary negation specification](https://www.ecma-international.org/wp-content/uploads/ECMA-335_6th_edition_june_2012.pdf#page=402).

CIL has no separate unsigned negation opcode. C# variable unary minus on uint
widens to Int64 before negation; an explicit signed cast supplies the unchecked
UInt64 bit-pattern case. `guest/IntegerNegation.cs` compiles unchanged against
both cores, with 584 boundary/seeded CoreCLR comparisons. Twelve cases checkpoint
each instruction with GC and original IDs. A durable nested Counter update
recovers 19 to -19, later returns to 19, then replays its first -19 receipt without
another mutation. Four SDK-free native resumes retain minimum-value results and
pinned execution IDs. These pure operations add no capability or checkpoint field.

`guest/IntegerBitwise.cs` compiles unchanged against both cores in a separate
test module. Its 4,128-case CoreCLR oracle covers signed/unsigned declarations,
high bits, zero/all-one patterns, boundaries and seeded operands. All 16 direct
methods checkpoint each instruction with GC. A nested flag update changes an
existing Counter 19 to 29 through OR/NOT/AND/XOR; after another invocation reaches
84, replay of the first receipt returns 29 and leaves 84 in the original object.
Fresh native processes finish paused operands and replay receipts without SDK
tools. The CLI's observed checkpoint generation can advance when it fences a
retry; pinned execution, method/code, arguments and result remain the same.

Shift instructions take an I32 or I64 value followed by an I32 count. Renraku
defines the effective count as the unsigned count bits AND 31 for I32 values or
AND 63 for I64 values, including negative/high counts. Left shift inserts zeros
and discards high bits; signed right shift replicates the sign bit; `shr.un`
inserts zeros. Counts 32/64 therefore act as zero at the corresponding width,
and -1 acts as 31/63. Result width follows the value. Invalid operands fault with
their original stack and continuation. Pure shifts do not suspend.

ECMA-335 III.3.58–60/Table 6 specifies the in-range operations and leaves results
unspecified when the count reaches/exceeds the value width. The masking above is
an explicit portable Renraku policy, not a claim that ECMA mandates it. Roslyn
also emits count masks for C# shifts; both those masks and raw CIL obey this
policy. See the [primary shift specification](https://www.ecma-international.org/wp-content/uploads/ECMA-335_6th_edition_june_2012.pdf#page=410).
Native/count-I64/checked operations remain unsupported.

`guest/IntegerShifts.cs` compiles unchanged against both cores. Its 5,570-case
matrix compares emitted C# and raw CIL with boundary/high/negative counts; 231
mix cases additionally agree with the host BCL's rotate operation. Test-only
raw fixture methods replace just Roslyn's three mask bytes with NOPs, retaining
offsets/metadata; independent host DynamicMethods record naked CoreCLR shifts.
Only specified raw counts must match CoreCLR on every host; out-of-range results
are pinned to Renraku's own policy. Recovery cuts every instruction across 80
raw cases with GC, and a nested rotate/XOR mutation keeps its original object,
execution and receipts through later updates and SDK-free native resumes.

Fixed-width conversions consume one I32/I64 stack value and produce the target
width. No signedness tag is stored: `conv.i4` and `conv.u4` produce identical low
32 bits. `conv.i8` sign-extends an I32; `conv.u8` interprets those 32 bits as
unsigned before widening. Both preserve all bits of an I64. For example, I32 -1
widens to I64 -1 with `conv.i8` and to 4294967295 with `conv.u8`. These are
unchecked, pure guest instructions; narrowing does not fault on lost high bits.
C# chooses the appropriate emitted sequence for its source cast: an unchecked
negative `int` to `ulong` uses `conv.i8`, while `uint` to `ulong` uses `conv.u8`.
Reference operands fault with their original stack/continuation intact. Native,
floating and checked conversions remain unsupported. See ECMA-335
III.3.27 and Table 8 in the
[primary conversion specification](https://www.ecma-international.org/wp-content/uploads/ECMA-335_6th_edition_june_2012.pdf#page=378).
`guest/IntegerConversions.cs` and 1,764 real CoreCLR comparisons cover narrowing,
both extensions, high bits and conversion sequences. Checkpoint stack validation
tracks the converted width; recovery tests cut every instruction with GC and
retain the original method/code/execution identities.

The smaller unchecked conversions also accept I32/I64 operands and produce I32
stack values. Signed forms truncate to 8/16 bits then sign-extend; unsigned forms
truncate then zero-extend. For example, low bits ff yield -1 with `conv.i1` and
255 with `conv.u1`; low bits ffff yield -1 with `conv.i2` and 65535 with `conv.u2`.
This enables integer signatures containing casts and existing Char return/local/
field slots. It does not enable Byte/SByte/Int16/UInt16 signatures, fields or
locals. Exact UTF-16 surrogate units remain valid Char values; no Unicode scalar
normalization is introduced. Reference failures preserve the original frame.

`guest/SmallIntegerConversions.cs` compiles unchanged against both cores and
compares 1,680 actual C# casts, including Char returns, with CoreCLR. Forty cases
restore every instruction with GC and original IDs. A durable Char-store method
retains d800, later stores dfff, then replays its first d800 result without
changing dfff or repeating either Counter mutation. Six fresh SDK-free native
resumes cover all four instructions and high/low surrogate Char results. A loader
fixture explicitly still rejects byte storage signatures. CoreLib is unchanged.

`guest/IntegerArithmetic.cs` compiles unchanged against both CoreLibs. Its
2,312-case CoreCLR matrix covers all four operators at both widths, signed
truncation/remainder signs, high unsigned bits, zero, and minimum/maximum values.
Recovery tests retain pending operands, nested Euclidean and decimal-digit loops,
a cyclic managed object, and terminal faults after a mutation across individual
instruction checkpoints, GC, failed saves, and SDK-free native process resumes.

## Execution and validation

One VM represents one universe with one active execution context.
The [asynchronous host driver](async-delivery.md) can drive independent universes
on one executor. Multi-context scheduling inside a universe and C# async/await
remain unsupported; see the [suspension-first BCL direction](suspension.md).
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

The separately installable [managed integer text library](integer-text.md) builds
bounded signed decimal formatting/parsing from these CIL primitives and string
operations. Its loops, calls, and result objects use the normal portable frames;
it adds no InternalCall or instruction. Existing core images remain unchanged.
The separate [managed string map](string-map.md) likewise uses ordinary classes,
calls, and fields. Its bounded mutators can pause with partial updates; method
boundaries, one-context exclusion, and durable receipts retain their distinct roles.
The separate [unsigned decimal module](unsigned-text.md) builds UInt32/UInt64
text from fixed-width conversion and unsigned division/remainder, with explicit
overflow results and no native formatter.
The separate [hexadecimal module](hex-text.md) uses bitwise/shift loops and the
existing unsigned result types; its foreign constructor frames recover normally.
The [ordinal search module](ordinal-text.md) uses bounded UTF-16 comparison loops
and ordinary result objects, with no host search intrinsic or normalization.

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
