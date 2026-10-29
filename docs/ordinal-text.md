# Bounded managed ordinal search

`guest/OrdinalText.cs` builds as the separate append-only `Renraku.OrdinalText`
module. `Renraku.Text.OrdinalText` searches exact UTF-16 units using ordinary
CIL loops and existing string Length/indexer primitives. It adds no host search
intrinsic, normalization, culture, regex, array, generic/byref API or checkpoint
field. CoreLib and saved module bodies remain unchanged.

| Method | Result |
| --- | --- |
| `IndexOf(string text, string needle, int startIndex)` | First ordinal match at/after startIndex |
| `StartsWith(string text, string needle)` | Match only at index 0 |
| `EndsWith(string text, string needle)` | Match only at text.Length - needle.Length |
| `MaxTextUnits()` | 256 |
| `MaxNeedleUnits()` | 32 |

All search methods return an ordinary `Renraku.Text.StringMatchResult` with
readonly `Success`, `Found`, `Index` and `Error` fields. Valid no-match results
have Success=true, Found=false, Index=-1 and Error=`none`. A match has both flags
true and its zero-based UTF-16 index. Invalid inputs have both flags false,
Index=-1 and one of these errors, in this precedence order:

1. `null_text`
2. `null_needle`
3. `text_too_long` (over 256 units)
4. `needle_too_long` (over 32 units)
5. `start_out_of_range` (IndexOf only, startIndex outside 0..text.Length)

Bounds are checked before searching, even if an empty needle could otherwise
match. Empty needles match startIndex for IndexOf, 0 for StartsWith, and
text.Length for EndsWith. Empty text is valid; IndexOf permits startIndex equal
to text.Length. A nonempty needle longer than the remaining text is a valid
no-match, rather than an input error. StartsWith/EndsWith have no start parameter.

Comparison is case-sensitive and ordinal by individual UTF-16 units. NUL,
surrogate pairs and lone surrogates are ordinary data. A match may begin on a low
surrogate; composed and decomposed text are not normalized. Inputs retain their
exact units and identities. Overlapping candidates are examined in increasing
index order, returning the first match.

IndexOf uses a bounded naive scan, O(text units * needle units), with at most
7,200 unit comparisons under these limits. StartsWith/EndsWith compare at most
32 units. The worst-case near-match regression uses 118,422 guest instructions;
all 2,913 oracle cases finish within the tested 150,000-instruction ceiling.
These are pure multi-instruction CIL methods; a smaller budget pauses the saved
helper frames and resumes them later. They do not reserve a host operation or
promise whole-method atomicity. Result allocation/constructor frames use the
same safe points and GC rules as other managed objects.

## Install and use

With an idle existing universe and built compiler driver, use fresh request IDs:

```sh
target/debug/renraku-vm compile-once target/site.db target/site-compiler.db \
  ordinal/build target/compiler/Renraku.Compiler.dll Renraku.OrdinalText \
  guest/OrdinalText.cs
target/debug/renraku-vm install-compilation target/site.db target/site-compiler.db \
  ordinal/build ordinal/install
target/debug/renraku-vm export-reference target/site.db \
  Renraku.Text.OrdinalText > target/ordinal.view.json
```

Compile callers against this frozen view, together with any other selected
module roots in a [reference set](reference-sets.md). Its result declaration stays
in the same module. The coding agent can search/read/describe the installed API
and select its root under the existing eight-root limit; no new tool or authority
is granted. Installation, invocation and publication retain their separate
contracts. Existing universes acquire the module only by explicit installation.

## Evidence

`tests/ordinal_text.rs` compares 2,913 cases against actual CoreCLR ordinal
IndexOf/StartsWith/EndsWith after applying this API's bounds/error contract. The
cases cover empty/null/oversized inputs, start-index precedence, overlaps,
case-sensitive matches, NUL, normalization differences, surrogate pairs and
lone units, seeded strings and the worst near-match bound. Six operations
restore each instruction with GC, preserving inputs, loops, result constructors,
method/code/execution IDs and shared string arguments. Three fresh native
processes resume saved operations and replay their original result objects
without SDK tools or changes to exact input units.

The external compiler test saves a Counter=19 and a named-root string containing
NUL/a lone surrogate before installing the library. A caller compiled through the
ordinal/Counter facade set searches that same saved string and pauses inside the
MatchAt loop. After GC and deleting external source/view/bundle files, an SDK-free
native process resumes, returns index 5 and updates the original Counter once to
24. Retained source, installation/invocation receipts, original object/type/field/
method/code IDs, result type and exact string/root survive. Replay after later
work/GC keeps Calls=1, Counter=24 and the original execution/result. These are
local regression fixtures; no model or external search provider is involved.
