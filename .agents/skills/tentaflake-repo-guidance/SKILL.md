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
│   └── pinnedImage.nix
├── pkgs/
│   ├── tentaflake-cli/
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
  agent with `autoStart = true` must have all three or evaluation fails.
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

`modules/shell.nix` writes `/etc/tentaflake/cli.conf` and
`/etc/tentaflake/agents.tsv`, installs the binary, and configures shell QoL.
It does not implement the CLI in shell.

Primary commands:

```text
status health doctor stats logs
restart start stop shell exec ps backup
rebuild update
```

`tentaflake-status` is a status alias. The deprecated `hermes` name is a shim.
The rebuild/apply paths are explicit runtime operations.

## Brokered egress

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

`modules/workspace-quota.nix` optionally mounts an exact-size ext4 backing file
at each controller workspace. Builder assertions bind key/path/UID/GID to the
agent and the container/worker units require the mount owner service. The
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

`tentaflake doctor --security` combines the generated manifest with narrow
live checks for root disk, Restic success age, Tailscale Serve/Funnel, and
backend-specific Docker/Podman inspect drift. Unavailable AF_UNIX, sudo,
stopped-container, or incomplete-schema evidence is warning/unknown, never
green. Exact broker `/healthz` endpoints distinguish unreachable/unknown from
an explicit credential/policy/audit-readiness failure. Backup success is recorded by
`tentaflake-backup-success.service` in a systemd-managed persistent state
directory; `modules/hardening.nix` exposes an opt-in PID 1 hardware watchdog
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

The only ISO is `installer-iso`. `installer/installer.sh` copies the core Nix
sources plus `Cargo.toml`, `Cargo.lock`, `crates/`, and `pkgs/` into the target
configuration. `scripts/generated-flake-test.sh` checks the generated flake
with a JSON agent fixture.

Building the ISO is safe verification. Writing it to USB, partitioning a disk,
and installing NixOS are destructive runtime operations and need an exact,
confirmed target.

## Verification

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
