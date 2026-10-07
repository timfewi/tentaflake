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

This fragment does not configure WireGuard or VPN readiness. Select the reference
observer below, or supply fresh, trusted observations under the same lease contract
described in the upstream
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

## VPN readiness selection

`nixosModules.default` already imports the pinned Research module. Configure its
`services.secureResearch.vpnObserver` options directly; there is no separate
Tentaflake observer wrapper. The observer is disabled by default, and its extra
evidence selections are opt-in. An operator-owned producer of the generic
observation lease remains valid; no VPN vendor is required.

For an existing, operator-configured WireGuard exit, extend the service declaration:

```nix
services.secureResearch = {
  vpnInterface = "wg0";
  resolvers = [ "9.9.9.9" ];
  drainSeconds = 60;
  vpnObserver = {
    enable = true;
    linkKind = "wireguard";
    egressPathEvidence = true;
    handshakeWithinSeconds = 180;
    # Illustrative public key: replace with the selected exit's exact peer key.
    peerPublicKeys = [ "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=" ];
    firewallMarker = "/run/research-vpn/firewall-ready";
    drainMarker = "/run/research-vpn/drain";
  };
};
```

This does not create a tunnel, enroll management, install VPN credentials or
declare that an exit is ready. Keep VPN credentials on the host. Only the
operator's firewall installer creates `firewallMarker`, after establishing the
independent egress firewall; creating the marker alone proves no kernel rule.
Markers must be distinct regular root-owned files without group/world write
access. Their ancestors must be real root-owned directories, without symlinks
or write access for other identities. A missing drain marker means no drain;
an unsafe marker makes readiness fail closed.

| Selection | Evidence and limits |
|---|---|
| `linkKind = "wireguard"` or `"tun"` | Checks the selected link kind and that the interface is up. The default is `null`; interface presence alone does not prove a usable exit. |
| `egressPathEvidence = true` | Evaluates the configured `egressUid` through policy-routing rules: unmarked IPv4 must use the tunnel, IPv6 must use it or be denied, and every `services.secureResearch.resolvers` address must use it. The default is `false`. |
| `handshakeWithinSeconds` | Selects WireGuard handshake-age evidence. Use 180 seconds or more; loss can take the selected window to detect. The default is `null`. |
| `peerPublicKeys` | Pins the exact WireGuard peer set, at most four base64 public keys. These values are public, not VPN secrets. The default is `[]`. |
| `drainMarker` | Requests a planned exit change. The default is `null`; it must differ from `firewallMarker`. |

Handshake and peer-key selections require `linkKind = "wireguard"`. They grant
only the root observer unit `CAP_NET_ADMIN` to read kernel WireGuard evidence;
agents receive no added capability, credential or readiness hook. Link/path-only
observation has no capabilities. The observer retains only public peer keys and
handshake times and must never log the kernel dump or secret attributes.

Tailscale or Headscale management membership supplies neither an Internet exit
nor proof of one. For a separately configured exit using a layer-3 tun interface,
select `linkKind = "tun"` and `egressPathEvidence = true`, leaving WireGuard
handshake/key options unset. Tun exit identity and liveness are not observed or
accepted by this reference implementation. `region` is an optional operator
assertion, as is `firewallMarker`; neither substitutes for observed link/path
evidence. Conservative or unknown routing evidence withholds readiness. The
independent kernel firewall remains responsible for direct IPv4, IPv6 and DNS
denial if the observer, controller or both fail.

See the pinned upstream
[VPN evidence contract](https://github.com/timfewi/tentaflake-research/blob/91ce54005c3c69a3eab9d8e2730ca9d2802d2c96/docs/vpn-adapters.md)
for route evaluation and its limits. Agents cannot select exits, write
observations or control leases, execute readiness hooks, or access VPN credentials.

## Planned exit change

An exit change is an operator action requiring its own authorization. Keep the
old tunnel and its firewall protection intact throughout draining. The observer
renews observations for at most ten seconds; the controller publishes leases for
at most three seconds. Repeated drain observations cannot extend the controller's
monotonic `drainSeconds` deadline (1–300 seconds, default 60).

1. Read the controller's control lease, not just the observer's input, and record
   the current `ready` generation. The default path contains no credentials,
   queries or exit address. Bound every read by time and bytes:

   ```sh
   sudo timeout 2s head -c 4097 -- /run/agent-research-egress/control/state.json
   ```

   Require a regular, protected root-owned lease of at most 4096 bytes, `version`
   1, a non-nil UUID `generation`, and `valid_until` later than now but no more
   than three seconds ahead. A timeout, missing, oversized, malformed or expired
   lease is unknown/offline, never permission to declare readiness.
2. Verify the marker parent protections above, then create the configured drain
   marker as root. For the example path, refuse an existing path or symlink and
   use noclobber; do not replace another operator's marker:

   ```sh
   sudo bash -c '
     set -eC
     marker=/run/research-vpn/drain
     test ! -e "$marker"
     test ! -L "$marker"
     umask 077
     : > "$marker"
   '
   ```

3. Repeat the bounded control read until a fresh lease reports `draining` with
   the saved generation. Allow at most `vpnObserver.refreshSeconds + 3` seconds
   for this transition (refresh defaults to five seconds). If it does not occur,
   leave the old exit protected and investigate; a marker alone does not prove
   controller drain admission. New work is refused once the controller drains.
4. Keep the old exit and firewall protected until existing work finishes or the
   drain deadline expires. Without independent completion evidence, wait for a
   fresh `offline` control lease; after a stalled controller, wait at least the
   configured `drainSeconds` from the first observed drain and for every last
   valid control lease to expire. Bound the wait to `drainSeconds + 3`; if the
   condition cannot be established, stop and inspect rather than changing exits.
5. Only after that completion/offline/deadline condition, change the host-owned
   exit configuration while retaining fail-closed firewall protection. Preserve
   the firewall marker and controller lock; do not fabricate a readiness lease.
6. Remove only the selected drain marker:

   ```sh
   sudo rm -- /run/research-vpn/drain
   ```

   Confirm a fresh `ready` controller lease with a generation different from the
   one saved before draining. The observer must re-prove all selected evidence,
   even if the exit ultimately stayed the same. If proof fails, remain offline.

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

The observer option fixture evaluates without building Research closures:

```sh
nix eval --option allow-import-from-derivation false --no-write-lock-file \
  --impure --json --expr '
    let flake = builtins.getFlake (toString ./.);
    in import ./tests/research-vpn-observer.nix {
      self = flake;
      pkgs = flake.inputs.nixpkgs.legacyPackages.x86_64-linux;
    }'
```

The broader policy and runtime gates are separate workloads:

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

Observer option evaluations cover direct module exposure, defaults, selected
link/path/handshake/peer/drain arguments and refusal of invalid combinations.
They establish configuration wiring, not a working tunnel or live firewall.
The pinned upstream separately records a real WireGuard observer/controller VM
fixture; its evidence does not establish Tentaflake host integration or a
non-WireGuard exit node. Issue [#141](https://github.com/timfewi/tentaflake/issues/141)
retains real-tunnel integration and production acceptance. That VM work was
explicitly excluded from the current lightweight-only run; no runtime acceptance
or production activation is claimed here.

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
