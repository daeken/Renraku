# Renraku initial semantic contract

Renraku runs CIL computations inside a persistent object universe. CIL is the
execution foundation for guest languages. External programs may be invoked as
capabilities, but their stacks, memory, and operating system resources do not
acquire automatic persistence.

The first milestone is coherent recovery of shared objects and nested active
calls after a host process is killed. The next workload is a basic LLM harness,
which introduces remote operations with uncertain outcomes.

The Rust crate implements the initial CIL interpreter, identity and value model,
heap lifetime rules, tracing collection, explicit frames, and atomic durable
checkpoints. The [CIL profile](cil-profile.md) defines the executable subset and
[checkpoint contract](checkpoints.md) defines recovery. A host outbox implements
durable external request intent, reconciliation, and portable waiting CIL calls.
Live dispatch replacement, type initialization, scheduling of multiple contexts,
and real provider capabilities remain requirements for subsequent work.

## Identity and object records

Every identity is an opaque 16-byte value. Rust types distinguish object, type,
field, method, code version, execution, effect, arena, and assembly IDs. IDs contain no addresses,
file offsets, database row numbers, or assembly metadata tokens. Null is an
explicit value, distinct from every object ID, including the all-zero ID.

Runtime identities are minted as random UUIDs; the low-level heap also accepts
caller-supplied IDs and rejects duplicate allocations. Foundation tests use
deterministic IDs. Checkpoints preserve the IDs and loader bindings. A fresh
assembly load creates fresh identities; recovery reparses with the saved IDs.

An object records its ID, logical type ID, schema version, allocation domain, and
fields keyed by stable field IDs. Field IDs are identities of declarations;
inherited fields with the same name remain distinct. Assembly tokens
resolve through a loader-owned mapping into these logical identities. A type or
method's logical identity is separate from each installed implementation version.

The low-level heap checks reference existence and lifetime. The interpreter uses
the loaded metadata registry to validate declared fields and slot types, and to
initialize fields before invoking constructors. Schema migration is future work.

## Allocation domains and roots

Ordinary allocation uses the managed domain. Reachability from named roots,
retained execution contexts, static storage, interned literals, and effect records
determines the durable object graph. Allocation alone does not retain an object
forever. Sharing and cycles preserve logical identity.

Reference stores obey this initial lifetime policy:

| Reference owner | Permitted reference target |
| --- | --- |
| Managed object | Managed object |
| Transient object | Managed object or transient object in the same arena |
| Named persistent root | Managed object |
| Retained durable execution | Managed object |

Cross-arena transient references are initially rejected so that one arena can
eventually be released independently. Rejected stores and rejected root
replacements leave the previous state intact. Dangling object references are
errors.

A running context may temporarily hold transient references. It cannot enter a
durable checkpoint until it releases them or replaces them with an explicit
durable capability record. A checkpoint must fail clearly on such references;
it must never silently omit a frame value. Arena allocation and disposal are
later work; the initial heap only models their lifetime rules.

## Initial values and primitive encoding

The implemented value subset is null, a 32-bit integer, a 64-bit integer, and an
object reference. Integer signedness is determined by the operation and declared
storage type. Floating point, native integers, aggregate value types,
arrays, and managed byrefs need explicit extensions before they can execute.
Boolean, Char, and Int32 enum slots use 32-bit stack values in the current profile.
Immutable strings are reference objects with exact UTF-16 payloads and durable
literal interning, rather than an additional inline primitive value kind.

Primitive value format version 1 is encoded as follows. All listed payload
widths are exact; trailing data and truncated data are rejected.

| Value | Version byte | Tag byte | Payload |
| --- | --- | --- | --- |
| Null | 1 | 0 | None |
| Int32 | 1 | 1 | 4 bytes, little endian, two's complement |
| Int64 | 1 | 2 | 8 bytes, little endian, two's complement |
| Object reference | 1 | 3 | The ID's 16 opaque bytes, in order |

Unknown versions and tags are errors. The complete checkpoint format has its
own version and consistency checks; this value codec is a component of that
format. Host-sized integers, Rust struct layouts, and native enum
discriminants must never become durable encodings.

The CIL implementation will follow [ECMA-335](https://www.ecma-international.org/wp-content/uploads/ECMA-335_6th_edition_june_2012.pdf)
for supported instructions. Its evaluation stack type rules must be implemented
explicitly. In particular, small integer storage types load into wider stack
values, and floating point storage and the internal evaluation type require
separate treatment. These types are outside the initial value subset.

## Frames and call continuations

An execution context has a stable execution ID, a status, and an ordered stack of
VM frames. The first frame is the oldest caller; the last is the active callee.
Contexts begin as separately indexed runtime records, rather than requiring a
guest-visible implementation of the runtime's own stack machinery.

Each frame records:

- Logical method ID and the exact immutable code version ID.
- A continuation containing unsigned 32-bit byte offsets into that CIL body.
- Arguments and local slots.
- An evaluation stack ordered from bottom to top.
- Any object that must be produced when a pending constructor call returns.

A local containing `None` is uninitialized; a local containing `Some(Null)` holds
a guest null reference. The interpreter must honor method initialization metadata
and reject reads of uninitialized locals.

An `Execute` continuation identifies the next instruction to run. An
`AwaitReturn` continuation identifies both the call site and the instruction to
run after successful return. Its arguments have been removed from the caller's
evaluation stack and installed in the callee; its result is not present yet.
A normal call takes its result, if any, from the callee. For `newobj`, the
constructor returns void and the caller instead produces the allocated object
recorded in its continuation. This retained reference participates in tracing.

An `AwaitEffect` continuation identifies a capability call site, its return
offset, the stable effect ID, and the prompt object ID. Its argument has moved
into the outbox and its result is absent. Only the active frame can await an
effect, and the context's waiting status must name that same unfinished effect.
Delivery atomically pushes the saved response, advances the continuation, and
makes the context runnable. A waiting context does not advance by running again.

Entering a call must atomically move arguments, set the caller's continuation,
and install the callee frame. Returning must atomically remove the callee, produce
the appropriate caller result, and advance the caller's continuation. No safe
point may expose only part of either transition.

Frame records are inspectable data. The loader and interpreter validate
instruction boundaries, signatures, operand indices, stack limits, return types,
and call continuations. Recovery additionally validates saved frame counts,
slot types, reachable positions, call relationships, and stack shapes. A complete
static CIL verifier remains future work. Exception handlers and byrefs are
unsupported until their full resumable state is represented. A VM fault message
is a diagnostic, not a guest exception representation.

Active frames retain their code versions even when logical dispatch changes.
New calls resolve the current implementation and pin it in their new frame.
Completed contexts retain returned object references until those contexts are
released. Paused and faulted contexts retain references in their remaining frames.

## Safe points and atomic checkpoints

Guest execution starts with one cooperative native execution thread. A guest
instruction is the smallest execution transition visible to checkpointing. A
safe point is a validated boundary before the next instruction, including a
complete call or return transition. Individual host writes made while executing
an instruction do not create intermediate safe points.

A checkpoint pauses all guest execution and commits the following together:

- The reachable managed heap and named roots.
- Retained contexts, their continuations, and all referenced values.
- Referenced code versions, type schemas, and logical dispatch bindings.
- Static storage and type initialization state.
- Durable effect records and, when supported, scheduler state.

The implementation uses SQLite in WAL mode with full synchronization. It commits
a versioned, checksummed full snapshot and validates recovery before exposing an
execution. Returned errors and actual process kills before commit leave the
previous checkpoint recoverable. Missing code, invalid offsets, dangling
references, unknown formats, and inconsistent state are rejected.

Recovery reconstructs one committed state. Runnable contexts resume from saved
continuations; paused contexts remain paused. Instructions after the last commit
may execute again, with their uncommitted heap changes rolled back together.
Logical dispatch and static state are restored from that same checkpoint.
Garbage collection must include execution roots and any retained recovery state,
and must retain code versions needed by active frames.

## Static storage and type initialization

Static fields belong to a logical type within a universe, rather than to a host
process or a newly opened assembly file. Their values are retained runtime roots.
An initial type initialization record distinguishes not started, running with an
owning context, completed, and failed. A running initializer's frames and static
writes participate in the same checkpoint as that record.

Recovery must not rerun an initializer whose completion is committed. It may
resume a committed running initializer, or replay uncommitted initialization from
the previous checkpoint. The precise CIL initialization triggers and reentrancy
rules require loader and interpreter tests. External capabilities are initially
unavailable during type initialization. Thread-local statics and concurrent type
initialization are deferred.

## LLM harness and external effects

The first harness should retain a conversation, a sequence of requests, completed
responses, and tool invocation records as ordinary managed state. One completed
response per model request is enough initially; token streaming is deferred.
The provider adapter is a host capability. Its socket, credentials, and native
stack are not ordinary persistent objects.

The effect protocol uses a durable outbox:

1. Guest execution records request intent, a stable effect ID, and its waiting
   continuation. The runtime commits these together before dispatch.
2. The host records an in-flight state durably before submitting the request.
3. The host submits through the named capability, supplying an idempotency key
   when the provider offers a suitable protocol.
4. The runtime commits the response and makes the waiting computation runnable
   in one transaction.
5. Resumption consumes the stored result; it does not repeat a completed effect.

The current [effect outbox](effects.md) implements this protocol for
`Renraku.Model.Complete` and `Renraku.Model.Chat` using the deterministic local echo provider. Receipts
commit outside the universe transaction. A linked C# conversation makes two
requests through portable waiting frames; response storage and guest wakeup
commit together. The CLI can queue, dispatch, inspect, and reconcile requests.
New guest model calls require explicit host opt-in on every fresh VM and restore.
The bounded host driver now coordinates execution and local delivery, counting
guest instructions and provider submissions or lookups independently. It stops
on a missing receipt and does not drain unrelated host requests. Its limits are
per-invocation host policy; they are not durable spending quotas. Real providers,
streaming, tools, and explicit retry policies remain later work. The selected
first real adapter is OpenAI Responses.

Chat captures a capability/model binding and ordered role/content messages from
ordinary linked guest objects. Captured identifiers and roles are values; content
strings retain their logical IDs and immutable UTF-16 payloads. Later guest
mutations cannot alter a saved request. Provider receipts compare the full
versioned request body, and unsupported saved models fail before external code.

An in-flight record recovered without a committed response has an unknown
outcome: the provider may have accepted and charged for the request. The runtime
must reconcile using the provider's supported protocol or expose that uncertainty
for an explicit retry policy. An idempotency field alone does not establish
exactly-once delivery. A subprocess or tool invocation needs its own effect and
recovery policy, including any external side effects it produces.

Required crash cases include termination before intent commit, after intent
commit, after marking in flight but before submission, after provider acceptance,
after receiving a response but before committing it, and after response commit
but before guest resumption. Begin with a scripted local adapter that can expose
each window deterministically; connect a real provider after these cases pass.

This minimal effect protocol follows the core crash-and-resume proof, before the
broader native integration phase in the architecture draft. Branches used for
experimentation must default to effects disabled. Model selection and the
credential mechanism remain host configuration decisions. The user has selected
the OpenAI Responses API using their ChatGPT subscription through OAuth for the
first real provider adapter.

## Acceptance criteria

The current tests exercise primitive encoding and corruption rejection, cyclic
and shared graph retention, rejected reference stores, transient arena boundaries,
and references held across nested frames and pending constructor results.

The initial executable acceptance test now compiles real C# with Roslyn against
the custom CoreLib and interprets its emitted method bodies. The simulation
allocates an object, mutates a field, executes nested calls and a loop, writes
static storage, and returns 48. Twenty guest cases are compared with CoreCLR,
including the linked conversation harness with a local echo reference shim.
Instruction-level pause/resume, frame and instruction budgets, static roots, and
unsupported capabilities have additional tests. This establishes the initial
runtime and its supported compilation profile.

The durability acceptance test now checkpoints a shared cyclic graph and several
active frames, kills the host process, and resumes in a fresh process using the
same logical identities and matching heap and stack state. Repeated crashes and
fault injection recover the previous complete checkpoint or the newly committed
complete checkpoint. The guest contains no serialization or reconstruction logic.
See [checkpoint validation](checkpoints.md#validation) for the tested cases.
A future live evolution test must replace method
dispatch while retaining an old active frame, checkpoint, and recover both code
versions with their intended bindings.
