# Managed hexadecimal integers

`guest/HexIntegerText.cs` builds as the separate append-only `Renraku.HexText`
module. The static `Renraku.Text.HexIntegerText` class uses ordinary CIL shifts,
bitwise operations, branches, calls and immutable strings. Its parse methods
return the existing `Renraku.Text.UInt32ParseResult` and `UInt64ParseResult`
classes from `Renraku.UnsignedText`. Their original module/type/field/method IDs
are preserved; hex installs no replacement result declarations or core bodies.

| Method | Result |
| --- | --- |
| `FormatUInt32(uint value)` | Canonical lowercase ASCII hex, 1..8 UTF-16 units |
| `FormatUInt64(ulong value)` | Canonical lowercase ASCII hex, 1..16 UTF-16 units |
| `ParseUInt32(string text)` | Existing `UInt32ParseResult` |
| `ParseUInt64(string text)` | Existing `UInt64ParseResult` |
| `MaxParseUnits()` | 64 |

Formatting emits zero as `0`, otherwise no leading zeros or prefix. All 32/64
bits participate. UInt32/UInt64 maxima format as `ffffffff`/`ffffffffffffffff`,
even though the VM stores those values as signed I32/I64 -1. There are no padding,
culture, format-string, alternate-radix or casing options.

Parsing accepts ASCII hex digits with an optional `0x` or `0X` prefix, exactly
`(?:0[xX])?[0-9A-Fa-f]+`. Leading zeros and mixed ASCII case are accepted. The
64-UTF-16-unit cap includes the prefix. Signs, whitespace, NUL, separators,
non-ASCII digits/letters and surrogate units reject. Only `0x`/`0X` is a prefix:
for example `0b11` is the ordinary hex value b11, and `1e2` is ordinary hex 1e2.
Inputs retain their exact UTF-16 units.

The reused readonly result fields are `Success`, unsigned `Value`, `Error`, and
`Position`. Failure Value is always zero; success Position is input length.
Positions count zero-based UTF-16 units, including the prefix:

| Error | Position and precedence |
| --- | --- |
| `none` | Success, input length |
| `null` | -1 |
| `empty` | 0 |
| `too_long` | 64; length preflight precedes every unit scan |
| `invalid_digit` | First invalid ASCII-hex unit; prefix alone reports 2 |
| `overflow` | First digit whose unsigned prefix exceeds the selected width |

After preflight, the first invalid digit or overflowing prefix wins. An invalid
unit immediately after a fitting maximum reports invalid_digit; a previously
overflowing prefix followed later by `X` reports overflow. Guards run before a
left shift could discard high bits. Parsing uses at most 64 units, formatting
at most 16 output units, and regression cases finish within 5,000 instructions.
Errors are result data, not guest exceptions. These pure multi-instruction
methods use the ordinary budget/checkpoint contract; no host formatter, intrinsic,
capability, array, generic/byref API or checkpoint field is added.

## Install through frozen results

First explicitly install [Renraku.UnsignedText](unsigned-text.md) if absent.
With a built compiler driver and idle existing universe, use fresh request IDs:

```sh
target/debug/renraku-vm export-reference target/site.db \
  Renraku.Text.UInt32ParseResult Renraku.Text.UInt64ParseResult \
  > target/unsigned-results.view.json
target/debug/renraku-vm compile-once target/site.db target/site-compiler.db \
  hex/build target/compiler/Renraku.Compiler.dll Renraku.HexText \
  --reference-view target/unsigned-results.view.json guest/HexIntegerText.cs
target/debug/renraku-vm install-compilation target/site.db target/site-compiler.db \
  hex/build hex/install
target/debug/renraku-vm export-reference target/site.db \
  Renraku.Text.HexIntegerText > target/hex.set.json
```

The result facade is scoped to its original unsigned module. HexText's return
declarations create a foreign dependency, so its export is a
[dependency-aware reference set](reference-sets.md): the unsigned result facade
is emitted before the hex facade. Later callers supply the whole set through
`--reference-view`; agent callers select the installed HexIntegerText root.
Missing/changed bindings reject. Facade tokens never recreate live types, and
implementation DLLs are not used as production compiler references.

Existing decimal methods, CoreLib, page state and invocation receipts stay in
their original modules. Code installation is append-only and distinct from
execution; normal typed roots and invoke-once identities own recovery. Once
installed, the coding agent can discover/read/describe the API under its existing
tool policy, without new authority.

## Evidence

`tests/hex_text.rs` compares 280 formats and 1,736 parses with identical C# on
CoreCLR. Canonical formatting agrees with invariant unsigned hex formatting;
valid inputs agree with unsigned hex TryParse after the optional prefix is
removed. An independent host BigInteger radix-prefix oracle supplies error
classification/positions, rather than reusing the guest's overflow guard.
Cases cover maxima/high bits, both prefix cases, zero/leading zeros, first-error
precedence, 64/65-unit input limits, signs, whitespace, NUL and lone surrogates.
Six operations restore each instruction with GC, retaining exact inputs, partial
strings, portable helper frames and foreign result constructors. Fresh native
processes finish paused formatting/parsing without SDK tools.

The external compiler test starts with the original unsigned module, a retained
decimal parse receipt and Counter=19. It compiles/installs HexText against frozen
result references, verifies the original type/method IDs, then exports the
three-module result/hex/Counter set for a caller. The caller pauses inside the
original UInt64ParseResult constructor before fields initialize. After GC and
deleting source/view/bundle files, a fresh native process without SDK tools
finishes and returns `ffffffffffffffff/14`, with the same Counter=20 and a result
whose original type stores I64 -1. Retained source, old decimal receipt, new
installation/invocation receipts and typed roots survive. Replay after another
execution/GC preserves the result/execution and keeps Calls=1 and Counter=20.
The loader/oracle fixture compiler separately emits a test DLL; that fixture is
not the production frozen-reference installation path. No inference is needed.
