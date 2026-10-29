# Placement and export preflight

The host-only `renraku_vm::placement` module implements the first preparatory
slice of [networked objects](distributed-objects.md). It persists bounded
fixed-owner placement policies and recipient-bound instance-method grants,
then validates an access request against them without executing guest code.
There is no network listener, remote reference, CIL wait, graph copy, ownership
transfer or hostile-code sandbox in this milestone.

## Identity and policies

NodeId and GrantId are opaque 16-byte identities, separate from ObjectId and
UniverseId. They do not encode endpoints or hardware addresses. An operator
initializes a universe's Owner with its original UniverseId, an explicit NodeId
and a positive u64 epoch. This milestone fixes that node/epoch; retrying the
same initialization is safe, while changing either rejects. A future handoff
protocol must supply fencing before those facts can change.

Each explicitly registered managed object gets its original logical type ID
and a PlacementPolicy:

- 1..16 allowed node IDs, including the current owner node.
- Up to 16 required resource labels, each 1..64 ASCII letters/digits/`._-:`.
- Separate movable and copyable booleans.

`PlacementPolicy::pinned(node)` permits that node and denies both movement and
copying. An operator can explicitly set copying true while movement remains
false, or the reverse. Missing object policies confer no authority. Policies
are immutable in this version; an identical retry returns the saved policy,
and a conflicting registration rejects. These booleans describe eligibility,
not a guest permission to initiate a copy or move.

OwnerContext is a trusted embedding's claim of the actual executing node,
epoch and available resources. Preflight requires the exact saved node/epoch
and all required resources. A resource label is not a device handle or proof
of availability; the embedding is responsible for that claim. Never populate
OwnerContext from untrusted request fields. Local `Vm::run` does not yet enforce
these placement constraints globally, and restarting a database on another
machine does not constitute an authenticated ownership transfer.

## Method grants

`export_method` binds a fresh GrantId to one other recipient UniverseId, one
registered object and one exact MethodVersion (method ID plus code ID). It
validates that the saved code is an ordinary non-constructor instance method
compatible with the object's actual type. Static methods, intrinsics, missing
or mismatched code, unrelated receivers, object-reference signatures and
exports to the owning universe reject.

The initial declaration profile permits at most 16 parameters, with I32, I64,
Boolean, Char and String slots, and a corresponding result or void. The call
argument profile is I32, I64, exact UTF-16 Text, or Null for a string slot.
There are no ObjectId-valued arguments, implicit boxing, width coercion or
Unicode normalization. Boolean values are 0/1; Char is 0..65535, including NUL
and lone surrogates. Each text is at most 4096 units; the total is at most 8192.
The declarations can be checked now; result projection still needs enforcement
when a real invocation/transport layer is implemented.

`authorize` reloads the current checkpoint and takes the actual authenticated
recipient separately from request
owner/epoch/target/code/arguments. It checks the exact recipient, grant, target,
method/code, owner node/epoch, resource constraints and argument profile. A UUID
claimed in a future network message is not authentication. There is no transport
authentication implementation in this API; trusted host callers must establish
that identity before using it.

A method grant denies InspectObject, InspectType, ReadCode, ReadStatic,
ReadField, WriteField, Discover, CopyGraph and MoveObject. In particular,
copyable=true does not turn a method grant into a copy grant. Authorization
returns its observed generation and registry hash with a preflight decision, not
an executable capability token, and neither
enters a guest frame nor reserves the one active context. A later admission must check/fence that generation atomically; a prior decision
cannot authorize a new call after revocation or another checkpoint. Busy-owner admission,
pinned instance invocation receipts, external-effect restrictions, result
attenuation and accepted-operation reconciliation are subsequent work. A grant
describes intentional access to its method's behavior; it does not certify that
body as pure or prevent that body from using the owner's trusted state.

Grants cannot be rebound. Revocation is permanent in this version: an export
retry returns the original revoked record, never reactivates it. Revocation
checks currently apply to preflight only because no remote work is accepted
here. Future accepted calls need their own finish/reconcile contract under the
[network design](distributed-objects.md).

## Checkpoints and retention

The registry is canonical format-1 JSON in an immutable managed string at
`renraku.placement-v1/registry`. IDs use exactly 32 lowercase hex digits; records
and constraint sets sort by identity/name. Unknown fields/formats, duplicate
records/constraints, reordered/noncanonical text and malformed bindings reject.
A SHA-256 hash identifies that exact registry view; it is not a signature or
an authorization credential.

Each policy also owns reserved root `renraku.placement-v1/object/<ObjectId>`
pointing directly to the original managed object. Logical ID strings alone are
not GC roots. These typed target roots retain the object's existing graph after
application statics change. This version has no unregister/release operation;
registered targets remain retained even after all their grants are revoked.
That lifetime must be considered before registering a large graph.

Limits are 256 object policies, 256 grants and 256 KiB registry UTF-8. Policy
lists, resource names and call values have the independent bounds above. The
checkpoint validator checks registry/root agreement, original universe, managed
allocation domains, exact types and pinned code both on save and recovery.
Metadata and target roots commit with the existing heap, statics, code, frames,
effects and receipts. A failed save publishes neither partial policy nor partial
grant/revocation. Host APIs use scratch restores and discard them on an error;
retry from the checkpoint. Idempotent reads/retries run no guest instructions.

The binary checkpoint remains format 7 with readers for 1..6. There are no new
heap/value/frame fields and no CoreLib or old module changes. Universes without
the reserved namespace retain their existing behavior. The namespace is owned
by trusted host APIs; guest CIL has no named-root mutation operation. A privileged
embedding or offline database editor remains trusted. General existing workbench
inspection is still unrestricted, so these records do not establish a child
universe security boundary.

## Verification

Nine integration tests cover default denial, exact principal/object/code/epoch
bindings, independent copy/mobility flags, pinned/resource placement, every
non-method access denial, constructor/static/reference signature rejection,
exact NUL/surrogate arguments, widths, booleans/Char and all text/list/record
bounds. Registration order yields the same canonical registry/hash. Full 256
object and 256 grant registries reject the next insertion without committing.

Typed roots preserve original objects, types and frozen-reference bytes through
static replacement, GC and checkpoints while unrelated frames are paused.
Injected SQLite failures preserve original policy/grant/revocation state.
Malformed registry/root/binding fixtures reject before commit; a forged checkpoint
with a repaired outer digest also rejects its unsupported registry on recovery. Fresh native
processes recover and validate a saved grant without SDK tools or executing a
single guest instruction; wrong recipients and revoked grants deny. This is
policy/recovery evidence, not network transparency or adversarial sandbox proof.
