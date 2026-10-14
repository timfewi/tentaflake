# Host environment API v1

Status: authoritative contract for [#152](https://github.com/timfewi/tentaflake/issues/152),
with experimental discovery, sessions, published capacity reporting,
original-principal operation reads and an HTTPS CLI client. Dynamic environment
lifecycle remains unavailable through HTTP. Schema validation, unit tests and
fixtures do not establish vendor, guest, private HTTPS relay or live-host
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

## Experimental authentication service

`tentaflake.api.enable` installs an unprivileged `tentaflake-api.service`, running
`tentaflake api serve <generated-store-config>`. It forwards bounded requests over
a protected local socket to `tentaflake-api-owner.service`, whose
`tentaflake api owner <generated-store-config> <generated-store-plan>` invocation
requires systemd socket activation and the host's root mount namespace.
The owner retains only `CAP_SYS_PTRACE` to inspect `/proc/1/ns/mnt`; Linux applies
ptrace access checks to that namespace handle even for UID 0. The HTTP process
has no capabilities.
Only that owner holds sessions, reads credentials and accesses the admission
ledger. The HTTP process has no direct credential or ledger access. The owner
requires an explicitly bootstrapped ledger and its installed, published plan.
The plan binds the non-secret API configuration; stale installation or publication
refuses authenticated requests. Pending maintenance still permits authentication
for recovery, but grants no resource mutation authority.

Configure explicit `host`, `origin`, `principal.id` and `principal.name` values in
the consumer flake. `listen` defaults to `127.0.0.1:18181` and accepts only loopback
addresses. The consumer owns the private HTTPS relay and its access policy. It
must preserve the configured Host and the browser's actual Origin; do not synthesize
an allowed Origin or expose the loopback service through a public proxy. The
service supplies no TLS endpoint, CORS policy or UI dependency.

`operatorTokenFile` and `cliBearerFile` are optional runtime source-path **strings**;
at least one is required. Systemd supplies their contents as protected
`LoadCredential` files in the owner service. Never use Nix path values or inline secret contents.
Configure distinct, cryptographically random credentials of 32–4096 printable
non-whitespace ASCII bytes. A single final newline is accepted. The loader checks
root ownership, private permissions, a single regular-file link and protected,
unchanged ancestry. It rejects Nix-store credential paths and equal operator/bearer
contents. Secrets do not appear in service arguments, responses or logs.

The implemented routes are `GET /api/meta`, `GET/POST/DELETE /api/v1/session`,
authenticated `GET /api/v1/capabilities`, catalog image/block lists and both operation reads described below. Discovery
reports an implemented backend because this service handles real requests; it
does not assert that environment features exist. Sessions return the configured
catalog grant, if any, and the current principal's exact durable resource grants.
Capabilities report the installed policy's ceilings after host
and Research reserves, bounded by per-instance limits. These are declarations,
not currently free capacity or live admission. Fractional ceilings that cannot
fit the integer request protocol report zero. Resource actions remain disabled
and both isolation choices explicitly lack API runtime acceptance. Other
authenticated operations, including planning and environment lists, return
`unsupported`; the real UI remains blocked on those
implementations. Bearer authentication uses its independently configured file;
cookie logout does not revoke that operator-managed credential.
Public OIDC negotiation routes also return `unsupported`; no provider or redirect
flow is enabled.

Catalog reads default to denied. To grant the single configured principal access
to the exact installed catalog, declare and publish an explicit grant:

```nix
tentaflake.api.catalogReadGrant = {
  id = "catalog-reader";
  action = "catalog.read";
  target = { kind = "catalog"; id = "installed-catalog"; };
  expiresAt = null;
};
```

This nonexpiring grant belongs to the immutable published API configuration, not
to an environment or a login. It grants neither planning nor creation. Creation
template principal lists remain the separate owner of future creation authority.
The catalog exposes only deduplicated public image metadata from installed
creation templates, validated against their retained original catalog. Private
source names, commands, environment, paths and principal lists are never returned.
An installation without templates returns empty lists. Blocks are currently empty;
image availability remains unsupported and evidence empty while public creation
is unavailable. Listing does not pull an image or probe a provider.

`revisions.catalog` hashes the actual public image/block projection;
`revisions.grants` hashes the canonical effective grants for this host/principal.
Login consumption and unrelated ledger changes do not alter those revisions.
The private core grant revision used for creation CAS is unchanged. Sessions
refuse with `unavailable` when their grant list exceeds 256 entries or the bounded
response size; they never truncate it. This does not disable receipt reads.
Catalog lists accept `limit` and opaque `cursor`, return arrays and optionally
`Next-Cursor`, and expose the quoted catalog revision as `ETag`. Cursors bind to
the principal, published configuration, effective grants and catalog contents;
stale or mismatched cursors return `cursor_expired`. The still-installed published
catalog remains readable during Pending maintenance; an installation/authority
mismatch fails closed. Configuration publication and service restart revoke old
sessions as before.

Sessions expire after 15 minutes idle or eight hours absolute and are revoked by
HTTP service restart/failure, owner restart or a changed authority/configuration
binding. The owner and its activation socket stop with the HTTP service, including
unexpected process exit. Once that dependent stop completes, no leftover socket
can reactivate the frontend. Up to 256 sessions
and ten login attempts per minute are permitted. Session and CSRF identifiers use
32 random bytes each. Before issuing a session, the existing admission ledger
durably records a domain-separated credential fingerprint. Reusing that token,
including after restart or publication, is refused. The 4096 retained consumption
records are never automatically pruned; exhaustion fails closed. A lost login
response can consume a token without delivering its session: supply a fresh runtime
token and restart the service explicitly, rather than deleting ledger history.
Older binaries that reject the new private ledger field cannot safely be used
to bypass this history during rollback.

HTTP headers, request bodies, connection lifetimes and active work are bounded.
The private channel authenticates both peers, preserves header duplicates and
accepts one closed, bounded request per connection. It accepts no caller-selected
host command, plan, principal or authority. Owner failure returns `unavailable`
without local authentication or replay. An aborted connection keeps its owner
work charged until that work finishes.
No request-body, credential or authenticated-resource access log is emitted.
The VM fixture generates its own guest-only credentials and tests real HTTP
requests, actual UID/filesystem boundaries, active-session revocation after restart
and process death, and owner unavailability. Successful execution of that fixture
is a separate acceptance gate, including the actual owner mount namespace and
systemd lifetime coupling. It does not prove the consumer's HTTPS relay configuration.

## Operation reads

`GET /api/v1/operations/{id}` returns only a receipt belonging to the authenticated
principal on this host; another principal's ID is indistinguishable from a missing
ID (`404 not_found`). `GET /api/v1/operations` requires exactly four query fields:
`idempotencyKey`, `mutationEpoch`, `method` and `target`. Decode the outer query
once; preserve the canonical target, including its semantic query. Duplicate,
unknown or malformed fields are refused. UUID key spelling is case insensitive.
Neither route grants resource access or permits dispatch.

The owner maps private creation receipts to the existing wire states: Queued to
`queued`, Unknown to `unknown`, Succeeded to `succeeded`, and operator Aborted to
`failed`. Aborted uses `unready` with a constant explanation that explicit operator
recovery ended creation, storage may remain retained and replay is forbidden; its
result is null. Unknown is not evidence that a job is running. Successful results
contain only the stored environment/workspace IDs and their historical revisions.
Timestamps and the operation revision come from the stored receipt; the ID route's
strong ETag equals that revision. Empty steps mean no detailed step history was
recorded. Private authority paths, supervisor identities, allocations and grants
are never serialized. Session/catalog revision placeholders are not used to invent
operation or resource revisions.

Historical `found` remains readable during Pending publication and after epoch
changes. Missing keys return `absent` only for the current epoch with no Pending
publication, observed together under the ledger lock; otherwise they return
`unknown`. An unavailable/stale installed authority or ledger returns `503`, never
safe absence. Before returning, the owner rechecks its installed/published binding
and active authentication. Unrelated logins do not change the receipt's ETag.

The focused transport regressions use real HTTP, Unix framing, owner sessions and
HTTPS CLI parsing with a test-only installed-state/receipt reader. Separate core
regressions exercise real durable ledger and publication transitions. The existing
creation VM additionally exercises its real retained ledger through
the generated API owner and installed CLI, using guest-only credentials and an
explicitly trusted local TLS relay. Its queued/Unknown/success/abort, Pending and
post-publication history assertions are written but have not run; unit fixtures
alone do not prove that deployment.
Public planning, creation, start and stop remain `501 unsupported`.

## Private ledger foundation

`tentaflake-admission::ledger::api` implements library transactions for stopped,
offline environments; public creation still returns HTTP `501 unsupported`.
The existing ledger atomically records the operation, exact issued action grants,
environment/workspace identities and both native storage charges. Operation keys
bind host, principal, epoch, method, canonical target and exact request digest.
Request UUIDs, including nil, normalize to lowercase for storage and lookup;
case differences do not create a second intent. Boot identities remain non-nil.
Only a new durable supervisor claim, bound to boot, unit and invocation identity,
returns a helper permit. Historical lookup or retry never redispatches work.
Unrelated login consumption does not change resource or grant revisions.

This slice accepts only nonexpiring installed creation and issued grants.
Ordinary native runtime/storage entrypoints refuse these dynamic identities;
publication refuses queued or unresolved work. Partial creation retains both
charges. Explicit recovery recognizes exact existing, stopped images only when
both native receipts already record completed preparation. A separate operator
abort releases only slots proven absent,
retains exact stopped images, and revokes effective grants. Queued work requires
both slots absent; claimed work additionally binds the fenced original supervisor.
Missing staging, mount, loop and writer evidence refuses the whole abort. Durable
release markers preserve all original IDs and history; nothing authorizes their
reuse or recreation. Exact terminal retries perform no observation or mutation.
The root maintenance collectors implement these observations; actual supervised
creation/recovery VM acceptance is still required before enabling public creation.
Library and collector checks do not establish a working HTTP create or VM lifecycle.

Existing private v3 ledgers without the new API field remain readable. Once a
new writer persists that field, older binaries reject it; rollback must preserve
the operation, grant and allocation history rather than remove the field.

The default-empty `security.admission.creationTemplates` policy selects installed
Generic source instances, reviewed base-image IDs, explicit principals and issued
actions. The initial mapping is offline, with fixed whole-CPU/MiB resources, two
volumes, no agent runtime, tools or skills. Ambiguous selections refuse. Each
manifest retains its original image catalog; updating today's catalog must not
invalidate historical cleanup. Source commands, environment and paths remain
private, and new filesystems do not copy source state or seed services.
The shared quota helpers accept only server-generated slot IDs for dynamic paths.
The internal `tentaflake-environment-create@` oneshot service reconstructs exact
bindings from the retained template and claims one accepted operation before
running those helpers. Direct CLI calls cannot impersonate its invocation. Both
volumes are already charged, so the physical-space check counts the retained pair
once. Initial execution requires an empty Docker inventory, no attached loops
and no dynamic mounts. Each sibling volume is created, mounted, initialized,
unmounted and verified; failures preserve Unknown and all remaining charges.
Historical claims return without helper execution. This internal owner does not
enable HTTP queueing or general concurrent environment lifecycle.

Explicit root reconciliation uses the current installed store plan, original
operation ID and expected ledger revision (reported by `admission publication status`):

```bash
tentaflake admission create-recover <generated-store-plan> <operation-id> <expected-ledger-revision>
tentaflake admission create-abort <generated-store-plan> <operation-id> <expected-ledger-revision>
```

Claimed work requires a different boot from its recorded execution because the
receipt does not retain an old cgroup identity. Queued work has no execution and
may be aborted in the same boot when both slots are absent. Operators must first
drain all launchers, mounts and loops; these commands never stop services or
delete staging. The collectors require both protected backing directories,
authenticate the original retained template, scan every slot against retained
charges and run only its bounded read-only verifier for existing target images.
They repeat the inventory and stopped-host observations before committing.
Successful recovery requires both native rows already Present: a valid Btrfs
image cannot prove owner initialization completed. Otherwise abort can retain
verified images and release only proven-absent slots. An error preserves all
charges; no path repeats creation or ownership helpers.

Maintenance includes recorded dynamic paths and creation units after template
removal or grant revocation. Current templates are checked through a fixed
metadata-only systemd instance, which is never started; unknown or removed
launchers must be absent or masked and drained. Exact terminal abort/recovery
retries return the durable receipt without loading an old template or observing
storage again, including after publication. Keep its original expected revision.

## HTTPS CLI client

The configuration-free `tentaflake api` client uses an explicit HTTPS origin
without a trailing slash. It verifies TLS certificates, follows no redirects,
uses no environment proxy and never automatically retries requests. Credentials
come only from a protected runtime file owned by the calling user, mode `0400`
or `0600`, with safe root/user-owned parent directories. Symlinks, hard links,
Nix-store paths and inline token arguments are refused.

For a self-hosted private CA, pass `--ca-file /absolute/protected/ca.pem` explicitly.
This adds roots for that invocation while keeping the default trust roots and all
certificate/hostname checks. It does not read proxy or CA environment overrides.
The PEM file must contain 1–16 certificates within 64 KiB, be a single-link regular
file owned by root or the invoking user, and have protected root/user-owned
ancestry without symlinks or group/world writes. Public readability is allowed;
private keys, other PEM objects and trailing non-certificate data are refused.
There is no insecure-TLS switch or automatic trust fallback.

```bash
tentaflake api --origin https://host.example.invalid meta
tentaflake api --origin https://host.example.invalid \
  --bearer-file "$XDG_RUNTIME_DIR/tentaflake/cli-bearer" session
tentaflake api --origin https://host.example.invalid \
  --bearer-file "$XDG_RUNTIME_DIR/tentaflake/cli-bearer" capabilities
tentaflake api --origin https://host.example.invalid \
  --bearer-file "$XDG_RUNTIME_DIR/tentaflake/cli-bearer" catalog images --limit 25
tentaflake api --origin https://host.example.invalid \
  --ca-file "$XDG_RUNTIME_DIR/tentaflake/ca.pem" \
  --bearer-file "$XDG_RUNTIME_DIR/tentaflake/cli-bearer" operation operation-id
```

The client also provides `catalog blocks`, `environments`, `environment ID`, `plan FILE`,
`create FILE`, `start ID`, `stop ID`, `operation ID` and `recover`. These commands
use the v1 contract; the current authentication service still refuses the
unimplemented resource routes. JSON request files preserve their exact bytes.
Creation and lifecycle mutations require an explicit `--epoch` from the session
and a UUID `--key`; start/stop also require the unquoted `--revision` of the
environment. The client sends a quoted `If-Match` header itself. It does not
generate new intent keys after a failure.

Successful JSON goes to stdout. Catalog and environment lists remain arrays; a returned
`Next-Cursor` is JSON-quoted on stderr and can be passed with `--cursor` to obtain
the next page. `--limit` accepts 1–500. `--json` is accepted; `--hide` is refused
because API JSON is not a redacted diagnostic format. Remote error messages and
credentials are not printed.

A failed mutation may already have been accepted. Use `recover --epoch E
--key UUID --method POST --target /api/v1/environments` (or the exact lifecycle
target), with the same origin and bearer file, to query its durable outcome.
An authoritative current-epoch `absent` response alone permits the same intent
to be retried. `found` returns its operation; `unknown` requires reconciliation.
The client validates this distinction but never performs the retry itself.

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
The [Rust admission core](10-security-profiles.md#shared-admission-core) provides
startup decisions and durable claims, with offline CLI explanation. It is not
an HTTP backend. Remaining #152 delivery includes mandatory live admission,
runtime reconciliation, private authenticated serving, owned workspace and terminal
implementation, generated consumer migration, and actual interactions. Acceptance
must exercise cross-owner denial, CSRF/Origin refusal, revoke/expiry, concurrent
revision conflicts, crash-after-dispatch recovery, quotas and overflow, path and
symlink races, streams/replay, explicit retention/deletion and strict-backend
refusal on an installed host. Keep that evidence separate from this schema gate.
