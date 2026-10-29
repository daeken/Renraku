# ChatGPT subscription sign-in

The host-only `renraku-openai` crate implements Renraku's own ChatGPT OAuth
registration, sign-in, profile selection, refresh, and sign-out. It is separate
from the CIL VM and does not change checkpoint format 5.

## Try sign-in

From the repository root on macOS or Linux:

```sh
cargo run -p renraku-openai -- login
cargo run -p renraku-openai -- status
```

Login prints a link and opens the system browser. Complete Continue with ChatGPT
and return to the terminal. The command reports the profile ID and whether
ChatGPT plan permission was granted. Identity sign-in alone does not authorize
inference. The callback waits up to five minutes.

The [Responses harness](openai-responses.md) adds account-specific model discovery
and an explicitly authorized persistent CIL prompt. Login and status make no
inference request.
The flow follows OpenAI's official
[registration contract](https://developers.openai.com/siwc/token-sharing-open-source/sign-in)
and [account/session guidance](https://developers.openai.com/siwc/token-sharing-open-source/profiles-and-sessions).

Additional commands:

```sh
cargo run -p renraku-openai -- refresh
cargo run -p renraku-openai -- login --new-profile
cargo run -p renraku-openai -- select <profile-id>
cargo run -p renraku-openai -- logout
```

Refresh and logout act on the active profile. A new profile becomes active only
after its identity is verified. Returning login keeps the selected registration
and rejects a different verified account; use a new profile to add another
account. Profile IDs remain distinct even when accounts have the same email.

## Protocol and bounds

The implementation loads OpenAI's production discovery document over HTTPS,
checks its issuer and authentication endpoints, and refuses redirects. It binds
the callback to an ephemeral `127.0.0.1` port with the `/auth/callback` path.
Fresh state, nonce, and PKCE values use the operating system random source.
It retains the issued registration ID before exchanging the authorization code.

The JWT library verifies signatures against the current published keys. Renraku
checks issuer, audience, expiration, issued time, authorized party, and the
original nonce. Supported signing algorithms are RS256 and ES256. Granted scopes
come from the token response; the callback's scope parameter grants nothing.

Callbacks require an exact loopback Host and path, reject duplicate query
parameters and mismatched state, and accept one result per attempt. Headers are
limited to 8 KiB, callback connection I/O to two seconds, and service responses
to 1 MiB. HTTPS requests have a 30-second timeout and a 10-second connect timeout.
HTTP retries and redirects are disabled.

## Host credential storage

The default directory is `$XDG_CONFIG_HOME/renraku/openai`, or
`$HOME/.config/renraku/openai` when XDG_CONFIG_HOME is unset. Override it with
`RENRAKU_AUTH_DIR` or a leading `--auth-dir <directory>`.

The initial storage backend is a private, unencrypted file on Unix hosts:
directories allow only their owner, and credential files use mode 0600.
Symlinked files, hard-linked credential files, permissive permissions, and
directories inside Git checkouts are rejected. Windows credential storage is
explicitly unsupported in this milestone.

`profiles.json` stores a stable host ID, separate issued registrations, verified
identities, granted scopes, expiries, and tokens. Tokens have no debug formatter
and are absent from status output, guest objects, universe checkpoints, and Git.
Authorization URLs omit ID-token hints. Server error descriptions and token
responses are not echoed into diagnostics.

Writes use a new private file, file synchronization, atomic rename, and directory
synchronization. A process lock serializes sign-in, refresh, selection, and
sign-out. Another operation fails clearly while the store is busy. Recovery
removes abandoned private temporary files before loading the committed store.

Refresh saves a pending marker before sending the rotating refresh token.
Only a validated replacement clears it. Network failure, a malformed reply,
identity mismatch, or a process crash leaves the profile requiring sign-in;
Renraku will not send the old refresh token again. New access, refresh, ID token,
scope, and expiry values commit together.

Sign-out commits local credential removal before attempting remote revocation.
The account/client mapping stays available for a later sign-in. If revocation
cannot be confirmed, the CLI says so and points to ChatGPT Settings for
disconnecting the app.

## Verification and inference

Local HTTP fixtures exercise complete sign-in and refresh flows, invalid JWT
signatures and claims, permission withholding, callback attacks, account
separation, redacted failures, private storage, sign-out, and uncertain refresh.
A subprocess test kills a host after the refresh marker commits and the request
reaches the server; reopening the store requires sign-in and sends no retry.
The ignored probe in the test listing is invoked by that parent test.

The live production discovery document was fetched successfully. An interactive
sign-in with the user's account has not yet been run. The Responses adapter now
binds each effect to a specific profile and selected account-available model,
with durable local completion receipts. `refresh_profile(id)` renews that exact
profile independently of active UI selection. Interrupted inference remains
uncertain; see [the adapter contract and try-it commands](openai-responses.md).
