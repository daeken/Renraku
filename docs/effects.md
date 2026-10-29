# Durable model-request outbox

The current implementation connects CIL callers, a host API, and CLI delivery
to the durable `local.echo` capability and `echo-v1` model. The provider returns
`echo: ` followed by the exact UTF-16 content of the final user message. The
selected first real provider is OpenAI Responses with subscription-backed ChatGPT
OAuth; its adapter and sign-in flow are still future work.

## Guest calls and waiting frames

CoreLib declares `Renraku.Model.Complete(string) -> string` and
`Renraku.Model.Chat(ModelBinding, ChatMessage) -> string` as explicitly
whitelisted InternalCalls. Their exact owner, names, and signatures are checked by
the loader. Complete accepts a non-null managed string and captures a single user
message with the default local binding. The host must opt in through
`Vm::set_model_calls_enabled(true)` or the trailing `--allow-model-calls` flag
on `run`, `run-persistent`, or `resume`. This policy is disabled on every fresh
VM and restore; a saved guest universe cannot grant host authorization.

The call performs one guest transition: remove the prompt from the evaluation
stack, create a prepared outbox record, install `AwaitEffect` in the active
frame, and mark the execution `Waiting` for that effect ID. The continuation
stores the call site, return offset, effect ID, and final user content ID. Older
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
unfinished effect and final user content, that the call targets Complete or Chat,
and that offsets and the post-argument stack shape are valid. Only the active
frame may await an effect. Intrinsic methods themselves never have saved frames.

## Captured conversation and model binding

`Renraku.ModelBinding` has Capability and Model string fields.
`Renraku.ChatMessage` has Role (0 system, 1 user, 2 assistant), Content (string),
and Next (ChatMessage) fields. These are ordinary mutable managed objects,
allocated and linked through ordinary CIL. Chat requires non-null arguments,
non-null content, an acyclic chain of 1–128 messages, and a final user message.
The aggregate content limit is one million UTF-16 units. Unsupported roles,
cycles, oversized requests, and malformed bindings fault with the original
call operands intact and no published effect.

Capability and model identifiers contain 1–128 ASCII letters, digits, or
`._-/:` characters. Capturing copies these identifiers and the ordered roles
into the effect record and retains each immutable content string by logical ID.
The effect's legacy request reference identifies the final user content.
Changing links, roles, content fields, or the model binding later cannot change
a captured request. Retained content strings participate in checkpoint tracing
and GC until the effect is forgotten or another guest root retains them.

The host `queue_chat_request` API accepts the same binding and message references.
`ModelProvider::supports_model` is checked before submission or receipt lookup;
the default and local implementation accept only `echo-v1`. The transport receives
a `ModelRequest` with the captured capability, model, and full ordered contents.
No credentials or native provider resources enter the managed request.

## Request state and commits

Each effect has a stable opaque `EffectId`, capability and model names, captured
message references, a final user content reference, and one of these states:

| State | Meaning | Permitted delivery action |
| --- | --- | --- |
| Prepared | Request recorded; no dispatch marker committed | Submit after committing the marker |
| InFlight | Dispatch marker written before external code is invoked | Reconcile only |
| OutcomeUnknown | No committed response proves the external outcome | Reconcile only |
| Completed | Managed response string committed with the effect record | Return the saved response |

`Vm::queue_model_request` creates an in-memory prepared record bound to
`local.echo` / `echo-v1`. `CheckpointStore::save` persists it along with heap, code,
roots, statics, and guest frames. The outbox retains its request and completed
response as GC roots. The host must save queued work before expecting it to
survive a crash.

`Vm::dispatch_model` checks the provider capability and model support, changes prepared to
in flight, and saves the complete universe **before invoking the provider**.
A save failure prevents submission. The provider receives the immutable request
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

## Bounded host driver

`Vm::drive_model(store, provider, DriverLimits)` coordinates checkpointed guest
execution and delivery of the active guest's waiting request. The current driver
requires host opt-in and a local.echo provider. It does not drain unrelated
host-created outbox records.

The caller supplies an instruction budget, a provider-operation budget, and a
positive checkpoint interval. Both submissions and reconciliation lookups consume
one provider operation, including a lookup that finds no receipt. Instructions
are charged by actual execution, so stopping early at a call does not consume
the unused part of a chunk. Budgets reset on each explicit driver invocation;
they are host policy, not durable quotas. Budgets that would overflow the
cumulative instruction counter are rejected before execution.

The report identifies completion, instruction exhaustion, provider-operation
exhaustion at a waiting call, or an unresolved outcome, with exact usage counts.
Every successful return saves the final state. Exhausting the instruction budget
leaves an ordinary runnable computation paused. A provider budget of zero still
allows pure guest execution until it waits or completes. An instruction budget
of zero still allows delivery to an already waiting caller, after which it pauses
without executing another instruction.

A prepared waiting effect follows the existing dispatch protocol. An uncertain
effect gets one receipt lookup; no receipt saves the waiting state and stops.
A submission or lookup error returns immediately without an automatic retry
or a second provider operation. A later explicit invocation may reconcile.
Guest faults are saved with their original frames and diagnostics before their
error is returned. Checkpoint failures remain errors, with no successful report.

The CLI form is:

```text
drive-local <universe.db> <provider.db> <instruction-budget> <provider-operation-budget> [checkpoint-interval] --allow-model-calls
```

It loads an existing universe and returns a nonzero status on unresolved
uncertainty or another error. Budget exhaustion returns normally with the saved
stop reason. The interval defaults to 100 guest instructions. Limits bound
operation counts, not wall-clock time or the duration of a synchronous provider
call. The CLI does not connect to a remote model.

## Local provider ledger

`LocalModelProvider` stores receipts in a separate SQLite database using WAL
and full synchronization. Its application ID is `0x52454d4f` and schema version
is 2. It rejects foreign or nonempty unmarked databases.

Each receipt holds the effect ID, versioned request bytes, exact UTF-16 response
bytes, and the number of submission attempts. Repeating a submission with the same ID and
request returns the existing response; reusing the ID for a different request
fails. Reconciliation validates the request against the receipt and does not
increment the submission count.

Request format 2 contains canonical body version 1: one version byte, length-
prefixed UTF-8 capability and model names, a 32-bit message count, then each
role byte, 32-bit UTF-16 unit count, and little endian units. Lengths are little
endian. The effect ID remains the separate receipt key. This binds all roles,
contents, and order, including earlier messages with the same final prompt.

Opening a schema 1 ledger upgrades it transactionally by adding a request-format
column. Existing receipts stay format 1, retaining their original raw UTF-16
prompt bytes. They match only the legacy single-user local.echo / echo-v1 request.
They cannot be reinterpreted as a different model or structured conversation.

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
Structured requests additionally enforce that limit across all message contents.

`Simulation.ModelHarness` stores conversation history in ordinary linked
`ModelMessage` objects, makes two requests through three nested CIL frames,
and returns 417 in 158 guest instructions. The first response becomes the next
prompt with the same managed object ID. Static guest storage retains all four
messages after completed outbox records are forgotten and collected. The
CoreCLR reference uses an echo shim; it is excluded from the persistent guest
assemblies.

`Simulation.ChatHarnessScore` extends and edits a linked conversation between
two Chat calls, then changes the source binding after completion. It returns 11,
matching CoreCLR. The first captured history retains its original system text
and binding, and the next request includes the first response by the same ID.

There is no real model connection, credential storage, streaming, tool
execution, cancellation, or automatic retry yet.
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
- Original format 1 through 4 checkpoints restore and upgrade to format 5.
- A format 4 waiting call and its schema 1 accepted receipt upgrade together
  and reconcile without resubmission.
- Structured histories restore at every instruction and through process kills;
  receipts reject changed earlier content, roles, message order, or models.
- A fixed golden encoding covers the canonical request body and arbitrary UTF-16.
- The driver obeys independent instruction and operation budgets across fresh
  invocations, including zero budgets, and leaves unrelated outbox records alone.
- Actual process kills during the driver's first and second submissions recover
  via receipt lookup; a kill after driver completion does not resend anything.
- Missing receipts stop the driver and CLI without guest progress or submission.
  Faults, authorization failures, and stale waiting drivers remain explicit.
