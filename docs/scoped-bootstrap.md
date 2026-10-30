# Scoped compiler bootstrap before sealing

`renraku_vm::bootstrap` connects selected child APIs to the external compiler
without giving the compiler client the full workbench or removing the closed
child registry seal. This is a trusted **pre-seal bootstrap**, not dynamic child
installation or a compiler capability available to guest code.

## Workflow and authority

1. The trusted embedding prepares a dedicated completed, idle universe and its
   original managed objects. It installs any opaque remote handles first.
2. `bootstrap::prepare(store, &endowment)` commits a bootstrap approval and returns
   a filtered `ReferenceSet`. The approval pins the universe, original module
   images/logical bindings, remote import roots/handles, current execution ID and
   lifetime instruction count, exact API filter, object identities and effect
   pairs. It requires no invocation/effect history, owner placement, peer calls
   or prior installations. Exact retries before installation are idempotent;
   `bootstrap::references(store)` recovers the same views afterward.
3. A trusted host captures a `CompilationRequest` with these views and the exact
   original runtime core, sources, options and pinned toolchain. The external
   compiler service performs only pure compilation and retains its terminal
   artifact/diagnostics under the existing WAL/FULL policy.
4. `artifact::install_once` checks the views against the saved approval, validates
   their actual authoritative schema and atomically appends code, source,
   manifests and installation receipts. Facade IDs, hashes and declarations are
   not admission credentials. A fabricated approval hash can produce a
   structurally valid facade and compile, but cannot authorize installation.
5. The host adds selected members of these newly installed caller modules to the
   final `Endowment`, then calls `endowment::seal`. Rights over original APIs,
   objects and effects may only attenuate. Every additional module must have a
   captured scoped installation receipt for this approval. New caller rights
   still pass the ordinary child CIL admission check, including unreachable
   private tokens. Guest execution between preparation and sealing rejects.
6. Execute through the existing child tools and remote driver. Compilation,
   installation, imports and policy widening remain unavailable after sealing.

The immutable approval is a canonical managed string at
`renraku.bootstrap-v1/approval`, with typed retention roots at
`renraku.bootstrap-v1/object/<original-ID>`. These retain original objects and
reachable graphs, including edges the child cannot use. There is no release API.
The approval is at most 256 KiB, with up to 16 original modules, the existing API
list bounds, 64 object IDs and 16 effect pairs. Save/recovery rejects orphan or
noncanonical roots, wrong scope, changed original bindings/imports, invalid API
IDs and unapproved new modules. Failed preparation/installation/sealing publishes
no partial authority. Checkpoint format 7 and all older readers/fixtures remain.

## Versioned views and compiler references

Legacy full reference views retain their original format-1/2 bytes and rules.
A scoped view uses **format 3**, with a `scope` field inside the exact hashed
`schemaJson`: scope format 1, universe ID, approval hash and canonical API filter
(type/code/construction/field read/field write ID sets). A complete set contains
only views with the same scope and exactly one filtered `Renraku.CoreLib` facade.
It keeps the existing format-1 `referenceSet` envelope and dependency-first
hashing. Original logical module/type/member IDs remain authoritative; module
identity in a declaration does not expose the rest of that module.

Only selected class/member declarations enter custom facades. Denied fields,
statics, constructors, helpers and types do not enter their declaration closure;
source, method bodies, manifests, object contents and credentials are absent.
Selected declarations preserve original signedness, visibility, readonly flags
and member bindings. A private extern constructor suppresses Roslyn's implicit
public default constructor when construction is withheld. Dependency types and
bases must also be endowed. Selected unsupported shapes reject explicitly. Unselected fields/methods and
properties/events do not enter the declaration closure. The exporter retains its
conservative parameter-attribute restrictions. Existing per-module/root/type/
binding/declaration/set bounds apply.

C# fields cannot express independent read and write capability directions while
preserving their actual declaration. The facade retains that declaration; the
final admission/runtime checks enforce the separate read/write sets.

The first scoped core profile contains fixed C# predefined-type and attribute
scaffolding, a fieldless String data declaration, and optional endowed public
parameterless Object construction. Other CoreLib members, including String
intrinsics and Model APIs, are explicitly unsupported in this exporter profile.
Predefined type syntax is not permission to use unsupported runtime storage,
constructors, fields or effects. Admission and the loader still apply.

For scoped compilation the compiler emits a metadata-only core facade without a
BCL reference, then builds other facades and caller code against that filtered
core. The **original** runtime CoreLib bytes remain captured separately for
installation and execution. Neither the core facade nor any dependency facade
is installed as implementation code or used to recreate live types. No runtime
core or saved body is replaced. Ordinary legacy compilation still uses the exact
original core as before.

## Verified evidence and limits

Five integrations exercise actual external C# compilation. A selected
`Node.Value` caller updates the same original object 19 -> 26 -> 23, with the
first receipt still returning 26. Its frames/arguments checkpoint after every
instruction with GC, and an injected terminal-save failure resumes without
repeating the increment or call counter. A UInt32 result facade retains its
original `public readonly uint Value`, hides its other fields/result types and
returns 4294967295 through an unsigned-to-UInt64 C# caller.

Negative C# cannot name a hidden edge, parent static/type, withheld constructor
or Model API, and cannot write the readonly field. Forged approval hashes,
mixed/missing-core sets, unknown API IDs, rebinding, wider final scopes, failed
preparation saves and repaired-digest metadata corruption reject. A deliberately
forged captured caller contains real private-field metadata that returns the
secret in an unsealed VM; final scoped child admission rejects the same body.
Compiler visibility therefore does not replace runtime authority.

A separately compiled captured caller uses only `Node.Value` and an opaque
Counter contract. Two SDK-free native processes update the original local node
19 -> 20 and owner Counter 19 -> 26, park/recover a portable remote frame through
GC, then return 46. The caller contains no Counter declaration/state. Replay
needs neither key, server nor compiler ledger and retains one local increment,
one owner mutation, original IDs and captured source/install/invoke provenance.
The existing remote crash/revocation/lost-reply tests remain part of regression.
All bounded native verification servers exit; no live inference is used.

Post-seal child code evolution, broader scoped CoreLib APIs, per-reference
attenuation, privileged local implementation frames and returned object grants
need separate contracts. Object copying, fenced mobility and cross-machine
transport remain later milestones.
