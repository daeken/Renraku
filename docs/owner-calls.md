# Durable owner-side instance calls

`renraku_vm::owner::accept` connects the [placement grants](placement.md) to
actual CIL instance execution on the authoritative object. It accepts an exact
operation once, resumes its saved frames, or returns its terminal result after a
lost reply. This is an owner backend with native recovery tests. Transport,
authentication, caller-side remote references/waits and secure child universes
are still unimplemented; this is not full network transparency.

## Explicit instance entry

`InstanceCall` contains an exact MethodVersion, a managed receiver ObjectId, and
ordinary arguments. `Vm::start_instance` and `invoke_instance_once` accept that
call through trusted host APIs. They reject constructors, intrinsic/static
methods, mismatched/missing code, incompatible or transient receivers and invalid
argument shapes before entering a frame or committing intent. Null/scalar
receivers cannot be supplied through the typed public API; saved receipts and
internal validation reject them too. These APIs do not grant remote authority.

The receiver occupies argument zero in both the frame and invocation receipt,
followed by normal arguments. The retained method's instance signature determines
that layout. Receiver/argument/result roots, original object identity, pinned
code and terminal faults use the existing invocation machinery. No object is
copied or implicitly boxed. An existing receiver can remain retained by its
receipt after its application static root is replaced.

Static `start`, `invoke_once`, native commands and agent actions remain static.
A trusted operator can use:

```sh
target/debug/renraku-vm invoke-instance-once target/universe.db request-1 \
  Renraku.Guest.Counter::Add 1000 ORIGINAL_COUNTER_ID i32:7
```

This requires an existing object and installed code, not the SDK. New instance
reports use host JSON format 2 with callKind=instance and an explicit receiver;
old static reports keep format 1 unchanged. Binary checkpoint format remains 7:
the existing argument vector and retained signature encode the receiver, without
new value, object or frame fields. All original fixtures/readers remain tested.

## Owner acceptance

A trusted embedding provides OwnerContext and an actually authenticated caller
UniverseId. An OwnerCall binds operation ID, owner universe/epoch, grant,
original receiver, method/code and exact scalar/string/null arguments. Node or
grant UUIDs are not authentication. The test subprocess is a privileged embedding,
not a network authentication implementation.

Operation IDs contain 1..64 ASCII letters/digits/`._-`. Owner request IDs and
immutable acceptance roots are `renraku.owner-v1/<caller-ID>/<operation-ID>`.
The owner supports at most 128 accepted operations. Acceptance metadata is strict
canonical format-1 JSON, at most 64 KiB per record, with exact UTF-16 arrays for
string input. It captures the observed placement registry hash/generation;
these observations are not credentials. Recovery validates logical owner/epoch,
original grant scope, exact receiver/method/arguments and the linked invocation.
Physical generation is not part of the binary payload and is fenced by the
checkpoint store rather than inferred from a historical observation.

For new work, accept loads the current checkpoint, verifies the owner placement,
epoch, recipient-bound active grant, argument profile and supported code closure,
and excludes another active context. It creates immutable accepted input and
starts the pinned instance receipt in the same scratch VM. The first checkpoint
commits both before any guest instruction. Concurrent revocation or another
writer fences that save; failure exposes no partial acceptance or mutation.

A repeated exact operation resumes its existing execution or returns its result.
Changed caller/grant/target/code/arguments/epoch cannot retarget the operation.
Accepted work may finish and reconcile after its grant is revoked; new operations
under that grant reject. The current node/epoch/resource placement must still be
valid. Revocation never erases retained scope or receipts. Terminal replay can
coexist with an unrelated paused context because it runs no guest instructions;
new execution still obeys one-context exclusion.

Checkpoints retain partial mutations and original continuations together. A VM
fault keeps earlier committed mutations and failing operands; it is not guest
EH or whole-method rollback. On any save error discard the scratch VM and reload
before retrying. A failed terminal save resumes the last committed instruction,
without repeating an earlier committed mutation.

## Effects, bounds and results

Owner acceptance conservatively traverses calls and constructors reachable from
all retained owner entries. It allows ordinary CIL and existing pure String
intrinsics, and rejects external capabilities, including indirect/unreachable
Model helpers, before the first instruction. Model calls are also disabled on
the restored execution. The aggregate code closure is limited to 256 methods
and 16,384 instructions; cycles count each code version once. This is an explicit
restriction for this backend, not a purity claim: ordinary owner heap/static
mutations and loops remain permitted. Body replacement and dynamic/nested remote
calls are outside the contract.

Arguments retain the placement profile: up to 16 declared values, 4096 UTF-16
units per string and 8192 total, with exact integer widths, bool 0/1 and Char
0..65535. Return values are void, scalar, null string or immutable string up to
4096 units. Void and null are distinct. A completed oversized string yields a
terminal InvalidResult projection from its retained immutable result; replay
neither executes it again nor rolls back earlier mutations. Version-1 projection
bounds/diagnostics must remain stable for these receipts. VM faults return a
bounded 4096-byte UTF-8 diagnostic prefix with an explicit truncation flag; the
full diagnostic stays in the invocation receipt.

No owner method may park on an external effect. The backend returns Pending on
instruction-budget exhaustion and uses at most 1024 instructions per checkpoint
quantum. Sockets, credentials, native threads and futures remain outside snapshots.
No new model action, publication authority, host filesystem tool or CoreLib body
was introduced. Returned object capabilities, distributed transactions, graph
copying, migration and an untrusted inspector still require separate contracts.

## Verification

Nine integration tests exercise instance and owner calls, plus two unit tests
check invalid scalar/null/transient receivers and aggregate code-closure limits.
Every instruction of nested success, exact-surrogate input/result and terminal
fault work recovers with GC, original receiver/code/argument IDs and pinned
executions. Old static receipts still replay after instance work.

An exported original Counter reaches 26 from 19 after acceptance and revocation;
a separate operation reaches 23, and the first receipt still returns 26 without
changing 23. Native SDK-free processes resume accepted work, reject a wrong
principal, and exercise the operator instance command/report. Further checks
cover changed requests, busy owners, stale node/epoch, invalid receiver/code,
external-effect denial, null/void/oversized results, full receipt/operation bounds,
failed acceptance/terminal saves, malformed roots/metadata and repaired-digest
recovery rejection. No live inference or actual network transport is used.
