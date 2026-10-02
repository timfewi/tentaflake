# Agent adapters

Status: foundation verified; OpenClaw remains a stopped scaffold with pending
runtime acceptance. This is partial delivery in the container template only.

The requirement is one discoverable adapter per integrated runtime, a common
consumer builder, unchanged Hermes/ZeroClaw state and public arguments, and
explicit instance identities. Verification uses `checks.*.agent-adapters`,
`checks.*.research-policy`, the installer-generated flake regression, Rust CLI
fixtures, and `just fast`. Vendor runtime acceptance is a separate workload.

## Consumer API

```nix
{ mkAgent }:
[
  (mkAgent {
    adapter = "openclaw";
    name = "assistant";
    autoStart = false;
  })
]
```

This creates private state/workspace directories and an intentionally stopped
refusal unit. It creates no OCI container, downloads no image, and grants no
network or Research capability. An explicit start fails with the missing
acceptance requirements. `autoStart = true`, runtime settings, and image/tool
overrides are unavailable until those requirements are verified. This is partial
delivery, not a working secure OpenClaw deployment.

For current runtimes use `adapter = "hermes"` or `"zeroclaw"`. Their supported
arguments and defaults are those of `mkHermesAgent` and `mkZeroClawAgent`.
Both legacy entrypoints remain available. The library exports `mkAgent` and the
lazy `adapters` registry alongside those helpers. Installed and consumer flakes
pass them through `specialArgs`; the installer copies the adapter directory.
An older `my-agents.nix` accepting only `{ mkHermesAgent }` still works because
the importer passes only the helpers that its function requests.

See [the complete secure consumer example](../examples/adapter-secure.nix) for
Hermes with matching LLM broker, offline worker, Btrfs workspace and Research
relay. Its operator inputs are runtime credential paths, exact model prices,
VPN interface and readiness configuration. There is no operational OpenClaw
example yet: copying those host declarations would not solve tool routing or
model compatibility.

## Schema version 1

`adapters/default.nix` explicitly imports only Hermes, ZeroClaw and OpenClaw.
Imports are lazy; discovering names requires no packages, image downloads or
services. `mkAgent` validates only the selected adapter. Unsupported options,
invalid settings shapes and unknown adapters fail during evaluation.

| Field | Contract |
| --- | --- |
| `schemaVersion` | `1` |
| `identity` | Runtime ID, investigated version and `compatibility`, `scaffold` or `operational` status |
| `artifact` | Reviewed digest-pinned OCI reference, or explicitly unselected scaffold artifact |
| `command` | Upstream default argument vector |
| `configuration` | Configuration format, read-only generation and `generate name settings` hook |
| `ownership` | Default positive numeric UID/GID |
| `layout` | Default state pattern, workspace suffix and required writable areas |
| `lifecycle` | Managed service or stopped scaffold |
| `model` | Protocol families and `required`, `optional` or `unknown` streaming requirement |
| `research` | Accepted capability and upstream settings hook; tool discovery needs separate runtime evidence |
| `execution` | Worker queue interface and whether automatic routing is verified |
| `build` | Runtime module generator with its precise supported argument set |
| `metadata` | Effective instance, container, unit, state/workspace and numeric ownership |

Adapter facts do not grant host authority. Shared `containerSecurity`, Research
socket projection, broker, worker, quota, image provenance and recovery modules
retain policy ownership. The existing runtime bodies move into their adapter
files; they keep their different optional operational features instead of
flattening them into a second configuration API.

`tentaflake.agentInstances` is a read-only map derived from adapter declarations.
Inventory consumes explicit adapter/name/container/unit/state fields, keeps the
five-column TSV contract and retains the original prefix/first-volume fallback
for unmanaged OCI containers. Duplicate effective container identities fail,
including duplicates across Nix imports and JSON input.
Different runtimes may retain the same short name; use their complete container
identities in CLI commands to select one unambiguously.

## Compatibility and state

| Runtime | Container / unit | State / workspace | Container owner |
| --- | --- | --- | --- |
| Hermes | `hermes-NAME` / `BACKEND-hermes-NAME.service` | `/var/lib/hermes-NAME` / `workspace` | 10000:10000, retaining overrides |
| ZeroClaw | `zeroclaw-NAME` / `BACKEND-zeroclaw-NAME.service` | `/var/lib/zeroclaw-NAME` / `data` | 65534:65534 |
| OpenClaw scaffold | Reserved `openclaw-NAME` identity / refusal unit | `/var/lib/openclaw-NAME` / `workspace` | 1000:1000 |

Custom existing state directories, host users, seed destinations, no-clobber
copying, numeric owners, commands and optional operational services stay intact.
There is no automatic state move, deletion, conversion or reset. OpenClaw's
upstream `/home/node/.openclaw` mount is only a future runtime layout; the current
scaffold mounts nothing. Existing ext4 workspace migration remains explicit;
new managed workspaces are Btrfs and at least 128 MiB.

## JSON input

```json
{
  "schemaVersion": 1,
  "agents": [
    { "adapter": "hermes", "name": "coding", "autoStart": false },
    { "adapter": "openclaw", "name": "assistant", "autoStart": false }
  ]
}
```

The generic `agents` array requires the declared `schemaVersion`. Entries use
the selected builder's arguments. Legacy `hermes` and `zeroclaw` arrays remain
supported without a version; their existing fields and generated settings are
retained. Generic and legacy arrays are additive, with no replacement or hidden
precedence. Effective container collisions fail. Unknown root/entry fields,
unknown adapters, malformed entries and unsupported schema versions fail with
the relevant input location. Input remains non-secret; real provider keys are
host runtime files, never settings or environment values in the Nix store.

## Research and execution

`researchClient.nix` projects only the shared pinned client closure and exact
per-agent read-only relay socket. Upstream settings are adapter hooks. Research
validation requires an explicit registered adapter with accepted capability,
rather than trusting an OCI name prefix. Hermes disables native web/browser
toolsets and accepts only local stdio MCP additions. ZeroClaw disables its
native web tools and replaces its MCP list with the isolated stdio server.
Remote additions, legacy fetch, missing read-only mounts and other-agent sockets
remain rejected. Revocation and peer-UID isolation remain service-owned.

Declaring a worker is not automatic mediation. Current adapters expose the
existing queue/results interface; this change adds no automatic interception
of built-in shell, build, plugin or foreign-code tools. The threat model and
[worker controls](13-disposable-worker.md) retain that limitation.

## Support and upstream evidence

| Adapter | Artifact/version | Support level |
| --- | --- | --- |
| Hermes | Existing reviewed digest in `lib/constants.nix`, image snapshot 2026-07-18 | Preserved integration; exact release version and vendor workload acceptance are not newly verified |
| ZeroClaw | Existing reviewed digest, v0.8.2 | Preserved integration; vendor workload acceptance is not newly verified |
| OpenClaw | Investigated v2026.9.7; no reviewed runtime artifact selected | Stopped scaffold only |

OpenCode and Goose are the next investigations. Codex, Claude Code, Aider, Pi
and OpenHands remain candidates. No adapter files or support claims are added
for them.

Primary OpenClaw sources read through the isolated Research service:

- [v2026.9.7 Dockerfile](https://github.com/openclaw/openclaw/blob/v2026.9.7/Dockerfile): Node user UID 1000, private `/home/node/.openclaw` state, default gateway command and image entrypoint.
- [v2026.9.7 license](https://github.com/openclaw/openclaw/blob/v2026.9.7/LICENSE): MIT, OpenClaw Foundation; third-party notices require review before packaging.
- [v2026.9.7 configuration reference](https://github.com/openclaw/openclaw/blob/v2026.9.7/docs/gateway/configuration-reference.md): JSON5 configuration and actual schema/parser validation commands.
- [v2026.9.7 custom providers](https://github.com/openclaw/openclaw/blob/v2026.9.7/docs/gateway/config-tools/custom-providers.md): OpenAI Completions/Responses protocol choices; no verified non-streaming mode. `supportsUsageInStreaming` concerns streaming usage metadata and does not establish a non-streaming fallback.
- [v2026.9.7 MCP/extensions](https://github.com/openclaw/openclaw/blob/v2026.9.7/docs/gateway/config-extensions.md): native `mcp.servers` stdio declarations and session discovery. This is documentation evidence, not proof of discovery in our image.
- [v2026.9.7 exec tool](https://github.com/openclaw/openclaw/blob/v2026.9.7/docs/tools/exec.md): exec targets gateway/sandbox/node and can mutate permitted paths; none constitutes the Tentaflake disposable-worker queue.
- [v2026.9.7 stream selection](https://github.com/openclaw/openclaw/blob/v2026.9.7/src/agents/embedded-agent-runner/stream-resolution.ts) and [transport factory](https://github.com/openclaw/openclaw/blob/v2026.9.7/packages/ai/src/transports/provider-transport-stream.ts): the embedded runner selects built-in OpenAI stream functions. Provider overrides are extension code, not a verified non-streaming setting.
- [v2026.9.7 Completions request builder](https://github.com/openclaw/openclaw/blob/v2026.9.7/packages/ai/src/transports/openai-completions-params.ts#L309): both direct and managed modes use a request with `stream: true`; `supportsUsageInStreaming` only controls `stream_options.include_usage`.
- [v2026.9.7 Responses provider](https://github.com/openclaw/openclaw/blob/v2026.9.7/packages/ai/src/providers/openai-responses.ts#L143) and [stream lifecycle](https://github.com/openclaw/openclaw/blob/v2026.9.7/packages/ai/src/providers/openai-responses-shared.ts#L172): the direct provider constructs `stream: true` and consumes a Responses event stream.

No runtime artifact was downloaded, built or pulled. Repository/release HTML exceeded
the bounded extraction limit; the older embedded-runner source path returned
404. The versioned directory index identified the current source paths above.
The inspected built-in OpenAI request builders require streaming, while the
broker accepts only non-streaming Chat Completions/Responses and rejects
streaming and Anthropic Messages. The scaffold therefore declares streaming
as required for those built-in transports and refuses activation. This is a
source-level incompatibility finding; no actual pinned executable was exercised.
It does not rule out a separately reviewed custom provider or an upstream change.
Setting `stream: false` in a payload hook alone does not establish compatibility
with an event-stream consumer. No transport is added, emulated or bypassed.

The existing broker credential-substitution regression also sends source-derived
streaming request shapes to both broker routes. It requires HTTP 400 with
`streaming is disabled`, unchanged budget state, and an audit entry without
prompts or credential values. Its upstream fixture is already closed, so an
accidental dispatch cannot satisfy the expected result. These deterministic
probes document the failing acceptance requirement, not vendor compatibility.

## OpenClaw acceptance and next design

Before enabling OpenClaw:

1. Review the exact OCI release digest and provenance, or separately review a
   local image with exact source/dependency hashes. Retain secure image checks.
2. Parse generated read-only configuration with the actual pinned executable;
   exercise startup, intentional stop, crash recovery and state after restart.
3. Capture a real bounded request against a deterministic broker fixture. Prove
   non-streaming compatibility, or design broker streaming separately with
   bounded partial output, cancellation, timeout/audit failure handling and
   conservative request/token/cost accounting.
4. With only the projected stdio relay, prove actual tool discovery and one
   Research call. Verify native web tools, remote MCP, provider-hosted tools,
   channels, remote plugins and background tools cannot create another path.
5. Implement a reviewed upstream execution extension mapping commands to
   worker-queue v1 requests, await exact-ID results and propagate rejection,
   timeout and cancellation. Disable built-in exec/process alternatives and
   prove a build runs in the disposable no-egress capsule.
6. Wire the exact broker/worker/Btrfs quota/Research declarations, matching paths
   and UID/GID. Operator-provisioned credentials and VPN readiness stay on the
   host. Repeat negative policy checks after overrides and provenance gates.

The first independent prerequisite is a reviewed model-transport design:
either separately add streaming to the broker or verify an upstream-supported
non-streaming provider that works with the actual pinned executable. A broker
streaming change needs a bounded SSE parser and output ceiling, first-event and
whole-request deadlines, cancellation on client disconnect, no replay after
dispatch, pre-dispatch audit/budget admission, and conservative settlement when
usage is missing or the stream ends early. Keep provider credentials host-only
and the existing model/tool allowlists. Verify partial-output failures, usage
accounting, cancellation and audit failure before any adapter activation.
Do not bundle that transport change into this adapter refactor.
Heavy VM/image/system builds and paid-provider probes need separate workload
authorization. Synthetic policy fixtures and package builds never establish
vendor operational compatibility.

## Progress checkpoint

2026-10-02: the adapter foundation and stopped-scaffold stage are implemented
and reviewed. Adapter files, compatible legacy wrappers, common builder,
metadata, JSON, Research and inventory are synchronized with consumer examples,
agent instructions and bundled skills. Legacy argument/default expressions
match the original builders; runtime-body differences are limited to adapter
hooks and effective automatic-start admission. Existing state requires no move.

The final `just fast` passed in the selected Git checkout: formatting, lint,
ShellCheck, Rust formatting/Clippy, all 75 Rust tests, CI selection/package-source
regressions, Nix policy/adapter evaluation, flake evaluation without builds, and
the generated installer-flake regression. Streaming rejection probes cover both
broker routes, budget preservation and audit redaction. VM suites were not run.

The two temporary planning/prompt files were deleted and excluded from Git at
the operator's request. OpenClaw still has no selected OCI image or accepted
runtime integration; the complete operational objective remains unachieved.
The next phase is separate streaming research and design. Actual upstream
configuration, model/Research calls, worker routing, lifecycle and persistence
acceptance remain pending and require the corresponding workload authorization.
