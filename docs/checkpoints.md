# Checkpoint storage and recovery

Renraku can restore a supported CIL computation after its host process is killed.
A committed checkpoint contains a mutually consistent heap, static storage,
code bindings, and execution stack. Guest code requires no serialization or
restart logic.

## Commit contract

`CheckpointStore::save` runs at a complete instruction boundary. It traverses
named roots, static storage, active frames, pending constructor results,
retained completion results, interned literals, and effect requests and responses.
Unreachable objects are omitted. Transient or
dangling references cause an error before the transaction starts.

The store encodes one complete universe and commits it as one SQLite row in a
transaction. Each successful commit increments a generation. The database uses
WAL mode and `synchronous=FULL`, following [SQLite's transaction durability
settings](https://www.sqlite.org/pragma.html#pragma_synchronous). Full snapshots
favor simple consistency over write amplification.

After termination, recovery returns the last committed generation. An interrupted
write cannot publish a mixture of generations. If no first checkpoint committed,
there is no execution to recover. The store retains one checkpoint; it does not
provide historical snapshots.

Both a store connection and a VM remember the generation from which they last
read or committed. A writer must match the current database generation and
universe identity. Stale VMs and stale connections must reload before writing.
This prevents stale replacement within one database; it is not a distributed
execution ownership protocol.

The observer API exposes `Prepared`, `Written`, and `Committed` boundaries for
fault injection. Before `Written`, SQLite's dirty cache is flushed so tests can
kill a process with uncommitted WAL pages present. An observer error before
commit rolls back. An observer error after `Committed` leaves the successful
commit intact, and both the VM and connection retain its acknowledged generation.

## Durable representation

The database has application ID `0x52454e52` and schema version 1. The checkpoint
row contains a generation, universe ID, payload format version, SHA-256 checksum,
and binary payload. The checksum detects damaged payloads; it is not an
authentication mechanism.

The writer emits payload format version 5 and still reads versions 1 through 4. All start
with the eight bytes `RENRAKU\0` followed by a
32-bit little endian version number and a 16-byte universe ID. It then records:

1. Original assembly bytes and their assembly identities.
2. TypeDef token to type ID bindings, FieldDef token to field ID bindings, and
   MethodDef token to logical method and immutable code version bindings.
3. Reachable objects with object ID, type ID, schema version, and payload.
4. Named roots and static fields.
5. Interned literal string identities.
6. Durable effect records.
7. Execution identity, status, optional result, and original entry method version.
8. The cumulative instruction count and configured frame limit.
9. Frames with pinned method versions, continuations, arguments, locals, and
   evaluation stacks.

IDs are opaque 16-byte values. Counts, byte lengths, IL offsets, schema versions,
and frame limits use unsigned 32-bit little endian integers. The instruction
counter uses an unsigned 64-bit little endian integer. Values use primitive
format version 1 from [the semantic contract](semantics.md#initial-values-and-primitive-encoding).
Tags and optional-value flags are explicit bytes. Root names and diagnostics use
UTF-8; guest string payloads use UTF-16. No Rust layouts,
native addresses, or host-sized integers are serialized.

Version 2 adds a byte after each object's schema version: zero identifies a
field map and one identifies an immutable UTF-16 string. String payloads contain
a 32-bit unit count followed by little endian 16-bit units. It also adds the
interned string ID registry after static storage. Version 1 has field maps
directly after the schema and no intern registry.

Version 3 adds the outbox after the intern registry. Each record has a 16-byte
effect ID, a 16-byte managed request string ID, a length-prefixed UTF-8 capability
name, and a state tag. Tag 0 is prepared; tag 1 is in flight; tag 2 is completed
and adds a response string ID; tag 3 is outcome unknown and adds a length-prefixed
UTF-8 diagnostic. Versions 1 and 2 have no outbox and restore with it empty.

Version 4 adds execution status tag 4 (waiting), followed by its 16-byte effect
ID, and frame continuation tag 2 (await effect), followed by call-site and return
offsets, effect ID, and request string ID. Existing status and continuation tags
are unchanged. The new tags are rejected under earlier payload versions.

Version 5 appends a length-prefixed model identifier and captured message sequence
after each effect's state payload. Each message is a role byte (0 system, 1 user,
2 assistant) and a managed content string ID. The sequence has a 32-bit count
and must contain 1–128 messages ending with a user message. Identifiers and the
aggregate UTF-16 content limit are validated on save and restore. Formats 3 and 4
restore with model echo-v1 and one user message referencing their existing request.

The next successful commit writes version 5. Unmodified fixtures captured from
the earlier writers verify active version 1 frames, exact version 2 strings,
and a version 3 completed effect alongside active frames. A format 4 waiting call
has a matching original schema 1 provider receipt, proving upgrade and
reconciliation without resubmission. All upgrade without
changing their identities. Host authorization for future model calls is never
serialized; it defaults to disabled after restoration.

Encoding orders identity maps deterministically. Decoding rejects duplicate
bindings, fields, roots, and object identities, unknown tags or versions,
truncated data, and trailing bytes. The current payload limit is 128 MiB and
each collection is limited to one million entries. These are initial format
limits, not performance targets.

## Restore validation

Recovery checks the checksum and format, then reparses the stored assemblies
using their saved logical identities. It does not mint replacement identities,
infer missing metadata, or consult the original assembly paths. Extra or missing
bindings and duplicate logical IDs are rejected.

Object schemas must be version 1 and match their complete inherited field sets.
Static storage must match its declarations. Every retained reference must resolve
to a managed object of a compatible type. Two-stage allocation reconstructs
cycles before installing field references.

String payloads must belong to System.String, have no ordinary instance fields,
and respect the unit limit. Intern entries must resolve to matching retained
literal contents. Pure string intrinsics cannot appear as suspended frames,
because they complete within one instruction.

Effect IDs must be unique, capability/model names must be valid 1–128 byte ASCII
identifiers, and
request, captured content, and completed response references must resolve to
managed strings. A request's final content must match its legacy request reference.
Recovery exposes saved in-flight records as outcome unknown: there is no saved
response proving whether the external provider accepted the request. Restoration
never invokes a provider.

For each retained method, control flow analysis reconstructs evaluation stack
shapes, including nominal reference types and common base types at merges.
Recovery checks argument and local counts and types, local initialization,
stack height and types, valid reachable instruction positions, and maxstack.

Every non-active caller must await a callee matching its call instruction and
code version. Return offsets must follow the saved call sites. Pending
constructor results must have the allocated type. The active frame must point at
an executable instruction or a valid capability call awaiting an effect.
An effect waiter must match the waiting execution's effect ID and an unfinished
outbox record, including its final user content. Its call must target the exact
Model.Complete or Model.Chat intrinsic; Complete additionally requires the
default binding and single user message. Its next offset must follow that call,
and its stack must match the call position after removing the arguments.
Completion status requires an empty stack of frames and a result matching the
entry signature.

This validator covers the supported profile and its checkpoint states. It is not
a complete static CLI verifier or an isolation boundary for hostile code.

## Execution and collection

`Vm::run_checkpointed` executes bounded chunks and checkpoints each chunk,
including completion. Intermediate automatic checkpoints mark the computation
runnable. Exhausting the requested budget saves a paused computation. Restoring
a snapshot preserves that status; the explicit `resume` command runs it again.
Fault diagnostics and their original frames can also be checkpointed. A model
call saves a waiting context and returns its effect ID; running it again before
delivery makes no progress.

Instructions and heap changes after the last commit may run again on recovery.
That is safe for the current managed instruction set because their saved heap
and stack state roll back together. The host [effect outbox](effects.md) commits
its dispatch marker before calling a provider. It commits the response, updated
guest continuation and result stack, and runnable status in the same snapshot.
Restoring does not dispatch effects or grant authorization for new model calls.

`Vm::collect_garbage` traces the same object roots and removes unreachable
objects from the live heap. Checkpoints also omit them. Current snapshots retain
all loaded code and its identity mappings. Code replacement, schema migration,
and collection of obsolete code versions are later work.

## Validation

The suite checkpoints and restores at every one of the first demo's 155
instructions, comparing frames, retained objects, roots, statics, code identities,
and instruction counts. It includes partially executed constructors and nested
calls. Another test covers inherited references and control flow merges.

The process crash probe reaches a mutable cyclic graph with at least three
active C# frames, commits it, and blocks at a test gate. The parent kills the
actual Rust host process, verifies SIGKILL on Unix, and restores in a fresh
process. Further kills before commit and after dirty WAL writes retain the old
checkpoint. Three more committed kill/recovery cycles preserve the universe,
execution, object, and code identities. The final computation returns **5526**,
matching CoreCLR.

Additional tests cover interrupted first commits, rollback on returned errors,
errors reported after commit, stale writers, corrupted or malformed payloads,
missing code and objects, invalid instruction offsets and stack types, roots,
garbage collection, completion, and fault diagnostics.

These tests establish process-crash recovery on the development host. The format
is architecture independent by construction; cross-architecture execution and
physical power-loss testing have not yet been performed.

String tests repeat checkpoint recovery at every guest instruction, preserve
literal identity across later executions, collect temporary concatenations,
retain exact NUL and surrogate units, and kill processes during checkpoint writes.

Outbox tests verify dispatch cannot precede a committed marker, a stale VM cannot
submit, and a failed response commit cannot expose an uncommitted cached result.
Process kills before and after local provider acceptance recover as uncertain;
receipt lookup completes accepted requests without submitting again. Malformed
outbox records are rejected even if their checksum has been recomputed.

The linked conversation harness restores at every guest instruction and effect
boundary. Process kills before intent commit, before provider acceptance, after
acceptance, and after response/wakeup commit preserve the corresponding guest
state. Repeated recovery completes two requests once each and retains all four
linked messages, matching the CoreCLR reference. Additional corruption tests
reject mismatched waiting IDs, prompts, call sites, offsets, stack shapes, and
completed effects with waiting continuations.

The bounded host driver uses the same checkpoint and delivery paths. Tests kill
its process during the first and second submissions and after completion, then
resume with exact instruction and provider-operation accounting. Missing receipts
leave a waiting guest without resubmission. Budget stops and guest fault
diagnostics are saved, and a stale waiting driver cannot invoke a provider.
The driver adds no persistent state of its own.
