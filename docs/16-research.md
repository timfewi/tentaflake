# Exclusive web research

Tentaflake uses the public MIT-licensed
[tentaflake-research](https://github.com/timfewi/tentaflake-research) service for
web access. Its stdio MCP server is named `secure-research-tool`; it exposes
`research_job`, `research_search`, `research_fetch`, `research_browser` and
`research_read`. Model requests continue through the separate LLM broker.

The service is a pinned flake input, imported by `nixosModules.default`. An
operator must configure the VPN and service before enabling a research agent.
The template does not create an Internet route or obtain provider credentials.

## Configuration

Declare a registered Research-capable adapter with `mkAgent` or either
compatible runtime builder, then use its complete OCI container name:

```nix
tentaflake.research.agents = {
  hermes-assistant.uid = 62101;
  zeroclaw-assistant.uid = 62102;
};
services.secureResearch = {
  serviceUid = 4201;
  egressUid = 4202;
  vpnInterface = "wg0";
  resolvers = [ "9.9.9.9" ];
};
```

Choose distinct unused host UIDs. Each relay has its own host UID, even when
two containers use the same internal UID. The service authorizes job ownership
using the relay's peer credentials. These UIDs are service identities, not host
user-namespace remapping for the containers.

This fragment does not configure WireGuard or VPN readiness. Supply fresh,
trusted readiness observations as described in the upstream
[operations guide](https://github.com/timfewi/tentaflake-research/blob/main/docs/operations.md)
and [egress control](https://github.com/timfewi/tentaflake-research/blob/main/docs/egress-control.md).
Missing or stale readiness prevents public requests. Optional search providers,
Chromium and OCR require their own explicit configuration. Provider keys and
data grants belong in the research service; never put keys in agent settings or
the Nix store. Research summarization remains disabled in this topology.

Keep the agent's [LLM broker](12-brokered-egress.md),
[disposable worker](13-disposable-worker.md) and
[workspace quota](14-workspace-quota.md) declarations. A secure automatically
started controller requires all four policies. A stopped scaffold can remain
offline without research. The old fetch broker must be disabled for every
research-enabled agent; its configuration fails evaluation otherwise.

## Enforced boundary

The shared projection installs the pinned `research-client` and its exact runtime closure
as read-only mounts. Only `/run/tentaflake-research/CONTAINER` is projected into
that agent as `/run/tentaflake-research`. The root-only host parent hides other
agents' sockets. The container receives no research credentials or upstream
service socket. Mounts must use normalized paths and retain these exact
read-only capabilities after caller overrides.
The security manifest records the declared mounts. `doctor --security` accepts
the research socket only when its source, destination and read-only mode match
that declaration; broad `/run`, writable and other-agent mounts remain unsafe.

gVisor runs with `--host-uds=open` to connect to projected sockets; host socket
creation remains disabled. Agent IP networking still permits only its declared
LLM broker, or remains `network=none`. Browser, HTTP and shell networking cannot
gain general egress by changing a tool setting.

Adapter hooks generate upstream-specific settings after shared socket/closure
projection. An arbitrary OCI container with a runtime-like prefix cannot opt
into Research; validation checks explicit registered instance metadata and the
adapter's accepted capability. OpenClaw's stopped scaffold has no accepted
Research discovery integration. See [agent adapters](agent-adapters.md).

Hermes settings disable the `web` and `browser` toolsets and register
`mcp_servers.secure-research-tool`. Additional MCP entries must be local stdio
commands. ZeroClaw settings disable `browser`, `http_request`, `web_fetch` and
`web_search`, and replace the MCP server list with this stdio service. These
settings are applied after caller settings. The LLM broker permits local
function/custom tool definitions but rejects provider-hosted tools, including
web search and remote MCP, and known provider extension fields. Operators must
select inference-only models; this does not constrain a provider's internal
implementation or make an arbitrary vendor extension safe.

Each relay has at most four connections and no IP networking. Healthy sessions
can remain idle between tool calls. Stop one relay socket to revoke accepted
connections immediately:

```sh
sudo systemctl stop tentaflake-research-hermes-assistant.socket
```

Stopping `agent-research.socket` also stops all relay sockets. Restart the
selected socket and the agent to establish a fresh client session. Upstream
readiness, budgets and source-retention policy remain enforced by the service.

## Verification and limits

```sh
nix build --no-link .#checks.x86_64-linux.research-policy
nix build --no-link -L .#checks.x86_64-linux.research-integration
```

The policy check exercises remote MCP rejection, missing capability mounts,
duplicate relay UIDs, legacy fetch rejection and disabled summarization. The VM
runs the real stdio client and service inside real Docker/gVisor capsules with
Hermes and ZeroClaw UID values, verifies the generated settings, performs MCP
initialization and job creation, and checks direct IP and offline-fetch denial.
Its container image is a synthetic Python probe. It does not run the actual
vendor agent images or prove their MCP discovery behavior.

The upstream synthetic VM suite separately checks encrypted WireGuard egress,
direct-path denial, parser credentials, rendering, resources, cross-agent
ownership, idle sessions and socket revocation. See its
[verification evidence](https://github.com/timfewi/tentaflake-research/blob/main/docs/verification.md).
Actual Hermes/ZeroClaw sessions, Podman socket transport, paid-provider billing,
production VPN policy, ARM and arbitrary kernel escapes still require deployment
acceptance. No production host is activated by these checks. Retrieved content
remains untrusted; the network boundary does not prevent prompt injection or
disclosure of sensitive text intentionally sent to an allowed destination.

## Client recovery

The pinned stdio client reconnects future calls after a lost upstream Unix RPC
session. Each fresh session repeats protocol negotiation and host peer-UID
authorization. Interrupted calls fail without replaying dispatched operations;
caller cancellation and explicit client close remain enforced. If the service
is unavailable, connection retries remain bounded. Socket revocation still
closes accepted relays and prevents research access.

The public research service fixture verifies a real upstream SIGKILL followed
by a successful new call from the same stdio process, plus the idle-session and
live socket-revocation regressions. Tentaflake's separate Docker/gVisor fixture
checks the packaged MCP interface, exact closure/socket mounts and UID policy.
These synthetic fixtures do not establish actual vendor-agent workloads or
production VPN/provider acceptance.
