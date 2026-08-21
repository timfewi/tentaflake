# Security profiles and broker migration

## Result

Installed hosts default to `balanced`. All three runtime builders pass their
final merged OCI configuration through one policy. The policy rejects an
unsafe configuration during NixOS evaluation instead of silently weakening
it. `strict` also fails evaluation because no tested separate-kernel boundary
is implemented yet.

Phase A provides the containment baseline. A stopped balanced scaffold may
omit a broker and remains `network=none`. An automatically started balanced
controller must have an enabled broker policy, disposable worker, and fixed
workspace quota or evaluation fails. Brokered capsules join one dedicated
internal network. Neither path gives an agent a real provider credential or
general egress, and the fetch boundary does not claim to stop prompt
injection. See [the worker guide](13-disposable-worker.md).

## Profiles

```nix
tentaflake.security.profile = "balanced";
```

| Profile | Intended use | Network and credentials |
|---|---|---|
| `dev` | trusted local development and migration | legacy host networking, env files, and loopback ports may be configured |
| `balanced` | installed 24/7 baseline | stopped scaffolds may use `network=none`; auto-start requires one isolated broker network, worker, and quota |
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
- CPU, memory, swap, PID, open-file, and process limits;
- no published ports, caller networks, devices, caller-supplied host namespace
  overrides, runtime sockets, real credential files, or secret-like
  environment keys;
- the only network/file exceptions are the module-generated internal broker
  network and its random runtime-only virtual credential environment file;
- only declared state/workspace writable binds and generated Nix-store files
  read-only; all other host paths are rejected, including other agent state;
- digest-pinned images; the mutable-image escape hatch is dev-only;
- optional fail-closed Cosign verification before an OCI unit starts; publisher
  identity is a separate policy decision from digest pinning;
- AppArmor on the host and no administrative Docker-group membership.
- the Tailscale management path with `tag:agent-host`, while the public
  OpenSSH module is rejected.
- an enabled broker, disposable worker, and fixed-size workspace quota before
  a secure controller may set `autoStart = true`.

Default per-agent limits are configurable under
`tentaflake.security.resources`: `memory = "2g"`, `memorySwap = "2g"`,
`cpus = "2.0"`, `nofile = 4096`, `tmpfsSize = "256m"`, and
`runTmpfsSize = "64m"`. PID defaults remain runtime-builder specific.

`tentaflake.workspaceQuota.agents` provides a backend-independent hard
workspace ceiling by mounting a fixed-size ext4 image before the controller.
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
   and either keep it stopped and offline or declare the broker, worker, and
   workspace quota required for automatic start. The broker path is described
   in [brokered egress](12-brokered-egress.md).
2. For a trusted development-only machine, acknowledge the old authority:

   ```nix
   tentaflake.security.profile = "dev";
   ```

The second path restores compatibility; it is not recommended for untrusted
24/7 agents. `agents.json` is currently a dev-only compatibility schema because
it always describes direct environment files and service ports.

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
exit code. `TFSEC-020` is high severity when a secure agent has no worker;
`TFSEC-021` reports a missing managed workspace ceiling; `TFSEC-022` and
`TFSEC-023` cover declarative Seccomp/AppArmor confinement. `TFSEC-024` warns
when no publisher-signature start gate is configured. `TFSEC-026` reports a
missing backup declaration; `TFSEC-027` reports a missing/stale success stamp.
`TFSEC-028` through `TFSEC-032` cover unknown or active Serve/Funnel state and
root-disk pressure. `TFSEC-033` marks unavailable live OCI evidence and
`TFSEC-034` is a critical live-inspect mismatch. `TFSEC-035` keeps an
unreachable broker unknown, `TFSEC-036` is a high-severity explicit unhealthy
response, and `TFSEC-037` detects inconsistent broker labels/modes/endpoints.

The check combines declarative desired-state evidence with narrow live probes
for Tailscale Serve/Funnel, backup freshness, root-disk pressure, and narrow
Docker/Podman inspect comparisons. Podman uses its documented `OCIRuntime`,
effective/bounding capabilities, AppArmor profile, and Docker-compatible
`HostConfig`; missing schema fields stay explicitly unknown. A blocked or
unparseable probe is a warning, never green. Configured broker endpoints are
also checked for credential, policy, and audit-path readiness through
`/healthz`. The generated manifest records each capsule's exact broker-network
name, and live inspection treats a different network as unsafe; a legacy
manifest without that field remains unknown rather than accepted. The check
still does not prove complete controller-tool audit
coverage, remote policy delivery, or cross-agent network denial; those require
target-host runtime and VM evidence.

The VM suite supplies part of that runtime evidence with separate Docker,
Podman, and external-attacker nodes plus locally built no-network fixture
images under `runsc`. It checks public-listener denial, direct
DNS/Internet/loopback/private/tailnet/link-local/metadata denial,
an actual scoped LLM-broker response, deterministic bridge interfaces,
non-root/empty capabilities/read-only root, bounded workspace and PID behavior,
Docker live resource-limit inspection, hidden sensitive state/sockets,
changed-remote rejection, worker timeout/approval/cleanup, broker crash
restart, unsafe-doctor exit status, backup/restore, and a controlled VM reboot.
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
