# Optional web UI and local host API

This guide describes unreleased development. Pin compatible revisions of the
host and your chosen UI before using them. The released documentation site may
still describe an earlier release.

Tentaflake works without a web UI or control service. The optional host API
lets an operator UI inspect the same declared agents and control their current
systemd lifecycle. Nix remains authoritative for agent definitions, security
policy, credentials and automatic startup. The API does not edit Nix, import
agents, activate configurations or provide arbitrary commands.

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
ZeroClaw and Hermes use the same host inventory; stopped scaffold entries remain
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
| `tentaflake.hostApi.package` | Shipped host API package | Override the compatible service implementation |

The service uses `/run/tentaflake-api/host.sock`, owned by root with group
`tentaflake-api` and mode `0660`. It exposes no TCP listener. Membership in that
group grants the enabled host operations: assign it only to trusted operator
clients, never to agent containers. The generated configuration and inventory
do not depend on enabling the interactive shell profile.

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

`taskAuthority = "none"` is a host-only mode. It opens no application database,
does not migrate existing boards and requires no Pi runtime. Existing Hermes or
HQ deployments can add `hostSocket` while retaining their current task authority
and application connections. Their tasks and chat history remain in their
existing systems.

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

Successful host responses contain `schema_version: 1`. Errors use
`{"error":"message"}` with an appropriate HTTP status. Clients must check
the version and advertised capabilities before offering actions.

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

The path identifies a canonical container ID such as `hermes-assistant`, not a
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

## What this interface does not claim

`chat` is advertised as `false`. Starting a systemd unit does not create a
native chat endpoint. The current Hermes and ZeroClaw declarations do not
supply the UI's session/run protocol; their chat integrations require separate
runtime adapters and acceptance evidence. The LLM broker is an outbound model
policy boundary, not an operator conversation API.

The pinned Hermes image already contains a native API at source revision
[`e45d126`](https://github.com/NousResearch/hermes-agent/blob/e45d12642d5d0753e492d51be17cccf687aa8b06/gateway/platforms/api_server.py).
Its TCP listener and required application bearer credential are not enabled or
securely exposed by the current Tentaflake declarations. A future opt-in
integration needs a dedicated runtime application credential, a fixed confined
relay, and exact-image tests for sessions, streaming, cancellation and restart.
Enabling this host API alone supplies none of those resources. ZeroClaw has a
different gateway protocol and requires its own application adapter.

The Host view does not expose shell/exec, provider keys, file writes, arbitrary
unit control, upgrades or NixOS rebuilds. Native task-board ownership and HQ
profile creation remain separate optional application features.

## Verification and recovery

Before activation, run the contributor fast checks and build the focused
`checks.x86_64-linux.host-api` and `host-api-evaluation` outputs. Protocol tests
use temporary Unix sockets, synthetic inventory and fake host tools. They
exercise routing, limits, disabled capabilities and durable action recovery
without mutating an installed host. Nix evaluation checks the inert default and
hardened service declaration; these checks do not establish live container or
provider acceptance.

After an authorized installation, verify read-only inventory/health first.
Check the intended agent ID before enabling lifecycle operations, then verify
its observed state through both CLI and UI. Test loss of the API connection:
the UI must show unavailable/unknown evidence rather than synthetic success.

To remove the web interface, disable the UI service in its consumer and disable
`tentaflake.hostApi.enable` if no other authorized client uses it. This does not
remove agent definitions or their state. Review retained API receipt state
before any explicit cleanup; deleting receipts discards deduplication history.
