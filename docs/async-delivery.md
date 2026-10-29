# Asynchronous host delivery

`AsyncModelProvider`, `Vm::dispatch_model_async`, `reconcile_model_async`, and
`drive_model_async` allow host delivery to yield while waiting. They use native
Rust futures outside the guest universe. The VM library requires no executor
dependency; an embedding host supplies an executor and an asynchronous provider.
The blocking provider/driver API remains available for existing hosts.

```rust,ignore
let report = vm.drive_model_async(&mut store, &mut provider, limits).await?;
```

Both drivers share one invocation state machine for budgets, authorization,
waiting-effect selection, fault handling, and stop reports. Both delivery paths
share capability/model checks, dispatch checkpoints, response commits, and
reconciliation transitions. Heap, code, and frames still commit together in
checkpoint format 5. Formats 1 through 4 retain their existing readers.

## Progress and limits

Several independent universes can be driven on one executor thread. While one
provider waits, another universe can execute and checkpoint. The async driver
also yields between guest quanta of at most 1024 instructions, reduced further
by the checkpoint interval and remaining instruction budget. These yields can
produce additional checkpoints; the configured interval is an upper bound.
Instruction/provider-operation accounting remains exact and per invocation.

This is not a multi-context scheduler inside a universe. Each VM still has one
active context, and a driving future holds exclusive mutable access to its VM
and store. Pure interpreter work and universe checkpoint I/O remain synchronous
within each bounded poll. Native futures, executor state, and network handles
are not written to checkpoints. The existing OpenAI provider is still blocking;
an asynchronous HTTP adapter is the next host integration step.

## Dropped futures and recovery

- During preflight, an abandoned future leaves the effect Prepared. No inference
  dispatch marker was committed. A later invocation may validate and dispatch.
- After the InFlight checkpoint, abandonment leaves the guest waiting with an
  InFlight effect in memory. Restore normalizes it to OutcomeUnknown. The next
  driver invocation performs receipt reconciliation, never submission.
- A provider receipt committed before abandonment can recover the response and
  wake the original guest frame. Missing evidence leaves the request uncertain.
- Response data, result stack, and the runnable continuation still commit as one
  checkpoint. An async return does not acknowledge an uncommitted guest result.

Dropping a future or wrapping it in a host timeout is not evidence that a remote
operation was canceled. It may have been accepted or charged. This milestone
does not implement durable guest cancellation, deadlines, futures, or joins.

## Validation

Six async-driver tests exercise independent progress on a current-thread executor,
CPU fairness, cancellation before/after local acceptance, committed receipt
recovery into the same frames, abandoned/rejected preflight, per-instruction
checkpoint recovery and budgets, and stale-writer fencing. Existing blocking
driver, outbox, checkpoint, and process-kill tests remain in force. Local receipts
are shared across the blocking and asynchronous host entry points.
