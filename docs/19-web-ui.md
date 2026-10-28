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
| Read-only web management | Enable the API and connect an optional UI | Host inventory, runtime states and health |
| Web lifecycle control | Additionally set `allowActions = true` | Explicit start, stop and restart for exact declared instance IDs |
| Operator log access | Additionally set `allowLogs = true` | Bounded journal entries for an exact configured unit |

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
