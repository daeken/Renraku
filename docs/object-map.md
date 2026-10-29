# Managed object map

`guest/ObjectMap.cs` is the separate append-only `Renraku.ObjectCollections`
module. `Renraku.Collections.ObjectMap` stores a linked graph in ordinary CIL.
Install it explicitly against the saved core. CoreLib, StringMap, decimal/hex
text and older installed bodies do not change. `guest/ObjectMapHarness.cs` is a
separate test module. There is no host dictionary, new intrinsic, opcode,
capability, value kind or checkpoint representation.

## Contract

A non-null ObjectMap exposes `Get(string key)`, `Set(string key, object value)`,
`Remove(string key)`, and `Count()`. `EntryLimit()` is 64 and `KeyUnitLimit()` is
128 UTF-16 units. These are fixed limits. Keys compare by exact case-sensitive
ordinal UTF-16 units: empty strings, NUL, pairs and lone surrogates are valid;
null keys reject. Culture and normalization have no role.

Values are nullable managed references, including strings, other maps and
mutable class instances. There is no value-size bound or implicit boxing of
scalars. The map neither copies graphs nor changes allocation domains. Existing
managed/transient store barriers still apply. A transient target cannot become
persistent by being passed to a map operation. Null receivers use ordinary
terminal VM faults.

`ObjectMapResult` has readonly `Success`, `Found`, `Changed`, `Value`, `Error`,
and `Count` fields. Value is declared object; use supported reference casts to
recover its installed class. Count records that completed operation's count.

| Operation | Successful result |
| --- | --- |
| Get existing key | Success/Found true, Changed false, stored Value |
| Set new key | Success/Changed true, Found false, inserted Value |
| Set existing key with a different reference | Success/Found/Changed true, new Value |
| Set existing key with the same reference | Success/Found true, Changed false, original Value |
| Remove existing key | Success/Found/Changed true, removed Value |

Successful Error is `none`. Missing Get/Remove return `missing`. Invalid keys
return `null_key` or `key_too_long`, before lookup. Adding a new key at capacity
returns `full`; replacing an existing key remains valid. Every failure has false
flags, null Value and current Count. Stored null has Found true and differs from
absence. Removing an entry frees capacity.

Reference identity governs value replacement: two separate strings containing
the same units are different values. This differs from StringMap's content
comparison. Replacement and no-op Set preserve the old node/key IDs. Only a
no-op also preserves the value reference. New nodes prepend; iteration/indexers,
generics, arrays and host Dictionary interoperability are outside the API.

## Roots, mutable targets and partial execution

Readonly result fields do not freeze referenced objects. A later permitted
mutation through another alias is visible through the same result.Value. Both
shared references and cycles remain ordinary managed reachability. A removed
node can be collected while its returned value remains rooted by a result or
invocation receipt. A historical object ID alone retains nothing.

Mutators have multiple instruction safe points. An insertion may pause after
linking a node and before increasing Count; a removal may pause before decrement.
Completed method boundaries promise chain/count invariants. Checkpoints retain
the intermediate graph together with its original continuation. One-context
exclusion prevents another guest call from entering the partially changed map.
VM faults preserve earlier instructions rather than rolling them back.

Keep the same invocation or HTTP request ID when recovering a lost response.
Receipts pin execution/code, typed arguments and results; they are not a reason
to restart a mutator. No new waiting operation or host authority is introduced.

## Frozen callers and an object-backed page

Install ObjectMap using the [compiler service](compiler-service.md), then export
its frozen root. A caller can combine it with saved Counter/Website declarations
through a [reference set](reference-sets.md). Facades stay scoped to the original
module; they are never implementation DLL references or new live classes.

`guest/examples/ObjectWebsite.cs` compiles against ObjectMap, IntegerText, Counter
and Website. Its explicit Initialize(Counter) method records an existing Counter
under `counter` only when that key is absent. It never replaces that Counter.
The handler retrieves/casts the same instance and stores the last accepted POST
body under `last-body`, so its map holds heterogeneous object references.

GET / or /page with an empty body renders the Counter, original global visits,
map count and original history. POST /visit accepts an optional signed decimal
delta in -10..10; an empty body means +1. It records last-body, adds to the same
Counter and calls the original Website handler. Invalid deltas, unsupported
routes, missing/wrong Counter, full map and visit limit produce explanatory
HTTP-v1 status-200 pages. Counter arithmetic retains the existing Add method's
wrapping behavior. Accepted requests replay their original page after subsequent
changes; changed data with the same ID conflicts. Publication still requires
separate host authority, and the handler receives no model capability.

## Verification

Four library tests compare a 454-operation trace of identical C# against CoreCLR
and an independent ordinal dictionary/reference-identity oracle. They cover
nullable values, equal-content distinct strings, boundary/invalid keys, capacity
reuse, shared/cyclic values, mutable result targets, stable node/key identities,
removed-node collection and receipt-owned removed values. Eight mutations recover
after every instruction with GC, including replacement of the map's static root
while a typed invocation argument retains the old map. Native recovery pauses
between node linking and Count and finishes/replays without SDK tools.

A fifth test installs the library through captured external compilation into a
universe with an existing Counter=19 and visit receipt. Its separately compiled
page pauses after linking last-body, before Count/Counter/visit changes. A fresh
SDK-free TCP process resumes it to 26, then another request reaches 23; old HTTP
responses remain immutable. A three-facade frozen caller retrieves/casts the same
Counter, pauses before isinst, then resumes in a fresh native process to 24 once.
A later HTTP call reaches 25; old caller replay returns 24, leaving Calls=1 and
Counter=25. Exactly one original map/Counter, logical type/field/method/code/node/
key IDs, byte-identical reference sets, source and install/invoke provenance
survive GC/restarts without a compiler ledger. Format 7, every legacy fixture
and existing demo universes are unchanged.
