# Suspension by default in the replacement BCL

Design direction: every BCL operation that may wait should suspend the logical
guest execution by default. This includes networking, filesystem access, timers,
synchronization, channels, and joins. Pure computation and ordinary heap access
remain ordinary CIL operations. Establish this before expanding the BCL surface.

## Ordinary calls and explicit concurrency

A sequential call can park its logical execution, retaining its portable frames,
while the host scheduler runs independent work. The existing model intrinsic
already demonstrates the guest side:

```csharp
string reply = Renraku.Model.Chat(binding, history);
```

This call installs an AwaitEffect continuation and returns control to the host.
After a durable result arrives, the same caller resumes with the returned string.
The caller needs no language-specific continuation transformation. Every guest
language targeting the supported CIL can use this mechanism.

Each waiting operation should have one canonical implementation. Conventional
synchronous-looking BCL members can park the logical context. Compatibility
`Async` members can expose explicit futures over the same operation when those
types and compiler patterns are supported. New Renraku APIs should avoid two
independently implemented synchronous/asynchronous paths for the same behavior.

Concurrency remains explicit. Starting child work, joining several operations,
and racing results require managed task/future objects with logical identities.
Structured scopes should own child lifetimes and join children on normal scope
exit. Failure or cancellation should request cancellation of unfinished children
and join them before the scope completes. Detached work needs explicit durable
ownership. Waiting at a normal call does not itself start additional guest work.

This is a language-neutral runtime direction. C# async/await and Task<T>
compatibility still require implementing the relevant CIL, generics, value
types, builders, and continuation machinery. None are newly supported by this
design note. Arbitrary .NET binary compatibility is not implied.

## Observable suspension and isolation

Suspension can expose shared state to other logical executions. Capability
metadata, API documentation, and runtime inspection must identify operations
that may suspend. The transitive calling behavior matters even when a caller
uses an ordinary method signature. Imported code must not acquire an unverified
no-suspend promise from the absence of an annotation.

Verified no-suspend regions must reject potentially waiting operations before
publishing an effect or changing a wait queue. An immediate `Try` operation may
report unavailability without parking, with its own precise contract. Ordinary
mutex scopes should forbid suspension by default; long-lived coordination across
I/O needs an explicit construct with durable ownership and recovery rules.

No-suspend is not transaction isolation. Interpreter scheduling, atomic regions,
checkpoints, and application transactions need separate rules. A whole-universe
atomic checkpoint does not isolate concurrent guest computations from each
other. Do not introduce an `atomic` API until its isolation, rollback, failure,
and checkpoint behavior are implemented and tested.

## Durable waits and host work

Managed waits must name logical operation/context IDs and durable wait reasons.
The checkpoint must capture all runnable/waiting contexts, their frames, managed
futures, wait ownership, result/fault values, and shared heap/code as one coherent
universe. Native Rust futures, sockets, thread-pool items, OS wait handles, and
host CLR Task objects are not checkpoint representations.

A future multi-context scheduler must use bounded interpreter quanta for CPU
fairness and durable wakeup transitions for waits. A checkpoint restores the
wait relation and reconciles its operation; it does not blindly recreate a native
request. Native I/O belongs in asynchronous host adapters, with bounded worker
pools for unavoidable blocking platform APIs. Parking a guest must not reserve
one OS thread for the duration of its wait.

An asynchronous API is not sufficient evidence of recoverability. Each operation
needs a capability-specific protocol: durable intent, external dispatch fencing,
reconciliation evidence, and atomic result/wakeup when appropriate. Transient
resources still cannot cross durable boundaries through managed references.
File descriptors, file positions, timers, locks, and channels each need explicit
identity and recovery policies before their BCL APIs are enabled.

Cancellation requests, ownership, deadlines, and terminal outcomes must be
managed state. Cancellation is cooperative; requesting it or timing out cannot
prove that an already dispatched external operation was canceled. Preserve
OutcomeUnknown when acceptance remains uncertain. Deadlines need a portable
clock policy across restart; saved host monotonic-clock values cannot be treated
as portable timestamps. These mechanisms are future work, not implemented
CancellationToken or timeout support.

## Implementation sequence and acceptance

The current VM has one active context. Model calls already park portable guest
frames, but ModelProvider and the reqwest adapter still block the calling host
while performing delivery. This is the first host limitation to remove.

1. Introduce asynchronous host delivery while retaining the existing durable
   intent, one-attempt receipt, generation-fence, and response/wakeup protocol.
   Keep native futures outside snapshots and retain a bounded CLI driver.
2. Add logical context scheduling and checkpoint all contexts, shared heap, code,
   and wait relationships together, with backward readers for existing snapshots.
3. Add managed futures, structured scopes, cancellation/deadline state, and
   explicit may-suspend/no-suspend contracts before broad BCL expansion.
4. Add language/BCL compatibility adapters and waiting primitives with recovery
   tests for their individual semantics.

Acceptance must demonstrate independent guest progress while a provider waits,
bounded host resource use, checkpoint/restore of several contexts, one wakeup per
completed operation, no repeat of uncertain effects, durable join/cancellation
relationships, and no-suspend rejection before external intent publication.
The current deterministic local provider and partial-stream/SIGKILL tests should
remain fixtures for this work.

For context, Microsoft's documentation distinguishes
[asynchronous waiting from extra threads](https://learn.microsoft.com/dotnet/csharp/asynchronous-programming/task-asynchronous-programming-model)
and describes [cooperative cancellation](https://learn.microsoft.com/en-us/dotnet/standard/threading/cancellation-in-managed-threads).
Renraku's additional design work is portable runtime suspension and durable
operation ownership across process death.
