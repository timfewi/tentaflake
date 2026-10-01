---
name: tentaflake-repo-guidance
description: Comprehensive reference for tentaflake modules, profiles, agent builders, Rust CLI, installer, checks, and architecture.
version: 2.0.0
---

# Tentaflake repository guidance

## Purpose and boundary

Tentaflake is a generic NixOS flake template for isolated Hermes and ZeroClaw
containers on one host. Never add company configuration, real
hostnames, hardware identities, secrets, private agent context, or deployment
policy to the template. Those belong in forks.

Source changes and verification do not authorize NixOS activation, deployment,
disk operations, secret changes, or VM mutation.

## Layout

```text
tentaflake/
├── .devcontainer/
│   ├── devcontainer.json
│   └── devcontainer-lock.json
├── flake.nix
├── configuration.nix
├── Cargo.toml
├── Cargo.lock
├── crates/tentaflake-cli/
├── crates/tentaflake-broker/
├── crates/tentaflake-worker/
├── modules/
│   ├── default.nix
│   ├── options.nix
│   ├── shell.nix
│   ├── worker.nix
│   ├── workspace-quota.nix
│   ├── image-provenance.nix
│   ├── optional/
│   │   ├── editor.nix
│   │   ├── hive-research.nix
│   │   └── piper-tts-server.nix
│   └── profiles/
│       ├── observability.nix
│       └── falco.nix
├── lib/
│   ├── mkHermesAgent.nix
│   ├── mkZeroClawAgent.nix
│   ├── agentsFromData.nix
│   ├── mkRustPackage.nix
│   └── pinnedImage.nix
├── pkgs/
│   ├── tentaflake-cli/
│   ├── tentaflake-broker/
│   ├── tentaflake-worker/
│   └── piper-voices/
├── installer/
├── tests/
└── docs/
```

Removed components are not compatibility surfaces: there is no live-agent ISO,
Auditd daemon, SQLite event store, custom web console, Go module, or interactive
agent wizard.

## Flake outputs

Configurations:

| Output | Purpose |
|---|---|
| `nixosConfigurations.tentaflake` | Built-in installed host |
| `nixosConfigurations.installer-iso` | Disk installer image |

Core and optional modules:

| Output | Import behavior |
|---|---|
| `nixosModules.default` | Core module set |
| `nixosModules.installer` | Installer ISO |
| `nixosModules.editor` | Optional editor |
| `nixosModules.hiveResearch` | Optional web research |
| `nixosModules.piperTts` | Optional TTS |
| `nixosModules.observability` | Optional metrics/logs |
| `nixosModules.falco` | Optional runtime detection |

Packages and checks:

| Output | Purpose |
|---|---|
| `packages.*.tentaflake-cli` | Rust CLI |
| `packages.*.tentaflake-broker` | LLM/fetch policy broker |
| `packages.*.tentaflake-worker` | Host worker orchestrator |
| `packages.*.tentaflake-worker-image` | Nix-built offline worker image |
| `packages.*.installer-iso` | Installer image |
| `packages.*.piper-voices` | Optional voice assets |
| `checks.*.image-pinning` | OCI reference tests |
| `checks.*.module-evaluation` | Profile assertions |
| `checks.*.vm-integration` | Boot/runtime test |

## Core options

Most options are declared in `modules/options.nix`.

| Option | Default |
|---|---:|
| `tentaflake.hostName` | `"tentaflake"` |
| `tentaflake.adminUser` | `"user"` |
| `tentaflake.timeZone` | `"UTC"` |
| `tentaflake.containerBackend` | `"docker"` |
| `tentaflake.security.profile` | `"balanced"` |
| `tentaflake.profile` | `"installed"` |
| `tentaflake.boot.enable` | `true` |
| `tentaflake.hardening.enable` | `true` |
| `tentaflake.locale.enable` | `true` |
| `tentaflake.networking.enable` | `true` |
| `tentaflake.nixSettings.enable` | `true` |
| `tentaflake.packages.enable` | `true` |
| `tentaflake.users.enable` | `true` |
| `tentaflake.tailscale.enable` | `true` |
| `tentaflake.ssh.enable` | `false` |
| `tentaflake.networking.legacyPortEgress.enable` | `false` (dev only) |

Shell options:

| Option | Default |
|---|---:|
| `tentaflake.shell.enable` | `true` |
| `tentaflake.shell.tentaflakeCli.enable` | `true` |
| `tentaflake.shell.motd.enable` | `true` |
| `tentaflake.shell.tools.enable` | `true` |
| `tentaflake.shell.starship.enable` | `true` |
| `tentaflake.shell.zsh.enable` | `false` |
| `tentaflake.shell.zoxide.enable` | `true` |
| `tentaflake.shell.lazygit.enable` | `false` |
| `tentaflake.shell.tmux.enable` | `false` |

## Agent builders

`lib/default.nix` exports all builders plus `agentsFromData`, `pinnedImage`,
and constants.

Common contract:

- one systemd-managed OCI container per agent;
- digest-pinned, shell-safe images by default;
- private state directory;
- balanced/strict reject direct runtime credential files; `envFile` and
  `agenixFile` are dev-only compatibility inputs;
- explicit resource, mount, port, and policy arguments;
- final shared policy after extra container configuration, so secure
  invariants cannot be overridden through that escape hatch;
- balanced uses gVisor, non-root, cap-drop, read-only root, bounded tmpfs and
  resources, no ports/devices/direct egress, and digest pins;
- an agent without broker policy uses `network=none`; an agent with policy
  receives exactly one internal network and one runtime virtual-key env file.
- an enabled worker adds one agent-specific read-only result mount; the builder
  asserts that worker workspace and numeric UID/GID match the controller.
- a stopped balanced scaffold may omit broker/worker/quota policy; a balanced
  agent with `autoStart = true` must have broker/worker/quota plus its research
  relay or evaluation fails. Research uses only `secure-research-tool`; model
  calls stay on the LLM broker. See `docs/16-research.md`.
- caller `--userns` overrides are rejected, but root-managed Docker/Podman do
  not yet prove daemon-level host UID remapping; do not claim that property.

Use the runtime-specific source as authority:

- `lib/mkHermesAgent.nix`
- `lib/mkZeroClawAgent.nix`

Before changing a builder, inspect the generated container definition,
systemd dependencies, tmpfiles rules, assertions, and tests. A navigation index
does not replace current source.

## Agent inputs

`configuration.nix` imports `my-agents.nix` when present. It also converts the
secret-free `agents.json` schema through `agentsFromData`. Both are declarative;
there is no command that mutates them. The current JSON schema is dev-only
because it describes direct env files and ports.

Secret values must remain in runtime-only files. `extraEnvironment` is Nix
store-visible and is not a secret channel.

## Rust CLI

The workspace root is `Cargo.toml`; the binary source is
`crates/tentaflake-cli/src/main.rs`. `pkgs/tentaflake-cli/default.nix` packages
it with the locked Cargo dependency graph.

CLI, broker, and worker packaging share `lib/mkRustPackage.nix`, whose source
fileset contains only the root Cargo files and `crates/`. Keep crate fixtures
and build scripts inside their crate; declare additional root build inputs
in that fileset when needed. Documentation and host configuration stay outside
the Rust package source.

`modules/shell.nix` writes `/etc/tentaflake/cli.conf` and
`/etc/tentaflake/agents.tsv`, installs the binary, and configures shell QoL.
It does not implement the CLI in shell.

Primary commands:

```text
help status health doctor stats logs
restart start stop shell exec ps backup
rebuild update
```

`tentaflake-status` is a status alias. The deprecated `hermes` name is a shim.
The rebuild/apply paths are explicit runtime operations.

`help`, `--help`, and `-h` work without valid generated host inputs;
management commands continue to require them.

`health` and `doctor` share host diagnostics: exit `0` means healthy, `1`
means failed units, unknown agent states, or root-disk usage at least 90%,
and `2` means systemd/disk evidence was unavailable. Stopped agents remain
valid. JSON includes `failed_agents` and `unknown_agents`; `--hide` redacts
host and agent names in either output format.

## Brokered egress

`modules/research.nix` integrates the pinned public `tentaflake-research` service.
`tentaflake.research.agents.CONTAINER.uid` selects a distinct host relay identity.
Builders apply stdio MCP settings after caller settings, disable native web
tools, and project only the exact client closure and agent socket. Legacy fetch,
remote MCP and research summarization are rejected. Provider-hosted tools/web
extensions are rejected by the LLM broker. Never expose the root service socket
or broad `/run` mounts. The synthetic gVisor VM proves transport and settings,
not actual vendor agent MCP discovery. See `docs/16-research.md`.

`modules/broker.nix` declares per-container internal networks, virtual
credentials, LLM/fetch services, budgets, and subnet firewall rules.
`crates/tentaflake-broker` implements strict HTTP parsing, model/host policy,
provider-key substitution, SSRF controls, quarantine, and prompt-free audit.

The real provider credential enters only the LLM service through systemd
`LoadCredential`. The agent receives only the runtime-generated environment
file below `/run/tentaflake-broker/<container>/`. Fetch targets require exact
HTTPS hosts and public DNS answers; redirects are revalidated and pinned.

Do not treat source evaluation as proof that Docker/Podman internal networking,
nftables, systemd credentials, or live provider TLS work on an activated host.
See `docs/12-brokered-egress.md`.

## Disposable execution and approval

`modules/worker.nix` declares per-container queues. The host-side
`crates/tentaflake-worker` securely opens an agent workspace without symlink
traversal, creates a bounded snapshot, and runs each accepted request in a
short-lived Nix-built `runsc` capsule with `network=none`, non-root user,
read-only root, no capabilities or runtime socket, and resource/time/tmpfs
limits. Artifacts return only through an agent-specific read-only mount.
The host queue service uses a statically declared group whose GID exactly
matches the controller and preserves tmpfiles' setgid result directory without
weakening `RestrictSUIDSGID`.

Only `local-reversible` runs automatically. Other allowed action classes wait
in private host state for an operator `approve`; `forbidden` is rejected.
Direct root operator commands adopt the declared capsule GID before touching
worker state, matching the systemd queue service without adding `CAP_CHOWN`.
The path-activated oneshot has a ten-second failure backoff but disables the
aggregate service start counter because systemd also counts successful drains.
Approval never expands capsule authority, so generic external actions still
need a separate narrow broker. The module cannot prove runtime-specific agent
tool configuration routed every shell command through the queue. See
`docs/13-disposable-worker.md` and the `TFSEC-020` posture finding.

## Persistent workspace ceiling

`modules/workspace-quota.nix` optionally mounts an exact-size Btrfs backing file
of at least 128 MiB. Existing ext4 images fail closed without reformatting;
backup/restore migration is required. Offline checks use `btrfs check --readonly`.
Each image mounts at its controller workspace. Builder assertions bind
key/path/UID/GID to the agent and the container/worker units require the mount owner service. The
managed mount starts after ordinary local filesystems; ownership and the
private worker-control directories are restored inside it before the path
watcher starts. First activation is a real disk mutation and fails when the
workspace is non-empty; size drift never resizes silently. Source/eval does
not prove the loop mount or `ENOSPC` path. See `docs/14-workspace-quota.md` and
`TFSEC-021`.

## Image provenance

`modules/image-provenance.nix` can gate a generated OCI service on exact
Cosign key or keyless-identity verification of its digest-pinned image. The
policy is keyed by the generated container name. Enabling
`requireForSecureAgents` fails evaluation if any balanced/strict agent lacks a
policy; a failed verifier prevents that OCI unit from starting. Digest identity
and publisher authorization remain distinct claims. See
`docs/10-security-profiles.md` and `TFSEC-024`.

The authoritative asset, attacker, trust-boundary, residual-risk, and
deployment-evidence analysis is `docs/15-threat-model.md`. Keep it aligned
when a change adds authority, a new data flow, or a new external dependency.

## Optional observability

`modules/profiles/observability.nix` enables Prometheus, node exporter, Loki,
Alloy, and Grafana. All HTTP listeners bind to loopback. The profile opens no
firewall ports and requires runtime files for Grafana's secret key and admin
password. Values enter Grafana through systemd credentials, not the Nix store.

Alloy reads journald and forwards to local Loki. Grafana data sources are
provisioned for Prometheus and Loki. A root oneshot exports only numeric broker
denial/budget state to node exporter's textfile collector. Prometheus rules
cover disk pressure, restart flapping, denials, fetch bursts, and near-exhausted
budgets; notification delivery remains deployment-owned. See
`docs/09-observability.md`.

## Runtime posture and recovery

Secure controllers and brokers use `lib/serviceRecovery.nix`: on-failure
restarts with exponential delays from 10 seconds to one minute over five
steps, no permanent start-limit exhaustion, and no restart after an explicit
stop. Dependencies and security gates still apply on each start. VM coverage
accelerates the delays to exercise six consecutive broker crashes and stop.
Broker units additionally use `RestartMode=direct` so automatic retries preserve
the dependent controller PID. Explicit broker stop still stops that controller;
starting the broker does not implicitly resume it. Direct retries skip systemd
failure/success hooks; health and restart counts remain the outage evidence.

`tentaflake doctor --security` combines the generated manifest with narrow
live checks for root disk, Restic success age, Tailscale Serve/Funnel, and
backend-specific Docker/Podman inspect drift. Unavailable AF_UNIX, sudo,
stopped-container, or incomplete-schema evidence is warning/unknown, never
green. Exact broker `/healthz` endpoints distinguish unreachable/unknown from
an explicit credential/policy/audit-readiness failure. Backup success is recorded by
`tentaflake-backup-success.service` in a systemd-managed persistent state
directory. The Restic module adds enabled quota mounts inside selected backup
paths as separate sources, retains `--one-file-system`, and requires/asserts
those mounts before backup. Other nested filesystems require explicit paths;
live file backup is not an application-consistent snapshot. VM coverage checks
state/workspace restore, missing-mount failure, and recovery.
`modules/hardening.nix` exposes an opt-in PID 1 hardware watchdog
only for explicitly tested devices. Keep build,
activation, live checks, and restore drills as separate evidence gates.

## Optional Falco

`modules/profiles/falco.nix` is separate from observability. It requires an
explicit reviewed package because the pinned nixpkgs does not package Falco.
The unit selects `modern_ebpf` and bounds capabilities to BPF, performance,
resource, and ptrace access.

Evaluation is not runtime proof. Verify kernel BTF/ring-buffer support, event
capture, rules, and alert delivery on the exact activated host.

## Optional integrations

Editor, Hive Research, and Piper live below `modules/optional/` and are exported
individually. Do not import them from `modules/default.nix`. External inputs,
credentials, network listeners, and voice assets remain opt-in.

## Installer

The only ISO is `installer-iso`. New root partitions use Btrfs; the UEFI ESP
remains FAT32. Generated hardware configuration records these types. Existing
hosts are not converted during an update. `installer/installer.sh` copies the core Nix
sources plus `Cargo.toml`, `Cargo.lock`, `crates/`, and `pkgs/` into the target
configuration. Both Nixpkgs and research inputs retain the ISO lockfile revisions.
`scripts/generated-flake-test.sh` checks the generated flake
with a JSON agent fixture.

Building the ISO is safe verification. Writing it to USB, partitioning a disk,
and installing NixOS are destructive runtime operations and need an exact,
confirmed target.

## Verification

`just` recipes load the pinned development shell even from an ordinary shell;
`just list` aliases the recipe listing. Direct Cargo/lint commands need `nix develop`.

The optional contributor Dev Container installs Nix only; `flake.lock` and
`lib/devshell.nix` remain the toolchain source of truth. Its base image and Nix
Feature are digest-locked, and it must not mount runtime sockets or credentials.
`just e2e-devcontainer` builds the source- and dependency-hash-pinned
`.#devcontainer-cli`, rebuilds the container from the frozen Feature lock, and
runs the Nix lint smoke test inside it.

`just e2e` aliases the complete automated local gate. `just e2e-installer`
builds the ISO and starts a UEFI VM with only a contributor-owned sparse QCOW2
disk below `/var/tmp/tentaflake-e2e-<user>/`; it never passes a host block
device. `just e2e-run-vm` reboots that installed test disk without the ISO.
The wrappers deliberately do not delete or reset VM state.

`just security` scans the Rust and Dev Containers CLI lockfiles against OSV's
current advisory database, then runs source- and rule-pinned Semgrep without
registry access or metrics. It needs network access for OSV and is intentionally
separate from the reproducible `just ci` gate.

Focused Rust gate:

```bash
cargo fmt --all -- --check
cargo clippy --workspace \
  --all-targets --offline -- -D warnings
cargo test --workspace --offline
```

Nix and integration gates:

```bash
nix fmt -- --ci
nix flake check
nix build \
  .#checks.x86_64-linux.vm-integration \
  -L
nix build .#installer-iso
```

Shell and generated installation:

```bash
shellcheck installer/*.sh scripts/*.sh
./scripts/generated-flake-test.sh
```

When the managed shell cannot create the Nix daemon socket, use the typed
`throne-operations` check if available. Otherwise report Nix build/runtime proof
as blocked; direct parse or dummy-store evaluation is narrower evidence.

## Documentation contract

Behavior, option, or usage changes must update:

- `README.md` and relevant `docs/` pages;
- `AGENTS.md` and `CLAUDE.md` when instructions change;
- this skill and other affected bundled skills;
- examples, CI, PR templates, and `CHANGELOG.md` when applicable.

Do not call a change complete without a linked reason, a focused verification,
and synchronized docs.

### Research client lifecycle

The pinned MCP adapter reconnects future calls after a lost Unix session, with
bounded retries and fresh negotiation/UID authorization. Never replay dispatched
operations or reopen after explicit close. Verify actual gVisor MCP behavior and
live socket revocation after changing the client pin; see `docs/16-research.md`.
