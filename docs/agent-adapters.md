# Agent adapters

`mkAgent` is the only public builder. Each `adapters/<id>.nix` owns that
runtime's implementation and pure `preset` facts: image, native configuration,
commands, layout, ownership, hooks and evidence. Shared host modules own
containment, broker, worker, quota, Research and recovery policy; selecting a
runtime never selects a weaker security implementation.

## Consumer API

```nix
{ mkAgent, ... }:
[
  (mkAgent {
    adapter = "hermes";
    name = "assistant";
    autoStart = false;
  })
]
```

Choose `hermes`, `zeroclaw`, `generic`, `openclaw` or `pi`. Unsupported adapter
arguments fail evaluation. The registry is lazy: discovery does not download
images or instantiate services. See [configuration](08-agent-cli.md) for JSON
and installed-host onboarding, and [the secure consumer example](../examples/adapter-secure.nix)
for matching host declarations.

All managed identities are `agent-<name>` with unit
`<backend>-agent-<name>.service`. Names must be globally unique across adapters,
Nix imports and JSON. Default host state is
`/var/lib/tentaflake/agents/<name>`; an explicit adapter `stateDir` preserves an
operator-selected path. Read `tentaflake.agentInstances` metadata for effective
container/unit, logical state root, actual `stateStorage`, workspace, numeric
ownership and `runnable`. Do not infer these from runtime prefixes. The CLI
retains a fallback for unmanaged OCI containers.

## Generic isolated workload

A reviewed custom OCI image requires no new adapter:

```nix
{ mkAgent, ... }:
[
  (mkAgent {
    adapter = "generic";
    name = "coding";
    definition = {
      schemaVersion = 1;
      # Replace with your reviewed artifact; this illustrative image is not runnable.
      image = "registry.example.invalid/agent@sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
      command = [ "agent" "--interactive" ];
      ownership = { uid = 10000; gid = 10000; };
      workspace = "/workspace";
      lifecycle = "stopped";
      capabilities = [ "files" "shell" "terminal" ];
    };
  })
]
```

The argument vector preserves the OCI entrypoint. The root-owned, mode-0700
instance root is unmounted; separate UID/GID-owned `state` and `workspace`
children mount at `/state` (`HOME=/state`) and the requested workspace, which
must be `/workspace` or a normalized child. Startup refuses symlink sources and
unsafe parent ownership/permissions. Generic `stateStorage` is
`/var/lib/tentaflake/agents/coding/state`, not the logical instance root.
Worker and quota declarations must match metadata paths and owners exactly.

Definitions reject unknown fields/versions, root ownership, mutable images,
extra mounts, environment/credential inputs, malformed commands and unsupported
capabilities. Resource overrides are positive bounds for `memory`, `memorySwap`,
`cpus`, `nofile`, `pidsLimit`, `tmpfsSize` and `runTmpfsSize`; host administrators
still own aggregate capacity. All input, including command arguments, is public
Nix-store material.

Generic workloads use the host Docker/Podman backend and common gVisor capsule:
read-only root, dropped capabilities, no-new-privileges, bounded resources and
`network=none`. There is no accepted model/Research integration or dev escape.
Automatic service startup remains refused by current capability/startup gates.
An operator may explicitly start a stopped offline workload after an authorized
host update. Shell/files tools stay in the capsule; declaring a worker does not
intercept native execution. No host or other-agent socket is granted.

## Native configuration and state

Native details belong here and in each adapter, not in shared host services:

| Adapter | Native configuration/layout | Default numeric owner |
|---|---|---|
| Hermes | YAML `/state/config.yaml`, `HERMES_HOME=/state`, `/state/workspace` | 10000:10000 |
| ZeroClaw | TOML `/zeroclaw-data/.zeroclaw/config.toml`, `data` workspace child | 65534:65534 |
| Generic | No generated native config; `/state` and requested workspace | 10000:10000, definition override |

Hermes mounts effective settings read-only when non-empty; Research adds
settings even without caller settings. ZeroClaw always generates read-only
configuration. Change declarative `settings`, not interactive configuration
files. Native option names and commands must be checked against the pinned
image. For example, `tentaflake exec agent-assistant -- hermes --help` invokes
the native command inside a Hermes container, not a host compatibility CLI.

Where supported, `seedDir` copies public initial content without overwriting
existing files. It is not synchronization; private personality, memories and
credentials need runtime provisioning. State quotas initialize directories only
after mounting and ownership setup; seeds/healing wait for that boundary.

Research hooks disable native web paths and project only the pinned stdio
client closure and exact read-only relay socket. Hermes permits local stdio
MCP additions; ZeroClaw replaces its native MCP list with the isolated server.
Remote MCP, provider-hosted web tools, legacy fetch and other-agent sockets are
not alternate routes. [Research](16-research.md) owns revocation and readiness;
[worker policy](13-disposable-worker.md) owns offline execution and approval.

## Support and upstream evidence

Edit runtime facts only in `adapters/<id>.nix`. `lib/runtimeCatalog.nix` projects
the pure presets; `adapters/catalog.json` is the generated CLI catalog, not a
second editable source. Regenerate JSON and the table together with
`python3 scripts/runtime-catalog-docs.py`. The same script's `--check` detects
drift. Catalog facts, source regressions and synthetic probes never establish
real vendor acceptance.

<!-- runtime-catalog:start -->

| Preset | Version | Declaration | Fixture | Vendor acceptance |
| --- | --- | --- | --- | --- |
| generic | 1 | definition | source-regression | operator-owned |
| hermes | image-snapshot-2026-07-18 | compatibility | source-regression | pending |
| openclaw | 2026.9.7 | scaffold | refusal-regression | pending |
| pi | 1.4.0 | scaffold | refusal-regression | pending |
| zeroclaw | 0.8.2 | compatibility | source-regression | pending |

<!-- runtime-catalog:end -->

OpenClaw and Pi create only stopped refusal units: no OCI artifact, state,
credential, network or socket. Runtime settings, image overrides and automatic
startup are rejected. Their reserved metadata is not provisioned storage.
Before enabling either, review an exact artifact and test actual configuration
parsing, startup/stop/recovery/persistence, broker model transport, Research tool
discovery and denial/revocation, and worker execution mediation. OpenClaw's
investigated built-in OpenAI transports require streaming; opt-in broker SSE
is not proof of compatibility with the pinned executable. Pi's target version
is not an installed SDK. Hermes/ZeroClaw also retain pending real-vendor
model/Research/execution/lifecycle acceptance. See [verification](17-builds.md).

## Breaking migration

This is a clean interface cutover, **not a data migration**:

1. Replace `mkHermesAgent`/`mkZeroClawAgent` with `mkAgent` and explicit `adapter`.
   Replace legacy `hermes`/`zeroclaw` JSON arrays with required
   `{"schemaVersion":1,"agents":[...]}`; only optional `_securityNote` root
   metadata is retained. Legacy arrays are rejected. Use the `tentaflake` host
   CLI; the old host `hermes` alias and runtime-specific skills are removed.
2. Resolve duplicate short names before switching: names are now globally
   unique. Map each old runtime-prefixed ID to `agent-<name>`.
3. Back up and verify state and mounted workspaces. Explicitly retain old
   storage with adapter `stateDir` where needed; no runtime data is moved,
   deleted, hidden, reformatted or reset automatically. Do not mistake a fresh
   default directory for migration. Preserve native layout and numeric owners.
4. Rekey every host unit/service reference and broker, worker, quota, Research,
   provenance and backup declaration to the new container ID. Match effective
   workspace/`stateStorage` and ownership metadata, not a guessed directory.
   Preserve quota backing images and mounted data; changing an ID can select a
   new image path. Follow [quota migration](14-workspace-quota.md) deliberately.
5. Preserve broker virtual-key/budget state and durable worker queues. Do not
   initialize a fresh budget to erase spend or held reservations; preserve and
   reconcile accounting through the [broker procedure](12-brokered-egress.md).
   Inspect old pending/running work using [worker recovery](13-disposable-worker.md);
   never replay uncertain execution.
6. Stop old controllers, brokers, workers, timers and other state writers as an
   explicit operator action before activating their replacements. Source edits
   alone do not stop installed units. Review the complete build and planned
   storage/accounting mapping, activate only with authorization, then verify
   old units are stopped and new units use the preserved data. Keep recovery
   copies until a restore and application check succeeds.

No automatic budget reset, quota recreation, unit shutdown or state relocation
is implied by selecting a new adapter or rebuilding.
