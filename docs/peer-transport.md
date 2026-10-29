# Authenticated loopback owner transport

`renraku_vm::peer` now connects two native processes and two saved universes to
[durable owner instance calls](owner-calls.md). It authenticates bounded messages,
commits caller intent before sending, reconciles accepted owner work after lost
replies, and commits caller results before returning them. This is a host API
and native CLI milestone. Ordinary CIL imported references and guest wait frames
are still future work; this is partial network-transparency evidence.

## Authentication and scope

The trusted host configures an owner universe/node/epoch, loopback socket address,
and one 32-byte shared secret for each authorized caller UniverseId. The server
checks the configured owner context against the saved registry. At most 32 peers
are allowed; duplicate principals and keys shared between principals reject.
A received UUID selects a candidate key but grants nothing without its MAC.
Grants still restrict the authenticated principal to the original target/code.

Requests and responses use full HMAC-SHA-256 tags with distinct versioned domains.
Verification uses [RustCrypto HMAC](https://docs.rs/hmac/0.13.0/hmac/)'s
constant-time tag verifier, following the [HMAC construction](https://www.rfc-editor.org/rfc/rfc2104).
The authenticated input contains direction/domain, fixed header, caller identity,
a fresh 32-byte attempt nonce, length and exact payload. Responses echo that
nonce and bind the exact request digest plus owner/node/epoch. Request tags
cannot be reused as response tags, and an old response cannot complete a fresh
attempt. Peers possessing a shared secret are mutually trusted for that pair;
this is not nonrepudiation or hardware attestation.

Keys and nonces come from the OS random source through getrandom. PeerKey has no
serialization implementation, redacts Debug, and zeroizes its retained key on
drop. Native private key files contain exactly 32 raw bytes. Unix loading uses
no-follow/nonblocking open, requires a regular file with no group/other access,
and rejects symlinks/trailing data. write_new uses create-new/private permissions
and syncs the file; partial files fail subsequent validation. Other platforms
can supply embedding keys, but this private-file helper is Unix-only.

Credentials, sockets and native futures never enter checkpoints. Bootstrap and
key distribution are explicit host responsibilities. The channel authenticates
plaintext loopback traffic; it does not encrypt it or protect against privileged
local administrators/secret holders. Public binding, DNS endpoints, TLS,
cross-machine authentication, key rotation and automatic owner failover remain
unsupported. Do not treat a node ID or copied database as physical attestation.

## Wire version 1

Each connection carries one request frame and one response frame, then closes.
There is no pipelining or processing of additional frames; remaining bytes are
discarded when closing. Length/MAC delimit the frame, without TCP half-close/EOF
as a message terminator.

| Header/body item | Encoding |
| --- | --- |
| Magic | 8 bytes: `RNKOWN1` plus NUL |
| Payload byte length | u32 little endian, at most 65,536 |
| Caller universe | Original 16 raw identity bytes |
| Attempt nonce | 32 random bytes |
| Payload | Strict canonical UTF-8 JSON, version 1 |
| Authentication tag | 32 HMAC-SHA-256 bytes |

The receiver validates the length before allocating payload storage, verifies
the MAC before interpreting the request, and rejects truncation/unknown versions
and shapes. IDs use 32 lowercase hex digits. I64 values are canonical decimal
**strings**, including signed extrema/raw unsigned bits. UTF-16 text is an array
of u16 units, preserving NUL and lone surrogates without normalization. Void and
null string remain distinct. Existing 16-argument/4096-per-string/8192-total
bounds apply. Fault/result projections retain the owner's version-1 limits.

Connect/read/write operations have five-second bounds; each read/write loop
checks its overall monotonic deadline with millisecond socket timeouts. Owner
work is separately instruction-budgeted, at most 1,000,000 per connection; a
caller timeout does not cancel already accepted work. The owner keeps its
1024-instruction checkpoint quantum and one-context exclusion. This synchronous
host API belongs off an async executor's latency-sensitive worker.

## Durable caller lifecycle

Host caller metadata uses reserved roots under `renraku.peer-v1/`:

- `call/<operation>`: immutable canonical intent, including original caller,
  owner/node/epoch, fixed endpoint, original method/object/grant and exact input.
- `sent/<operation>`: an alias of that intent, committed before the first write.
- `ack/<operation>`: the authenticated original execution ID and intent hash.
- `reply/<operation>`: immutable terminal reply and intent hash.

There are at most 128 calls and 64 KiB per metadata record. Save/recovery validates
root agreement, caller binding, exact canonical bytes, hashes, acknowledgements,
result bounds and terminal state. All metadata commits with existing code, heap,
statics, frames, effects and receipts in format 7, with no new guest value/frame
kind. The caller does not create a private Counter object; remote identity is
recorded as data here, not yet as a guest reference. Owner registration continues
to retain its authoritative object; distributed leases/GC are not introduced.

An explicit call_once first commits intent. It connects without sending, then
commits sent before any request bytes. A connection failure before sent leaves a
prepared intent; a later explicit call can still attempt acceptance. After sent,
every explicit retry is **receipt-only** owner resume. It never falls back to new
acceptance when the owner ledger is missing. A crash after sent but before the
write can therefore remain conservatively unresolved even if no work reached
the owner. No new operation ID is generated automatically.

Authenticated Pending replies commit an acknowledgement before being returned.
Later requests include the expected execution; the owner checks that identity
before resuming. Lost/bad/unauthenticated replies leave the caller unresolved.
Owner validation, busy-state or save errors return authenticated Unresolved,
which is not evidence of non-acceptance. This deliberately conservative version
does not turn busy/missing receipts into automatic fresh acceptance; an explicit
negative-acceptance/queue contract would be separate work.

The owner commits accepted state/results before socket output. If the caller's
terminal save fails, no uncommitted result escapes; reload and reconcile the
same operation. Completed local reply replay needs no network, key, source file
or SDK. Changed calls/endpoints with the same ID reject. Transport resume uses
the original accepted authority after grant revocation; new revoked calls deny.
Secret revocation/removal independently stops authentication. Restoring an old
owner database under the same epoch/keys is not a supported ownership protocol.

## Native commands

With existing owner/caller universes, an installed export grant and privately
provisioned key, the operator commands are:

```sh
target/debug/renraku-vm peer-serve owner.db NODE_ID 1 127.0.0.1:59000 CALLER_ID private.key 10000 4
target/debug/renraku-vm peer-call caller.db OWNER_ID NODE_ID 1 127.0.0.1:59000 private.key call.json
target/debug/renraku-vm peer-resume caller.db operation-ID private.key
target/debug/renraku-vm peer-resume caller.db operation-ID
```

The last form replays a completed local receipt without credentials; a pending
call still requires them. encode_call/decode_call define the versioned request
file, including explicit method/code/object/grant IDs and lossless arguments.
The single-peer CLI uses an empty resource set; embeddings can configure a
trusted OwnerContext and multiple peers. No new agent action/model/publication
capability is exposed. Native commands keep one JSON result on stdout and
bounded generic connection errors on stderr.

## Evidence

Ten integration tests and three transport units cover real authenticated TCP,
wrong keys/principals, exact wire I64/UTF-16/Char/bool/null values, nonce/direction/
header/payload MAC binding, framing/count/shape bounds, private keys and duplicate
key denial. Caller reply-save failures, orphan/corrupt/repaired-digest journals,
missing authoritative owner receipts and Pending execution acknowledgements
retain their explicit recovery behavior.

Two fresh SDK-free native processes use separate universes. A verification server
is killed after Counter=26 commits but before writing the reply. The caller
records Unresolved, the grant is revoked, and a restarted server at the same
endpoint reconciles the original call once. After deleting the source/key and
stopping the server, native caller replay still returns the saved result. Another
request reaches 23 while the original local receipt stays 26. Caller snapshots
contain no copied Counter or secret key. All bounded verification servers exit;
existing demos and every original checkpoint fixture remain unchanged.
