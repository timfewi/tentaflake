# Optional web UI and local host API

This guide describes unreleased development. Pin compatible revisions of the
host and your chosen UI before using them. The released documentation site may
still describe an earlier release.

Tentaflake works without a web UI or control service. The optional host API
lets an operator UI inspect the same declared agents and control their current
systemd lifecycle. `tentaflake.toml` owns instance definitions; Nix owns host
security policy and credentials and evaluates automatic-startup gates. The API
does not edit TOML or Nix, import agents, activate configurations or provide
arbitrary commands.

## Choose the interface

| Choice | Configuration | Result |
| --- | --- | --- |
| CLI only | Leave `tentaflake.hostApi.enable = false` | Existing CLI and declarative management; no API service or socket |
| Read-only web management | Enable the API and connect an optional UI | Inventory, health, agent policy details and configuration proposals |
| Web lifecycle control | Additionally set `allowActions = true` | Explicit start, stop and restart for exact declared instance IDs |
| Operator log access | Additionally set `allowLogs = true` | Bounded journal entries for an exact configured unit |
| Review tool requests | Additionally set `allowToolRequests = true` | Captured pending worker requests with complete arguments and content digests |
| Decide tool requests | Additionally set `allowToolDecisions = true` | Record approve/deny for an exact reviewed request, then request a worker drain |

The shipped host contract works with the Host view in a compatible operator UI.
It is independent of native chat connections or the UI's optional HQ task model.
All adapters use the same host inventory; stopped scaffold entries remain
visible but cannot be started or restarted through this interface.

## Enable the host API

The default Tentaflake module includes the option definitions. They have no
service effect until explicitly enabled:

```nix
{
  tentaflake.hostApi = {
    enable = true;
    allowActions = false;
    allowLogs = false;
  };
}
```

| Option | Default | Meaning |
| --- | --- | --- |
| `tentaflake.hostApi.enable` | `false` | Enable the local Unix HTTP service |
| `tentaflake.hostApi.allowActions` | `false` | Allow the fixed lifecycle actions |
| `tentaflake.hostApi.allowLogs` | `false` | Permit bounded agent journal reads |
| `tentaflake.hostApi.allowToolRequests` | `false` | Permit reading captured private worker requests, which may contain sensitive arguments |
| `tentaflake.hostApi.allowToolDecisions` | `false` | Permit digest-bound worker decisions; requires `allowToolRequests` |
| `tentaflake.hostApi.peers` | `null` | Exact Unix UID, canonical agent IDs and independent operation grants; `[]` denies everyone, including root |
| `tentaflake.hostApi.package` | Shipped host API package | Override the compatible service implementation |

The service uses `/run/tentaflake-api/host.sock` and exposes no TCP listener.
With `peers = null`, the root-owned socket has group `tentaflake-api`, mode
`0660`, and a `0750` parent. Membership grants the enabled legacy operations;
assign it only to trusted operator clients, never agent containers. Exact-peer
mode instead uses a `0711` parent and `0622` socket: connection permission is
not authority. Kernel `SO_PEERCRED` UID plus the exact resource/operation grant
authorizes every route before any runner socket or credential access. Root has
no bypass. `/v1/info.authorization` reports `mode: "exact-peer"` and
`resource_scoped: true`; legacy mode reports `"legacy-group"` and `false`.
Permissions are `read`, `actions`, `logs`, `propose`, `tool_requests`,
`tool_decisions`, `host_health`, `run_read`, `run_submit` and `run_stop`.
They do not inherit one another; configuration switches still gate legacy
mutations. Inventory is filtered to the caller's exact agents. An authenticated
multi-tenant UI bridge may receive the union of its tenants' host grants, but
must separately enforce each browser principal's narrower ownership.

## Connect an optional UI

Supply a compatible UI flake as a consumer-owned input named `operator-ui`.
Its origin and deployment are the operator's choice; the Tentaflake flake does
not download or require it. Import its NixOS module in the consumer:

```nix
{ inputs, pkgs, ... }:
{
  imports = [ inputs.operator-ui.nixosModules.default ];

  tentaflake.hostApi.enable = true;

  services.tentahq-ui = {
    enable = true;
    webRoot = inputs.operator-ui.packages.${pkgs.stdenv.hostPlatform.system}.web;
    operatorTokenFile = "/run/secrets/operator-ui-token";
    hostSocket = "/run/tentaflake-api/host.sock";
    taskAuthority = "none";
    instances = { };
  };
}
```

The compatible UI module grants its service access to the `tentaflake-api`
group when `hostSocket` is configured. Its Python bridge checks the Unix peer
UID before making requests; the default expected UID is root. Keep native
application relays and host-control sockets separate.

`taskAuthority = "none"` is host-only: it opens no application database, migrates
no task state and requires no agent runtime. Existing frontends can add
`hostSocket` while retaining their task authority and application connections;
their tasks and chat history remain in their existing systems.

Provision the operator token in a private runtime file containing at least 24
random characters. Serve the loopback UI through the existing authorized
private HTTPS transport; keep Secure cookies enabled. Evaluate and build the
consumer configuration, review it and activate it explicitly. This guide does
not enroll a tailnet, publish a host or change SSH/firewall policy.

For read-write management, deliberately enable `allowActions`. Log access is
independent because agent journals may contain sensitive application content.
The browser talks only to its authenticated, same-origin bridge; it cannot
choose sockets, executable paths or systemd unit names.

## Version 1 contract

Successful host responses contain `schema_version: 1`, except the runner
projections described below, which preserve their own application protocol.
Errors use `{"error":"message"}` with an appropriate HTTP status. Clients must
check the version and advertised capabilities before offering actions.

| Method and route | Result |
| --- | --- |
| `GET /v1/info` | Host/backend/security profile and enabled capabilities |
| `GET /v1/agents` | Stable instance `id`, display name, runtime, observed state, `runnable` and capabilities |
| `GET /v1/health` | `healthy` and the structured CLI health report |
| `GET /v1/agents/{id}/logs?lines=100` | Up to 200 journal entries, timestamps and explicit truncation |
| `POST /v1/agents/{id}/actions` | Durable receipt for one explicit lifecycle action |
| `GET /v1/agents/{id}/details` | Declared non-secret access/tool policy, policy revision and editable fields |
| `POST /v1/agents/{id}/config-proposals` | Structure-validated downloadable Nix proposal; no application or activation |
| `GET /v1/agents/{id}/tool-requests` | Bounded captured pending requests; no live inbox or execution history |
| `POST /v1/agents/{id}/tool-decisions` | Durable digest-bound authorization receipt and separate drain scheduling state |

The bridge maps those routes beneath `/api/host`. The browser continues to use
its existing operator login and HttpOnly session cookie. Both request and
response sizes, concurrency and command duration are bounded.

An action accepts exactly:

```json
{"action":"restart","request_id":"operator-action-unique-id"}
```

The path identifies a canonical container ID such as `agent-assistant`, not a
display name. Each receipt records `request_id`, `agent_id`, `action`,
`state` (`succeeded`, `failed` or `unknown`) and `observed_state`. A successful
command is not a model/provider readiness claim. Observe the resulting runtime
state and health separately. Stop includes the agent's declared LLM/fetch
brokers, preserving operator CLI semantics.

The service durably records an action before dispatch. Repeating the same
request ID and intent returns its receipt without dispatching another restart;
reusing the ID for different intent is a conflict. If dispatch times out or the
service crashes before confirming its result, the receipt stays `unknown`.
The bridge and browser must retry an uncertain HTTP request with the same ID.
Inspect current state before choosing a new action with a new ID; never replay
an unknown command automatically. Receipts live in the private
`/var/lib/tentaflake-api/actions.sqlite3` state, not in an agent workspace.

The ledger retains at most 10,000 distinct action IDs and never automatically
prunes them. At capacity, new actions are refused while existing receipts remain
readable through matching retries. Treat ledger maintenance as an explicit
operator operation: deleting it also deletes the protection against replaying
old request IDs.

## Agent access and configuration proposals

The Host view supports searching names and canonical IDs, filtering runtime and
state, and inspecting one agent at a time. Details project declared model
allowlists and prices, LLM budgets, Research/fetch configuration, worker limits,
storage quotas and management policy. They expose no credential values,
credential file paths, arbitrary environment variables or provider login state.
Unmanaged OCI entries have no editable policy. A configured connection does not
establish live readiness, successful authentication or remote access grants.

The host advertises additive `details`, `config_proposals`, `tool_requests` and
`tool_decisions` capabilities. Clients treat missing keys on older hosts as
unsupported. Per-agent capabilities can be narrower than the host's capabilities.

A configuration proposal accepts the exact current `policy_revision` and a map
of supported changes. For example, an agent with an enabled worker can prepare:

```json
{
  "policy_revision": "<revision from the current agent details>",
  "changes": {"worker.max_timeout_seconds": 120}
}
```

Editable settings are limited to the existing enabled LLM model/price list,
existing enabled fetch host allowlist, and five limits on an existing worker:
timeout, queue entry count, queue bytes, snapshot bytes and log bytes. Model
entries require explicit positive input/output prices. Fetch hosts are exact
DNS names, without schemes, ports or wildcards. Budgets, credentials, automatic
startup, Research routing, storage resizing and service enablement remain
outside this proposal editor.

The response includes the changes, a filename and a Nix module. It explicitly
reports `validation: "structure-only"` and
`activation: "explicit-host-update-required"`. A stale declaration revision
returns a conflict. The host validates types, bounds and allowlisted fields;
it does not evaluate the whole consumer, write a flake, build or activate it.
Submitted values replace the corresponding declared options in the generated
module rather than silently appending to an existing allowlist.

Review the proposal, reconcile it with the owning consumer declarations, and
evaluate/build the full resulting host configuration before explicit activation.
In particular, a structurally valid proposal can still violate a host assertion
or operational requirement. Downloading it has no effect on a running agent.

## Review and decide captured tool requests

Tool review is independently optional because request arguments can contain
sensitive application data. Enable both grants only for an operator who may
authorize worker jobs:

```nix
tentaflake.hostApi = {
  enable = true;
  allowToolRequests = true;
  allowToolDecisions = true;
};
```

The API reads only the host's captured private pending queue through the fixed
worker CLI. It never interprets a writable agent inbox as reviewed work. A list
contains complete argument vectors, action classes, timeouts and SHA-256 request
digests, with an explicit truncation indicator when additional rows are omitted.
The worker bounds each snapshot to 100 complete rows and 256 KiB; it never
truncates an argument inside a row that the operator is asked to approve.
Arguments are untrusted text. Inspect the complete vector and its boundaries
before authorizing the exact job and digest. A busy queue returns a conflict
instead of waiting for a long-running job to finish.

A decision accepts exactly:

```json
{
  "job_id": "cargo_test_1",
  "request_digest": "<digest from the reviewed captured request>",
  "decision": "approve",
  "request_id": "unique-operator-decision-id"
}
```

The worker records the first decision durably under its queue lock and verifies
the digest again. Repeating the same decision is idempotent; a different digest
or conflicting decision is refused. The decision command never drains jobs
synchronously. Before committing a new decision it durably publishes a private
drain marker under the queue lock. The worker path unit rechecks that marker
after the service exits, so a decision cannot lose its wakeup when a start
request joins a retiring oneshot. Only a subsequent drain consumes the marker.
The host API additionally asks systemd to start only that agent's configured
worker service without waiting for the job to execute.

The API receipt separates `state` (`recorded`, `failed`, `unknown`) from
`drain_state` (`queued`, `unknown`, `not-requested`). A recorded decision proves
only that authorization was saved. A queued drain proves only that systemd
accepted scheduling. Neither proves execution, completion or successful output.
Reuse the same API request ID after a lost reply, including after reloading the
browser; an uncertain request must never trigger an automatic new decision.

The API retains up to 10,000 tool-decision receipts separately from lifecycle
receipts in its private state database. At capacity it refuses new IDs while
existing receipts remain readable through identical retries. Receipt cleanup is
an explicit operator operation and removes the corresponding replay protection.

Approval preserves the worker's offline, no-credential, disposable containment.
Even an approved financial, communicative or privileged class gains no network,
secrets or extra host rights. Forbidden requests cannot be approved. The UI does
not create arbitrary tool commands, add runtimes or change the capsule policy.
See [worker approval and containment](13-disposable-worker.md).

## Exact-scoped Pi runner mediation

Only declared Pi instances that are runnable with `tentaflake.piRuntime.enable`
and an admitted image bundle receive internal runner descriptors. The bundle's
verification version, exact image selector and selected runtime package must
match, together with the supported dedicated rootful `runsc-pi` transport
prerequisites. The internal descriptor has the exact marker
`transport: "runsc-pi-v1"`; there is no configurable peer UID or generic native
socket transport. Descriptors and bearer contents are never included in
inventory or policy responses. Such agents advertise
`capabilities.runner = {read, submit, stop, resource_scoped: true, transport: "host-api"}`;
the three booleans reflect the exact peer's independent run grants. Legacy
group mode does not acquire runner authority.

All routes below are relative to `/v1/agents/{canonical-id}/runner`:

| Route | Grant | Runner request/result |
| --- | --- | --- |
| `GET /capabilities` | `run_read` | Fixed `GET /v1/capabilities`; live application readiness, not host security evidence |
| `POST /sessions` | `run_submit` | Fixed `POST /api/sessions`, exactly `{title, request_id}`; returns `{session:{id}}` |
| `POST /runs` | `run_submit` | Exactly `{request_id, session_id, input, context_mode:"persistent"}`; returns `{run_id}` |
| `GET /runs?request_id=…` | `run_read` | Same-ID reconciliation; returns runner projection |
| `GET /runs/{run_id}` | `run_read` | Runner projection including `state`, `uncertain`, `output` and `error` |
| `GET /runs/{run_id}/events` | `run_read` | One bounded SSE frame converted to `{events:[…]}`; a waiting heartbeat returns an empty array |
| `POST /runs/{run_id}/stop` | `run_stop` | Exactly `{}`; returns runner projection, not evidence that an external effect was undone |

Session titles are at most 200 characters. Request/session identifiers match
`[A-Za-z0-9_.:-]{1,128}`; run path IDs match `[A-Za-z0-9-]{1,128}`.
Run input is nonblank and at most 65,536 UTF-8 bytes; its entire incoming JSON
body is bounded to 70,000 bytes. Other host bodies retain the 4,096-byte limit.
No arbitrary path, URL, query, header, socket or credential argument is accepted.
Encoded runner paths are rejected. Poll bounded events and run state rather than
holding an unbounded event stream through the host API.

The trusted root host API mediates an untrusted application. It retains an empty
capability bounding set; only when descriptors exist does its service receive
supplementary numeric group `10100`. The UI receives neither that group nor the
runner credential. Host-derived paths are
`/run/tentaflake-pi/{canonical-id}/control/runner.sock` and the sibling
`api-token`. Instance ancestry remains root-owned `0700`; control is
`10100:10100` mode `0710`, socket `10100:10100` mode `0620`, and token
`root:10100` mode `0440`. The host uses no-follow directory-FD traversal and
exact provenance checks, then connects the pinned socket inode via `O_PATH`
to prevent an agent-owned basename swap. Directory traversal requires execute
rather than directory-read access. Agent state/workspace remain `0700` and are
not granted to the host's group.

For this named transport only, the expected outbound kernel peer UID is `0`:
source analysis of rootful, non-remapped runsc identifies its host Gofer as the
listener creator, distinct from the socket inode's `10100:10100` ownership.
The bearer is sent only after that exact peer check and the fixed inode,
ancestry and credential checks. This is not an inbound root grant: root remains
denied by exact-peer host API authorization. The dedicated `runsc-pi`
`--host-uds=all` runtime must not broaden the existing global/Research runsc
transport. Source-derived peer expectations require real-VM acceptance; native
Unix fixtures alone do not establish Gofer credentials or container admission.

Each runner exchange has one five-second deadline, 8 KiB headers and 768 KiB
response bounds. Only bounded JSON or one SSE frame is accepted, with no
redirects, transfer encoding or retries. Upstream error bodies are not exposed.
Readiness is checked live before session/run submission, but readiness and
returned state are application claims, never authorization or trusted host
security evidence. Authorization is enforced before consulting them.

The host persists mutation identity and uncertainty before dispatch. Matching
successful retries return the saved response; changed intent conflicts. A lost
response or crash leaves a durable conflict/uncertain receipt and never
redispatches that mutation, including after daemon restart. Reconcile runs with
the same `request_id`; do not manufacture a new ID to escape uncertainty.
The runner-dispatch ledger is capped at 10,000 identities without automatic
pruning. This API does not prove model acceptance, execution success or approval.

## What this interface does not claim

`chat` remains advertised as `false`: a started systemd unit is not a native
chat endpoint. Only the separately admitted Pi runner and exact run grants above
enable its narrow application protocol. Other runtimes need their own reviewed
adapter, credential/relay boundary and exact-image acceptance. The outbound LLM
broker is model policy, not an operator conversation API.

The Host view does not expose shell/exec, provider keys, file writes, arbitrary
unit control, upgrades or NixOS rebuilds. Native task-board ownership and HQ
profile creation remain separate optional application features.

## Verification and recovery

Before activation, run the contributor fast checks and build the focused
`checks.x86_64-linux.host-api` and `host-api-evaluation` outputs. Protocol tests
use temporary Unix sockets, synthetic inventory and fake host tools/runners.
They exercise scope and permission denial before IO, fixed routing, peer checks,
frame/deadline limits and durable no-replay recovery. Credential fixtures use
real filesystem traversal with explicitly substituted ownership metadata because
the tests are unprivileged. Native socket fixtures substitute only the expected
outbound peer constant, retain real Linux peer credentials, and verify a wrong
peer receives no request bytes. Nix evaluation checks descriptor admission,
grants and hardened service declarations. Source/native evidence is separate
from real-VM transport evidence. These fixtures do not establish live DAC,
container, model/provider or vendor acceptance; those require deployment-level
acceptance under the real identities and image.

After an authorized installation, verify read-only inventory/health first.
Check the intended agent ID before enabling lifecycle operations, then verify
its observed state through both CLI and UI. Test loss of the API connection:
the UI must show unavailable/unknown evidence rather than synthetic success.

To remove the web interface, disable the UI service in its consumer and disable
`tentaflake.hostApi.enable` if no other authorized client uses it. This does not
remove agent definitions or their state. Review retained API receipt state
before any explicit cleanup; deleting receipts discards deduplication history.
