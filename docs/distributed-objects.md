# Networked objects, placement and universe boundaries

This is the next development direction selected by the user on October 29,
2026. It specifies the implementation sequence and acceptance boundaries;
remote object references, placement enforcement, graph copying and security
sandboxes are not implemented yet. The existing localhost website is an HTTP
application edge, not object-level network transparency.

## Position in the original rollout

The [original phases](architecture-draft-v0.1.md#11-implementation-phases) are
useful landmarks, but implementation has crossed their boundaries.

| Original phase | Current evidence / remaining work |
| --- | --- |
| 0–3: semantics, CIL, heap, persistent execution | Implemented supported subset; real process kills retain objects, code and active portable frames |
| 4: introspection and live evolution | Saved-code/source and object/static inspection, frozen references, append-only compilation/installation and model coding loop work; body replacement, rollback and a general browser explorer remain |
| 5: transient integration | Durable external effect boundaries and real subscription inference work; general resource handles and guest transient-arena APIs remain |
| 6: process mobility | Fresh-process restart with stable identity is proven; explicit fenced ownership transfer between hosts is not |
| 7: networked universe | Remote references, cross-node invocation and ownership routing are not implemented |
| 8: ecosystem | Narrow pure CIL libraries are useful now; broad BCL, JIT and concurrency remain future work |

Networked objects and universe authority are next, ahead of further standalone
opcode/library expansion. Initial network invocation does not depend on finishing
body replacement, general concurrency or cross-architecture execution migration.
The full original phase-7 exit criterion remains a later milestone.

## Identity, ownership and placement are separate

An object's stable logical identity must not encode a process, address, URL or
hardware location. Its authoritative owner is the universe responsible for its
mutable state and durable operation receipts. The node currently hosting that
owner is a routing fact, with an ownership epoch for fencing stale hosts.
Administrative authority to move or copy an object is separate from authority
to invoke its methods.

Represent these policies explicitly in trusted runtime metadata, independent of
ordinary mutable guest fields:

| Property | Meaning |
| --- | --- |
| Owner universe + authority epoch | Which authority can commit changes; stale epochs cannot execute |
| Current placement | Which authenticated node currently hosts that authority |
| Placement constraints | Eligible nodes and required resources; an object may be pinned to one node or resource owner |
| Mobility permission | Whether identity-preserving relocation is allowed |
| Copy permission | Whether an independent snapshot copy may be created in another universe |
| Export grants | Which recipient may use which object/type/member operations |

Hardware affinity and copy permission are independent. A camera-backed object
can remain pinned while exporting a copyable image value. A private object may
be movable among trusted nodes yet forbidden to copy into another sandbox.
Unspecified placement/copy/export permissions deny those operations. Placement
ineligible for execution fails explicitly; routing never silently moves or copies
an object to satisfy a call. A resource requirement describes a constraint, not
a serialized operating-system handle. External hardware/resources remain outside
checkpoints and need explicit binding/recovery capabilities.

Initially an entire owner universe stays on one node, retaining one guest
context. Per-object policies still matter to eligibility and graph copying, but
do not imply that the current atomic heap checkpoint can be arbitrarily sharded.
Independent owner universes can progress asynchronously using existing drivers.
Distributed heap partitioning is a later transaction/GC contract.

## Reference, move and copy have different identities

A remote reference denotes the same authoritative object. It neither carries a
private mutable replica nor authorizes inspection of the owner's whole universe.
Repeated imports of that reference must preserve aliasing and identity tests.
A move retains logical identity and requires an explicit fenced ownership
handoff. A copy creates fresh object IDs and independent mutable state.

A bounded graph copy must freeze one committed source generation and validate
the entire selected closure before publishing anything at the destination. Use
one old-to-new identity map to preserve cycles and aliases within that copy.
Keep original source IDs as provenance, not as replacement live identities.
Match destination type/code contracts explicitly; facade metadata cannot create
live types. Statics, executable frames, receipts, capabilities and external
resources are not implicitly cloned with instance fields.

At the closure boundary, each outgoing edge needs an explicit rule: include a
copyable target, retain an authorized remote reference, or reject the copy. Never
silently drop an edge, duplicate a pinned resource or expand an export grant.
The initial copy implementation should reject remote/resource/noncopyable edges
until its boundary rule is implemented. Object/byte/depth/work limits and durable
copy IDs must make failed publication atomic and retries reconcile to the same
copy. Graph copy is a separate milestone after the first remote-call proof.

## A universe receives an explicit endowment

A sandbox starts with a finite set of granted objects, permitted class APIs,
local code, and explicitly allowed effects. A type API grant can allow using a
class or constructing local instances without granting its source, private
members, static storage or containing module. An object grant identifies the
allowed operations and recipient universe; an opaque ID alone conveys no rights.
Grant records are runtime-owned and cannot be minted or widened by guest code.
Transport credentials remain outside guest checkpoints.

There is no ambient parent-universe link, global object directory, inherited
model authority or automatic access to sibling universes. All ways to acquire
another reference must respect the same boundary: field access, method results,
arguments, casts, static roots, symbol search, source/CIL inspection, compilation
references, callbacks and nested grants. Finding a name, guessing a UUID,
resolving a base class or knowing a metadata token cannot broaden authority.
A successful cast changes the permitted type view, never the grant.

An object reference does not automatically expose every reachable private
field. Crossing the boundary needs explicit permitted views/methods; methods
that return references must attenuate those references before delivery. Compiler
facades help describe contracts but are not a security mechanism. Runtime load,
resolution, dispatch, inspection and static access must enforce the restrictions
even for handcrafted CIL. Same-process universes must not share an unrestricted
registry or static table. Keep the existing trusted workbench separate from any
untrusted child inspector/controller.

The current coding agent controls trusted computation and is **not** a hostile
code sandbox. That claim cannot change until runtime enforcement and adversarial
escape tests exist. ObjectMap is a useful container of endowed references, not
an enforcement boundary by itself.

## First independently demonstrable network slice

Implement an explicit host-created export/import grant for one saved Counter
between two independent owner/caller universes in two native processes. Begin
with authenticated loopback transport, fixed endpoints and no remote public
listener. Freeze type/member/code bindings and bounded integer/string/null
arguments/results; imported references must not become copied Counter instances.
Do not route the general workbench or all of a module across the network.

A normal supported CIL instance call through the imported reference should park
an explicit portable guest frame while the owner performs it. Keep network
sockets and native futures outside the checkpoint. Unsupported remote field
access, callbacks and nested remote calls reject explicitly in this first slice;
method invocation is the initial transparency boundary. General field access,
returned reference graphs and call cycles need subsequent explicit contracts.

The caller commits its intent with a stable operation ID before sending. The
owner durably accepts exact caller/grant/target/method/code/arguments/epoch and
uses the existing invoke-once semantics under its single-context exclusion.
Repeated identical requests reconcile to that original execution/terminal
receipt; changed contents conflict. The owner commits heap, frames and response
before sending. The caller commits the returned value/fault and resumed frame
atomically. Prior owner mutations remain on a terminal guest fault, as today.
There is no distributed rollback or cross-universe atomic checkpoint.

Timeout/partition leaves a durable wait or explicit uncertainty. It does not
prove non-acceptance, permit a new mutation ID, elect a second owner or trigger a
copy fallback. An unavailable/lost authoritative receipt ledger stays uncertain.
Model and arbitrary external effect requests keep their separate no-resubmission
policy. Initially reject network methods with external capabilities or nested
remote work rather than silently weakening that policy.

Revocation blocks new acceptance. Initially, work already accepted under a
grant finishes its pinned operation and its original principal can reconcile
that receipt; it cannot create a new call. Execution migration, automatic owner
failover, cancellation and transfer of partially running remote calls require
later contracts. Even the fixed-owner prototype must reject stale owner epochs.

Exit evidence must include two real processes, owner Counter identity/state,
caller instruction-cut/GC/restart while waiting, owner restart after acceptance,
lost response reconciliation, exact once-only mutation, changed-request denial,
missing/wrong/revoked grants, scalar/target bounds, busy-owner behavior and no
access to ungranted objects/types/statics/source. Test local deterministic
transport faults before any machine-to-machine exposure. A phase is complete
only when its implementation and evidence meet this boundary; a descriptor or
protocol design alone is not network transparency.

## Follow-on exits

1. Enforce the initial endowment throughout child-universe metadata, execution
   and inspection; prove attempted parent/sibling escape with handcrafted CIL.
   Denial tests are a prerequisite to advertising a security sandbox.
2. Extend remote references deliberately to returned object capabilities and
   selected field operations, retaining identity, attenuation and durable waits.
3. Publish bounded independent graph copies with fresh IDs, preserved cycles and
   aliases, policy checks, code bindings and atomic durable copy receipts.
4. Prove identity-preserving ownership handoff with epochs/fencing, placement
   eligibility and a pinned object that must remain at its resource owner.
5. Add authenticated cross-machine routing and cross-architecture execution
   migration, then address concurrency, distributed GC and reentrant call cycles.

The first slice must enforce its own export boundary even before complete child
sandbox enforcement. Placement/grant descriptors, validation and local denial
fixtures can be delivered first as a narrow coherent implementation, but they
must be labeled preparatory rather than networked-object acceptance.
