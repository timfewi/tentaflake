# Agent adapters

Status: common schema-v1 OCI definitions and preset discovery are available.
Generic workloads remain stopped pending capability-specific admission. OpenClaw
remains a stopped scaffold; vendor runtime acceptance is separate.

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

## Generic isolated workload

A local definition needs no new Rust adapter or upstream security module:

```nix
{ mkAgent }:
[
  (mkAgent {
    adapter = "generic";
    name = "coding";
    definition = {
      schemaVersion = 1;
      # Replace with an independently reviewed artifact, never a mutable tag.
      image = "registry.example.invalid/agent@sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
      command = [ "agent" "--interactive" ];
      ownership = { uid = 10000; gid = 10000; };
      workspace = "/workspace";
      resources = { memory = "2g"; memorySwap = "2g"; cpus = "2.0"; pidsLimit = 512; };
      lifecycle = "stopped";
      capabilities = [ "files" "shell" "terminal" ];
    };
  })
]
```

The same `definition` object is accepted in a schema-v1 JSON `agents` entry.
The argument vector follows OCI semantics and preserves the image entrypoint.
Image review/provenance remains operator-owned; a digest alone is not proof of
trust. Evaluation neither downloads the image nor activates the host.

State lives at `/var/lib/generic-NAME`, mounted at `/state` with `HOME=/state`.
Its private `workspace` subdirectory is mounted at the requested workspace path
and becomes the working directory. That destination must be `/workspace` or a
normalized child. Non-root ownership defaults to 10000:10000. Resource fields
inherit host security limits; allowed overrides are `memory`, `memorySwap`,
`cpus`, `nofile`, `pidsLimit`, `tmpfsSize` and `runTmpfsSize`. Values must be
positive. Host administrators remain responsible for appropriate aggregate bounds.

Unknown versions/fields, root ownership, unpinned images, extra mounts,
environment/credential inputs, malformed commands and unsupported capabilities
fail before a module is produced. Definitions are non-secret, including command
arguments. Native shell/files run inside the ordinary gVisor capsule; this
contract does not redirect native execution to the worker. `worker` is an
additional declaration requiring matching host workspace/identity policy; no
automatic vendor routing is claimed.

The workload uses the existing host-selected Docker/Podman backend, gVisor,
read-only root, no-new-privileges, dropped capabilities, bounded resources and
`network=none`. It exposes no host/other-agent socket or model credentials.
Generic model and Research hooks are not yet accepted. `lifecycle = "service"`
or forcing `autoStart` still fails current broker/worker/quota/Research gates;
capability-specific admission is tracked in #116. `dev` is unavailable for the
new generic contract. After an explicitly authorized host update, an operator
may manually start an intentionally stopped offline workload and attach with
`tentaflake shell generic-NAME` or `tentaflake exec generic-NAME -- ...`.
These source fixtures do not establish vendor support or live isolation evidence.

### OpenShell reuse decision

Keep the existing OCI/gVisor backend for this contract. The investigated NVIDIA
OpenShell 0.1.x architecture supports separating agent commands from trusted
policy and credentials, which this design adopts. There is no verified NixOS,
gVisor, exact broker or isolated Research compatibility, so OpenShell is not a
required dependency. A future backend needs its own bounded compatibility and
isolation acceptance; selecting another agent must not select weaker containment.
See [the comparison and evidence limits](roadmap.md#inspiration-and-evidence-limits).

## Schema version 1

`adapters/default.nix` explicitly imports the generic workload and the Hermes,
ZeroClaw and OpenClaw presets.
Imports are lazy; discovering names requires no packages, image downloads or
services. `mkAgent` validates only the selected adapter. Unsupported options,
invalid settings shapes and unknown adapters fail during evaluation.

| Field | Contract |
| --- | --- |
| `schemaVersion` | `1` |
| `identity` | Runtime ID, investigated version and `compatibility`, `scaffold`, `definition` or `operational` status |
| `artifact` | Reviewed digest-pinned OCI reference, unselected scaffold, or operator-supplied generic instance artifact |
| `command` | Upstream default argument vector |
| `configuration` | Configuration format (`none` for generic), read-only generation and `generate name settings` hook |
| `ownership` | Default positive numeric UID/GID |
| `layout` | Default state pattern, workspace suffix and required writable areas |
| `lifecycle` | Managed service or stopped scaffold |
| `model` | Protocol families and `required`, `optional` or `unknown` streaming requirement |
| `research` | Accepted capability and upstream settings hook; tool discovery needs separate runtime evidence |
| `execution` | Worker queue interface and whether automatic routing is verified |
| `build` | Runtime module generator with its precise supported argument set |
| `metadata` | Effective instance, container, unit, state/workspace and numeric ownership |
| `capabilities` | Declared available hooks; declarations do not prove vendor acceptance |
| `evidence` | Source/refusal regression level and separately recorded vendor acceptance |

Adapter facts do not grant host authority. Shared `containerSecurity`, Research
socket projection, broker, worker, quota, image provenance and recovery modules
retain policy ownership. The shared `containerSecurity.apply` builder validates
the final image and owns containment for generic, Hermes and ZeroClaw workloads. Legacy native features
remain in their existing builders.

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
| Generic workload | `generic-NAME` / `BACKEND-generic-NAME.service` | `/var/lib/generic-NAME` / `workspace` | 10000:10000, with definition ownership |
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

`adapters/catalog.json` is the versioned source of preset, capability and support
facts. Nix imports it, `tentaflake runtimes [--json]` embeds it in the CLI, and
`python3 scripts/runtime-catalog-docs.py` renders this table. Its `--check` mode
runs in `checks.*.agent-adapters`. A declaration or source regression is not
actual vendor runtime acceptance. The generic definition's artifact is chosen
and reviewed by the operator; its catalog entry selects no image.

<!-- runtime-catalog:start -->

| Preset | Version | Declaration | Fixture | Vendor acceptance |
| --- | --- | --- | --- | --- |
| generic | 1 | definition | source-regression | operator-owned |
| hermes | image-snapshot-2026-07-18 | compatibility | source-regression | pending |
| openclaw | 2026.9.7 | scaffold | refusal-regression | pending |
| zeroclaw | 0.8.2 | compatibility | source-regression | pending |

<!-- runtime-catalog:end -->

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
The inspected built-in OpenAI request builders require streaming. The broker
now offers opt-in bounded SSE for Chat Completions/Responses; Anthropic Messages
remain rejected. See [streaming policy](12-brokered-egress.md#streaming-model-responses). The scaffold therefore declares streaming
as required for those built-in transports and refuses activation. This is a
source-level incompatibility finding; no actual pinned executable was exercised.
It does not rule out a separately reviewed custom provider or an upstream change.
Setting `stream: false` in a payload hook alone does not establish compatibility
with an event-stream consumer. Broker streaming does not establish acceptance with that pinned executable.

The existing broker credential-substitution regression also sends source-derived
streaming request shapes to both broker routes. It requires HTTP 400 with
`streaming is disabled`, unchanged budget state, and an audit entry without
prompts or credential values. Its upstream fixture is already closed, so an
accidental dispatch cannot satisfy the expected result. These deterministic
probes preserve the default denial. Separate live loopback-provider tests cover
the opt-in SSE transport, not vendor compatibility.

## OpenClaw acceptance and next design

Before enabling OpenClaw:

1. Review the exact OCI release digest and provenance, or separately review a
   local image with exact source/dependency hashes. Retain secure image checks.
2. Parse generated read-only configuration with the actual pinned executable;
   exercise startup, intentional stop, crash recovery and state after restart.
3. Capture a real bounded request against a deterministic broker fixture. Prove
   transport compatibility using the opt-in broker SSE path. Repeat bounded
   partial-output, cancellation, timeout/audit failure and conservative budget
   checks with that executable.
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

The broker streaming prerequisite is implemented separately from the adapter
refactor, with bounded SSE events/output, first/idle/total deadlines, cancellation,
no replay, pre-dispatch audit/budget admission and conservative reservation.
Actual OpenClaw transport acceptance remains unverified. Keep provider credentials
host-only and retain model/tool allowlists; repeat partial-output, usage,
cancellation and audit-failure checks before any adapter activation.
Heavy VM/image/system builds and paid-provider probes need separate workload
authorization. Synthetic policy fixtures and package builds never establish
vendor operational compatibility.

## Implementation and evidence status

The adapter foundation, compatible wrappers, explicit inventory, versioned
JSON and opt-in broker SSE are implemented. Focused policy and protocol
fixtures cover shared admission, default streaming denial and bounded streaming
failure paths. See [brokered egress](12-brokered-egress.md) for transport details
and [build boundaries](17-builds.md) for the verification entrypoints.

OpenClaw has no selected OCI artifact. Actual native configuration parsing,
model/Research calls, worker mediation, lifecycle and persistence acceptance
remain pending. Hermes/ZeroClaw retain their existing integrations; synthetic
fixtures do not establish actual vendor workloads. No host activation is
implied by source checks.
