# Brokered egress

## Result

Balanced agents can use external models and explicitly approved web hosts
without receiving a provider credential or general internet access. The
default remains fail-closed: an agent without a broker declaration uses
`network=none`.

Each enabled agent gets:

- one dedicated OCI `--internal` IPv4 `/30` network with a deterministic,
  policy-matched host bridge interface;
- no container DNS and no IPv6;
- host INPUT access only when both the exact bridge interface and agent source
  subnet match its own broker ports;
- a host FORWARD drop for the whole agent subnet;
- one random virtual key generated under `/run` on every boot;
- optional LLM and fetch broker processes with separate state and budgets.

The OCI container key must exactly match the generated container name, for
example `hermes-coding` or `zeroclaw-research`.

## Declaration

```nix
tentaflake.broker.agents.hermes-coding = {
  enable = true;
  networkName = "tf-hermes-coding";
  subnet = "10.203.20.0/30";
  gateway = "10.203.20.1";

  maxConcurrency = 4;
  maxRequestsPerMinute = 30;
  dailyRequestBudget = 1000;
  dailyTokenBudget = 1000000;
  dailyCostMicrousd = 10000000;

  llm = {
    enable = true;
    port = 7810;
    upstreamBaseUrl =
      "https://api.openai.com/v1/";
    providerCredentialFile =
      "/run/agenix/openai-key";
    maxCompletionTokens = 4096;
    allowedModels = [
      {
        name = "gpt-5-mini";
        inputMicrousdPerMillion = 250000;
        outputMicrousdPerMillion = 2000000;
      }
    ];
  };

  fetch = {
    enable = true;
    port = 7811;
    allowedHosts = [
      "platform.openai.com"
    ];
    allowedContentTypes = [
      "application/json"
      "text/html"
      "text/plain"
    ];
    maxRedirects = 3;
  };
};
```

Use a unique `/30` per agent. The network address must end on a four-address
boundary and `gateway` must be its first usable address. Evaluation rejects
duplicate networks/subnets, unrelated gateways, missing modes, plain HTTP
providers, provider files outside `/run`, duplicate models, wildcard hosts,
wildcard media types, and deterministic bridge-interface collisions.

For a host with many agents, set aggregate ceilings explicitly. They are checked
at evaluation time against the sum of enabled agent budgets, so a configuration
cannot accidentally admit more aggregate provider cost or concurrency than the
operator planned:

```nix
tentaflake.broker = {
  maxEnabledAgents = 1000;
  maxTotalConcurrency = 400;
  maxTotalRequestsPerMinute = 3000;
  maxTotalDailyTokenBudget = 100000000;
  maxTotalDailyCostMicrousd = 1000000000;
};
```

These are admission controls for declared limits, not a distributed rate
limiter. `maxEnabledAgents` bounds the generated broker units, internal
bridges, and state directories; the other limits bound declared broker policy.
Choose them from measured host, provider-account, and incident-budget capacity;
leave an option unset only after explicitly accepting that no corresponding
host-wide ceiling is enforced.

Every broker process is additionally contained by systemd with a default
128 MiB `MemoryMax`, `TasksMax = 64`, and `LimitNOFILE = 4096`. Tune the
`serviceMemoryMaxBytes`, `serviceTasksMax`, and `serviceNoFileLimit` options
only from measured request/response and connection behavior; these per-service
limits complement, but do not replace, host capacity planning or the aggregate
admission ceilings.

## Credential boundary

The real provider credential is supplied to the LLM unit through systemd
`LoadCredential`. It is read again for each request, so rotating the source
credential and restarting only that broker is sufficient. It is never added
to the agent environment, container mounts, Nix store, or audit log.

systemd exposes each loaded credential read-only and grants the dynamic service
user access with a named ACL. The broker accepts the resulting group-class ACL
mask bit only for an exact file directly below `$CREDENTIALS_DIRECTORY`;
ordinary credential files remain restricted to owner-only permissions, and
other-access bits are always rejected.

The credential setup unit creates these files in a sandbox with no network,
private temporary and device views, no additional Linux capabilities, and no
namespace or set-ID transitions:

```text
/run/tentaflake-broker/<container>/agent-token
/run/tentaflake-broker/<container>/agent.env
```

The environment file contains the virtual key, `OPENAI_BASE_URL`, and the
Tentaflake broker URLs. It is the only environment file accepted by the
balanced container policy. The virtual key is scoped to one agent network and
is rejected by every other broker instance.

The broker writes one startup-readiness event after it has proved that its
private audit file can be opened and synced. `/healthz` rechecks credential and
audit readiness without appending an event, so health polling cannot rotate
useful audit history merely by probing. Audit files also reject symlink
targets. LLM input-token reservation uses request bytes as a conservative
upper bound rather than an optimistic characters-per-token estimate;
provider-reported usage remains separately recorded.

Host-side `tentaflake doctor` and VM probes reach `/healthz` through the exact
agent gateway address. VM readiness probes stop after 30 seconds and print the
raw health response, route, listener, firewall chains, broker unit status, and
journal instead of waiting for the test driver's global timeout.

## LLM policy

The LLM broker exposes only:

```text
POST /v1/chat/completions
POST /v1/responses
```

It accepts strict JSON, disables streaming, requires an exact model, clamps
the completion-token ceiling, reserves conservative token/cost budget before
the upstream call, disables environment proxies and redirects, resolves the
configured provider on the host, rejects non-public addresses, pins the
validated DNS answer into the TLS client, and rebuilds the Authorization
header from the host credential.

Audit events contain agent, route, model, outcome, status, reserved token/cost
figures, and timestamps. They never contain request bodies, prompts, virtual
keys, provider keys, or provider response bodies.

## Fetch policy

The fetch broker exposes only:

```text
POST /v1/fetch
```

Its body is `{"url":"https://exact-host/path"}`. It requires HTTPS and an
exact lowercase hostname. Every initial target and redirect is resolved and
checked again. The broker rejects loopback, private, link-local, metadata,
CGNAT/Tailnet, multicast, documentation, benchmark, and other non-public IP
ranges. All DNS answers must be public, and the validated addresses are pinned
for the request to block rebinding.

Responses are bounded by size/time and exact media type. Raw bytes are written
to a mode-0700 quarantine directory. Returned JSON strips unsafe control
characters and includes both `trust = "untrusted_external_content"` and an
instruction boundary. This narrows exposure; it cannot prove that content is
free of prompt injection.

## Failure and revocation

The brokers fail closed when authentication, policy, DNS, upstream TLS,
budget-state persistence, quarantine, or audit persistence fails. Budget
reservations are conservative and are not refunded after a failed upstream
call. Each budget-state replacement synchronizes both the new file and its
parent directory before the request is considered reserved, preventing a
successful rename from being lost across a host crash. The current rate window
is stored with the daily budget, so a broker restart cannot reopen it. A
backward host-clock step does not reopen a previously spent daily or rate
window; it fails closed until time reaches the already observed window.

`GET /healthz` verifies that the virtual key, provider credential when
applicable, and prompt-free audit path are readable. Broker units restart on
failure with a five-second delay and stop after five starts in five minutes.
The generated security manifest records each exact host endpoint. The security
doctor treats connection failure as unknown (`TFSEC-035`) and an answering but
non-ready broker as high severity (`TFSEC-036`). Label/mode/endpoint drift is a
critical desired-state inconsistency (`TFSEC-037`).

To revoke one agent immediately, use the combined CLI kill switch:

```text
tentaflake stop <agent>
```

Runtime mutation still requires an operator decision. Do not delete or
recreate the network automatically; if a subnet declaration changes, the
network unit detects the drift and fails until the operator resolves the exact
old network. The same fail-closed migration applies to networks created before
the deterministic bridge-interface option existed: inspect the exact stopped
agents and old network, remove/recreate that one network in an approved runtime
window, then restart its broker/controller units.

## Verification boundaries

Repository checks prove formatting, static policy logic, mock credential
substitution, SSRF literals, prompt-free audit, redirect revalidation, and a
deterministic DNS-rebinding fixture where an allowed name changes from a public
to a blocked answer. Nix evaluation covers the declarative policy. The VM
suite starts a scoped local LLM broker, reaches it from an isolated `runsc`
capsule, and receives a fixture response; this validates the permitted network
path but not external provider TLS. A full VM or host test must additionally
prove the OCI backend's internal network, nftables rules, systemd credentials,
live provider TLS, restart behavior, and cross-agent denial. The repository VM
declares Docker and Podman internal bridges, checks their deterministic
interface names, exercises `runsc` with both backends, and checks denial from a
separate external node; it still does not prove a real provider, authoritative
rebinding DNS server, or tailnet. A successful source evaluation is not
live-runtime proof.
