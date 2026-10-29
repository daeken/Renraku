# Durable model-request outbox

The current implementation connects CIL callers, a host API, and CLI delivery
to the durable `local.echo` capability. The provider returns `echo: ` followed
by the exact UTF-16 request. A real model connection is later work.

## Guest calls and waiting frames

CoreLib declares `Renraku.Model.Complete(string) -> string` as an explicitly
whitelisted InternalCall. Its exact owner, name, and signature are checked by
the loader. It accepts a non-null managed string. The host must opt in through
`Vm::set_model_calls_enabled(true)` or the trailing `--allow-model-calls` flag
on `run`, `run-persistent`, or `resume`. This policy is disabled on every fresh
VM and restore; a saved guest universe cannot grant host authorization.

The call performs one guest transition: remove the prompt from the evaluation
stack, create a prepared outbox record, install `AwaitEffect` in the active
frame, and mark the execution `Waiting` for that effect ID. The continuation
stores the call site, return offset, effect ID, and prompt object ID. Older
callers remain ordinary `AwaitReturn` frames. No native stack represents a
suspended guest call.

`Vm::run` returns `RunOutcome::Waiting(effect)`. Further runs leave the context
and instruction count unchanged while it waits. `run_checkpointed` saves this
state before returning. Plain `run` is an in-memory operation; the host must
checkpoint before expecting its work to survive.

Dispatch or reconciliation commits the completed response, pushes its reference
onto the waiting frame's evaluation stack, advances the continuation to its
return offset, and marks the execution runnable **in the same checkpoint**.
Guest execution resumes with the stored result. A failed response checkpoint
restores the original waiting context in memory and keeps the effect uncertain.
Completing an unrelated host request does not wake the guest.

Recovery checks that the waiting status and active continuation name the same
unfinished effect, prompt, and capability, that the call targets Model.Complete,
and that offsets and the post-argument stack shape are valid. Only the active
frame may await an effect. Intrinsic methods themselves never have saved frames.

## Request state and commits

Each effect has a stable opaque `EffectId`, a capability name, a managed request
string reference, and one of these states:

| State | Meaning | Permitted delivery action |
| --- | --- | --- |
| Prepared | Request recorded; no dispatch marker committed | Submit after committing the marker |
| InFlight | Dispatch marker written before external code is invoked | Reconcile only |
| OutcomeUnknown | No committed response proves the external outcome | Reconcile only |
| Completed | Managed response string committed with the effect record | Return the saved response |

`Vm::queue_model_request` creates an in-memory prepared record bound to
`local.echo`. `CheckpointStore::save` persists it along with heap, code,
roots, statics, and guest frames. The outbox retains its request and completed
response as GC roots. The host must save queued work before expecting it to
survive a crash.

`Vm::dispatch_model` checks the provider capability, changes prepared to
in flight, and saves the complete universe **before invoking the provider**.
A save failure prevents submission. The provider receives the immutable prompt
and the stable effect ID. A successful response becomes a new managed string;
the completed record and response commit in one universe transaction before
the caller receives the result.

Any submission error is conservatively uncertain. If receipt of a response is
followed by a failed checkpoint, the in-memory effect also remains uncertain;
it cannot return an uncommitted response on a later dispatch call. Stale writer
conflicts require reloading the universe.

Recovery changes saved in-flight records to outcome unknown. Even if a crash
occurred before the provider was actually invoked, recovery cannot infer that
from the dispatch marker. Neither restoration nor ordinary guest execution
dispatches an effect automatically.

`Vm::reconcile_model` looks up the same request through the provider's receipt
protocol. A receipt follows the normal response commit path. No receipt leaves
the outcome uncertain and never falls back to submission. Completed requests
return their saved response without a provider call. Only completed records
can be forgotten; callers must save that removal and retain any response still
needed by the guest.

## Local provider ledger

`LocalModelProvider` stores receipts in a separate SQLite database using WAL
and full synchronization. Its application ID is `0x52454d4f` and schema version
is 1. It rejects foreign or nonempty unmarked databases.

Each receipt holds the effect ID, exact UTF-16 request and response bytes, and
the number of submission attempts. Repeating a submission with the same ID and
request returns the existing response; reusing the ID for a different request
fails. Reconciliation validates the request against the receipt and does not
increment the submission count.

The ledger deliberately commits outside the universe transaction. Losing a
universe process after provider acceptance leaves a receipt that a fresh
process can discover. The provider database must be retained to reconcile its
requests. A different or missing ledger may contain no receipt; that still does
not authorize retry. The local adapter demonstrates idempotency and receipt
lookup, not an exactly-once guarantee for arbitrary external systems.

Dispatch requires a single active host for a universe. Optimistic generation
checks reject stale writes; they are not a distributed ownership or provider
locking protocol. Snapshot forks and restores must not become independent
effect dispatchers.

## Demonstration and current limits

The CLI commands are `queue-model`, `dispatch-model`, `reconcile-model`, and
`inspect-checkpoint`. Queue accepts a UTF-8 command-line prompt; the Rust API
and persistent representation preserve arbitrary UTF-16 units, including
embedded NULs and lone surrogates. Requests and responses share the current
one-million-unit string limit.

`Simulation.ModelHarness` stores conversation history in ordinary linked
`ModelMessage` objects, makes two requests through three nested CIL frames,
and returns 417 in 158 guest instructions. The first response becomes the next
prompt with the same managed object ID. Static guest storage retains all four
messages after completed outbox records are forgotten and collected. The
CoreCLR reference uses an echo shim; it is excluded from the persistent guest
assemblies.

There is no real model connection, credential storage, streaming, tool
execution, cancellation, automatic retry, or automatic delivery loop yet.
Experiment branches must default to effects disabled. A real adapter needs an
explicit capability identity, request schema, and reconciliation policy.

## Recovery tests

The integration suite checks:

- Prepared requests and completed responses survive checkpoints and GC with
  exact UTF-16 units.
- The dispatch marker is visible through a separate store before provider code
  runs, and stale VMs cannot invoke a provider.
- A process killed before local acceptance recovers as uncertain with no
  submission receipt; it does not retry.
- A process killed after local acceptance recovers the receipt and commits the
  response without another submission.
- Failed response commits cannot return uncommitted cached results; reload and
  reconciliation complete the request.
- The linked CIL conversation restores at every instruction and effect boundary
  and matches CoreCLR. Repeated process kills retain live waiting frames and
  provider receipts without duplicate submission.
- Killing before intent commit restores the prior guest state without an effect;
  killing after response/wakeup commit restores a runnable frame with its result.
- Disabled calls and null prompts fault with their original operands intact.
  Malformed waiting continuations and completed effects still marked waiting
  are rejected even with recomputed checksums.
- Capability mismatch, idempotency-key reuse with changed data, duplicate effect
  IDs, malformed state tags, and missing payload objects fail explicitly.
- Original format 1, 2, and 3 checkpoints restore and upgrade to format 4.
