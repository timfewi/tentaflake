# Threat model

This document defines the security model for a tentaflake host running
long-lived, potentially compromised AI agents. It describes the intended
boundaries of the generic template. A deployment fork must add a target-host
review, credential inventory, tailnet policy, provider policy, and recovery
evidence.

## Security objective

The `balanced` profile assumes that prompts, retrieved content, dependencies,
and the agent process can become hostile. Compromise of one controller should
not by itself grant access to host secrets, another agent, direct Internet or
LAN egress, a privileged container runtime, or an unrestricted host shell.
Required external access is mediated by narrow host services. Risky local code
execution is delegated to a disposable, offline worker.

The `dev` profile is a compatibility mode, not a security boundary. The
`strict` profile fails evaluation because a tested separate-kernel boundary is
not yet implemented. There is no silent downgrade to either profile.

## Assets

The model protects:

- the host kernel, NixOS configuration, boot state, container engine, and
  system services;
- operator access, tailnet identity, and recovery authority;
- provider, Git, backup, signing, deployment, and optional attestation
  credentials;
- agent workspaces, broker audit records, backups, and operational logs;
- other agents and their state, credentials, budgets, and broker endpoints;
- external provider accounts, repositories, websites, and cost budgets;
- the integrity and availability of the declarative configuration and pinned
  software artifacts.

## Adversaries and failure modes

The design considers:

- prompt injection or malicious model output controlling an agent;
- hostile documents, websites, repositories, packages, and generated code;
- a compromised or malicious controller image or dependency;
- attempts to escape a container, abuse a Linux capability, exhaust resources,
  scan local networks, reach metadata services, or steal runtime credentials;
- cross-agent access, broker impersonation, SSRF, DNS rebinding, redirect
  abuse, request smuggling through oversized inputs, and cost exhaustion;
- unauthorized Git pushes or external side effects;
- a remote attacker reaching an accidentally published management service;
- operator error, stale backups, policy drift, storage exhaustion, service
  crash loops, and incomplete monitoring;
- supply-chain substitution of a mutable or unauthorized container image.

Denial of service against the whole host through a kernel, storage, container
engine, or hardware failure is reduced but not eliminated.

## Trust assumptions

The following remain trusted computing base or operator assumptions:

- the physical machine, firmware, host kernel, NixOS closure, systemd, Nix
  evaluation, and the operator account;
- the selected Docker or Podman engine, gVisor `runsc`, firewall, filesystem,
  and their correct activated configuration;
- the deployment fork's runtime secret provisioning and file ownership;
- reviewed image-signing identities and keys when provenance enforcement is
  enabled;
- an optional Sui attestation issuer, relayer budget, exact RPC endpoint, and
  custody of the AdminCap and any UpgradeCap;
- correct provider, repository, backup, tailnet, DNS, and alerting policy owned
  outside this generic template;
- operators review the exact build and target before activation and perform
  real restore and incident-response drills.

An image digest gives reproducible identity, not trust. Cosign verification
shows only that the configured signer authorized that exact artifact. Neither
claim proves that the software is benign.

## Trust boundaries and data flows

### Operator to host

Management enters through the operator account and an explicitly installed
tailnet grants/SSH policy. Tentaflake does not publish its CLI or observability
listeners to a public interface. Tailscale transport alone is not authorization,
and the template cannot verify the remote control-plane policy.

### Host to controller capsule

The host starts a digest-pinned controller with `runsc`, a non-root user, no
added capabilities, a read-only root filesystem, bounded resources, selected
AppArmor/Seccomp policy, and only declared mounts. Secure profiles reject host
networking, published ports, extra devices, unconfined overrides, sensitive
mounts, mutable image tags, and direct provider credential files.

Caller-supplied host user-namespace overrides are rejected. The current
root-managed Docker/Podman paths do not prove daemon-level host UID remapping:
Docker remapping would change every persistent bind-mount owner, and Podman's
`keep-id`/`nomap` modes are rootless-only. `runsc` and the non-root container
UID are enforced boundaries; separate host UID mapping remains an explicit
residual risk until an ownership migration and both runtime paths are tested.

The controller workspace is persistent and writable. An optional exact-size
filesystem bounds that workspace, but the controller may still corrupt or
delete its own data. Every enabled worker has a separate fixed-size private
state image with a post-mount layout marker; its queue, controller result bind,
and cleanup consumer stop if that mount disappears. Other state outside these
managed mounts still needs a deployment capacity policy.

### Controller to external services

A balanced controller either has `network=none` or joins one agent-specific
internal bridge. The bridge exposes only that agent's host LLM and fetch
brokers. Deterministic interface and source-subnet firewall rules reject
forwarding and unrelated host services. The controller receives only a
runtime virtual key, never the upstream credential.

The LLM broker enforces exact routes and models, clamps completion size, and
applies concurrency, rate, token, and cost budgets. The fetch broker permits
only configured HTTPS hosts and media types, resolves public addresses, pins
the connection address, revalidates redirects, ignores environment proxies,
bounds responses, and labels returned content as untrusted. These controls do
not make retrieved content safe to execute or trust.

### Controller to disposable worker

The worker receives a bounded, descriptor-safe workspace snapshot and runs a
short-lived `runsc` capsule with no network or secrets. It returns artifacts
through a controller-specific read-only import. Only `local-reversible`
actions run automatically; other permitted classes require a host decision, and
`forbidden` is rejected. A worker-wide root lock and non-overwriting atomic
`pending/` to `inflight/` claim ensure that drain, approve, and deny cannot
perform two terminal decisions for the same request. A crash after a claim is
recovered as a terminal outcome-unknown result, never an automatic replay.

Admission into the root-owned pending queue has explicit count and byte limits,
and each activation completes only a bounded number of ready requests. A full
queue rejects the excess request instead of moving unbounded agent-controlled
data out of the workspace. The fixed state image is the primary disk boundary;
static validation reserves snapshots, pending metadata, logs, control space,
inodes, and ext4 headroom, while runtime checks actual free blocks and inodes
before starting a capsule.

This boundary cannot prove that every tool exposed by every upstream agent
runtime routes shell or file work through the worker. Runtime-specific tool
configuration remains a deployment review item. Approval also does not grant
network access or implement a generic external-action adapter.

### Host to external systems

Git auto-push, backups, provider access, signature verification, remote
tailnet policy, and alert delivery execute with host-held authority. Their
credentials remain runtime files or systemd credentials. Each integration
must be narrowly scoped by the deployment: exact repository and branch,
provider account and models, backup repository, signer identity, tailnet
grants, and alert receiver.

### Optional Sui attestation

The experimental Sui reference is a host-side attestation boundary, not an
agent capability. A balanced controller receives no chain RPC route, gas
wallet, issuer key, AdminCap, or arbitrary transaction relay. A fixed collector
may derive only declared, root-owned evidence; a separate issuer signs a
canonical commitment payload; and a distinct, budgeted relayer may submit only
the fixed transaction to an exact endpoint after chain-ID and finality checks.
The shared on-chain objects contain opaque commitments rather than prompts,
outputs, customer data, or deployment identities.

A valid on-chain record means only that the configured issuer attested fresh
evidence under the configured registry. A consuming Move package must pin its
exact package, registry, record, and commitment values; accepting a proof type
alone is not an authorization boundary. It also does not establish that an
agent is safe. See [the Sui attestation guide](17-sui-agent-attestation.md).

### Telemetry and detection

Systemd journals are the primary local audit trail; there is no custom SQLite
audit database or web console. The optional Alloy/Loki/Prometheus/Grafana
profile is loopback-only and supplies baseline metrics and rules. The separate
Falco profile detects selected runtime behavior but does not prevent it.
Retention, off-host evidence, notification delivery, and sensitive-log review
remain deployment responsibilities.

## Control and evidence map

| Risk | Primary control | Required evidence |
|---|---|---|
| Host or peer access | gVisor capsule, mounts, capabilities, firewall | eval, build, live inspect, negative VM test |
| Direct egress | internal per-agent bridge and broker-only rules | activated firewall plus DNS/LAN/metadata tests |
| Credential theft | host-held credentials and virtual broker key | generated unit and live mount/environment review |
| SSRF and rebinding | fetch allowlist, public-IP checks, pinned DNS | broker tests and target-host broker health |
| Cost exhaustion | rate, concurrency, token, and cost budgets | policy review, audit output, alerts |
| Unsafe code execution | disposable offline worker and approval classes | queue, timeout, cleanup, and no-egress tests |
| Unauthorized Git write | exact remote and branch host helper | denial tests and least-privilege credential review |
| Mutable image substitution | digest pin and optional Cosign start gate | verified policy plus failed-signature start test |
| Optional on-chain evidence replay or false claim | registry-bound issuer signature, sequence, expiry, pause, and revocation | Move, cross-language BCS, host, and relay negative tests |
| Resource exhaustion | memory, CPU, PID, tmpfs, log, workspace, and fixed worker-state limits | live limits and disk/inode exhaustion tests |
| Lost state | encrypted Restic policy and success freshness | real fresh-host restore drill |
| Runtime anomaly | journald, optional observability and Falco | retention, receiver, and response drill |
| Remote management exposure | loopback listeners and tailnet policy template | live listeners and remote policy audit |

Source inspection proves intended code. Nix evaluation proves option types and
assertions. A successful build proves the closure. VM tests prove only their
fixture. Activation changes a particular host. Live inspection and adversarial
tests are required before calling that host secure. Unavailable AF_UNIX,
container-inspect, tailnet, broker, backup, or alert evidence is unknown, never
green.

## Explicit non-goals and residual risks

Tentaflake does not:

- make Docker, Podman, gVisor, the kernel, firmware, or hardware immune to
  vulnerabilities;
- provide a tested MicroVM or separate-kernel `strict` profile today;
- attest that a signed image, model, dependency, website, generated artifact,
  or optional on-chain commitment is safe or correct;
- prevent an agent from damaging its own writable workspace;
- transparently mediate every tool in every supported upstream agent runtime;
- provide generic adapters for arbitrary purchases, messages, deployments, or
  other external side effects;
- configure production tailnet grants, provider accounts, registry trust,
  backup storage, Grafana retention, Alertmanager delivery, or off-host logs;
- guarantee availability under host-wide storage, kernel, engine, network,
  power, or hardware failure;
- replace patch management, key rotation, capacity planning, incident response,
  or human review of requested actions.

Falco and observability improve detection. Backups improve recovery. Neither is
a preventive isolation boundary. A deployment must document accepted residual
risks and the owner of every external control before unattended operation.

## Deployment acceptance checklist

Before treating a host as ready for unattended agents:

1. Keep every untrusted controller on `balanced`; confirm `strict` still fails
   closed and document any deliberate `dev` exception.
2. Review generated units, exact workspace and worker-state mounts, identities,
   resource limits, image digests, provenance policies, and agent-specific
   broker policy.
3. Build and run the unit, module-evaluation, and VM suites without activating
   the production host as a side effect.
4. Review and separately approve the exact host activation.
5. Run `tentaflake doctor --security`, live OCI inspection, listener checks,
   and direct-egress, LAN, metadata, peer, and broker negative tests.
6. Audit the deployed tailnet grants/SSH policy and all external credentials
   for least privilege and rotation.
7. Configure log retention, workspace and worker-state capacity alerts,
   notification delivery, and an operator response path; validate them end to
   end.
8. Perform a real encrypted backup and fresh-host restore drill.
9. Record remaining unknown evidence and accepted residual risks. Do not count
   a warning or unavailable check as a pass.
10. If the optional Sui integration is selected, verify its Move and
    cross-language test vectors, issuer and relayer separation, RPC chain-ID
    and finality checks, explicit wallet budget, multisig capability custody,
    and the absence of agent chain authority.
