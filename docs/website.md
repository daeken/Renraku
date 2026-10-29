# Persistent localhost website

The first HTTP edge runs ordinary CIL handlers against persistent managed state.
An accepted request is an invocation inbox record containing its exact text
arguments, pinned method/code, and portable execution. Heap updates and the
immutable response string commit before the host writes a socket response.

## Run the demo

Build the current guest and choose an unused universe path:

```sh
dotnet run --project tools/GuestCompiler -- . target/guest
cargo run -p renraku-vm -- run-persistent target/website.db target/guest/Renraku.CoreLib.dll target/guest/Renraku.Simulation.dll Renraku.Guest.Website::Initialize 1000 100
cargo run -p renraku-vm -- web-serve target/website.db Renraku.Guest.Website::Handle 127.0.0.1:9000 10000
```

Open [the local page](http://127.0.0.1:9000/). Record a visit, stop the host, and
restart only the `web-serve` command against the same database and port. Reloading
the page shows the saved visit IDs. `Website.Visits` and `Website.History` are
ordinary managed statics. The guest includes routing, page content, mutation, and
response construction, without serialization or checkpoint calls.

`web-serve` prints its bound address and universe as one JSON line before serving.
Port 0 chooses an available port. An optional final connection limit permits
bounded fixtures; the default is to keep serving until stopped. The instruction
budget applies to each managed request attempt and caps at one million. Serving
saved code requires neither the SDK nor the original DLLs after initial loading.

The small host bootstrap supplies the page shell and transport script. Its
browser requests carry fresh IDs; one pending request is retained in
`sessionStorage`, scoped to the origin and universe. A broken request can be
retried with its original ID. Guest buttons are disabled while an unresolved
request is pending. The managed demo supports GET `/page` and an empty POST
`/visit`, retaining at most 64 visits. New read IDs show current state; replaying
an old read ID returns its original saved page.

## Request and response contract

Handlers use this initial ABI:

```csharp
public static string Handle(string id, string method, string target, string body)
```

The return value is a non-null immutable managed string containing HTML, bounded
to 32 KiB of UTF-8. HTTP-v1 successful responses use status 200 and
`text/html; charset=utf-8`. Custom managed status codes, headers, and binary bodies
are future work. Guest application routing errors can return an HTML page under
this same status. Unpaired UTF-16 surrogates, null, or oversized response strings
produce a terminal 500 response. Original strings remain intact in the universe.

Managed requests require `X-Renraku-Request-Id`, with 1..64 ASCII letters, digits,
dots, underscores, or hyphens. For example:

```sh
curl -i -X POST -H 'X-Renraku-Request-Id: visit-one' http://127.0.0.1:9000/visit
curl -i -X POST -H 'X-Renraku-Request-Id: visit-one' http://127.0.0.1:9000/visit
```

Both responses have the same execution ID and body. The guest mutates once.
Repeat that ID with changed method, target, or body to receive 409 without a
second invocation. These four text arguments are the complete supported request
data; headers beyond transport/framing/identity are not exposed to the guest.
Targets are passed exactly, including percent encodings and query text, with no
decoding or Unicode normalization. Bodies are exact valid UTF-8, including NUL.

GET `/` without an ID serves only the host bootstrap, without guest execution or
an inbox record. Other managed requests without an ID reject. Successful managed
responses include `X-Renraku-State` and `X-Renraku-Execution`. A budget-paused
handler returns 202 / `pending`; retry the same request to resume its frames.
New IDs receive 503 while another execution is active. A completed older receipt
can still be replayed without starting a new context.

## Durable inbox and recovery

Receipt IDs are `http-v1/<universe>/<client-request-ID>`. Before the first handler
instruction, `WebHost` saves the pinned method/code, four immutable argument
references, and one execution through `invoke_once`. A retry first checks those
retained arguments and reuses the saved execution or terminal receipt. It never
allocates replacement argument identities for a known request.

Code, arguments, frames, managed state, and result share the ordinary atomic
universe checkpoint. Receipt references remain GC roots. The string result is
immutable, so later requests cannot alter an earlier response. The HTTP-v1 fixed
response mapping and header profile make completed replay deterministic. Pending
202 and infrastructure errors are transport feedback, not terminal guest results.
Storage errors discard the unsaved host VM; the next attempt reloads its latest
checkpoint before returning a result. A socket write failure cannot undo a
committed invocation and never authorizes another execution under that ID.

Faulted handlers have terminal fault receipts and return 500 on replay. Guest
mutations committed before a fault are retained; a whole method is not a rollback
transaction. An invalid response string likewise does not restart its completed
handler. Inspect these receipts and frames with:

```sh
cargo run -p renraku-vm -- inspect-checkpoint target/website.db
cargo run -p renraku-vm -- invocation target/website.db http-v1/UNIVERSE_ID/visit-one
```

A restarted host may choose a different installed handler for new IDs. Existing
requests keep their original binding and saved result. The initial host selector
is fixed until restart; a durable route table and live route publication remain
future work. The host also pins its universe ID and rejects an unexpected database
replacement. No sockets, native threads, or browser storage become guest state.
The existing checkpoint format remains 7, with readers for formats 1 through 6.

## Supported HTTP profile and bounds

The dedicated blocking host binds only loopback IP addresses and processes one
connection at a time. Each accepted socket has an overall five-second read
deadline and five-second write timeout. The implemented HTTP/1.1 subset accepts
GET/POST, strict CRLF framing, one nonempty Host header, and optional decimal
Content-Length. An absent length means an empty body. Framing follows the
[HTTP/1.1 message-length rules](https://www.rfc-editor.org/rfc/rfc9112.html#section-6.3)
within this restricted endpoint profile.

Limits are 8 KiB of ASCII headers, 32 fields, a 2,048-byte request line, a
1,024-byte ASCII origin-form target, and an 8 KiB UTF-8 body. Duplicate headers,
transfer/content codings, Expect, buffered excess/pipelined bytes, and other
methods or protocol versions reject before acceptance. Chunked transfer,
keep-alive, streaming bodies, upgrades, TLS, CORS, cookies, authentication, and
ASP.NET compatibility are unsupported. Every response closes the connection.

The global invocation-receipt limit is 10,000 with no eviction, and the existing
128 MiB checkpoint limit still applies. Serving does not enable model calls or
other host capabilities for the handler. Same-universe parallel contexts and
guest Listen/Accept/Task APIs are not implemented. The HTTP edge is an external
host seam; it does not persist a native networking stack inside CIL.

## Validation

```sh
cargo test -p renraku-vm --test web
```

Eight tests exercise real TCP and native CLI delivery, immutable old-page replay
after later mutations, changed-data conflicts, every handler instruction with
GC/restore, pending-work exclusion, disabled model calls, terminal fault/invalid
text receipts, malformed and unsupported HTTP, failed checkpoint recovery, and
fragmented UTF-8/NUL request and response bytes. A native CLI fixture runs with no
tools on PATH. Subprocess SIGKILL tests stop before execution, after a partial
mutation, and after response commit before socket delivery. The same port rebinds,
and retry returns one mutation under the original execution ID.

A browser smoke test recorded visit `e6625702-7e2b-4df8-9e56-f66024f46d1b` in universe
`746a4bbe12be43eb8fb13fcce285cc4e`, restarted the host on the same port, and reloaded
the same saved visit. Local artifacts are `target/web-demo-20261029.db` and
`target/web-demo-20261029.png`. The demo server was left available at
`http://127.0.0.1:56903/` for this session; restart it manually if the process ends.
