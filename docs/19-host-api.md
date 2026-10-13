# Host environment API v1

Status: authoritative **contract only**, for [#152](https://github.com/timfewi/tentaflake/issues/152).
The dynamic host API and matching CLI environment lifecycle are not implemented.
Schema validation and fixtures do not establish backend, vendor, guest or live-host
acceptance. Existing installed-host `agent plan`/`agent import` commands retain
their documented stopped configuration-file behavior.

[`api/v1/openapi.json`](../api/v1/openapi.json) owns routes, request/response types,
bounds and stream frames. Generate client types from that document; do not maintain
a second authoritative type catalog. [`fixtures.json`](../api/v1/fixtures.json)
provides an exchange for each operation, positive examples and rejected inputs.
Exchanges are independent shape examples, not a successful lifecycle transcript.
The sample host advertises `backend: unavailable` and no login method.

Run `just api-contract` in the pinned contributor environment. The checker uses
JSON Schema 2020-12 with format validation, resolves only local references, checks
route/security/receipt invariants and validates every fixture. It does not execute
requests or prove a server implements the described enforcement.
The existing nixpkgs lock supplies `jsonschema` 4.26.0 and its required
`rfc3339-validator` 0.1.4 format checker; dependency updates follow that reviewed
pin rather than an independent Python installation.

## Transport, identity and authority

Serve the UI and API on the same private HTTPS origin. Discovery is `/api/meta`;
all other routes use `/api/v1`. Discovery reveals only minimal version, host and
configured login-method information. No permissive CORS, public listener,
arbitrary redirect, environment credential or provider credential is implied.

Cookie sessions use `__Host-tentaflake-session`, `Secure`, `HttpOnly`,
`SameSite=Strict`, `Path=/`, and no `Domain`. Rotate the session and CSRF token at
login, bound idle/absolute lifetime and verify current revocation server-side.
Cookie mutations require the session's `X-Tentaflake-CSRF` value and exact configured
Origin/Host. Login validates Origin/Host and JSON content type before consuming a
rate-limited, runtime-only single-use operator token. OIDC is optional and returns
unsupported until configured; its state, nonce, PKCE and issuer/identity checks
remain server-owned. An OIDC login-state cookie is short-lived and `SameSite=Lax`
to permit the issuer callback; the resulting authenticated session is Strict.
CLI bearer authentication is an independently configured runtime credential path,
with the same principal/grant enforcement. Reject mixed cookie/bearer credentials.

The server resolves identity, owner, resource bindings, policy/catalog/grant/host
revisions and exact grants. Requests cannot select their owner or supply new
grants. Ownership alone does not authorize an action. Missing grants, features,
legacy evidence and incomplete readiness fail closed. A feature's availability
never grants it. Cross-owner resources return `not_found` without revealing their
metadata. Responses and streams are `Cache-Control: no-store`; neither session
credentials nor private workspace data belong in browser persistent storage.

Grant targets are exact IDs, with no wildcard or parent-resource inheritance:

| Actions | Required target |
| --- | --- |
| Create environment/preset | Current host |
| Read/change/start/stop/delete environment; terminal/logs/artifacts | Exact environment |
| Workspace retention, reuse, deletion and files | Exact workspace |
| Read/update/delete preset | Exact preset |
| Read/import catalog | Configured catalog |

List endpoints filter to owned individually granted resources; a host grant does
not grant access to every child. File routes resolve the environment's current
workspace binding server-side. Upload/download additionally require ordinary
file write/read grants. Environment deletion separately requires retain/delete
authority on its exact workspace. Operation recovery is restricted to the original
principal through retained minimal receipt/tombstone authorization, including
after the target is deleted. This never restores resource authority or bypasses
principal/session revocation; private content still needs current grants.

## Admission, lifecycle and durable recovery

Plans pin owner, policy/catalog/grant/host revisions, immutable image/tool digests,
requested resources and exact retained-workspace revision. They expire within
five minutes and do not reserve resources. Commit repeats admission under a host
lock, including aggregate active limits and host/Research reserves. Changed
authority or content produces `plan_stale`; it never silently substitutes a plan.
An explicitly configured host creation-policy template proposes new resource
IDs and the exact per-resource grants in the resolved plan. Those proposed grants
have no authority before commit. Commit issues only that exact grant set atomically
with resource creation and its durable receipt, then updates the grant revision.
An absent creation policy is forbidden; host grants do not implicitly inherit to
children. Preset creation likewise issues only its configured exact grants.
Retained workspaces keep their existing ownership and grants. Starting at creation
requires the exact proposed start/startOnCreate grants. Proposed IDs reserve no
resources; collisions invalidate the plan. Environment creation within installed
ceilings must not rebuild or activate the host. A start request repeats readiness
and authority checks. Reconfiguration
previews explain live/restart/recreate/unsupported effects and bind the environment
revision; apply never implicitly starts, stops or destroys a workspace.

`container` shares the host kernel. `guest-kernel` identifies the separate-kernel
track and stays unsupported/unready until usable KVM and actual guest acceptance
exist. Never downgrade one to the other. Source checks, fixtures and operator
declarations are separately labelled evidence; they cannot become runtime
acceptance. A declared model, Research or agent capability is not proof it works.

Durable mutations require a fresh `Idempotency-Key` UUID and the server-provided
`Idempotency-Epoch`. Scope is host, authenticated principal, epoch, HTTP method,
canonical target including semantic query, and key. Under one durable claim, hash
the exact accepted request body bytes, content type and revision preconditions.
The same key with different content is `idempotency_conflict`; an exact duplicate
returns the stored receipt/result only after current authorization checks.
Record the claim before dispatch, retain unknown/interrupted outcomes after a
crash and never automatically redispatch an abandoned claim. Synchronous mutations
also return `Operation-Id`, so files, presets and terminal creation have the same
recovery path as asynchronous environment operations. Successful receipts retain
typed resource references, including the exact terminal ID and environment binding,
so losing a create response does not lose the created resource's identity.

`GET /operations` requires the exact key, epoch, method and canonical target.
Its result is `found`, `absent` or `unknown`. Only authoritative absence in a
complete healthy current-epoch ledger permits retry of that same intent and key.
An expired, pruned, corrupt, unavailable or uncertain record is never absence.
Network loss, cancellation after dispatch, malformed success or proxy failure
cannot prove rejection. Refetch the receipt and reconcile; never invent a new key
to make uncertainty disappear. `unknown` and `interrupted` operations require
backend-specific reconciliation rather than replay.

Bound operation records and bytes. Keep durable key tombstones when large results
are discarded; reject new mutations when ledger capacity is exhausted. An explicit
operator ledger-epoch rotation must invalidate old plans and require session
refresh. Old-epoch mutations fail `idempotency_expired`, and their unresolved
lookups remain unknown. Rotation never converts an old uncertain intent into a
safe fresh request. Session logout/relogin alone does not rotate the epoch.

Deletion requires stopped state, no conflicting operation, exact revision and an
explicit `workspace=retain|delete`. Retained workspaces keep their owner, quota
and revision and become unbound. Reuse atomically claims an exact retained
revision; delete rejects any bound or active workspace. Idle expiry stops compute
and attachments according to installed policy; it does not silently delete data.

## Files, transfers and streams

File paths are canonical workspace-relative paths beginning with `/`, decoded
once by HTTP. Reject dot components, residual percent escapes, control characters,
backslashes, repeated separators and non-root trailing slashes. Resolve beneath
an already verified managed workspace directory descriptor, with no symlink or
magic-link traversal and no mount escape; string-prefix checks are insufficient.
Recheck workspace binding and file identity inside the same mutation lock.
Strong compare-and-mutate additionally requires proven workspace quiescence:
stop or fence **all** environment, terminal, worker and transfer writers during
comparison and commit. An API mutex cannot serialize arbitrary workload writes.
Until the backend proves safe quiescence, refuse with `environment_running` or
`unready`; do not claim race safety from a pre-write hash alone.
Symlinks may be listed as untrusted read-only metadata, never followed. Refuse
devices, sockets, FIFOs and unmanaged filesystems. Root cannot be moved/deleted.

Strong revisions bind bytes and metadata and change whenever either changes.
Reads/downloads require the opened file's exact `If-Match` revision. Replacements
require `If-Match`; creation requires `If-None-Match: *`; exactly one is allowed.
Directory creation binds the parent revision. Moves bind both source and
destination-parent revisions and require an absent destination. Deletes bind the
entry revision and remove only files or empty directories, never recursively.
Check preconditions atomically; conflicts cannot overwrite or discard data.

Stage streamed writes inside the same quota-controlled filesystem, with aggregate
staging, byte, concurrency, idle and total deadlines. Sync and atomically rename
only after a complete accepted upload and revision check. Interrupted transfers
leave no partial target. Enforce physical quotas, including staging. Downloads
use an immutable quota-accounted regular-file snapshot, attachment disposition and `nosniff`;
never serve workspace HTML as application-origin content. V1 has no archive
extraction, URL imports or byte-range resume. Pagination binds the owner, query
and snapshot; changed/expired snapshots fail explicitly.

SSE logs/events use JSON payloads from the schema and an `id` equal to the payload
cursor. Cursors are opaque and scope-bound; conflicting `after`/`Last-Event-ID`
values are invalid. Expired replay returns `cursor_expired` or an explicit event
resync request, never silent gaps. Refetch state after resync. Bound frame bytes,
replay, rate, duration and connections. Filter replay and live events against
current grants and terminate on session expiry/revocation. Treat logs as untrusted
text and redact credentials before they enter retained buffers.

WebSocket terminal attachment uses the same cookie or independently configured
CLI bearer, exact Host/Origin checks, owner/grant checks and terminal/environment
binding **before** upgrade or any PTY input/output. Browser cookie requests without
Origin are rejected. No credentials in URLs and no attachment ticket endpoint.
Only one writer attaches per terminal. Frame input, resize, output and exit use
the shared schemas; output offsets count UTF-8 bytes at frame boundaries. Reject
expired offsets rather than replaying partial history. Never replay terminal
input after reconnect. Bound frames/backlog, terminal count, rates and idle/total
lifetimes. Closing an attachment leaves its process alive; explicit revisioned
terminal deletion terminates only the proven owned process group. Revocation
immediately closes attachments; it does not grant a host shell or delete data.

## Consumer handoff and implementation acceptance

The separate UI currently owns a provisional TypeScript contract. Its route names
are retained where practical, but migration is required before backend connection:

- Generate types from this OpenAPI document; replace provisional duplicate types.
- Read nested availability/evidence, server revisions, grants and limits. Preserve
  explicit unavailable, unready, unknown and guest/container distinctions.
- Send revisions and durable keys/epochs for all mutations, including files,
  presets, rename, retained-workspace deletion and terminal creation/deletion.
- Replace empty-array operation recovery with the three-state response; preserve
  the original intent through unknown results and expired sessions.
- Add bounded upload/download and pagination handling, revisioned file reads and
  conflict-safe moves; reconnect streams using exact cursors/offsets.
- Validate responses/frames at the client boundary; malformed responses cannot
  prove a mutation failed. Retain keyboard/accessibility and real-browser tests.

No edits or acceptance claims are made for that separately owned UI here.
Remaining #152 delivery includes a real shared admission/CLI/API service, durable
store/reconciliation, private authenticated serving, owned workspace and terminal
implementation, generated consumer migration, and actual interactions. Acceptance
must exercise cross-owner denial, CSRF/Origin refusal, revoke/expiry, concurrent
revision conflicts, crash-after-dispatch recovery, quotas and overflow, path and
symlink races, streams/replay, explicit retention/deletion and strict-backend
refusal on an installed host. Keep that evidence separate from this schema gate.
