# Durable CIL calls through imported contract handles

`renraku_vm::remote` connects ordinary C# instance calls to the authenticated
[peer transport](peer-transport.md). A saved caller parks a portable frame,
invokes the original owner object, and resumes from its committed reply. The
first Counter demonstration now runs in two native processes/universes with
independent core/type IDs, caller and owner crashes, GC and lost replies.

This is a narrow method-call contract. It does not supply arbitrary foreign
classes, remote fields, returned object references, graph copying, migration,
cross-machine transport. The separate [closed child profile](child-endowments.md)
now enforces static endowments around this path; a general dynamic hostile-child
sandbox remains future work.

## Type and identity contract

The trusted host reads the authoritative owner's saved registry and active
recipient-bound grant with `remote::import_method`. It checks the owner universe,
node, positive epoch, original object/method/code, caller principal, method name
and supported runtime slot profile. I32/I64 retain raw bits; Boolean, Char and
String remain distinct profiles. Core IDs are never assumed equal across
universes. Each reference slot must resolve to that universe's actual String.

The caller supplies an already installed **local contract declaration** such as:

```csharp
public sealed class CounterPort
{
    public int Add(int delta) { return -999; }
}
// Ordinary caller code:
public static int Use(CounterPort counter) { return counter.Add(7) + 1; }
```

This declaration has its own local type/method/code IDs. It is not a recreated
Counter type, an implementation reference DLL, or a foreign type inlined into a
facade. The owner never exposes its fields, statics, private source, registry or
ungranted methods. The host compares authoritative schema, rather than treating
facade tokens or UUIDs as access rights. Compilation/installation and frozen
reference sets keep their existing meaning; importing is a separate explicit
host operation, not a compiler side effect.

Version 1 requires a sealed fieldless contract directly extending the actual
local Object, with one ordinary instance method plus constructors. No additional
methods/statics/fields are allowed. Binding requires no preexisting contract
instances. The host allocates a managed, fieldless **handle**, skipping its
constructor. Once bound, its marker method body and constructors cannot execute
locally. Direct local instance entry, null/fake receivers and ungranted member
calls reject. Handcrafted `call` and normal Roslyn `callvirt` use the same runtime
boundary. Ordinary casts operate on the local contract type; this does not make
the handle castable to the owner's Counter or grant field/source access.

The referent identity is the original `(owner UniverseId, ObjectId)` pair. The
local handle has a separate carrier ObjectId, contains no Counter state, and is
canonical for that pair in the caller. Exact repeated import returns that same
handle; changed contract/grant/endpoint aliases reject. This first version allows
one imported referent per contract type and one method per handle, at most 64
imports. General multi-object/method contracts are follow-on work. Immutable
canonical import strings and typed handle roots live under
`renraku.remote-v1/import/<handle>` and `object/<handle>`, retained indefinitely.
There is no release/distributed-GC protocol yet. No copy/move permission follows
from method import, and transient references are not promoted.

## Freeze, wait and atomic return

At the CIL call instruction, the runtime checks the handle and declared input
slots, freezes exact primitive/string/null arguments and the pinned endpoint,
owner/epoch/grant/object/method/code into a peer intent. No socket is used by
`Vm::run`. The stable operation is `g.<execution-ID>.<instruction-counter>`;
loops and later executions cannot reuse an accepted mutation ID. Counter
exhaustion or the existing 128-peer-call limit rejects before another intent.

The frame removes the receiver/arguments, preserves the lower evaluation stack,
locals and nested caller frames, and becomes:

- `Continuation::AwaitRemote { call_site, next, request }`
- `ExecutionStatus::WaitingRemote { request }`
- `RunOutcome::WaitingRemote(request)` on subsequent runs, with no progress.

The request ObjectId roots the immutable peer intent; the imported handle stays
retained by its typed import root. This is a separate wait contract from Model
AwaitEffect. There is no native future/socket/key on the portable guest stack.
Intent and waiting continuation checkpoint with the same whole universe before
any network request. Existing instruction budgets still bound ordinary CIL.

`remote::drive` performs one bounded host delivery/reconciliation, off an async
executor's latency-sensitive worker. Missing credentials return Unresolved and
leave the exact saved wait. A Pending acknowledgement pins the original owner
execution. Sent markers, authenticated bindings, original-operation-only resume,
revocation semantics and missing-ledger uncertainty follow peer version 1
unchanged. Wrong credentials or busy/rejected requests never become permission
to accept again or mint a fresh operation ID.

For a terminal reply, caller result allocation, declared result checks, peer
ack/reply, evaluation-stack push and continuation wakeup commit **together**.
String results allocate in the caller's actual String type, preserving exact
UTF-16 units; strings cross this profile as immutable data, not shared object
references. Void and null differ. A failed save exposes no result and reloads
the same parked frame; reconciliation never repeats the owner mutation. A
remote terminal fault retains the frozen request and AwaitRemote continuation,
updates any owning invocation fault receipt atomically, and retains earlier
caller/owner mutations. It does not implement guest EH or rollback.

Payload format remains 7, with explicit new status tag 5 and continuation tag 3.
Old state encodings and every original format-1-through-7 fixture remain
unchanged; the current reader understands the new tags only in format 7. No
stored guest value kind, heap representation, core image or module body changes.
Save/recovery validates import roots/schema/IDs and the active call site's
abstract stack, request binding, operation/execution, return offset and status.
Malformed frame code is a diagnostic, including after a repaired digest.

## Host commands and limits

```sh
renraku-vm remote-import owner.db caller.db OWNER NODE 1 127.0.0.1:59000 GRANT Local.Contract::Add
renraku-vm remote-drive caller.db private.key
renraku-vm remote-drive caller.db
```

Importing requires trusted local access to the original owner checkpoint. The
network listener exposes only granted calls; it never exposes the workbench or
accepts a compiler/host path. Normal invoke-once/resume commands start/continue
caller CIL. A running invocation waiting remotely reports format 3 with a
`waitingRemote.request` identity; old static/instance reports retain their
original formats. The remote driver commits the reply/wakeup but does not run
subsequent CIL. Finished invocation receipts replay locally without credentials,
network, source or SDK. No new agent action or implicit network driver was added.
Model/agent drivers stop explicitly at a remote wait rather than spinning or
using model authority to deliver it.

Owner invocation conservatively rejects any universe containing imports in
this initial profile; accepted owner records and imports cannot coexist. This
prevents nested remote work/callbacks, including through unreachable helpers. Existing owner code-closure checks still deny external capabilities.
Arguments/results retain peer version-1 bounds and authenticated plaintext
loopback limits. Multi-context scheduling, cancellation, negative-acceptance
queues, returned reference grants and unavailable-owner failover remain absent.

## Evidence

Sixteen integrations use actual compiled C# against independently loaded caller
and owner cores. They cover every-instruction checkpoint/GC/frame recovery,
primitive/String/null/void wakeups, exact I64 extrema/Char surrogate units,
repeated loop/call sites and lower-stack results, scope/busy/revocation/credential/
shape/bounds denials, malformed metadata/frame recovery and success/fault save
failures. Old success/fault invocation receipts survive later execution and GC.

The original Counter reaches 19->26, caller returns 27, then another request
reaches 23 and caller returns 24. First invocation replay remains 27 and leaves
23. No Counter declaration/state is installed in the caller. In the native
crash test, both processes are killed after owner Counter=26 commits before
reply output. The saved caller still waits. After grant revocation, GC and
same-endpoint owner restart, a fresh caller reconciles the original execution.
After deleting the key/stopping the server, SDK-free native CIL completes with
27, and local replay leaves Counter=26. All verification servers are bounded
and exit; the two existing demo universes remain untouched.
