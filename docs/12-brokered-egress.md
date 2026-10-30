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
example `agent-coding` or `agent-research`, independently of adapter.

## Declaration

```nix
tentaflake.broker.agents.agent-coding = {
  enable = true;
  networkName = "tf-agent-coding";
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
and must not contain secrets or personal data. Clients may need an integration
to emit it, including on auxiliary calls. Broker forwarding alone establishes
neither client session propagation nor provider acceptance.

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
targets. LLM input-token reservation uses request bytes rather than an
optimistic characters-per-token estimate; provider usage is recorded separately.
That estimate is not proof of the provider's actual tokenization or invoice.

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

Chat Completions requests support exactly one completion choice in ordinary
and strict profiles, with or without streaming. Omit `n` or set it to the JSON
integer `1`; multiple choices and malformed values (including `null`, strings,
booleans and fractional numbers) are rejected before budget reservation or
provider dispatch. The token/cost reservation covers one completion, not a
batch of choices.

Audit events contain agent, route, model, outcome, status, reserved token/cost
figures, and timestamps. They never contain request bodies, prompts, virtual
keys, provider keys, or provider response bodies.

### Strict cost envelope

`llm.strictCostEnvelope = true` is an opt-in, default-off profile for clients
whose complete provider request shape and pricing have been reviewed. It accepts
only `/v1/chat/completions`, with one choice, local text messages and local
function definitions/calls/results. Text content may be a string or an array of
`{"type":"text","text":"..."}` parts. Function schemas are local JSON objects;
unknown message, tool, call and choice fields are rejected. Multimodal or file
references, provider-hosted tools, remote/conversation state, background work,
provider extensions and the `x-opencode-session` affinity header are not allowed.
Do not add an SDK-specific field until its billing and state semantics are reviewed.

The only top-level fields are `model`, `messages`, `tools`, `tool_choice`,
`stream`, `stream_options`, `max_tokens`, `max_completion_tokens`, `n`,
`temperature`, `top_p`, `stop` and `parallel_tool_calls`. The latter must be
false; `n` must be absent or one; the two completion limits are mutually
exclusive. `stream_options` is an object containing at most the boolean
`include_usage`. Streaming still requires `llm.streaming.enable = true` and uses
the same bounded SSE implementation; invalid requests fail before reservation
or dispatch. The ordinary profile retains both routes and its existing session
contract.

Strict mode requires exact allowed model names and positive input and output
prices. It holds an OS lock for the protected budget state and persists and syncs
the reservation and parent directory before dispatch. Uncertain reservations
are never refunded on a failure or restart. It refuses a missing, malformed or
symlinked budget, a budget changed while running, a missing/invalid initialization
marker, a replaced lock, and observed clock rollback, including within the same
UTC day. Its durable clock checkpoint advances on startup and on both accepted
and budget/rate-denied requests. Other brokers sharing the state use a shared
lock and cannot enter strict state or race strict initialization. The budget
directory and its lock must be protected from external replacement; filesystem
snapshot rollback and clock rollback between observations cannot always be
detected. A clock jump forward can also cross a UTC-day boundary early; use a
trustworthy host clock and provider-side limits. The existing per-minute rate
window is in memory and starts fresh after a restart; it is not a durable
account-wide provider rate limit.
Strict updates require Linux `renameat2` exchange support from the budget
filesystem; an unsupported filesystem fails closed instead of recreating a
missing state file.

Each strict reservation remains counted for its admission day even when local
preflight proves no upstream send was attempted. Resolution/address rejection,
unavailable credentials and audit failures before dispatch can clear only the
uncertainty, not that day's charge. Once a send may have begun, only a verified
complete provider response, a durable terminal audit, and successful
downstream writes settle its uncertainty. A completely read non-2xx provider
response (for example 400, 401, 429 or 500) ends the provider request and
settles without a refund; a truncated, slow or oversized error body does not.
Malformed or partial responses, cancellation, timeouts, post-dispatch audit
failures and observed delivery failures leave it unsettled. Non-streaming
success must identify the allowed model and one finished choice whose output
is well formed: text (possibly empty or withheld by a content filter), a
refusal, or valid local function calls. Every successful SSE completion chunk
must carry that model and the same nonempty completion ID, and each choice must
have a valid delta object before the real terminal event. The allowed model
also matches its dated provider snapshot, such as `gpt-4o-2024-08-06` or
`model-20241022`; any other suffix, such as `-mini`, stays unconfirmed. Ordinary
(non-strict) streams do not require deltas, so choice-level metadata chunks such
as content filter results are forwarded unchanged.

Strict mode persists a usage-accounting hold with each reservation before
dispatch. If provider-reported numeric usage exceeds the reserved token total
or its configured-price calculation exceeds the reserved cost, it durably
exhausts both daily counters before forwarding the usage-bearing response.
Unrepresentable token sums or prices exhaust the counters too. Partial or
inconsistent usage remains incomplete for audit, but numeric components can
still establish an overage. This intentionally blocks all further admission for
the affected day rather than attempting an exact refund or reconciliation.
Requests already admitted or concurrently being admitted may still incur
additional provider charges.

The hold is cleared after normal processing, without refunding the reservation.
If the process dies while it is held, or an overage update cannot be persisted,
future admission is blocked even within the same day, including after restart,
until separately authorized operator reconciliation (see below). A crash before
dispatch can therefore also require reconciliation. Live requests can run concurrently, but
unresolved usage holds prevent a day reset.

An unsettled prior-day reservation prevents a UTC-day reset. If one request
completes after midnight while another prior-day request remains unresolved,
admission remains blocked. Every reservation stays charged to its admission day:
once all prior holds settle after midnight, the next admission starts a fresh
UTC day, so a request running across midnight does not charge the previous
day's spend, or a previous-day overage exhaustion, against the new day.
Completion capabilities are not recreated after a restart, so a lost upstream
response blocks later-day admission until operator reconciliation.
Do not remove the marker or initialize an empty replacement.

### Reconciling held strict budgets

Reconciliation is an explicit operator decision, never part of startup or
restart. First stop the agent and its broker, then verify the provider-side
outcome and usage of the held requests, for example in the provider's usage
console. Then run the generated one-shot unit:

```sh
systemctl start tentaflake-broker-llm-reconcile-agent-coding.service
```

The underlying command is `tentaflake-broker --reconcile-strict-budget
/absolute/path/budget.json`. It takes the exclusive budget lock, so it fails
while a broker owns the state, and requires a valid marker, checksum and
non-rolled-back clock. It clears held reservations and orphaned usage holds and
reports how many it resolved. Counters are never refunded: the held requests stay
charged to their admission day. If the provider reports usage above those
reservations, keep admission disabled for that day and adjust provider-side
limits; reconciliation does not record that overage. A missing or corrupt state
is not reconcilable and still needs separate investigation.

Successful downstream `write_all` confirms only that the local kernel accepted
the bytes; it does not acknowledge that the runner received them. Consequently
this broker cannot guarantee retention for every undetected downstream response
loss, or guarantee provider invoices. If that receipt guarantee is required,
keep the integration blocked pending a separately reviewed application-level
acknowledgement protocol; it is not part of these broker routes.

The strict state includes a SHA-256 digest of its canonical counters, unsettled
count and clock checkpoint, and is checked before admission and on restart. It
detects accidental valid-JSON corruption; it is not a MAC, a defense against a
writer who can recompute it, or a witness against storage rollback. Protect the
directory and retain the provider-side spending prerequisite.

Initialization is deliberately **not** part of service startup or restart.
For a new, unused budget only, an operator may separately authorize and run the
generated one-shot unit, for example:

```sh
systemctl start tentaflake-broker-llm-initialize-agent-coding.service
```

Stop the broker and its agent first and ensure no other broker uses that state.
The unit and broker use the same dynamic service identity and state directory;
the unit creates a private lock, initialization marker and empty budget exactly
once, syncing them before it exits. It has no automatic start dependency. The
dynamic user and group name must not collide with configured host identities;
operators must also avoid collisions with external user/group databases. The
underlying explicit command is `tentaflake-broker --initialize-strict-budget
/absolute/path/budget.json`; direct users must arrange the same protected
directory and ownership. An existing ordinary budget is not an empty new
budget and is never converted automatically. An interrupted initialization or
missing/corrupt state must remain stopped for operator reconciliation, not be
"fixed" by deleting the marker or reinitializing. Never initialize an empty
budget over spending already incurred on the provider account.

Local token and cost reservations are not an account-wide invoice cap. Before
paid dispatch, independently verify current provider pricing and all applicable
billable features, or establish a provider-enforced hard spending cap sufficient
for the account. The operator must also account for other processes/accounts
and storage rollback. If those bounds or the client's exact supported envelope
cannot be established, keep paid admission disabled; fixture tests and positive
configured prices alone do not establish live provider behavior or costs.

The ordinary (non-strict) profile retains its legacy shared-state behavior and
does not reconcile provider-reported overages. Concurrent ordinary brokers can
overwrite stale budget counters; neither their token nor price counters provide
the strict mode's durable overage guarantee. Use the strict envelope and an
independent provider-side cap when these bounds are required.

## Streaming model responses

Enable SSE explicitly for each exact agent broker; existing declarations keep
streaming disabled:

```nix
tentaflake.broker.agents.agent-coding.llm.streaming = {
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

Reservations are never refunded for reported usage, including successful
streams and missing usage. Strict-mode overage handling is described above.
Chat usage is optional; Responses usage comes from the terminal response. Only
consistent numeric input/output/total token counts enter streaming audit.
Malformed or missing streaming usage is `null`; prompts, tool arguments,
output and provider error text never enter audit. A stream ending without its
genuine terminal event fails.

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

The legacy fetch implementation retains at most 64 MiB of logical file bytes
and 1024 objects by default, configured through
`fetch.maxQuarantineBytes` and `fetch.maxQuarantineEntries` (both positive).
Binary JSON uses `max_quarantine_bytes` and `max_quarantine_entries` with the
same defaults. These controls do not enable legacy fetch in balanced profiles.
Admission scans the host-private directory under a process-exclusive directory
lock and a writer mutex before creating each object. The scan is bounded by the
entry limit; quarantine-directory/entry symlinks, nonregular files, hard links,
foreign ownership, public permissions and already-over-limit legacy contents
fail closed. Safe existing files count across restarts. Empty responses cost an
entry, and the entire response length is reserved before writing so partial
writes remain charged. No retained evidence is automatically deleted; operators
must stop the broker and explicitly archive/remove retained files or adjust the
limits before resuming when full. Logical byte limits do not include filesystem
metadata/block overhead; the entry limit bounds object-count growth.
The existing host-controlled state parent is resolved before opening a held
directory descriptor, preserving systemd's private-state parent aliases. The
quarantine directory itself is never followed through a symlink.

## Failure and revocation

The brokers fail closed when authentication, policy, DNS, upstream TLS,
budget-state persistence, quarantine, or audit persistence fails. Budget
reservations are conservative and are not refunded after a failed upstream
call.

Unauthenticated ingress shares one absolute `timeout_seconds` deadline (binary
configuration, default 60 seconds) across
headers and body, beginning at connection admission. Every socket read uses
only the remaining time; trickling bytes cannot renew the deadline or retain
preauthentication concurrency slots indefinitely.

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
