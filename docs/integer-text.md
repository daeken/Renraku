# Managed decimal integers

`guest/IntegerText.cs` is the first ordinary managed library module. It builds as
`Renraku.Library`, separately from CoreLib, and can be compiled and installed
append-only in an existing universe. Its formatting and parsing use CIL calls,
loops, arithmetic, immutable strings, and ordinary result objects. No new native
intrinsics, interpreter opcodes, capabilities, or checkpoint fields are involved.
The original CoreLib and existing object/code identities remain in place.

## API and bounds

The static class `Renraku.Text.IntegerText` exposes:

| Method | Result |
| --- | --- |
| `FormatInt32(int value)` | Canonical signed ASCII decimal string, at most 11 UTF-16 units |
| `FormatInt64(long value)` | Canonical signed ASCII decimal string, at most 20 UTF-16 units |
| `ParseInt32(string text)` | `Renraku.Text.Int32ParseResult` |
| `ParseInt64(string text)` | `Renraku.Text.Int64ParseResult` |
| `MaxParseUnits()` | 64 |

Formatting emits `0` for zero, otherwise no leading zeros, and `-` only for a
negative value. There is no culture, grouping, padding, alternate radix, unsigned
formatting, or format-string argument. The implementation retains a nonpositive
magnitude, so signed minimum values need no unrepresentable negation.

Parsing accepts exactly `[+-]?[0-9]+`, with a maximum of 64 UTF-16 units including
the sign. Leading zeros and negative zero are accepted. Whitespace, NUL, Unicode
digits/signs, fractions, grouping, exponents, and surrogate units are rejected.
Input strings keep their exact units. Both signed limits are accepted; values
outside the selected width fail before a CIL multiplication/subtraction could
wrap. Parsing examines at most 64 units and formatting emits at most 20 UTF-16
units. The VM's normal instruction budgets can pause either operation.

Both result classes declare readonly `Success`, `Value`, `Error`, and `Position`
fields. `Value` has the requested integer width and is zero on every failure.
`Position` is a zero-based UTF-16 offset; successful results report the input
length. Error strings and precedence are:

| Error | Position and behavior |
| --- | --- |
| `none` | Success; position is input length |
| `null` | Null input; position -1 |
| `empty` | Empty input; position 0 |
| `too_long` | Length exceeds 64; position 64; no digit scan |
| `invalid_digit` | First non-ASCII digit after the optional sign; a sign alone reports the end offset 1 |
| `overflow` | First digit whose signed prefix exceeds the target width |

After the null/empty/length checks, scanning stops at the first invalid unit or
overflowing prefix. For example, an overflowing number followed by `X` reports
the earlier overflow. Errors are result data, not guest exceptions or VM faults.
The result types and all helper frames/strings are ordinary managed state and
participate in tracing and instruction checkpoint recovery.

The host differential fixture compares formatting with
[`ToString(CultureInfo.InvariantCulture)`](https://learn.microsoft.com/en-us/dotnet/api/system.int64.tostring?view=net-10.0).
For the declared strict lexical subset, successful parsing agrees with
[`TryParse` using `AllowLeadingSign` and invariant culture](https://learn.microsoft.com/en-us/dotnet/api/system.int64.tryparse?view=net-10.0).
An independent host BigInteger prefix oracle supplies failure positions and
overflow classifications. This is a deliberately bounded API, rather than a
claim of the full .NET parsing surface or its treatment of every trailing unit.

## Install and use against saved state

With an existing demo universe and a built compiler driver, install the library
using the existing durable compiler service. These examples use fresh action IDs
and assume no module named `Renraku.Library` is already installed:

```sh
dotnet build tools/Compiler --output target/compiler
target/debug/renraku-vm compile-once target/site.db target/site-compiler.db \
  integers/build target/compiler/Renraku.Compiler.dll Renraku.Library guest/IntegerText.cs
target/debug/renraku-vm install-compilation target/site.db target/site-compiler.db \
  integers/build integers/install
target/debug/renraku-vm export-reference target/site.db \
  Renraku.Text.IntegerText > target/integers.view.json
target/debug/renraku-vm export-reference target/site.db \
  Renraku.Guest.Website > target/site.view.json
target/debug/renraku-vm compile-once target/site.db target/site-compiler.db \
  numbered/build target/compiler/Renraku.Compiler.dll Generated.IntegerWebsite \
  --reference-view target/integers.view.json --reference-view target/site.view.json \
  guest/examples/IntegerWebsite.cs
target/debug/renraku-vm install-compilation target/site.db target/site-compiler.db \
  numbered/build numbered/install
target/debug/renraku-vm web-publish target/site.db numbered/publish IntegerWebsite::Handle
target/debug/renraku-vm web-serve target/site.db @published 127.0.0.1:8080 10000
```

`IntegerWebsite` renders the existing `Website.Visits` and `Website.History`.
An empty `POST /visit` preserves the original one-visit behavior. An optional
decimal body is the expected current count (0..64); a matching count permits one
visit. Invalid input, a stale count, or the visit limit returns an explanatory
page without a visit mutation. `GET /page` reads the same saved state. HTTP-v1
still returns status 200 for managed handler validation pages. Client request
IDs, exact request binding, one-context exclusion, code pinning, and immutable
response replay retain the [existing contract](website.md).

An accepted retry returns its original response even if the count or publication
has since changed; the precondition is evaluated only in that request's original
execution. Reusing an ID with a different body conflicts at the durable inbox.
Use distinct client IDs for different requests. Publication and installation
remain separate, with their existing receipts and authority policies.

The coding agent can discover `IntegerText`, inspect its saved source/CIL and
declarations, and compile callers using its frozen type view after installation.
Existing universes do not automatically acquire the module. The bootstrap core
stays unchanged; an old module/body is never silently replaced. The fixture
compiler also emits `Renraku.Library.dll` for direct loader tests, but normal
compiler/install provenance comes from the captured service above.

## Verification

`tests/integer_text.rs` covers 286 boundary/seeded formatting cases and 1,188
parsing cases, including both minima/maxima, overflow, signs/zeros, length limits,
whitespace, NUL, Unicode digits and lone surrogates. Six representative operations
checkpoint and restore after every instruction with GC, retaining original
method/code/execution identities, frames, arguments, partial strings, and result
objects. Fresh native processes finish paused formatting/parsing without the SDK.

The website test installs the library into a universe with an existing visit,
exports its reference closure, compiles and publishes the page, and resumes an
accepted numeric request in seven-instruction quanta with GC/reopens. It checks
exclusion, errors, lost-result replay, conflicting bodies, old pre-library
receipts, and independent native TCP delivery without the compiler ledger/SDK.
No inference or external formatting service is needed.
