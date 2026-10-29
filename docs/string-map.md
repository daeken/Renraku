# Managed string map

`guest/StringMap.cs` builds as a separate `Renraku.Collections` module. Its
`Renraku.Collections.StringMap` stores keys, values, and links in ordinary managed
objects. Install it append-only against a universe's saved core; it does not
replace CoreLib or the existing `Renraku.Library` decimal module. No host
dictionary, new intrinsic, opcode, capability, or checkpoint representation owns
the collection. `guest/StringMapHarness.cs` is a separate test module, not part
of the production library.

## Contract

Create a non-null map with `new StringMap()`. It exposes instance methods
`Get(string key)`, `Set(string key, string value)`, `Remove(string key)`, and
`Count()`. Static methods `EntryLimit()`, `KeyUnitLimit()`, and `ValueUnitLimit()`
return 64, 128, and 4096. These are fixed bounds, rather than capacity hints.
Null receivers retain the runtime's ordinary terminal VM fault behavior.

Keys compare by exact, case-sensitive UTF-16 units, independent of culture or
normalization. The differential oracle uses CoreCLR's
[`StringComparer.Ordinal`](https://learn.microsoft.com/en-us/dotnet/api/system.stringcomparer.ordinal?view=net-10.0).
Empty keys, embedded NUL, surrogate pairs, and lone surrogate units are allowed;
null keys reject. Values may be null or empty and otherwise have at most 4096
UTF-16 units. Storage keeps the original string references, with no conversion,
normalization, or recursive copying.

`StringMapResult` declares readonly Success, Found, Changed, Value, Error, and
Count fields. Count is a historical observation at that completed operation,
not a live property of later map state.

| Operation | Successful result |
| --- | --- |
| Get existing key | Success/Found true, Changed false, Value is stored value |
| Set new key | Success/Changed true, Found false, Value is inserted value |
| Set existing key with different value content | Success/Found/Changed true, Value is new value |
| Set existing key with equal value content | Success/Found true, Changed false, Value retains the original stored reference |
| Remove existing key | Success/Found/Changed true, Value retains the removed value |

Successful Error is `none`. Missing Get/Remove return `missing`, with all flags
false and Value null. A stored null value has Success/Found true, so absence
does not depend on Value. Errors `null_key`, `key_too_long`, `value_too_long`, and
`full` also return false flags, null Value, and unchanged Count. Validation
precedes lookup: invalid keys reject first, then Set checks its value bound.
These failures do not imply knowledge of whether a matching entry exists.

New entries prepend a node. Replacing a value preserves node and key identities;
equal-content Set preserves the value identity too. Removing a node unlinks it
and frees a capacity slot. At full capacity, replacing an existing entry remains
valid. Enumeration order, resizing, generic values, indexers, and interoperability
with the host BCL's Dictionary are outside this API.

## Persistence and partial mutations

Each operation uses ordinary CIL safe points. No native stack, iterator, pointer,
or host collection is checkpoint state. Keys, values, nodes, partial constructors,
arguments, and return objects follow the existing managed tracing rules.

A whole mutator is not an atomic transaction. For example, an insertion can
pause after linking its initialized node and before increasing Count. A removal
can pause between unlink and decrement. Completed method boundaries promise the
map's count/chain invariant; native inspection of an active execution can observe
its intermediate fields. One-context exclusion prevents another guest operation
from running against that partial update. The checkpoint owns both the partial
graph and the continuation that finishes it. General scheduler concurrency will
need an explicit synchronization contract.

Use existing durable invocation or HTTP request IDs when a host caller needs
retry semantics. Resume the same pinned execution; do not start the mutator
again after losing its reply. Terminal receipts retain results and typed argument
roots. Returned removed values remain live while a result/receipt owns them;
unreachable nodes are collectable. Holding a remembered logical ID alone does
not keep an object alive. VM faults do not roll back earlier CIL instructions.

## Compile callers and serve grouped visits

Install the module using the [durable compiler service](compiler-service.md):

```sh
target/debug/renraku-vm compile-once target/site.db target/site-compiler.db \
  maps/build target/compiler/Renraku.Compiler.dll Renraku.Collections guest/StringMap.cs
target/debug/renraku-vm install-compilation target/site.db target/site-compiler.db \
  maps/build maps/install
target/debug/renraku-vm export-reference target/site.db \
  Renraku.Collections.StringMap > target/maps.view.json
```

This assumes an existing universe, a built compiler driver, and a fresh module
name/action IDs. Exporting StringMap includes its own-module result/node closure
and symbol-to-logical-ID bindings. Existing module bodies and objects keep their
IDs. Callers select this frozen view, rather than loading facade types as live
replacement objects.

`guest/examples/GroupedWebsite.cs` compiles against separate Website, IntegerText,
and StringMap views. After [installing IntegerText](integer-text.md), export those
three views, compile the source as `Generated.GroupedWebsite`, install its captured
artifact, and publish `GroupedWebsite::Handle`. Publication and installation keep
their existing independent receipts/authority rules.

The handler stores its map in `GroupedWebsite.Counts`. A POST /visit body is a
group key, with empty body selecting the default group. It reads/parses that
group's saved decimal count, updates it through StringMap, and invokes the
original Website handler to retain global visits/history. The page formats
numeric totals through IntegerText and does not echo arbitrary group keys.
GET /page displays the default group. The original global 64-visit limit stays
in place. Invalid group keys and invalid/limited stored counts return explanatory
HTTP-v1 status-200 pages without recording a visit. Accepted retries replay their
original page/execution after later map mutations; changed request data conflicts.

The [dependency-aware exporter](reference-sets.md) can now freeze a reference set
for GroupedWebsite and its foreign Counts declaration. A compiled caller can use
the same existing map while each facade retains its own module and logical IDs.
The legacy single-view Rust API remains module-scoped. Cross-module cycles and
ambiguous names reject; foreign types are never inlined into the wrong module.

## Verification

Four library tests compare a 441-operation stateful trace with CoreCLR's ordinal
dictionary and cover capacity reuse, validation, empty/null strings, normalization
distinctions, NUL, and lone surrogates. They verify shared value identities,
equal-content no-op updates, removed-node collection and retained receipt values.
Eight mutations recover after every instruction with GC, original logical
identities, pinned receipts, and a typed map argument after replacing its static
root. Replays execute no additional instructions. Native process recovery needs
no SDK.

The website test installs both libraries into an existing universe, compiles and
publishes the handler, then pauses after its group link changes and before the
original visit counter changes. GC and a fresh native HTTP process resume the
same execution, finish once, record a second request, and replay the original
response. Further checks cover multiple/default groups, invalid keys, one-context
exclusion, conflicting request bodies, and pre-library response receipts. Native
serving needs neither compiler ledger nor SDK. Checkpoint format 7, original
fixtures, and existing demo universes are unchanged.
