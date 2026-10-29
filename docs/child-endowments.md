# Closed child endowments

`renraku_vm::endowment` adds an enforced, opt-in child execution profile and a
separate child-facing tool surface. It permits selected local code/class APIs,
objects, fields and effects, plus existing opaque remote-method handles. It
checks actual logical identities and CIL operations rather than trusting C#
visibility, compiler facades, UUID knowledge or a model prompt.

This first profile is **closed**: compile/install local code and import remote
handles before sealing an idle child. After sealing, code/registry/import
bindings and permissions cannot expand through this API. It is not yet the
existing coding agent in a hostile sandbox, a general dynamic-code compartment,
or a per-reference delegation system. The trusted Rust embedding, operator
workbench, checkpoint files and host credentials remain outside the child-facing
surface. An unsealed Vm retains the existing trusted behavior; child tools reject
universes without a policy.

## Explicit bootstrap and admission

The trusted host creates a dedicated independent universe, loads supported code,
prepares any [remote imports](remote-calls.md), and builds an `Endowment`:

- `types`: permitted local reference-class declarations, including needed bases
  and reference slots in admitted method signatures/locals.
- `methods`: exact code-version IDs permitted for entry, calls and constructors.
- `construct`: classes that may be allocated, together with permitted constructor
  code. A type grant alone does not grant construction or all its members.
- `read` / `write`: exact field IDs, separately for access in either direction.
  Stored instance and static fields use their original declarations, without name-based
  or visibility-based privilege.
- `objects`: existing managed object identities the child may receive/use. This
  does not grant the object's whole reachable graph.
- `effects`: exact `(capability, model)` pairs. Transient host model authorization
  and provider credentials remain separately required and default off on restore.

`endowment::seal` requires a completed idle context, no invocation/effect history,
no owner placement registry and no peer calls. It does not discard accepted or
uncertain work. Trusted bootstrap heap/statics remain, original IDs and lifetime
instruction counters remain, and no parent-universe link is created. The prior
bootstrap result is not exported as a child result. Objects from another owner
remain remote through explicit imports; sealing never copies a parent graph.

Admission pins the complete registry's image bytes and logical bindings, and
separately fingerprints remote import roots/handle identities. Every permitted
method's instructions are checked, including unreachable branches. Calls,
constructors, field operations, casts and string literals must fit the selected
permissions. Reference slots/bases must be explicitly present. Missing private
helpers, fields or static permissions reject admission before publication.

All local child code executes with the same explicit rights. Granting a method
here does **not** give its implementation hidden privilege to read private state
on the child's behalf. Use the opaque remote-method boundary for that service
pattern. Local privileged frames/private implementation compartments are future
work. Field/member rights are uniform within this child, intersected with its
permitted object identities; different rights for two local references to the
same object are not represented by this version.

Raw runtime entry, instruction execution, calls, receiver/argument/result paths,
field/static access and casts check the policy too. Installation/rebinding and
new remote imports reject after sealing. This is not merely a descriptor checked
by one command. Guessed metadata tokens and source-language access modifiers
cannot bypass the logical-ID checks.

## Objects, data and durable allocation provenance

An existing object is usable only when explicitly endowed. Its permitted type
alone does not grant every instance. A field read or method result that would
introduce an unendowed reference faults before placing it on the guest stack;
prior instructions' mutations remain. A cast preserves the same object and does
not expand field/method rights. Managed/transient lifetime barriers still apply.

New permitted local allocations receive durable `(ObjectId, TypeId)` provenance.
Immutable strings are bounded data, with a 4096-UTF-16-unit child limit; string
operations still need their selected methods. Literals embedded in admitted code
permit that literal data. If interning reuses an existing immutable literal ID,
that ID is recorded without replacing it or granting an enclosing private root.
NUL and surrogate units retain their original semantics.

The allocation ledger is preflighted before heap effects and updated with the
same guest step/frame transition. It records IDs as text, not retention edges:
unreachable child-created objects can be collected, while receipt/frame roots
retain their normal meaning. Historical IDs are not made available for reuse.
The 256-record lifetime limit does not reset after collection, new goals or
native restart. Exhaustion rejects before another allocation. Endowed objects
have typed retention roots; their mutable fields are not frozen snapshots.

Canonical format-1 policy/ledger strings live at `renraku.child-v1/policy` and
`owned`, with `object/<ObjectId>` roots. The ledger binds the policy digest;
policy, imports, code, roots, types, frames, receipt references and effect bindings
validate on save/recovery. Hashes are consistency bindings, not credentials or
protection against a privileged checkpoint writer. Metadata strings are host
records, not guest-owned data. In particular, peer intent/ack/reply bookkeeping
neither consumes guest string authority nor gives the child those record objects.

No new checkpoint/value/frame tag, core image or saved module body is added.
Writer format 7 and all original format-1-through-7 fixtures remain intact.

## Effect and reference boundaries

Model request capture checks exact effect bindings before publishing an outbox
record. ModelBinding/ChatMessage traversal checks each object, field and returned
reference too: an endowed head cannot smuggle a private content string or next
node through an intrinsic. Frozen message contents, original receipt ledgers and
uncertain-effect reconciliation keep their existing no-resubmission contract.

A granted remote call still freezes/parks the existing portable frame and uses
host-supplied credentials for explicit delivery. Its original authoritative object
is not copied. Scalar results and caller-local bounded string allocation respect
the child profile; replies and wakeup remain atomic. Host errors, missing receipts
or credentials never permit a fresh remote mutation ID. A response that cannot
be represented within the child's data/allocation budget leaves its original
saved wait/receipt available for explicit recovery; this profile does not refill
budgets or retry external inference automatically.

## Child-facing tools

`endowment::tool` accepts one strict, bounded JSON request against a host-selected
store. Its only actions are `search`, `type`, `object`, `static` and `invoke`.
There is no parent/sibling/database selector, raw Vm/Program, root directory,
source/CIL, build manifest, compiler reference, installation, policy update,
generic host effect or host/compiler path action. Ordinary operator tools remain
privileged and must not be substituted for this surface.

Search/type views project only permitted declarations/members. Object inspection
returns selected readable fields, at most 32, with bounded data previews. It does
not disclose hidden field counts or follow edges. A readable field whose target
is unendowed projects `{"kind":"denied"}` without its UUID/type/content. Object
permission is checked before heap lookup, so unknown and unendowed IDs do not
form an object-existence directory. Source/CIL requests deny uniformly.

Invoke starts approved static CIL entries; ordinary instance/remote calls use
those entries as wrappers, and constructors are not direct tool entries.
Arguments are I32, canonical decimal-string I64, null, or an existing
object ID checked against the endowment/provenance. They are not boxed/coerced.
Inline text creation is not a child tool action in this version. Results expose
only authorized values and sanitized stop/fault data; the trusted operator keeps
full diagnostic receipts. Invoke-once replay uses existing exact arguments and
execution/receipt identities, including after later context changes.

The trusted native wrapper is:

```sh
renraku-vm child-tool child.db request.json
```

Example requests:

```json
{"action":"search","query":"Node","limit":10}
{"action":"invoke","operation":"one","code":"CODE_VERSION_ID","budget":1000,"arguments":[{"kind":"object","value":"OBJECT_ID"},{"kind":"i32","value":7}]}
```

Requests are at most 16 KiB; replies at most 12 KiB. Queries/limits, 16 arguments,
4096-unit data and 150,000 instructions per invocation are explicit bounds.
Metadata has limits of 128 types, 256 code versions / 16,384 instructions, 256
read/write fields each, 64 endowed objects, 16 effect pairs, 64 frames with at
most 256 locals/stack slots each, 64 instance fields per allocatable layout,
and 256 KiB per
record. Large projections reject rather than leak an unrestricted fallback.

## Evidence and next boundary

Thirteen integrations cover selected object/field/static/method authority, graph
edge/result denial, private metadata/source/tool denial, unknown-ID equivalence,
exact-once receipts and prior-mutation faults. Four test-only PE variants preserve
metadata/body lengths but encode direct private method/field/cast/constructor
tokens, independently of what C# permits. A fifth PE variant raises declared
maxstack to 257; admission rejects it and a valid 65-field allocation layout. Admission rejects them and unreachable
forbidden branches. One private-call variant returns the secret under the old
unsealed runtime, proving the new policy is doing the denial.

Instruction-cut checkpoints/GC retain constructor frames, original IDs and
self-cycles. Collected owned objects leave provenance without remaining heap
roots, and lifetime exhaustion survives collection/new goals. Failed seal and
terminal checkpoints publish no new authority or repeated mutation; repaired
checksum corruption rejects. Exact effect-pair tests use a local echo receipt,
and linked chat capture cannot reach an unendowed private content string.

A separately hosted native owner holds the original Counter. A sealed native
child receives only its fieldless handle, calls it, retains its waiting frame
through GC, and completes 27 after owner Counter=26. SDK-free native replay needs
no key/server and does not mutate again; private-parent search returns no rows.
Existing caller/owner crash/revocation tests and all older persistence/compiler/
model/website/legacy-reader evidence remain required. No live inference is used.

Next extend scoped compiler/reference admission and delegation deliberately.
Filtered class APIs must not become full-module/source authority; append-only
child code requires a versioned atomic admission contract, not removal of the
current registry seal. Returned object grants, per-reference attenuation,
privileged local frames, dynamic scope changes and a general hostile-agent
sandbox remain unimplemented. Copy/mobility/cross-machine policies retain their
separate milestones.
