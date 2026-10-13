# Security profiles and broker migration

## Result

Installed hosts default to `balanced`. The common adapter builder and compatible
Hermes/ZeroClaw wrappers apply one policy to final merged OCI configurations.
OpenClaw remains a stopped scaffold without a container or operational support.
The policy rejects unsafe configurations during NixOS evaluation instead of
silently weakening them. `strict` also fails because no tested separate-kernel boundary
is implemented yet.

Phase A provides the containment baseline. A stopped balanced scaffold may
omit a broker and remains `network=none`. An automatically started balanced
legacy controller must have an enabled broker policy, disposable worker, fixed
workspace quota, and research relay or evaluation fails. Brokered capsules join one dedicated
internal network. Neither path gives an agent a real provider credential or
general egress. [Web research](16-research.md) uses only `secure-research-tool`;
model calls remain on the LLM broker. Retrieved content remains untrusted.
See [the worker guide](13-disposable-worker.md).
Generic instances instead require their explicitly requested model, Research
and worker grants, plus fixed-size workspace and private-state quotas. Missing
grants also block manual starts; unrequested grants fail evaluation. See the
[generic contract](agent-adapters.md#generic-isolated-workload).

## Profiles

```nix
tentaflake.security.profile = "balanced";
```

| Profile | Intended use | Network and credentials |
|---|---|---|
| `dev` | trusted local development and migration | legacy host networking, env files, and loopback ports may be configured |
| `balanced` | installed 24/7 baseline | generic instances require exact requested grants and both quotas; legacy auto-start retains broker, worker, workspace quota and Research gates |
| `strict` | future high-risk workloads | evaluation error until a tested MicroVM/separate-kernel implementation exists |

There is no automatic fallback between profiles.

## Enforced balanced invariants

`lib/containerSecurity.nix` is applied after `extraContainerConfig`, so caller
overrides cannot bypass the final checks. It enforces:

- gVisor `runsc` selected explicitly for Docker or Podman;
- explicit non-root UID/GID and `privileged=false`;
- `cap-drop=ALL`, `no-new-privileges`, and read-only root;
- the runtime default seccomp policy with every unconfined override rejected;
  Docker additionally receives explicit `apparmor=docker-default`, while the
  Podman path requires host AppArmor and forbids an unconfined override;
- private tmpfs mounts for `/tmp`, `/run`, and `/var/tmp` with `nosuid`,
  `nodev`, `noexec`, and size ceilings;
- CPU, memory, swap, PID, open-file, and process limits; controller memory and
  total memory-plus-swap are bounded to 1 TiB each, tmpfs to 64 GiB each, and
  CPU quota to 1024 with at most five fractional digits;
- no published ports, caller networks, devices, caller-supplied host namespace
  overrides, runtime sockets, real credential files, or secret-like
  environment keys;
- the only network/file exceptions are the module-generated internal broker
  network, its random runtime-only virtual credential environment file, and
  the agent's exact read-only research socket/client closure;
- only declared state/workspace writable binds and generated Nix-store files
  read-only; all other host paths are rejected, including other agent state;
- digest-pinned images; the mutable-image escape hatch is dev-only;
- optional fail-closed Cosign verification before an OCI unit starts; publisher
  identity is a separate policy decision from digest pinning;
- AppArmor on the host and no administrative Docker-group membership.
- configured private management and SSH authorization capabilities under
  `tentaflake.management`; the supported Tailscale backend retains
  `tag:agent-host` and restrictive operator-maintained remote policy, while
  the public OpenSSH module is rejected. See the
  [management contract](11-tailscale-management.md#host-contract-and-migration).
- normalized mount paths; `.` and `..` escapes are rejected;
- exact capability and storage declarations before automatic startup, with
  legacy requirements retained and the generic manual-start guard enforced.

Default per-agent limits are configurable under
`tentaflake.security.resources`: `memory = "2g"`, `memorySwap = "2g"`,
`cpus = "2.0"`, `nofile = 4096`, `tmpfsSize = "256m"`, and
`runTmpfsSize = "64m"`. Generic `pidsLimit` defaults to 512; legacy PID defaults
remain builder-specific. Generic overrides may only reduce host ceilings.
Manifest v3 records exact effective OCI limits and requested capabilities;
missing or ambiguous resource declarations remain unknown. Older manifests
remain readable with conservative admission diagnostics.

`tentaflake.workspaceQuota.agents` provides a backend-independent hard
workspace ceiling by mounting a fixed-size Btrfs image before the controller.
It is opt-in because first activation formats a new backing file and existing
data requires an explicit migration. See
[persistent workspace quota](14-workspace-quota.md). Runtime-specific mutable
state outside that workspace remains outside this limit.

### User-namespace evidence boundary

Secure policy rejects caller-supplied `--userns`, including `--userns=host`.
That prevents a builder override from weakening a daemon-level mapping, but it
does not itself enable Docker `userns-remap`. Docker remapping is daemon-wide
and changes ownership requirements for every persistent bind mount. The
current root-managed Podman OCI path likewise cannot use the rootless-only
`keep-id` or `nomap` modes. Tentaflake therefore relies on the tested `runsc`
sandbox plus a non-root in-container UID today and does not claim a separate
host UID mapping. Adding one requires an explicit state, seed, Git-helper, and
workspace ownership migration with Docker and Podman runtime tests.

## Shared admission core

`crates/tentaflake-admission` provides one typed Rust decision function for future
host lifecycle callers. It checks exact owner/capability grants, revisions,
fresh live prerequisite evidence, service generations, per-instance limits and
aggregate CPU, memory, combined memory-plus-swap, PIDs, workspace and state.
Host and Research reserves count against capacity. Unknown runtime reservations
block startup; independent volume allocations retain storage charges for stopped,
never-started and removed instances. Unsupported guest-kernel
isolation is denied. This library does not collect evidence or enforce containment.

An offline operator command re-evaluates a recorded decision:

```bash
tentaflake admission explain crates/tentaflake-admission/tests/recorded-start.json
```

The input is a regular JSON file of at most 4 MiB containing
`nowUnixSeconds`, `snapshot` and `request`. The snapshot and request use the
versioned, closed Rust types in `src/lib.rs` (currently schema version 2); this internal diagnostic format
is separate from the [HTTP contract](19-host-api.md). Output is a JSON decision;
exit codes are 0 for allowed, 1 for denied and 2 for invalid input. `--hide`
redacts reservation identity. Recorded time and evidence are caller-supplied;
this command neither verifies them nor starts, reserves or reconciles anything.
The checked-in example is synthetic evidence.

The private on-disk ledger now uses schema version 3; pure snapshots and requests
remain version 2. Its initialization API requires a new directory, an explicit
`PublishedAuthority` and a complete storage inventory. Ordinary opens refuse
missing, corrupt, unsafe or older-version state; migration is explicit and never
interprets old state as empty. There is no host bootstrap command yet.
A nonblocking exclusive lock serializes decisions, and a durable unknown
reservation precedes any new dispatch permit. The in-memory permit retains
that lock through the launch handoff. An identical operation key returns the
existing receipt without another permit; a different request using that key
conflicts. Crash or uncertain completion retains capacity. Confirmed stopping
releases compute but retains workspace and private-state capacity; it does not
delete data. Receipt and byte limits refuse additional work rather than evicting
recovery records. Initialization must account for pre-existing workloads before
the ledger can be treated as complete.

Storage allocations are separate from runtime receipts so each volume is charged
once, including orphaned and uncertain images. Startup requires exact owned,
present workspace and state allocations. A storage-only claim also supports
legacy workspace-only quotas without requiring model or Research access. Its
new durable unknown allocation yields the only image-creation permit, under the
same ledger lock. Existing receipts never grant another creation permit; a missing
previously recorded image cannot be recreated. Resize, rebinding, deletion and inventory
recovery require separate explicit operations. The host must scan every managed
backing location, fence all volume creators and verify and sync the final image
before recording it present; the library does not perform those filesystem steps.

`PublishedAuthority` records the durable authority epoch, policy revision and
immutable manifest path. The policy, readiness snapshot and ledger share that
epoch; boot and service generations remain separate live evidence. Current
policy and ownership are checked under the ledger lock, including receipt
recovery. Publication, runtime and storage claims share operation-ID conflict
checks; an old receipt never supplies fresh mutation authority.

The publication API persists `Pending` before a host caller drains work or
installs a candidate plan. Pending blocks new claims and `with_present_storage`
calls while allowing recovery observations. `finish_publication` requires stopped
runtime reservations, Present storage, the exact installed candidate and a
trusted callback proving both plans' mutators are fenced. Explicit
`cancel_publication` requires the original plan restored with the same fence.
Failed observations leave Pending; restart does not cancel it or discard Unknown
charges. Terminal retries return receipts without replaying callbacks. These are library
transitions: the host still has to implement draining, installation and fencing.

`with_present_storage` holds the same lock through a current-authority check and
a bounded verification/ownership callback on an existing Present allocation.
It provides no creation permit or durable idempotency: retries may repeat the
callback. Callers must use repeat-safe helpers, verify exact files and mounts,
and keep helper processes within the callback's lifetime.

The optional `tentaflake.security.admission` declaration prepares a closed
version-1 `HostPlan` for balanced Docker hosts. It takes `authorityEpoch`,
`capacity`, `hostReserve`, `researchReserve`, `maxActive` and
`evidenceMaxAgeSeconds`; per-instance ceilings remain owned by
`security.resources` and quota options. Legacy compute, independent workers and
other unmanaged workloads must fit the explicit host reserve. The generated
store file is linked at `/etc/tentaflake/admission.json`; it contains configured
policy, exact quota bindings and immutable helper paths, with no live readiness.

This option is preparation only. It does not initialize a ledger, publish its
authority or put quota/OCI units behind ledger admission. The root-only
[`admission inventory` diagnostic](14-workspace-quota.md#read-only-admission-inventory)
always reports `complete: false`; its output cannot bootstrap the ledger.
Mandatory unit/lifecycle gating, live collectors and the HTTP service remain
unfinished. Existing generic units retain their declarative capability/quota
gates. A successful core decision or inventory scan does not establish runtime
isolation, upstream model authorization or vendor acceptance.

## Image signatures and provenance

Digest pinning makes image bytes reproducible but does not identify or trust a
publisher. When an upstream publishes Cosign signatures, record the exact
signing identity (or reviewed public key) per generated container name:

```nix
tentaflake.imageProvenance = {
  requireForSecureAgents = true;
  agents.hermes-assistant = {
    mode = "keyless";
    certificateIdentity =
      "https://github.com/example/project/"
      + ".github/workflows/release.yml@"
      + "refs/tags/v1.2.3";
    certificateOidcIssuer =
      "https://token.actions.githubusercontent.com";
  };
};
```

The generated `tentaflake-image-verify-*` oneshot runs `cosign verify` against
the same digest-pinned reference. The OCI service requires that unit, so a
missing signature, wrong identity, registry failure, or verification error
prevents the controller from starting. `mode = "key"` instead accepts an
absolute `publicKeyFile`; public verification keys are not secrets. Do not
invent an identity for an unsigned upstream. Leave signature enforcement off,
retain the digest pin, document the reviewed source, and accept the explicit
`TFSEC-024` warning until the publisher supplies verifiable signatures.

## Breaking migration

Old configurations that inject provider keys, publish dashboards/APIs, select
host networking, or use mutable images now fail under the installed default.
Choose one of two explicit paths:

1. Keep the fail-closed balanced capsule, remove direct credentials and ports,
   and either keep it stopped and offline or declare the LLM broker, worker,
   workspace quota, and research relay required for automatic start. See
   [brokered egress](12-brokered-egress.md) and [web research](16-research.md).
2. For a trusted development-only machine, acknowledge the old authority:

   ```nix
   tentaflake.security.profile = "dev";
   ```

The second path restores compatibility; it is not recommended for untrusted
24/7 agents. Legacy `agents.json` entries with direct environment files or
service ports require dev. The schema-v1 generic `agents` array accepts balanced
declarations; onboarding imports remain stopped. See [agent adapters](agent-adapters.md).

The former `tentaflake.networking.egress` option is renamed to
`tentaflake.networking.legacyPortEgress`. It is permitted only in `dev`. It
filters host `OUTPUT` by destination port and is explicitly not a destination
allowlist, container `FORWARD` policy, DNS control, or secure egress broker.

## Git auto-push

Every enabled host-side push needs a runtime `tokenEnvFile`, a non-empty list
of canonical full GitHub HTTPS remotes, and at least one branch:

```nix
gitAutoPush = {
  tokenEnvFile = "/run/agenix/github-coding";
  allowedRemotes = [
    "https://github.com/example/coding.git"
  ];
  allowedBranches = [ "main" ];
};
```

The host helper canonicalizes scheme, host, owner, and repository, rejects
userinfo, subdomains, alternate schemes, extra paths, query/fragment syntax,
and double `.git` suffixes, then pushes the exact checked branch. It uses a
repository-scoped `safe.directory`, never `*`, and writes allow/deny/error
records to journald. Use a fine-grained repository token or GitHub App
credential with only contents-write permission.

## Posture check

```text
tentaflake doctor --security
tentaflake doctor --security --json
```

The generated desired-state manifest covers profile agreement, host OpenSSH,
AppArmor, Docker-group membership, broker presence, networking, ports,
privilege, user, capabilities, `no-new-privileges`, root writability, mounts,
resource flags, `runsc`, digest pins, environment files, and disposable-worker
presence. Finding IDs are stable and critical/high findings return a non-zero
exit code. `TFSEC-020` is high severity when a requested worker is missing
(older manifests conservatively retain the mandatory-worker requirement);
`TFSEC-021` reports a missing managed workspace ceiling; `TFSEC-022` and
`TFSEC-023` cover declarative Seccomp/AppArmor confinement. `TFSEC-024` warns
when no publisher-signature start gate is configured. `TFSEC-026` reports a
missing backup declaration; `TFSEC-027` reports a missing/stale success stamp.
`TFSEC-028` through `TFSEC-032` cover unknown or active Serve/Funnel state and
root-disk pressure. `TFSEC-033` marks unavailable live OCI evidence and
`TFSEC-034` is a critical live-inspect mismatch. `TFSEC-035` keeps an
unreachable broker unknown, `TFSEC-036` is a high-severity explicit unhealthy
response, and `TFSEC-037` detects inconsistent broker labels/modes/endpoints.
`TFSEC-038` keeps legacy/incomplete management declarations unknown;
`TFSEC-039` reports a disabled declared private SSH policy. `TFSEC-040` and
`TFSEC-041` report missing requested model and Research grants, `TFSEC-042`
a missing required private-state quota, and `TFSEC-043` unrequested grants.
The generated manifest starts with `manifest<TAB>3`, retaining the v2 management
record and appending typed per-instance capability declarations. Legacy enable
flags never prove private operator access; declarations never prove live readiness.

The check combines declarative desired-state evidence with narrow live probes
for Tailscale Serve/Funnel, backup freshness, root-disk pressure, and narrow
Docker/Podman inspect comparisons. Podman uses its documented `OCIRuntime`,
effective/bounding capabilities, AppArmor profile, and Docker-compatible
`HostConfig`; missing schema fields stay explicitly unknown. A blocked or
unparseable probe is a warning, never green. Configured broker endpoints are
also checked for credential, policy, and audit-path readiness through
`/healthz`. The generated manifest records each capsule's exact broker-network
name, bind mounts, tmpfs sizes, absence of ports, and exact resource limits.
Inspection requires `State.Running=true`, the exact network set and internal
broker bridge, no published ports, and matching mounts and limits. Missing or
legacy manifest fields remain unknown. Each OCI subprocess has a five-second
deadline and an 8 MiB combined stdout/stderr ceiling, including inherited
output handles; overflow remains unknown. Broker
health probes run in batches of at most 16, with a two-second total deadline
and strict HTTP framing and JSON readiness checks. The check still does not prove
complete controller-tool audit coverage, remote policy delivery, or cross-agent
network denial; those require
target-host runtime and VM evidence.

The VM suite supplies part of that runtime evidence with separate Docker,
Podman, and external-attacker nodes plus locally built no-network fixture
images under `runsc`. It checks public-listener denial, direct
DNS/Internet/loopback/private/tailnet/link-local/metadata denial,
an actual scoped LLM-broker response, deterministic bridge interfaces,
non-root/empty capabilities/read-only root, bounded workspace and PID behavior,
Docker live resource-limit inspection and doctor detection of deliberately
changed memory limits, hidden sensitive state/sockets,
changed-remote rejection, worker timeout/approval/cleanup, broker crash
restart, unsafe-doctor exit status, backup/restore, and a controlled VM reboot.
The pinned Podman/runsc fixture reports an empty AppArmor profile and `null`
effective/bounding capability fields. Its live doctor result is therefore
critical for absent AppArmor; the VM verifies that negative result, not full
Podman confinement. Complete-schema Rust fixtures test parsing independently.

It does not emulate a real tailnet policy, upstream provider, production
registry, or a production-host activation/reboot drill.

## Threat model and residual risk

The complete asset, adversary, trust-boundary, control/evidence, non-goal, and
deployment-acceptance analysis is maintained in the
[threat model](15-threat-model.md). The summary below is intentionally brief.

Assets are the NixOS host, other agents, operator access, persistent state,
provider/Git/backup credentials, and external accounts. Attackers include a
compromised agent runtime, malicious web/document content, dependencies, image
content, and hostile model output. The model assumes the host kernel, NixOS
closure, gVisor package, OCI daemon, and operator identity are trusted.

Phase A narrows filesystem and kernel authority. Phase B prevents direct
egress, substitutes host-held provider credentials, and constrains external
web retrieval. The disposable worker additionally bounds offline untrusted
execution and requires host approval for non-local action classes, but cannot
prove every controller tool actually routed work through it. It does not prove
that an image digest is trustworthy, make
Docker/gVisor immune to vulnerabilities, prevent malicious writes inside the
agent's own workspace, bound runtime-specific state outside the workspace,
prove a host UID remap, enforce tailnet policy, or make untrusted web content
safe. Cosign verification
proves only that the configured signer authorized the pinned artifact; it does
not prove that the software is benign. Those are explicit residual risks and
dependencies, not hidden fallbacks.
