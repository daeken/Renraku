# Inspect persistent state

The runtime exposes bounded read-only views of one managed object or one stored
static field. These views read heap/storage directly. They do not run constructors,
getters, type initializers, or guest instructions, and do not grant capabilities.
Logical IDs and checkpoint generations tie observations to the existing universe.

## Native CLI

```sh
cargo run -p renraku-vm -- inspect-static target/universe.db Renraku.Guest.Simulation::Saved
cargo run -p renraku-vm -- inspect-object target/universe.db OBJECT_ID 16 64
```

`inspect-static <db> <field-ID-or-Declaring.Type::Field> [string-units=64]`
returns a format-1 JSON view with a `field` row. Its value can supply an object ID
for `inspect-object <db> <object-ID> [max-fields=16] [string-units=64]`.
Qualified selectors use the exact declaring type, including for inherited fields;
logical IDs disambiguate repeated type or field names. Missing/ambiguous fields,
instance fields selected as statics, and literals/constants reject explicitly.
The inspector does not evaluate C# properties.

Both commands restore saved code/state and need neither the SDK nor credentials.
They do not advance the checkpoint generation or create a durable root. They can
observe a paused universe without resuming its frames. Model-call opt-in is not
accepted by these read commands.

## Agent actions

```json
{"op":"inspect_static","field":"Renraku.Guest.Simulation::Saved","stringUnits":64}
```

```json
{"op":"inspect_object","id":"LOGICAL_OBJECT_ID","maxFields":16,"stringUnits":64}
```

Unknown fields, host paths, malformed/missing IDs, and excessive limits return
saved tool errors. Static selectors freeze to their logical field ID; object
selectors freeze to the existing managed object ID. Object inspection passes that
reference to the ordinary CIL Freeze helper, whose arguments and typed
AgentArgument nodes retain it before host execution. GC can therefore run between
Freeze and inspection, even after the original static root is replaced. No new
guest schema or checkpoint format is required.

The actual result is retained in the linked CIL transcript. The object retained
by the inspected turn remains an ordinary root through that turn's argument
nodes. Reference IDs appearing inside a historical view do not independently pin
every observed target forever. A later lookup can report that an unrooted target
has been collected. Host CLI/API reads create no roots of their own.

Reads observe values at execution, not at selector Freeze. Every agent/CLI view
includes its source checkpoint generation. An unappended result lost with a
controller may be re-read from a later checkpoint; a static field may then contain
a different object/value. A committed managed tool result remains historical
transcript data. Inspection introduces no separate read receipt ledger and never
replays a mutation to obtain a value.

## View contract and bounds

All views include `format: 1`, `kind`, `universe`, and `checkpoint`.

| View | Fields |
| --- | --- |
| Object | `objectId`, actual `type`, `schemaVersion`, domain `managed`, field count/returned count, `fieldsTruncated`, bounded `fields`, and limits |
| Static | One `field` row and limits |
| Field row | Logical field `id`, name/`nameTruncated`, declaring `owner`, `declaredSlot`, and current `value` |
| Reference value | `kind: "object"`, logical `id`, actual `type`, and string preview when applicable |
| Primitive value | `kind: "null"`, `kind: "i32"` with an integer, or `kind: "i64"` with an exact decimal string |

`declaredSlot` describes runtime storage (`i32`, `i64`, `boolean`, `char`, or a
reference with its logical type identity). I32/I64 values expose signed storage
bit patterns. Source-language signedness and enum names are not reconstructed;
use the compiler contract from `describe` for those declarations. Boolean and Char
slots remain I32 values with their distinct declared slot. I64 values are strings
so a JSON consumer cannot silently round them to floating point.

Object field rows include inherited declarations, sorted by logical field ID.
Two fields named Value on different declaring types remain separate rows with
distinct IDs/owners. Reference fields return IDs rather than recursively walking
graphs; cycles and sharing do not expand a view indefinitely. There is no field
pagination in this initial contract.

Limits are **1..32 fields**, **0..256 string units**, and **12 KiB of encoded JSON
per view**, leaving room for the agent's 16 KiB result wrapper. Defaults are 16
fields and 64 units. Type/member label prefixes contain at most 128 UTF-8 bytes,
cut at a complete Unicode scalar with `nameTruncated`. Field rows stop before the
JSON budget is exceeded, and `fieldsTruncated` reports omitted rows. Name, field,
and string truncation are independent and explicit. Unsupported transient objects
or references reject rather than becoming durable roots.

## Exact UTF-16 previews

An inspected string, or a string reference in a field, includes `stringPreview`:

```json
{"unitCount":3,"units":[65,0,55296],"previewWellFormed":false,"text":null,"truncated":false}
```

`unitCount` measures the full immutable payload. `units` contains the bounded exact
UTF-16 prefix. `text` decodes that prefix only when well-formed; otherwise it is
null, without replacement characters or changes to the stored string. A prefix
can split a surrogate pair even when the full string is valid, so the flag is
named `previewWellFormed`. Request another unit when needed. A zero-unit preview
still reports the full length and explicit truncation. JSON unit arrays safely
transport lone surrogates through the existing text-only model adapter.

## Embedding and verification

Rust embedders use `Vm::inspect_object`, `Vm::inspect_static`, and
`InspectionLimits`. These methods borrow the VM immutably. `checkpoint` identifies
the VM's base generation; CLI/agent callers restore it immediately before reading.
Embedders viewing an in-memory VM with unsaved changes must retain that distinction
instead of treating the generation as proof those changes committed.

```sh
cargo test -p renraku-vm --lib inspection
cargo test -p renraku-vm --test inspection
cargo test -p renraku-vm --test agent inspect
cargo test -p renraku-openai --lib http_inspection
```

Tests cover shared cycles, inherited hidden fields and declaring owners, exact
large integers, labels/output bounds, surrogate/NUL prefixes, unsupported selectors
and transients, GC and snapshot recovery, and SDK-free CLI delivery without a save.
Agent tests replace a static root and collect after object Freeze, then recover a
lost result while retaining the original object. A lost static observation instead
re-reads the new generation/value, without secretly rooting its discarded result.

The subscription-adapter loopback fixture discovers Saved, inspects the Counter at
19, locates/reads the existing Advance helper, invokes it once with 7, and observes
26 by inspecting the same object's field. No getter, compiler, or installation is
needed. Lost read/mutation results recover across restores and GC, every full HTTP
history matches the managed transcript, and signed-out finished recovery is inert.
These fixtures are scripted regression evidence, not autonomous model evidence.

A [live seven-turn acceptance](acceptance/live-inspection-2026-10-29.md) used
GPT-6.1 Sol to read 19, invoke the existing helper once, and inspect 26 on the same
object across process restarts, without a compiler or getter. Native read-only
views preserved the checkpoint digest/generation; invocation replay retained the
original execution and value, and finished recovery used zero operations.
