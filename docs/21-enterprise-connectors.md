# Enterprise connectors and advisory security

`tentaflake-integrations` provides an optional Linux Unix-socket front door and
read-only library, separate from public Research. `tentaflake.integrations.enable`
defaults to false. Live accounts, provisioning, deployment and model calls have
not been exercised; synthetic tests are not provider or model acceptance.

## Read-only connector contract

The trusted host constructs one immutable `Grant` and `Gateway` per authenticated
agent/account. The host must bind the caller to that gateway; `agent` is policy
metadata, not proof of peer identity. Requests are closed JSON with only tenant,
account, resource, read method and generation. Unknown fields fail. They cannot
supply endpoints, credentials, HTTP verbs, scopes, policy, page tokens or replay
instructions. Tenant/account equality and exact resource/method allowlists are
checked before credential access and dispatch and again before releasing data.
Revocation is irreversible for a gateway; replacing a generation requires a new
host-owned gateway. In-flight responses after revocation or expiry are discarded.
Revocation cannot retract bytes already sent to a provider.

The complete reviewed operation catalogue is:

| Method | Provider API | Fixed read route |
| --- | --- | --- |
| `drive_metadata` | Drive v3 | `GET https://www.googleapis.com/drive/v3/files/{id}`; fixed metadata fields |
| `drive_file` | Drive v3 | Same route with `alt=media`; bounded binary content |
| `gmail_message` | Gmail v1 | `GET https://gmail.googleapis.com/gmail/v1/users/{account}/messages/{id}?format=full` |
| `hub_spot_contact` | CRM v3 | `GET https://api.hubapi.com/crm/v3/objects/contacts/{id}`; fixed email/name properties, `archived=false` |

All IDs are single encoded path segments, never paths or queries. Dot segments
are rejected. Each operation reads **one resource and one response**: there is no
list/search endpoint, pagination traversal, batch, attachment fetch, Drive native
document export, send/share/write/delete, login or refresh capability. This is a
bounded read-by-ID catalogue, not full provider feature coverage. Provider payload
links, next-page tokens and instructions never trigger secondary requests.

Bodies are capped at a host-selected 1 byte–4 MiB, concurrency at 1–16, and request
deadlines at at most 60 seconds. Raw request JSON is capped at 4096 bytes. JSON
responses must be objects whose `id` matches the authorized resource; malformed
JSON and excessive nesting fail through the bounded serde parser. Files remain
untrusted bytes. Consumers must not render provider HTML as trusted markup or
execute downloads. Provider errors and audit records contain no response bodies,
credentials, URLs, account/resource IDs or raw network diagnostics.

`HttpsTransport` uses the existing broker's exact reqwest 0.12.28 with TLS hostname
verification. Redirects, proxies and retries (including default protocol NACK
retries) are disabled. Host provisioning supplies at most 16 reviewed public IP
pins for the fixed provider hostname; no request-time DNS or DNS fallback occurs.
Private, loopback, link-local, mapped IPv6, multicast and conservative special-use
ranges are refused. This prevents rebinding without unbounded synchronous DNS in
the request path. Pin refresh and provider endpoint ownership remain trusted host
responsibilities; arbitrary public IPs are not evidence of provider ownership.
The same transport and TLS policy must be retained by any service integration.

`RuntimeCredentials` holds one token only in process memory, bound to the fixed
provider/tenant/account and grant generation. The exact root Unix peer may call
`credentials/rotate` with `{token,generation,expires_unix,read_only_consent}` or
`credentials/revoke` with `{}` after protocol initialization. Expiry is required,
at most 24 hours away; generation and explicit read-only consent must match.
Rotation invalidates old in-flight leases; revoke discards the token and old
in-flight results. Restart always starts without credentials and does not reload
or refresh them. Root provisioning attests the opaque token's actual provider,
account and read-only scopes: the service cannot introspect OAuth claims. A
rotation changes the runtime lease, not the immutable host grant. Changing the
grant requires a reviewed config and service restart.

Only the privileged local provisioning channel carries a token; never put one
in Nix, argv, agent tool JSON, UI sessions or durable state. No credentials are
provisioned during development. Memory zeroization and protection against a
compromised trusted host are not claimed.

The synthetic `Transport` seam receives only already authorized canonical routes;
it is trusted test/embedding code, not a selectable agent backend. It must honor
caps/deadlines; the gateway independently checks returned status, body size,
elapsed time and revocation. `AuditSink` is likewise trusted bounded local code.
Calls have no automatic recovery/replay queue; one read invokes transport once.
Explicit new reads may retrieve the same resource. There is no durable operation
ledger, restart deduplication guarantee, or write/replay capability.

### Optional front door and deployment

The module owns `tentaflake.integrations.enable`, `package`, `configFile`, and
`advisory.enable`. Configuration is root-owned, not group/world writable, bounded
to 64 KiB, outside the Nix store and provided at runtime. The service runs as
`tentaflake-integrations`, without capabilities or writable host access beyond
its runtime/state directories. No host/container engine socket is required.
The runtime JSON has fixed `socket`, `caller_uid`, `reviewer_uid`, `agent`,
`tenant`, `account`, `provider`, `resources`, `methods`, `generation`,
`expires_unix`, public IP `addresses`, `max_body_bytes`, `deadline_seconds`,
optional `advisory` and optional `activity`. Use socket
`/run/tentaflake-connectors/front.sock`. Root must install the runtime config
readable by the service; it contains no token. UIDs for caller/reviewer are
distinct and non-root. File permissions are not authentication: every connection
is authenticated using Linux `SO_PEERCRED` before accepting requests.

The wire format is newline-delimited JSON-RPC 2.0, at most 64 KiB per request,
five seconds per incoming/outgoing frame, 64 messages per connection, 16 ordinary
connections plus two reserved root slots, and eight provider reads. Provider
output rechecks lease generation and expiry before each write; already delivered
bytes cannot be retracted. Start with `initialize` using
`protocolVersion: "2024-11-05"`, object `capabilities` and `clientInfo`, then
the `notifications/initialized` notification. Request IDs are bounded strings
or unsigned integers, unique within a connection. Batches, unknown envelope
fields, malformed frames and invalid lifecycle transitions fail closed.
No remote MCP, arbitrary URL or stdio launcher is advertised.

The caller UID gets `tools/list` and `tools/call` only. The sole tool is
`connector_read` with `{resource,method}`; tenant/account/generation are injected
from the immutable grant, never caller-selected. The reviewer UID gets
`connections/list`, `proposals/list` and `proposals/review` only. Connection
metadata contains tenant, agent, provider, generation, expiry, credential
availability and `read_only: true`, never tokens or account/resource IDs.
UI bridges must verify service peer UID and exact tenant/agent association.
No UI role can rotate/revoke credentials or alter connector grants.

Drive file results are a JSON text content item with `{encoding:"hex",data:"..."}`,
preserving arbitrary bytes without interpreting markup or UTF-8. Decoding does
not confer permission to execute a file.

Optional `activity` is `{socket,server_uid,producer_id,run_id,workload_id,
policy_revision}`. The service verifies the activity collector's Unix peer and publishes
version-one `publish_activity` frames using four-byte big-endian lengths.
The collector must independently configure the exact service UID/producer/tenant/
agent/run/workload/generation/policy grant. Events have random session-qualified
IDs and omit provider content, tokens, URLs and resource/account IDs. Projection
failure emits only `connector_activity_gap`; it never retries provider work.
Publication uses a nonblocking Unix socket and one absolute one-second deadline
covering connect, request write and framed reply reads. A full listener backlog
fails immediately rather than blocking connector threads. Missing/revoked
credentials, expired grants and ungranted resources pass through the gateway's
single minimized terminal audit; credential unavailability is a denied decision.
An unavailable telemetry sink does not replace the provider result.

Custom connector extensions require reviewed compiled changes to the closed
`Provider`/`ReadMethod` catalogue, fixed route/origin, TLS pins, grant checks and
negative tests. `Transport` is a trusted embedding/test seam, not runtime plugin
selection. There is intentionally no URL/template/script-based custom proxy.

## Evidence-only advisory contract

`advisory::Advisory` accepts trusted per-system policy with exactly one configured
observer identity, an explicit admission bit, a resource allowlist and bounded
telemetry/finding content. The embedding authenticated channel supplies observer
identity; logs and classifier output cannot supply it. Evidence references must
match supplied allowed-resource telemetry. Missing evidence, missing model output,
non-admission and mismatched identity fail closed. The host must validate reference
provenance and redaction before supplying telemetry: reference matching alone does
not authenticate a source or prove a finding true.

The only proposal payload is `ReviewProposal` in `AwaitingHumanReview` state,
with category, text and evidence references. It has no action, activation,
lifecycle, credential or policy grant and no remediation executor.
Enable `advisory.enable` to select the explicit `serve-advisory` command;
ordinary `serve` refuses an advisory config. Runtime advisory configuration is
`{observer_uid,policy,state_directory}`; use state directory
`/var/lib/tentaflake-integrations`. Policy has the fields of `advisory::Policy`.
The observer UID is distinct from caller, reviewer and root, and alone may
submit `{telemetry,finding}` via `proposals/submit`. It may not review.
The host observer is responsible for authenticating/redacting evidence before
submission; this service validates evidence linkage, not source truth.

Proposals persist in a private locked, bounded store (100 rows, 8 MiB).
The persisted envelope binds the tenant, account/provider, system, agent,
resource/method grants and caller/reviewer/observer identities. A restart under
a different context refuses to open the store; legacy unbound stores also fail
closed. Nothing is deleted or silently reassigned. Preserve the original store
and configure a distinct private directory for a new context.
Publication fsyncs the file and directory before success; uncertain writes
poison the process until restart, and abandoned temporary files are never
replayed. `proposals/list` returns `{proposals:[{id,proposal,review}]}`.
`proposals/review` takes `{id,decision}` with only `acknowledged` or `dismissed`.
The first review wins; identical repeats are idempotent, conflicting repeats
fail. Pending cannot be restored through the API. Acknowledgement never grants
approval or execution authority. Store exhaustion fails closed and needs an
explicit owner retention procedure; no automatic evidence deletion occurs.
Combined finding/telemetry bytes and at most 64 evidence references are bounded.

Real Pi Durable observer admission remains **blocked** by unverified upstream
recovery/no-replay controls. Keep `Policy.admitted = false` in production until
that separate boundary is accepted. This crate does not replace Pi Durable, run a model
loop, or claim reliable injection classification. Hostile log fixtures demonstrate
that classifier content cannot acquire action authority, not that every malicious
instruction can be recognized.

## Documentation basis and verification

Provider contract lookup used the configured Context7 documentation service:

- [Drive v3 files](https://github.com/googleapis/google-api-python-client/blob/main/docs/dyn/drive_v3.files.html)
- [Gmail v1 messages](https://github.com/googleapis/google-api-python-client/blob/main/docs/dyn/gmail_v1.users.messages.html)
- [HubSpot CRM v3 contacts](https://github.com/hubspot/hubspot-api-python/blob/master/_autodocs/endpoints-crm-1.md)
- [reqwest retry policy](https://docs.rs/reqwest/latest/src/reqwest/retry.rs.html)

Provider documentation matches the named API major versions, not a live account
acceptance. Context7's reqwest response is current documentation rather than an
exact 0.12.28 snapshot; parent compilation must verify that pinned API compatibility.
Synthetic tests cover tenant/account/resource/method/generation denial, expiry,
revocation including in-flight discard, closed request schemas, route encoding,
response bounds/malformed IDs, redirect/private-target refusal, concurrency,
redacted audit, absence of write/replay inputs and advisory evidence/admission.
No checks, builds, formatter, network/provider calls or live model runs were
performed by the source worker. Parent integration should run the owning fast
gates and focused integrations unit tests after workspace registration. Source
and fixtures do not establish runtime containment, enterprise readiness or live
provider health.
