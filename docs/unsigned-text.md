# Managed unsigned decimal integers

`guest/UnsignedIntegerText.cs` builds as the separate append-only module
`Renraku.UnsignedText`. Its static `Renraku.Text.UnsignedIntegerText` API uses
ordinary CIL loops, unsigned arithmetic, immutable strings and result objects.
Existing CoreLib, Renraku.Library and other saved module bodies stay unchanged;
install this module explicitly in an existing universe.

| Method | Result |
| --- | --- |
| `FormatUInt32(uint value)` | Canonical ASCII decimal, 1..10 UTF-16 units |
| `FormatUInt64(ulong value)` | Canonical ASCII decimal, 1..20 UTF-16 units |
| `ParseUInt32(string text)` | `Renraku.Text.UInt32ParseResult` |
| `ParseUInt64(string text)` | `Renraku.Text.UInt64ParseResult` |
| `MaxParseUnits()` | 64 |

Formatting emits zero as `0` and otherwise no leading zeros or sign. All bits
participate, including values above the corresponding signed maximum. There are
no culture, grouping, radix, padding or format-string options.

Parsing accepts exactly `[+]?[0-9]+` within 64 UTF-16 units including the optional
plus sign. Leading zeros are accepted. Every minus sign, including `-0`, rejects;
this intentionally narrower syntax differs from some .NET unsigned parsing
inputs. Whitespace, NUL, non-ASCII signs/digits, fractions, grouping, exponents,
and surrogate units reject. Input strings retain their exact units.

Readonly result fields are `Success`, unsigned `Value`, `Error`, and `Position`.
The selected width's full range is accepted, including 4294967295 and
18446744073709551615. Guards reject overflow before multiplication or addition
could wrap. Failure Value is always zero. Position is a zero-based UTF-16 offset;
success reports the input length.

| Error | Position and precedence |
| --- | --- |
| `none` | Success, input length |
| `null` | -1 |
| `empty` | 0 |
| `too_long` | 64; length preflight takes precedence over any unit scan |
| `invalid_digit` | First non-ASCII digit after an optional plus; plus alone reports 1; minus reports 0 |
| `overflow` | First digit whose unsigned prefix exceeds the selected width |

After preflight, scanning stops at the first invalid unit or overflowing prefix.
An overflowing prefix followed by `X` reports the earlier overflow. Errors are
managed result data rather than guest exceptions or VM faults. Pure operations
use normal instruction safe points and can pause under the VM's budget. The
64-unit input and 20-unit output bounds limit guest work; the regression cases
finish within 5,000 instructions.

Unsigned values still use I32/I64 storage bits. For example, UInt64 maximum is
stored as I64 -1. The inspector and native invocation arguments expose those
bits; the managed formatter supplies unsigned decimal meaning. No host formatter,
new capability, native collection, checkpoint field or byref/generic API is used.
The runtime's [fixed-width conversions](cil-profile.md) include zero extension
and narrowing, while floating/native/checked conversion remains unsupported.

## Install and compile callers

Use the existing durable compiler service with fresh request IDs:

```sh
dotnet build tools/Compiler --output target/compiler
target/debug/renraku-vm compile-once target/site.db target/site-compiler.db \
  unsigned/build target/compiler/Renraku.Compiler.dll Renraku.UnsignedText \
  guest/UnsignedIntegerText.cs
target/debug/renraku-vm install-compilation target/site.db target/site-compiler.db \
  unsigned/build unsigned/install
target/debug/renraku-vm export-reference target/site.db \
  Renraku.Text.UnsignedIntegerText > target/unsigned.view.json
```

Select the new module's type roots when compiling a caller; multiple module
roots can share a [frozen reference set](reference-sets.md). The exporter includes
the ordinary parse-result declarations. Installation and invocation retain their
existing receipts, original IDs and separate boundaries. An old saved core or
signed-library module is never replaced to acquire these APIs. The coding agent
can search/describe/read the module once installed; no new tool action is needed.

## Verification

`tests/unsigned_text.rs` compares 278 boundary/seeded formats and 1,182 parses
against the identical C# running under CoreCLR. The host checks canonical format
with invariant unsigned ToString, valid syntax with unsigned TryParse, and errors
with an independent arbitrary-precision prefix oracle rather than the guest's
overflow guards. Inputs include both unsigned maxima, signed high-bit boundaries,
overflow precedence, signs, zeros, 64/65-unit limits, NUL and lone surrogates.
Six operations checkpoint/restore after every instruction with GC, preserving
exact inputs, nested helper frames, intermediate strings, result objects and
logical identities. Native processes finish paused operations with no SDK/tools.

`tests/compiler.rs` starts with an existing signed library and Counter at 19,
installs the unsigned module append-only, and exports three module facades. An
external caller parses UInt64 maximum, increments the same Counter once and
formats `18446744073709551615/20` through both managed libraries. It pauses in a
foreign parser frame, checkpoints with GC, deletes external sources/views/bundles,
then resumes in a fresh native process with SDK tools absent from PATH. Original
type/field/method/code/object IDs, the unsigned result's raw -1 storage, retained
source and installation receipts survive. Invocation replay after another
execution/GC returns the same result/execution and keeps Calls at 1 and Counter
at 20. These tests use no inference or model authority.
