# Agent adapters

`tentaflake.toml` is the only public instance-definition source. Each
`adapters/<id>.nix` owns that runtime's implementation and pure `preset` facts:
image, native configuration, commands, layout, ownership, hooks and evidence.
Shared host modules own
containment, broker, worker, quota, Research and recovery policy; selecting a
runtime never selects a weaker security implementation.

## Consumer API

Consumer NixOS modules load their own tracked TOML with the public file-only
helper. Keep the upstream commented template empty; see
[installation-owned configuration](08-agent-cli.md#installation-owned-configuration)
for the consumer/fork layout, Git filtering and missing-source refusal:

```nix
{ systemFromData, ... }:
{
  imports = [ (systemFromData { file = ./tentaflake.toml; }) ];
}
```

Pass `tentaflake.lib.${system}.systemFromData` through `specialArgs`.
Inline `data`, `mkAgent` and adapter `.build` functions are not public APIs.

Choose `hermes`, `zeroclaw`, `generic`, `openclaw` or `pi-durable`. Unsupported adapter
arguments fail evaluation. The registry is lazy: discovery does not download
images or instantiate services. See [configuration](08-agent-cli.md) for TOML
and installed-host onboarding, and [the secure consumer example](../examples/adapter-secure.nix)
for matching host declarations.

All managed identities are `agent-<name>` with unit
`<backend>-agent-<name>.service`. Names must be globally unique across adapters
and must not conflict with other host OCI declarations. Default host state is
`/var/lib/tentaflake/agents/<name>`; an explicit adapter `stateDir` preserves an
operator-selected path. Read `tentaflake.agentInstances` metadata for effective
container/unit, logical state root, actual `stateStorage`, workspace, numeric
ownership and `runnable`. Do not infer these from runtime prefixes. The CLI
retains a fallback for unmanaged OCI containers.

Public `stateDir` accepts normalized `/var/lib` paths, preserving existing
operator-selected state such as `/var/lib/private/kept` without relocating data.
The shared schema excludes the known container-engine state roots
`/var/lib/docker` and `/var/lib/containers` and every descendant, including for
stopped agents. Component boundaries are exact: `/var/lib/docker-agent` and
`/var/lib/containers_agent` remain valid. This exclusion is not proof that every
other host path is safe: operators must still select dedicated agent-owned
state, and runtime containment and ownership checks still apply. Low-level
trusted Nix configuration is a separate authority boundary, not permission to
bypass the public desired-system rule.

## Generic isolated workload

A reviewed custom OCI image and its in-capsule argv are supported:

```toml
schemaVersion = 1

[[agents]]
adapter = "generic"
name = "coding"

[agents.definition]
schemaVersion = 1
image = "registry.example.invalid/agent@sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
command = ["agent", "--interactive"]
workspace = "/workspace"
lifecycle = "stopped"
capabilities = ["files", "shell", "terminal"]

[agents.definition.ownership]
uid = 10000
gid = 10000
```

Replace the illustrative image with your reviewed digest-pinned artifact.
The argument vector preserves the OCI entrypoint and runs only inside the
offline capsule, never on the host. The root-owned mode-0700 instance root
remains unmounted; separately owned state and workspace children mount at
`/state` and the declared workspace (`/workspace` or a normalized child).
Read effective paths and owners from instance metadata for quota/worker policy.
Startup refuses unsafe ownership and symlink sources.

Definitions reject mutable images, root ownership, extra mounts, environment or
credential inputs, malformed commands and unsupported capabilities. The capsule
has a read-only root, dropped capabilities, bounded resources and `network=none`.
There is no accepted provider/model or Research integration, dev escape or
automatic startup. Host commands are not public schema inputs. Migration refuses
unsupported fields rather than silently dropping behavior.

## Native configuration and state

Native details belong here and in each adapter, not in shared host services:

| Adapter | Native configuration/layout | Default numeric owner |
|---|---|---|
| Hermes | `HERMES_HOME=/state`, `/state/workspace`; optional generated read-only YAML at `/state/config.yaml` | 10000:10000 |
| ZeroClaw | Generated read-only TOML `/zeroclaw-data/.zeroclaw/config.toml`; managed workspace `/zeroclaw-data/data` | 65534:65534 |
| Generic | No generated native config; `/state` and declared workspace | 10000:10000, definition override |

Native configuration is owned by the fixed adapter presets and host policy.
The public schema accepts `name`, `adapter`, `autoStart` and `stateDir` for
native/scaffold adapters; `user`, `group`, `uid`, `gid`, `containerUid` and
`containerGid` are Hermes-only. Generic agents accept only `name`, `adapter` and
the validated `definition` above; use `definition.lifecycle = "stopped"`, not
an `autoStart` field. Arbitrary `settings`, host commands,
environment, mounts and `seedDir` are refused.
Preserve existing unsupported native configuration during migration by stopping
and resolving the unsupported input, never by dropping it from the candidate.

Hermes generates its YAML only when internal settings or an enabled Research
hook produce nonempty configuration; an ordinary public stopped declaration
without Research creates no such read-only config mount. ZeroClaw always
generates its TOML. Internal seeds are no-clobber copies run as the capsule's
numeric owner: Hermes copies into its state root; ZeroClaw copies into
`.zeroclaw/data`, **not** the separate `data` workspace used by worker/quota
metadata. Seeding that native directory does not populate the managed workspace.

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
| pi-durable | 1.4.0 | scaffold | refusal-regression | pending |
| zeroclaw | 0.8.2 | compatibility | source-regression | pending |

<!-- runtime-catalog:end -->

### Pi Durable: pinned thin runner with fail-closed admission

The adapter ID is `pi-durable`. It embeds the Pi Durable SDK through Tentaflake's
mediated runner; it does not launch the interactive Pi terminal coding agent.
When upgrading from `pi`, change declarations to `adapter = "pi-durable"` and
host admission options to `tentaflake.piDurableRuntime`. Package/command names
start with `tentaflake-pi-durable`; the transport is `runsc-pi-durable-v1`.
Old names are not aliases. Stop admitted runners before the operator changes
the runtime/socket configuration; retain existing private state and workspace
paths and review uncertain work before restarting.

The runner compiles the reviewed, immutable **upstream source**
[`earendil-works/pi` revision `35dfa757378bd7a8570670e99c2687bdd1039a06`](https://github.com/earendil-works/pi/tree/35dfa757378bd7a8570670e99c2687bdd1039a06).
Its [`packages/durable/package.json`](https://github.com/earendil-works/pi/blob/35dfa757378bd7a8570670e99c2687bdd1039a06/packages/durable/package.json)
names `@earendil-works/pi-durable` **1.4.0**, ESM, Node **>=22.19.0**.
The [root MIT license](https://github.com/earendil-works/pi/blob/35dfa757378bd7a8570670e99c2687bdd1039a06/LICENSE)
is copyright 2025 Mario Zechner and requires retaining its copyright and
permission notice. The package declares MIT too. This is not a dependency-wide
license audit or proof that a published tarball matches this source revision.
Declared upstream runtime dependencies are Chord, Pi AI, Pi Codemode and Pi
Telemetry 1.4.0, Photon 0.3.3, diff 8.0.4 and typebox 1.3.27.
`pkgs/tentaflake-pi-durable/upstream.nix` builds the selected workspace closure with the
upstream lock, not a global npm installation. The runner uses pinned Nixpkgs
Node 24. Generated provider JSON and its manifest come from the separately hashed
MIT `@earendil-works/pi-ai` 1.4.0 archive
(`sha256-tA4rqLtvwzKQj4uW6DfcLR/ftF65zTgvWu0RFGKYHjw=`); packaged JavaScript is
not substituted for reviewed source. Upstream model-data checks remain enabled.
The retained upstream notice is `pkgs/tentaflake-pi-durable/UPSTREAM-LICENSE`.

The exact **default upstream recovery hazard**, avoided by the runner, is graceful
close followed by resume:

- [`scheduler.ts`, `join()`, lines 563–583](https://github.com/earendil-works/pi/blob/35dfa757378bd7a8570670e99c2687bdd1039a06/packages/durable/src/harness/scheduler.ts#L563-L583)
  aborts invocations and writes still-running tasks back as `pending`; its
  comment explicitly says the next open counts no restart.
- [`generation.ts`, request phase, lines 264–268](https://github.com/earendil-works/pi/blob/35dfa757378bd7a8570670e99c2687bdd1039a06/packages/durable/src/harness/generation.ts#L264-L268)
  takes the interrupted-attempt branch only when `task.restarts` is defined.
  Consequently disabling `settings.retry.enabled` does not prevent this
  pending request from being sent again after close/reopen/resume.
- The upstream
  [`harness-generation-recovery.test.ts`, lines 125–165](https://github.com/earendil-works/pi/blob/35dfa757378bd7a8570670e99c2687bdd1039a06/packages/durable/test/harness-generation-recovery.test.ts#L125-L165)
  explicitly expects a committed partial to become an aborted entry and the
  same messages to be sent again. These are inspected test sources, not tests
  executed by Tentaflake.

This is **not** a claim that upstream lacks all recovery controls.
[`generation.ts`, `interrupted()`, lines 496–544](https://github.com/earendil-works/pi/blob/35dfa757378bd7a8570670e99c2687bdd1039a06/packages/durable/src/harness/generation.ts#L496-L544)
does honor disabled retries for a crash-marked request.
[`tool.ts`, lines 193–207](https://github.com/earendil-works/pi/blob/35dfa757378bd7a8570670e99c2687bdd1039a06/packages/durable/src/harness/tool.ts#L193-L207)
commits execution intent with default `replay: "unsafe"` and re-executes only
when both stored and current tool policy say `"safe"`. These are useful
prerequisites for Research and worker mediation, not end-to-end acceptance.
An abort-before-close design must prove durable cancellation of every affected
task before scheduler shutdown, including concurrent submission, compaction,
failure during cancellation and restored older state. Simply substituting
`close()` for cancellation, disabling retries, or supplying a duplicate
submission ID is insufficient. No replacement model loop is supplied.

The public seam needs care beyond these retry flags:

- [`scheduler.ts`, lines 2627–2643](https://github.com/earendil-works/pi/blob/35dfa757378bd7a8570670e99c2687bdd1039a06/packages/durable/src/harness/scheduler.ts#L2627-L2643)
  catches generation-hook failures and reports `extension_error`, continuing
  unless the invocation is already aborted. A thrown journal/fsync failure in
  `beforeRequest` or `beforeProviderRequest` is not an authorization barrier.
- [`harness/types.ts`, lines 945–987](https://github.com/earendil-works/pi/blob/35dfa757378bd7a8570670e99c2687bdd1039a06/packages/durable/src/harness/types.ts#L945-L987)
  supplies hooks with stable task/conversation identities, but `afterResponse`
  runs before classification. Clearing an outer intent there loses protection
  if the process dies before the SDK commits its result.
- [`compaction.ts`, lines 164–190](https://github.com/earendil-works/pi/blob/35dfa757378bd7a8570670e99c2687bdd1039a06/packages/durable/src/harness/compaction.ts#L164-L190)
  calls `models.completeSimple` without generation request hooks or the
  generation task's interrupted-attempt guard. Recovery can repeat the summary
  request; disabled retry settings alone do not cover this path.
- [`ai/src/models.ts`, lines 258–367](https://github.com/earendil-works/pi/blob/35dfa757378bd7a8570670e99c2687bdd1039a06/packages/ai/src/models.ts#L258-L367)
  is a public structural interface accepted by `HarnessOptions.models`. A
  fail-closed facade could mediate all model methods, while independently
  refusing catalog refresh, login, images, classification and deferred requests
  outside the allowed protocol. Model options carry conversation session
  identity, not the SDK task ID.

`pkgs/tentaflake-pi-durable/{ledger,mediation,engine,runner}.mjs` implements the outer
single-writer receipt ledger and whole-instance quarantine around the real SDK;
it is not a replacement scheduler. An exclusive SQLite owner transaction holds
the process lease. FULL-synchronous admission and operation receipts precede
every model, Research and worker dispatch. Numeric SDK task IDs are retained as
stable correlations. A repeated request ID returns its existing run; a different
body under that ID is refused. One active submission is allowed.

The submission barrier remains until committed SDK settlement **and idle**, never
merely `afterResponse` or transport EOF. Stop records uncertainty before aborting.
Already-dispatched provider or worker operations may still finish; stop does not
claim to retract them, and their uncertainty is never converted into permission
to submit again.
Restart marks every unsettled admission uncertain before any writable SDK open.
Even a nominally clean ledger is checked against a read-only Harness inspection
and both directions of the SDK submission/receipt mapping; live or mismatched
restored state is durably quarantined. Upstream documents that `readOnly` recovers
nothing and rejects scheduling (`harness/types.ts`, lines 731–738).
Quarantine still permits receipt/status inspection, but neither writable Harness
open, new submissions nor redispatch. There is no quarantine-clear, restore/import,
or automatic reconciliation API. Restoring historical snapshots is not an admitted
resumption procedure; an internally consistent rollback cannot be detected by a
local receipt database alone.

The only model path is the pinned OpenAI Responses `streamSimple` implementation,
with one fixed broker model, a virtual key, SSE, zero retries, no redirects and
an injected guarded fetch. The source forwards that fetch through
`ai/src/api/simple-options.ts` to `openai-responses.ts`; the latter also sets the
OpenAI SDK's own `maxRetries` to zero. Each physical send requires a one-use task
correlation and the persisted barrier. All other Models facade methods, including
deferred responses, images, classification, login, refresh and ordinary completion,
are denied. Compaction is disabled and its completion path is unavailable.
Public/invalid broker addresses are rejected; production configuration comes only
from the exact broker instance, not agent settings.

Research runs only the pinned `${researchClient}/bin/research-client --socket
/run/tentaflake-research/socket` stdio client, with the five-tool manifest and
bounded MCP initialization. That client, not this runner, implements the framed
relay protocol and Hello/version/tool validation. The pinned relay authenticates
the client UID; the client does **not** perform reciprocal server `SO_PEERCRED`
authentication. Exact root-owned socket provenance and read-only closure/mounts
therefore remain required. There is no generic MCP server configuration.
Execution publishes only versioned `local-reversible` requests to the existing
offline worker queue; native bash, arbitrary extension loading, direct execution,
direct internet access and unrelated host/engine sockets are unavailable.

`adapters/pi-durable/options.nix` keeps host admission disabled by default. Enabling it
requires the exact runner package and local OCI bundle, their matching immutable
image selector/proof, shared balanced containment, one Responses-compatible
SSE-enabled broker model, the exact Research relay, and the exact offline-worker
workspace/UID plus **both workspace and durable-state quotas**. A chat-only strict
cost envelope is incompatible and refused, not silently weakened. After loading,
startup checks the exact immutable **configuration digest**, not merely an image
tag. Classic engine IDs provide that digest directly. Docker's containerd image
store may instead report a manifest ID: only for Docker, a differing well-formed
ID triggers a strictly local fixed-selector export and hashing of the exact raw
config bytes. Export uses a 2 GiB file-size limit and 150-second subprocess
timeout; the outer fallback has a 180-second timeout plus five seconds before
forced kill. Parser address space is limited to 256 MiB, the manifest to 64 KiB
and config to 4 MiB. Ambiguous members, links, malformed paths, mismatches and export failures
refuse startup; no registry/pull fallback exists. Podman retains direct-ID-only
verification. User image/stream, native command and arbitrary mount overrides
remain prohibited.

Admission also requires explicit `tentaflake.piDurableRuntime.transport = "runsc-pi-durable-v1"`.
The dedicated `runsc-pi-durable` wrapper alone enables `--host-uds=all`; ordinary runsc
is unchanged, including Research's existing open-only override. Docker uses
the exact canonical wrapper without runtime arguments; Podman uses its exact
containers.conf mapping. Rootless engines, user mappings, alternate wrapper
paths/flags and selecting this runtime as the engine default are refused.
The internal transport predicate is shared with host API descriptor admission
and security evidence. Selecting a flag is not runtime acceptance.

In this reviewed rootful transport the host listener is the **UID 0 gofer**,
while its socket inode belongs to **10100:10100**. The outbound API connection
trusts root/engine administration only after private ancestry and pinned-inode
checks, and still requires the bearer token; this is not an inbound root grant.
Research outbound credential impersonation is independently authenticated by
the host relay; no reciprocal server-UID handshake is claimed.

The runtime has private `state` and `workspace` directories owned by 10100:10100,
mode 0700, below a root-owned private instance parent. Its root-owned runtime
parent is mode 0700. Only the control directory is 10100:10100 mode 0710 and its
Unix socket mode 0620, permitting the separately authorized host API's exact
supplementary group to connect. The API token is root:10100 mode 0440, regular,
single-link and read-only in the container; the broker virtual token remains
10100:10100 mode 0400. Real provider credentials never enter the capsule.

The bearer-authenticated native socket supports `POST /api/sessions`,
`POST /v1/runs` (`session_id`, `input`, `context_mode: "persistent"`, `request_id`),
`GET /v1/runs/{id}`, lookup by `request_id`, `/result`, `/events`, and
`POST /v1/runs/{id}/stop`. Responses carry a client-compatible `run_id` and native
run projection; uncertain work is `state: "interrupted", uncertain: true`.
SSE carries terminal results only, not fabricated token deltas. Capabilities
explicitly deny live inputs and session import. Isolated context is unsupported.
Task ownership stays with the client's own task system; the runner API does
not grant enterprise approvals, connector permissions, or standalone UI admission.

Focused verification uses separate package, evaluation and VM gates:

- Build `.#tentaflake-pi-durable-sdk`, `.#tentaflake-pi-durable` and `.#tentaflake-pi-durable-image`.
- Run the packaged `bin/tentaflake-pi-durable-test`: exclusive ownership, duplicate and
  conflicting requests, stop/reopen, durable barriers, denied provider paths,
  credential permissions, and real pinned-SDK kill/close-after-send fixtures.
  Restored-receipt mismatch cases must stay quarantined with zero redispatch.
- Evaluate `tests/stopped-scaffold.nix` and the shared adapter/containment suite.
- Run `tests/research-integration.nix` when VM verification is authorized; its
  added Node probe uses this runner's actual Research class, pinned real client
  and real relay with an offline-denied fetch, not a generic MCP mock.
- Evaluate `tests/pi-durable-runtime-evaluation.nix { inherit pkgs; }` for exact
  configured transport and wrong-path/flags/rootless/user-mapping negatives.
- Build `.#checks.x86_64-linux.pi-durable-runtime-vm` when VM verification is
  authorized; it exports the two-agent fixture from
  `tests/pi-durable-runtime-vm.nix`. It is not part of the fast gate. The VM is
  limited to two cores, 4096 MiB and 900 seconds, with synthetic credentials,
  loopback-only unused provider configuration, real pinned runner/SDK and
  offline Research. It exercises listener-versus-inode identity, empty-capability
  API transport, private-path negatives, containment and Research connectivity.
  It does not yet prove model Responses exchange, relay upstream attribution,
  stale-inode replacement races or Podman runtime behavior. Those remain
  acceptance gaps, not successes implied by a source/evaluation check.

These fixture sources are not evidence of live model/provider acceptance.
OpenClaw still provisions only a stopped refusal unit. Its investigated OpenAI
transports require streaming; broker SSE alone is not executable compatibility
proof. Hermes/ZeroClaw also retain pending real-vendor model/Research/execution/
lifecycle acceptance. See [verification](17-builds.md).

## Breaking migration

This is a clean interface cutover with an explicit, offline migration:

1. Use `tentaflake config migrate` on the legacy `agents.toml` source.
   Migration must retain every reviewed instance, identity, supported path and
   owner, startup intent and optional `_securityNote` metadata; unsupported
   settings, host commands or host policy require refusal, not data loss.
   `schemaVersion = 1` and `agents` are required: use `agents = []` only for an
   intentionally empty host, otherwise `[[agents]]` tables as shown above.
   TOML has no JSON `null`; omit optional unset fields rather than inventing
   values. Keep credentials outside the inventory and Nix store.
   Review the complete translation before removing the old live file; keep
   any backup outside the live inventory path. A remaining `agents.toml` or `agents.json`
   is refused, even alongside TOML, never silently ignored or automatically
   converted. A remaining `my-agents.nix` is refused without evaluation.
   No separate example inventory is shipped: the checked-in and installer
   default is empty, not a migration of existing instances.
   Consumer flakes use file-only `systemFromData { file = ./tentaflake.toml; }`.
   Legacy `hermes`/`zeroclaw` arrays and public builders are removed. Use the
   `tentaflake` host CLI; the old host `hermes` alias and runtime-specific
   skills are removed. CLI import writes the same TOML source and never starts
   agents. Review, track, evaluate and build the migrated source before a
   separately authorized activation.
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
