# Persistent Responses harness

`renraku-openai` now connects an ordinary CIL `Model.Chat` call to the OpenAI
Responses API through a verified ChatGPT subscription profile. The host owns
OAuth, networking, and completion receipts. The guest owns its linked messages,
model binding, waiting frame, and returned managed string. Checkpoint format 5
is unchanged.

The CLI's `demo` and `drive` commands use AsyncOpenAIProvider and the asynchronous
VM driver. Network/stream waits yield; profile/refresh and receipt I/O use a
bounded blocking pool. The existing blocking host adapter remains compatible
with the same receipts. See [async delivery and cancellation boundaries](async-delivery.md).

## Try it

From the repository root, compile the sample guest and sign in:

```sh
dotnet run --project tools/GuestCompiler -- "$PWD" "$PWD/target/guest"
cargo run -p renraku-openai -- login
cargo run -p renraku-openai -- status
cargo run -p renraku-openai -- models PROFILE_ID
```

Replace `PROFILE_ID` with the ID reported by login. Discovery lists the selected
account's available models, preserving their order. Choose a displayed slug;
there is no built-in model default. Login and model discovery do not submit
inference. The [auth guide](openai-auth.md) covers profiles and storage.

Choose an unused universe path and a private receipt directory outside Git:

```sh
mkdir -p "$HOME/.local/state/renraku"
chmod 700 "$HOME/.local/state/renraku"
cargo run -p renraku-openai -- demo \
  "$HOME/.local/state/renraku/openai-demo.db" \
  "$HOME/.local/state/renraku/openai-receipts" \
  target/guest/Renraku.CoreLib.dll target/guest/Renraku.Simulation.dll \
  PROFILE_ID MODEL_SLUG "Say hello from a persistent CIL program." \
  --allow-model-calls
```

Replace `MODEL_SLUG` with your choice. This command explicitly authorizes one
provider operation and up to 10,000 guest instructions. It sends a real request
using that profile's granted ChatGPT plan permission. It prints the completed
reply after saving it and completing the CIL method. The guest retains the
prompt and assistant message in ordinary linked objects.

To resume the same universe, including after a crash:

```sh
cargo run -p renraku-openai -- drive \
  "$HOME/.local/state/renraku/openai-demo.db" \
  "$HOME/.local/state/renraku/openai-receipts" \
  PROFILE_ID MODEL_SLUG 10000 1 --allow-model-calls
```

`drive` takes instruction and provider-operation budgets. Submissions and local
receipt lookups each count as one operation. Catalog queries and token refresh
are authorization work, outside that count. Budgets reset on each explicit
invocation; they are not spending quotas. Zero budgets are supported. A saved
completed program returns its result without inference. An unresolved stream
returns an error and stays waiting; rerunning does not authorize a resubmission.
This demo handles one prompt. The [persistent coding agent](agent.md) adds a
foreground `agent` / `agent-drive` command with bounded model-selected tools and
a managed transcript. It uses one strict JSON action per final-text response;
native Responses function calls remain unsupported. Both commands require
explicit model opt-in. A general interactive conversation command is later work.

## Profile and transport contract

The capability is `openai.chatgpt.<profile-uuid>`, and the model is the explicitly
chosen slug. A provider instance pins the profile's verified issuer, subject,
and issued registration. Changing the active profile cannot retarget a saved
request. Expired access renews the specified profile, using the existing durable
refresh marker; an uncertain refresh requires sign-in. There is no API-key
billing fallback.

Before the VM commits a dispatch marker, the adapter checks request encoding,
the profile's granted scopes and usable credentials, and model availability.
These checks do not submit inference. A rejection leaves the effect Prepared.
The generation fence must then commit before the adapter can reserve or send
an inference attempt. The bounded driver accepts an explicitly supplied host
provider; each waiting effect must match its capability and model.

The implementation follows the official OpenAI documentation for
[models and inference](https://developers.openai.com/siwc/token-sharing-open-source/models-and-inference)
and [preview limitations](https://developers.openai.com/siwc/token-sharing-open-source/preview-limitations):
OAuth bearer credentials, the public `/v1/models` and `/v1/responses` endpoints,
`store: false`, `stream: true`, and the full message history in every request.
Guest system messages map to developer messages for this route. Unsupported
request fields are omitted. Production endpoint overrides, redirects, HTTP
retries, background requests, and remote response retrieval are absent.

Text transport rejects unpaired UTF-16 surrogates before dispatch; managed
strings and checkpoints still preserve them exactly. NULs and valid surrogate
pairs retain their text. Only completed final assistant text is returned;
reasoning items and commentary are skipped. Tool calls, refusals, and other
non-text output fail explicitly. Partial text never becomes a guest result.

SSE framing supports LF/CRLF, comments, multi-line data, and fragmented UTF-8.
An absent Content-Type header is accepted, as observed during live testing;
the same bounded parser and terminal validation still apply. An explicitly
different content type is rejected with bounded transport/shape diagnostics.
Only a fully framed `response.completed` with completed status and matching
response identity can produce a receipt. Failed/incomplete events, malformed
data, missing terminal events, transport failures, and unsupported outputs leave
the attempt uncertain. Events are bounded to 8 MiB, streams to 32 MiB, and the
managed reply to one million UTF-16 units. Connection establishment times out after ten
seconds and inference requests after 120 seconds. A timeout does not prove
remote cancellation. Diagnostics expose bounded error codes and request IDs,
never arbitrary service messages or token responses.

## Durable receipts and recovery

The receipt directory uses the same private Unix directory and file policy as
auth storage, including Git exclusion. Its `receipts.db` has application ID
`0x52454f41`, schema 1, WAL, and FULL synchronization. It contains conversation
data and profile identity/registration metadata, with no OAuth tokens. Retain
the universe, receipt ledger, and original profile metadata together. Signed-out
profile metadata is enough for offline receipt reconciliation.

1. The VM saves the immutable intent, waiting CIL frame, and InFlight marker.
2. An immediate SQLite transaction reserves the effect ID, exact canonical
   request bytes, and pinned profile identity before the HTTP POST.
3. A terminal successful stream commits its response ID and exact UTF-16 text
   in the receipt ledger before returning to the VM.
4. The VM commits the response, result stack, and runnable continuation together.

A second reservation for the same ID cannot send again. A changed request or
identity fails. Concurrent hosts sharing a ledger can reserve only one attempt.
This is not a distributed ownership protocol: keep one active dispatcher per
universe and retain the same ledger across recovery.

A crash after step 3 recovers the reply locally and wakes the original CIL
continuation without another POST, even offline or signed out. A crash before
the completion receipt leaves OutcomeUnknown, even if the server accepted or
charged for the request. Missing receipts never prove non-acceptance. This
milestone has no command that retries or resolves uncertain requests by fiat.

## Verification

Local HTTP fixtures cover account pinning, discovery order, full histories,
transport parameters, UTF-16 and grant/model rejection, streamed errors,
incomplete streams, output bounds, offline receipts, changed requests, failed
receipt commits, and concurrent reservations. Actual SIGKILL tests stop during
a partial stream and after receipt commit but before VM response commit. They
restore the original CIL frames and assert that no second POST occurs. CLI
tests cover opt-in, durable preflight rejection, zero budgets, and token absence
from checkpoints. The test-only HTTP endpoints are inaccessible in production.
The [coding-agent HTTP tests](agent.md#validation) also run full managed tool
histories through this adapter, recover a compiler type error, reconcile lost
results, and preserve a committed invocation across SIGKILL. Interrupted or native
function-call streams cannot become actions; a saved completion receipt recovers
offline into one action and mutation. These fixtures do not establish autonomous
model intelligence or perform live inference.
On October 29, 2026, a separately authorized [live coding-agent run](acceptance/live-agent-2026-10-29.md)
used GPT-6.1 Sol to discover and inspect the Counter API, write/compile/install a
caller, invoke it once to reach 26, and inspect its retained source. Ten real
model turns survived planned process stops; finished recovery used zero model
operations. The invocation receipt replayed the same result and execution ID.
Live validation on October 28, 2026 verified user-completed sign-in, granted plan
permission, account-specific discovery, and a GPT-6 Luna request through the CIL
harness returning `Persistent CIL is alive.` in 49 guest instructions and one
provider operation. Reopening in a fresh process returned the same saved reply
with zero instructions and zero provider operations.

The first two live probes exposed an omitted Content-Type header and were kept
as uncertain attempts without completion receipts. They were not resubmitted.
After the parser fix, a new probe completed and saved its receipt. Reopening an
earlier uncertain probe still stopped without sending another request. Local
regression cases cover headerless terminal/partial streams, ordinary SSE MIME
parameters, declared non-SSE bodies, bounded diagnostics, and receipt behavior.
