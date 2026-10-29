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
are not written to checkpoints.

`AsyncOpenAIProvider` uses asynchronous reqwest HTTP and streaming reads on a
Tokio runtime. Protected profile access/refresh and receipt transactions run in
the runtime's blocking pool. Both OpenAI adapters use one request/catalog and
diagnostic policy, one incremental SSE decoder, and the same schema 1 ledger.
The original OpenAIProvider remains a blocking compatibility API.

The `renraku-openai demo` and `drive` commands now use the async path on a
current-thread runtime with at most four blocking workers. Their arguments and
budgets are unchanged. Auth commands and standalone model discovery remain
blocking command-line operations. Async hosts should use AsyncOpenAIProvider;
the blocking compatibility adapter is unsuitable inside an executor worker.
Universe checkpoint I/O and bounded native parsing still run synchronously.
This milestone frees model network waits; it does not claim all host I/O is
nonblocking or add several guest contexts to one universe.

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

Four async HTTP tests cover a paused real local stream while another universe
finishes on the same executor, active-account changes, abandoned HTTP futures,
offline cross-adapter receipt reuse, changed-request rejection, invalid UTF-16,
bounded diagnostics, and failed streams without retry. The Responses SIGKILL
parent now runs both adapters at partial-stream and post-receipt/pre-VM-commit
boundaries. The original continuation resumes from a receipt without another POST.
