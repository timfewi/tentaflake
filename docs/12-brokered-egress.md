# Brokered egress

## Result

Balanced agents can use external models without receiving a provider credential
or general internet access. Web access uses [secure-research-tool](16-research.md);
research-enabled agents must disable the legacy fetch broker described below. The
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
    enable = false;
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

Declare the research relay separately as described in [web research](16-research.md).
The fetch options remain for stopped compatibility scaffolds; they cannot be
enabled alongside a research relay. Model allowlists must select inference-only
models. Local function/custom tool definitions are forwarded; provider-hosted
tools such as web search, file search and remote MCP, plus `web_search_options`,
`plugins`, `extensions` and `data_sources`, are rejected before budget reservation.

Both `/v1/chat/completions` and `/v1/responses` preserve the optional
`x-opencode-session` header for JSON and enabled SSE requests. Its value must be
1–128 ASCII characters from `A–Z`, `a–z`, `0–9`, `_`, `.`, `:`, and `-`;
malformed values return HTTP 400 before budget reservation or upstream dispatch.
Only this client header is forwarded alongside broker-owned transport headers;
arbitrary client headers are dropped and the virtual key is replaced by the
host-held provider credential. Session values are not written to audit logs.

The client owns this opaque conversation-affinity value. The broker neither
generates it nor verifies conversation ownership; it grants no authorization
and must not contain secrets or personal data. Older Hermes images or clients
configured against the local broker may need a client-side integration to emit
it, including on auxiliary calls. Broker forwarding alone does not establish
Hermes session propagation or provider acceptance.

Use a unique `/30` per agent. The network address must end on a four-address
boundary and `gateway` must be its first usable address. Evaluation rejects
duplicate networks/subnets, unrelated gateways, missing modes, plain HTTP
providers, provider files outside `/run`, duplicate models, wildcard hosts,
wildcard media types, and deterministic bridge-interface collisions.

## Host resource admission

Each LLM and fetch process has a systemd memory ceiling of 128 MiB, 64 tasks,
and 4096 open files. Override `tentaflake.broker.serviceMemoryMaxBytes`,
`serviceTasksMax`, or `serviceNoFileLimit` when a reviewed workload needs more.
The existing capped restart backoff still applies after failure or exhaustion.

Optional host admission ceilings live below `tentaflake.broker` and default to
`null` (no aggregate ceiling):

| Option | Counted declarations |
|---|---|
| `maxEnabledAgents` | Enabled broker agents |
| `maxTotalConcurrency` | `maxConcurrency` per enabled LLM/fetch service |
| `maxTotalRequestsPerMinute` | Request-rate budget per enabled LLM/fetch service |
| `maxTotalDailyTokenBudget` | Declared daily token budget per enabled agent |
| `maxTotalDailyCostMicrousd` | Declared daily cost budget per enabled agent |

An agent enabling both modes counts twice for concurrency and request rate.
Disabled agents do not count. Token/cost sums conservatively include every
enabled agent, even fetch-only declarations. Exceeding a ceiling rejects the
configuration during evaluation. These are admission checks on declared
budgets, not a shared runtime counter or a guarantee of total host memory use.

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

The credential setup unit creates:

```text
/run/tentaflake-broker/<container>/agent-token
/run/tentaflake-broker/<container>/agent.env
```

The credential unit owns only its agent-specific `RuntimeDirectory`, mode
`0700`, under `ProtectSystem=strict`; token and environment files use `0400`.
The shared parent is explicitly read-only and only the exact agent directory
is reopened writable. Credential and broker units explicitly clear their
capability bounding sets; an empty Nix list would omit that systemd directive.
The runtime directory survives unit stop/start and restart, so existing
brokers and controllers retain the same virtual key until the next boot.

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

It accepts strict JSON, requires an exact model, clamps
the completion-token ceiling, reserves conservative token/cost budget before
the upstream call, disables environment proxies and redirects, resolves the
configured provider on the host, rejects non-public addresses, pins the
validated DNS answer into the TLS client, and rebuilds the Authorization
header from the host credential.

Audit events contain agent, route, model, outcome, status, reserved token/cost
figures, and timestamps. They never contain request bodies, prompts, virtual
keys, provider keys, or provider response bodies.

## Streaming model responses

Enable SSE explicitly for each exact agent broker; existing declarations keep
streaming disabled:

```nix
tentaflake.broker.agents.hermes-coding.llm.streaming = {
  enable = true;
  maxEventBytes = 64 * 1024;
  firstEventTimeoutSeconds = 30;
  idleTimeoutSeconds = 30;
  totalTimeoutSeconds = 120;
};
```

The same authenticated Chat Completions and Responses routes accept
`"stream": true`. `stream` must be a boolean. Streaming supports one completion
choice (`n` absent or `1`) and foreground requests; `background: true` is
rejected. Chat requests can use `max_tokens` or `max_completion_tokens`, but
cannot specify both. Responses use `max_output_tokens`. All retain the exact
model and local-function-tool allowlists, host-only credential substitution,
DNS address checks, runtime budget persistence and rate admission.

The host forwards complete events as HTTP/1.1 chunked `text/event-stream`
responses. It buffers at most one event, normalizes SSE line endings, handles
fragmented UTF-8 and multiline data, and retains text, reasoning and function
argument deltas. `maxEventBytes` must fit the existing `maxResponseBytes` total
upstream-body ceiling (8 MiB by default). Heartbeats do not satisfy the first
data-event deadline. Read/write inactivity and the whole exchange have separate
deadlines; the latter includes connection and downstream writes. All durations
must be positive and the first/idle limits must fit the total limit.

Client disconnects cancel pending sends, reads and writes, including a silent
provider. Clients must keep their connection open while receiving the stream;
a closed write half or additional request data terminates the exchange. The
parallelism slot stays occupied until forwarding finishes or aborts. Redirects,
environment proxies and request retries are disabled. Cancellation closes the
broker connection; it does not prove that a remote provider stopped generation
or will waive charges.

Before output, failures return bounded broker JSON errors when time remains;
an exhausted whole-exchange deadline closes the connection. After output starts,
a failure closes the incomplete chunked response. The broker never manufactures
`[DONE]` or a Responses completion. Valid `response.failed` and
`response.incomplete` events retain their actual outcomes. A terminal event is
released only after the audit record is durably synced and its parent directory
is synced to preserve log creation or rotation. A sync failure closes the stream
without releasing that event. Later client delivery failure
is recorded as an abort when the audit remains writable. Audit completion
admission is not proof that the client received every byte.

Budget reservation remains conservative for every request, including successful
streams and missing usage: reported usage does not refund tokens or cost.
Chat usage is optional; Responses usage comes from the terminal response. Only
consistent numeric input/output/total token counts enter audit. Malformed or
missing usage is `null`; prompts, tool arguments, output and provider error text
never enter audit. A stream ending without its genuine terminal event fails.

The implementation uses the pinned `reqwest 0.12.28` and `tokio 1.52.3`, with
explicitly disabled HTTP retry policy. Protocol references are the official
[Chat streaming events](https://developers.openai.com/api/reference/resources/chat/subresources/completions/streaming-events)
and [Responses streaming events](https://developers.openai.com/api/reference/resources/responses/streaming-events).
Local broker tests exercise forwarding, fragmentation, tools, usage, truncation,
deadlines, disconnects, audit failure and concurrency. They do not establish
OpenClaw runtime support or acceptance against a paid provider.

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
call.

`GET /healthz` verifies that the virtual key, provider credential when
applicable, and prompt-free audit path are readable. Broker units restart on
failure with exponential backoff from ten seconds to one minute. Explicit
operator stops prevent further retries.
Broker units use `RestartMode=direct`: automatic retries keep dependent
controller processes running instead of stopping and starting them. The broker
endpoint remains unavailable during recovery, so in-flight calls may fail;
the existing health probes still report unavailable or non-ready service.
Explicitly stopping a broker stops its `Requires` controllers. Starting the
broker afterward does not restart those controllers automatically.

Direct retries skip systemd `OnFailure`/`OnSuccess` hooks. Monitor broker health
and restart counts rather than relying on those hooks for transient outages.
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
