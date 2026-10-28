# Persistent Object Universe
**Architecture & Implementation Specification — Draft v0.1**

*Working title: TBD — spiritual successor to RenrakuOS*

**Status:** Initial design  
**Primary implementation:** Rust  
**Initial guest language:** C# compiled to CIL  
**Frontend:** Web  
**Deployment:** Userspace runtime on existing operating systems

---

## 1. Vision

Build a persistent, introspectable, dynamically modifiable computational environment in which objects, executable code, and active computations are first-class inhabitants of a shared universe.

Unlike traditional applications, this environment is not fundamentally organized around processes, files, or application lifetimes.

Instead:

- Objects have durable identities independent of memory addresses and process lifetimes.
- Reachable managed objects persist without application-level serialization.
- Active computations, including their call stacks and execution positions, can survive runtime shutdown and restart.
- Classes, methods, and higher-level system behaviors can be inspected and modified while the environment is running.
- Eventually, objects and computations can migrate between machines without changing their logical identities.
- Humans and AI agents can inspect, extend, and reshape the environment using the same underlying facilities.

The system should feel less like an application platform and more like a **living, persistent computer implemented on top of conventional operating systems**.

The eventual user experience should combine the malleability of Smalltalk, the programmability of Emacs, the durability of an object database, and the flexibility of a distributed runtime.

These are architectural aspirations, not dependencies for the first release.

## 2. Foundational Principles

### 2.1 Orthogonal persistence is fundamental

Persistence must be a property of the runtime's object model, not a library feature.

Guest programs must not need to serialize objects, define persistence mappings, or explicitly reload their application state.

Objects reachable from persistent roots, including reachable execution contexts, are candidates for durable retention regardless of their types.

Durability is guaranteed at defined transaction/checkpoint boundaries, not necessarily after every individual instruction.

### 2.2 Execution is data

An executing computation must have an inspectable, portable representation within the runtime.

This includes:

- Call frames and their ordering.
- Method identities and implementation versions.
- Instruction positions.
- Arguments, locals, and evaluation stacks.
- Exception-handling state.
- References to managed objects.
- Scheduler state necessary for resumption.

The fundamental execution model must not depend on retaining native CPU registers, stack addresses, or operating-system thread identities.

### 2.3 Runtime modification is normal

Methods, classes, commands, tools, and higher-level services should be replaceable without requiring a wholesale restart.

Active computations should not automatically become invalid when an implementation changes.

A minimal trusted Rust substrate may remain outside the mutable universe. The goal is to make all higher-level environment behavior modifiable, not to require a self-hosting operating system.

### 2.4 Identity is independent of location

An object's logical identity must not depend on its host machine, memory address, storage page, or execution process.

Even though the initial runtime is single-node, its identity and reference model must not require redesign to support distribution.

Location transparency must not imply that remote operations are free, instantaneous, or immune to network failures.

### 2.5 Compatibility is optional

CIL and C# are selected because they provide a mature language and instruction format, not because full .NET compatibility is a foundational requirement.

Compatibility features may be added incrementally when valuable.

**No compatibility requirement may dictate the fundamental persistence or execution model.**

### 2.6 Every phase must stand alone

Each major development phase must produce an independently demonstrable result.

Later capabilities must not become prerequisites for proving earlier milestones.

## 3. High-Level Architecture

The system consists of five major components.

### 3.1 Native runtime substrate

Implemented in Rust.

Responsible for:

- CIL metadata loading and interpretation.
- Managed object allocation and reference resolution.
- Persistent heap management.
- Execution scheduling and continuation management.
- Transactional storage and crash recovery.
- Runtime introspection primitives.
- Safe execution boundaries and external resource interfaces.

It should remain small and deliberately avoid implementing a full .NET runtime.

### 3.2 Persistent object universe

The logical heap contains:

- Ordinary managed objects and arrays.
- Persistent roots.
- Logical type and method definitions.
- Versioned executable code.
- Execution contexts and their frames.
- Runtime-defined services and abstractions.

The object universe must not be coupled to a particular storage engine.

### 3.3 Guest programming environment

C# serves as the initial authoring language.

Roslyn compiles source into CIL against a deliberately minimal custom core library.

The runtime loads the resulting metadata and IL, and interprets the supported instructions.

Other languages targeting suitable CIL subsets may be supported later.

### 3.4 External resource and compatibility layer

A separate mechanism exposes nonpersistent resources and conventional operating-system facilities.

Examples include:

- Files and sockets.
- Native libraries.
- Operating-system processes.
- Conventional .NET libraries.
- GPU and device resources.

Initially, these are explicitly bounded capabilities rather than transparently persistent objects.

### 3.5 Web frontend

A lightweight web application connects to the runtime through a management API and event stream.

The frontend provides inspection, development, debugging, and interaction facilities.

It owns no authoritative universe state.

Browser refreshes, disconnections, and frontend process restarts must not interrupt guest computations or invalidate persistent objects.

## 4. Managed Object Model

### 4.1 Object identity

Every managed reference object receives a stable logical identifier.

Identifiers must:

- Remain valid across runtime restarts.
- Be independent of physical storage locations.
- Be suitable for eventual globally unique identity.
- Survive changes to an object's class implementation or physical layout.

An object reference resolves through the runtime's object manager, rather than encoding a native memory address as its durable representation.

### 4.2 Object representation

A logical object contains:

- Stable object identity.
- Logical type identity.
- Schema/layout version.
- Field values or equivalent payload representation.
- Internal metadata required for storage and garbage collection.

The physical layout is an implementation detail.

Primitive values and value types may be represented inline where appropriate. Managed references must resolve through stable logical identities.

### 4.3 Persistent roots

The runtime provides a named root registry.

Reachability from these roots, or from other retained runtime roots such as durable execution contexts, determines the persistent live graph.

Objects no longer reachable from any retained root may be reclaimed.

No per-class persistence annotations are required.

### 4.4 Transient arenas

The runtime also supports explicitly transient allocation domains.

Transient objects may reference persistent objects.

Persistent objects may not retain direct references to transient objects that could disappear. Such references must be rejected or represented through an explicit durable resource abstraction.

Transient arenas are intended for temporary computation, native interoperation, compatibility modules, and other resources unsuitable for ordinary persistence.

### 4.5 Type identity and evolution

Logical type identity must be separate from assembly file identity or a particular compiled version.

New definitions can potentially update existing logical types.

Changes to object layouts require versioned schemas and migration policies.

Initial implementations may restrict compatible modifications to simpler cases. The full schema-evolution mechanism is a later milestone.

## 5. CIL Execution Engine

### 5.1 Initial strategy

Implement a portable CIL interpreter in Rust.

Do not begin with a JIT compiler.

The interpreter defines the canonical semantics of execution, persistence, and suspension.

Native optimization may be introduced later, provided optimized computations can be reconstructed into canonical VM state at supported safe points.

### 5.2 Supported instruction subset

The first implementation must support enough CIL to compile and execute simple C# programs involving:

- Primitive values and arithmetic.
- Local variables and arguments.
- Conditional and unconditional branching.
- Basic object allocation.
- Instance fields.
- Static fields with persistent runtime-defined storage.
- Method calls and returns.
- Basic class and constructor behavior.

Features such as generics, delegates, exceptions, async/await, interfaces, byrefs, unsafe code, and native interop should be introduced according to explicit dependencies rather than promised immediately.

Unsupported instructions must fail clearly during validation or execution, never silently change semantics.

### 5.3 Minimal core library

Provide only the definitions necessary for the supported C# compilation profile.

This will initially include core compiler-required types such as `System.Object`, `System.ValueType`, `System.String`, primitive types, and related required metadata.

The exact minimum set should be established experimentally using Roslyn.

No initial requirement exists for LINQ, conventional collections, threading APIs, filesystem APIs, or broad BCL compatibility.

### 5.4 Execution contexts

Each guest computation is represented by an execution context managed by the VM.

Its state includes a stack of VM frames, an execution status, and a scheduler identity.

Each frame stores the information required to resume a particular method version at a particular instruction boundary.

References into stack frames, including managed byrefs when eventually supported, require logical relocatable representations.

Unsupported native pointers or operations may temporarily make a computation ineligible for checkpointing or migration.

### 5.5 Scheduling

Begin with a single-threaded cooperative guest scheduler.

Multiple guest execution contexts may be supported without requiring multiple simultaneous native execution threads.

The scheduler must provide safe points at which execution can be paused, inspected, and durably checkpointed.

The design must permit later concurrent execution, but multithreaded guest semantics are not an initial requirement.

## 6. Persistence and Recovery

### 6.1 Storage model

Use an existing transactional storage engine initially rather than implementing a new database.

SQLite in WAL mode is a reasonable starting candidate, subject to testing.

The runtime is responsible for converting managed objects and execution state into a canonical, architecture-independent durable representation.

This serialization is internal to the VM and must be invisible to ordinary guest programs.

### 6.2 Checkpoint consistency

A durable checkpoint must represent a mutually consistent state of:

- Persisted heap objects and references.
- Persistent roots.
- Execution contexts and frames.
- Code versions required by those frames.
- Necessary scheduler and runtime metadata.

A restored execution must not observe a heap snapshot inconsistent with its saved call stack.

The first implementation may pause guest execution at safe points to produce atomic checkpoints.

Incremental and concurrent checkpointing are future optimizations.

### 6.3 Durability semantics

The initial guarantee is:

**After an unexpected process termination, the runtime can restore the most recent successfully committed, internally consistent checkpoint.**

It is not required to recover from the exact instruction being executed at the moment of failure.

Checkpoints should eventually be generated automatically according to configurable runtime policies, with explicit controls also available to developers.

### 6.4 Garbage collection

Persistent reachability determines long-term object retention.

The first implementation may use a simple tracing collector over persisted roots and objects.

Compaction, generational collection, and advanced incremental collection are deferred.

Garbage collection must respect any object and code versions retained by active computations or recovery checkpoints.

### 6.5 External effects

External effects are not automatically atomic with persistent heap transactions.

The runtime must provide an explicit boundary for operations such as network requests, external database writes, or filesystem changes.

Initial capabilities may use transactional outboxes, idempotency keys, or other effect-specific recovery policies.

The system must never claim exactly-once external execution without an appropriate protocol.

## 7. Live Code and Object Evolution

Executable definitions should be durable, inspectable entities.

The environment should support:

1. Compiling a new method or type definition.
2. Validating it against the runtime's supported semantics.
3. Storing the new definition as a versioned entity.
4. Updating logical dispatch to the new implementation.
5. Retaining older implementation versions while active frames require them.
6. Reverting the dispatch change when necessary.

Existing stack frames should initially continue using their original method versions.

New calls may resolve to newer versions.

On-stack replacement and live migration of active frames between method implementations are future capabilities.

Class layout evolution requires explicit migration rules. Arbitrary incompatible changes must not be silently applied.

Changes should ideally be tested in isolated, versioned branches of the object universe before becoming visible to running workloads.

## 8. Management API and Web Experience

The runtime must expose a management interface supporting:

- Listing and resolving persistent roots.
- Inspecting objects and their relationships.
- Reading type and method metadata.
- Enumerating active, paused, and suspended computations.
- Inspecting call stacks, locals, and instruction positions.
- Pausing and resuming execution.
- Triggering durable checkpoints.
- Loading compiled code.
- Inspecting runtime errors and events.

Initially, a simple HTTP API with WebSocket events is sufficient.

The web interface should begin with an object explorer, execution inspector, and basic code-loading controls.

A full IDE, graphical application framework, and persistent UI description language are explicitly deferred.

The management endpoint should be local-only by default, with authentication and capability controls required before remote exposure.

## 9. AI Agent Integration

Agents are expected to become first-class participants in the environment, but agent integration must not be necessary to prove the core runtime.

Eventually, agents should be able to:

- Inspect object graphs and execution state.
- Read and modify runtime definitions.
- Compile and install new code.
- Construct new tools and interfaces.
- Create isolated test branches.
- Propose and apply validated runtime changes.

Agent experimentation must not be allowed to unintentionally produce external side effects.

The runtime should provide transactional mutation, version checks, bounded execution, and controlled capabilities.

Agents should use the same fundamental inspection and modification interfaces available to human developers.

## 10. Future Distribution Model

Distribution is a planned capability, not an initial implementation requirement.

### 10.1 Location-independent identity

Object and execution IDs remain stable when their physical locations change.

References resolve through a future ownership or location-routing system.

### 10.2 Execution migration

A computation may eventually be:

1. Suspended at a valid safe point.
2. Checkpointed with its referenced state.
3. Assigned to another runtime node.
4. Reconstructed on that node.
5. Resumed under the same logical execution identity.

The transfer protocol must ensure that only one node has authority to execute a particular context at a time.

### 10.3 Distributed objects

Objects may eventually be hosted by different nodes while participating in a shared logical universe.

The runtime must explicitly handle:

- Network failures and partitions.
- Remote call latency.
- Ownership and concurrency.
- Object migration and routing.
- Distributed transaction boundaries.
- Capabilities and authorization.

Transparent identity and invocation are desirable. Pretending remote access has identical cost and reliability to local memory access is not a requirement.

Initial distribution may use a single authoritative owner per object, avoiding the need to solve distributed shared-memory coherence.

## 11. Implementation Phases

Each phase has a strict exit criterion. Work outside the phase's scope should not block its completion.

### Phase 0 — Semantic foundation

**Goal:** Settle the minimum persistent execution model.

Deliverables:
- Object identity and reference specification.
- Persistent/transient lifetime rules.
- Canonical value and frame representation.
- Transaction and checkpoint semantics.
- Initial CIL compatibility profile.
- Focused acceptance tests.

**Exit:** The model is precise enough to implement without inventing fundamental semantics during development.

### Phase 1 — Minimal CIL runtime

**Goal:** Execute simple C# programs inside a Rust-hosted virtual machine.

Deliverables:
- CIL assembly and metadata loader.
- Basic object allocator and in-memory heap.
- Interpreter with explicit managed frames.
- Minimal custom CoreLib.
- Basic execution and inspection CLI.

**Exit:** A C# program can allocate an object, mutate a field, call methods, execute a loop, and return correctly.

No persistence is required yet, but all execution state must already use portable VM-owned representations.

### Phase 2 — Persistent object heap

**Goal:** Make ordinary managed objects survive runtime termination.

Deliverables:
- Stable object IDs.
- Root registry.
- Durable object and reference storage.
- Atomic heap checkpoints.
- Recovery and basic garbage collection.

**Exit:** A nontrivial object graph survives a full runtime shutdown and restart without guest-level serialization.

### Phase 3 — Persistent execution

**Goal:** Make computation state durable.

Deliverables:
- Persistent execution contexts.
- Portable frame encoding.
- Checkpoint-safe scheduling.
- Atomic checkpointing of heap and execution state.
- Recovery and resumption.

**Exit:** A computation with multiple active frames resumes from a committed checkpoint after the entire Rust process is forcibly terminated.

Repeated crash/recovery cycles must preserve coherent behavior.

**This is the first major project milestone.**

### Phase 4 — Introspection and live evolution

**Goal:** Make the universe useful and modifiable while running.

Deliverables:
- HTTP management API.
- Browser-based object and execution explorers.
- Persistent code definitions.
- Runtime method replacement.
- Versioned code retention.
- Initial change rollback.

**Exit:** Modify a method in a running computation, observe new calls using the replacement, and restore the prior implementation without losing the persisted state.

### Phase 5 — Transient integration

**Goal:** Give the universe access to useful external facilities without weakening persistence.

Deliverables:
- Transient allocation arenas.
- Resource handle abstractions.
- External effect boundaries.
- An initial native or host-process integration.
- Defined checkpoint restrictions for nonportable state.

**Exit:** Persistent computations can use an external capability and recover correctly under its documented failure and restart policy.

### Phase 6 — Process mobility

**Goal:** Prove that execution identity is independent of a host process.

Deliverables:
- Multiple runtime processes.
- Execution ownership handoff.
- Portable checkpoint transfer.
- Host fencing and recovery.
- Reference resolution across processes.

**Exit:** A computation moves between two independent runtime processes while retaining its logical identity and execution state.

### Phase 7 — Networked universe

**Goal:** Extend process mobility across machines.

Deliverables:
- Globally resolvable object identities.
- Remote invocation and reference routing.
- Cross-machine execution migration.
- Remote ownership management.
- Failure-aware communication.

**Exit:** A computation can migrate between x86-64 and ARM64 hosts, resume execution, and continue accessing a referenced object remaining on the original host.

### Phase 8 — Ecosystem and optimization

Potential work includes:

- Broader CIL and BCL support.
- More sophisticated C# integration.
- Concurrent execution.
- JIT compilation with safe-point state reconstruction.
- Rich persistent graphical environments.
- Automated object placement and migration.
- Full agent-driven environment evolution.

These are opportunities rather than commitments.

## 12. Explicit Non-Goals for the Initial System

The project must not initially attempt to:

- Implement a standalone operating system or kernel.
- Bootstrap its compiler or runtime inside itself.
- Reimplement the complete .NET BCL.
- Execute arbitrary third-party .NET assemblies.
- Implement a JIT compiler.
- Support unrestricted native interop.
- Implement transparent distributed shared memory.
- Provide a complete graphical application platform.
- Reproduce the entire Smalltalk or Emacs ecosystem.
- Solve all live schema evolution cases.
- Provide full security isolation for untrusted code before the required enforcement mechanisms exist.

These features may eventually be valuable, but none is necessary to validate the core project.

## 13. Engineering Requirements

The project should favor correctness, inspectability, and simplicity over early performance.

Requirements:

- The interpreter must have deterministic, testable semantics for its supported instruction subset.
- Durable formats must be versioned and architecture-independent.
- Fault injection must be used to test interrupted checkpoint writes.
- Recovery must never silently accept inconsistent durable state.
- Every supported CIL instruction should have focused execution tests.
- Differential tests against CoreCLR should be used where ordinary CIL semantics are intended to match.
- Execution and object state must be inspectable without relying on native debugger internals.
- Unsupported capabilities must produce explicit errors.
- Storage engine details must not leak into the guest language's object semantics.

Performance optimization should occur only after the core persistence and recovery tests pass reliably.

## 14. First End-to-End Demonstration

The definitive early demonstration should be a C# application that:

1. Creates a persistent object containing mutable state.
2. Starts a computation with several nested active calls.
3. Repeatedly modifies the object while executing.
4. Is automatically checkpointed by the runtime.
5. Is interrupted by forcibly killing the Rust host process.
6. Is recovered by launching a fresh host process.
7. Resumes at the saved instruction position with its call stack and references intact.
8. Continues executing against the same logical object identities.

The test must prove that the recovered frame state and heap contents correspond to the same committed checkpoint.

The guest program must not implement its own serialization, restart recovery, or reconstruction logic.

**If this works, the project's foundational architectural thesis has been demonstrated.**

## 15. Open Design Questions

The following require explicit decisions or experiments before their dependent phases:

- How should persistent object values be encoded and indexed?
- How should managed byrefs and interior references be represented?
- What are the minimum safe-point requirements for resumable CIL?
- Should persisted execution contexts be ordinary heap objects or separately indexed runtime entities?
- How should static fields and type initialization behave across recovery?
- What is the initial consistency model for concurrent guest computations?
- How should incompatible type migrations interact with active frames?
- What checkpoint frequency and write amplification are acceptable?
- How should execution branches isolate external resource operations?
- Which managed BCL implementations are practical to reuse without CoreCLR?

Not all of these need answers before Phase 1. Each should be settled only when it becomes a genuine dependency.

## 16. Project Success Criteria

The project is successful at several independently meaningful levels.

**Foundational success:** Persistent objects and execution contexts survive runtime termination and resume correctly.

**Development-environment success:** Code and object definitions can be inspected and rewritten live, making the universe genuinely useful as an interactive programming environment.

**Mobility success:** Computations can move between runtime processes without losing their identities or execution state.

**Distributed-universe success:** Objects and computations can inhabit different machines while participating in one coherent logical environment under explicitly defined distributed semantics.

**Long-term vision:** The environment becomes a continuously evolving computational world in which developing software and using software are largely the same activity.

---

## Final Design Constraint

**Never sacrifice orthogonal persistence or portable execution semantics to achieve compatibility with an existing runtime, framework, or library.**

The project exists to explore a different foundational model of computing.

Compatibility, performance, UI sophistication, and distribution should be built around that model—not the other way around.
