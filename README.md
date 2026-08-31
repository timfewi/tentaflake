# Tentaflake

> [!WARNING]
> **Pre-1.0 project:** Tentaflake is under active development and has not yet
> reached version 1.0. APIs, NixOS options, defaults, installation flows,
> security boundaries, and documented workflows may still change
> substantially between releases, including breaking changes. Pin a release
> or commit for deployments, and review the [changelog](CHANGELOG.md) and
> migration notes before updating.

Tentaflake is a generic NixOS flake template for running isolated AI agents
on one machine. Hermes and ZeroClaw agents are declared as OCI
containers and supervised by systemd.

The core is intentionally small. It contains the host modules, agent builders,
an installer ISO, and a Rust operator CLI. Editor support, Hive Research,
Piper TTS, observability, and runtime detection are separate opt-in modules.

## Current scope

| Component | Status |
|---|---|
| Installed NixOS host | Core |
| Hermes and ZeroClaw builders | Core |
| Rust `tentaflake` CLI | Core |
| Rust LLM/fetch policy broker | Core, opt-in per agent |
| Disposable no-egress tool worker | Core; required for balanced auto-start |
| Fixed-size persistent workspace | Core; required for balanced auto-start |
| Fixed-size private worker state | Core; created for every enabled worker |
| Versioned Golden host-policy evaluation suite | Core; covered by VM integration |
| Cosign image start gate | Core, opt-in per agent |
| Encrypted Restic backup policy | Core, opt-in |
| Sui Move agent-attestation reference | Experimental optional source; no agent egress, signing, or deployment |
| Installer ISO | Core |
| Prometheus, Grafana, Loki, Alloy | Optional profile |
| Falco runtime detection | Optional profile |
| Hive Research, Piper TTS, editor | Optional integration |

There is no live-agent ISO, bundled audit daemon, SQLite event store, custom
web console, or Go workspace.

## Quick start

Enter the contributor shell and run the focused checks:

```bash
nix develop
cargo fmt --all -- --check
cargo clippy --workspace \
  --all-targets -- -D warnings
cargo test --workspace
```

Run the full flake check when Nix daemon access is available:

```bash
nix flake check
```

Alternatively, open the checkout in a Dev Container-compatible editor. The
committed `.devcontainer` installs the repository's Nix version, then warms the
same lock-file-backed `nix develop` environment used above. It does not mount a
container runtime socket or inject credentials.

Build the installer ISO:

```bash
nix build .#installer-iso
```

The result is written below `result/iso/`. Writing it to a block device is
destructive; follow [the install guide](docs/00-install.md) and resolve the
target device explicitly.

Contributor end-to-end entry points keep the pinned tool and VM setup behind
short recipes:

```bash
just e2e                 # complete automated local gate
just golden-evals         # run the versioned host-policy corpus in the VM
just e2e-devcontainer    # rebuild and smoke-test the locked Dev Container
just security            # Semgrep source scan + OSV dependency scan
just e2e-installer       # install into one isolated UEFI/QCOW2 VM
just e2e-run-vm          # boot that installed VM again
```

The installer VM never receives a host block device. Its persistent test disk
and UEFI variables live below `/var/tmp/tentaflake-e2e-<user>/`.

## Define agents

Copy the example and keep it generic in this repository:

```bash
cp my-agents.nix.example my-agents.nix
```

`my-agents.nix` may return any combination of the two builders:

```nix
{ mkHermesAgent, mkZeroClawAgent, ... }:
[
  (mkHermesAgent {
    name = "assistant";
    autoStart = false;
  })
  (mkZeroClawAgent {
    name = "assistant";
    autoStart = false;
  })
]
```

Alternatively, use the non-secret data shape in `agents.json.example`.
`agents.json` is declarative input; the removed interactive wizard no longer
edits it.

Installed systems default to `tentaflake.security.profile = "balanced"`.
Balanced agents are fail-closed capsules: no host network, published ports,
direct egress, real provider credentials, or mutable images. Without a
per-agent broker declaration they remain at `network=none`. With one, they
join exactly one internal network and can reach only their host LLM/fetch
brokers. `autoStart = false` in the example keeps activation explicit.
Setting `autoStart = true` under `balanced` is accepted only after the exact
container also has an enabled broker, disposable worker with its fixed private
state image, and fixed-size workspace quota. This prevents an incomplete 24/7
declaration from silently
starting with missing policy boundaries.

Existing configurations that require direct provider credentials or host
networking must deliberately select `dev`; this is a breaking change and is
not suitable for untrusted 24/7 agents. Never put secret values in Nix
expressions, JSON, Git, or the Nix store. See
[security profiles and migration](docs/10-security-profiles.md) and the
[threat model](docs/15-threat-model.md).

## Operator CLI

The CLI is built from `crates/tentaflake-cli` and installed by
`modules/shell.nix`.

```text
tentaflake status [--json] [--hide]
tentaflake health [--json] [--hide]
tentaflake doctor [--json] [--hide]
tentaflake doctor --security [--json] [--hide]
tentaflake logs <agent>
tentaflake restart <agent>
tentaflake shell <agent>
tentaflake exec <agent> -- <command>
tentaflake stats
tentaflake ps
tentaflake backup <agent>
```

`tentaflake-status` is a status alias. The deprecated `hermes` executable is
retained as a compatibility shim. `tentaflake top`, `console`, and the agent
wizard were removed with the old audit stack.

The `rebuild` and `update` subcommands are explicit runtime operations. Source
evaluation or a successful build does not authorize activation.

See [the CLI guide](docs/06-shell.md) and
[agent configuration guide](docs/08-agent-cli.md).
Management-plane policy is covered by the
[Tailscale policy guide](docs/11-tailscale-management.md).

## Security profiles

| Profile | Meaning |
|---|---|
| `dev` | Compatibility path; broad authority may be configured |
| `balanced` | Default; gVisor capsule, non-root, read-only, no direct egress or real credentials |
| `strict` | Reserved; evaluation fails until a tested separate-kernel boundary exists |

The shared policy in `lib/containerSecurity.nix` is applied after caller
overrides and asserts the secure invariants. The secure path drops all
capabilities, uses `runsc`, applies CPU/RAM/swap/PID/ulimit/tmpfs limits, and
rejects ports, devices, caller networks, real credential files, sensitive
mounts, secret-like environment keys, and attempts to override OCI security
flags. The only secure network exception is the module-generated internal
broker network and its runtime-generated virtual credential file.
The administrative user is not placed in the root-equivalent Docker group.
Balanced also requires Tailscale, advertises `tag:agent-host`, and rejects the
public OpenSSH module; the tailnet grants/SSH policy remains an operator-owned
external control that must be installed separately.

Phase B adds per-agent LLM credential and SSRF-safe fetch brokers, model/host
allowlists, request and daily budgets, prompt-free JSONL audit, DNS pinning,
redirect revalidation, quarantine, and host/FORWARD firewall rules. Agents
without that explicit declaration stay at `network=none`. Phase C adds an
opt-in disposable worker with bounded FD-safe snapshots, gVisor, no network or
secrets, runtime/resource/tmpfs limits, host-side action approval, cleanup, and
a read-only result path. Every enabled worker also receives a fixed-size ext4
state image for private requests, snapshots, results, and audit records; its
services and controller result bind stop if that mount disappears. An opt-in
fixed-size ext4 volume places a hard ceiling on each persistent controller
workspace. Other mutable host state still needs capacity monitoring. The broker
marks web material as untrusted; it does not claim that prompt injection is
solved. Digest pinning is mandatory in secure profiles. Optional per-agent
Cosign policies add a fail-closed publisher-signature gate before controller
start; they do not prove that signed software is harmless.

## Architecture

```text
flake.nix
├── modules/             core NixOS modules, brokers, worker
├── modules/optional/    editor, Hive, Piper
├── modules/profiles/    observability, Falco
├── lib/                 agent builders/helpers
├── crates/              Rust workspace
├── pkgs/                Nix package wrappers
├── installer/           installer ISO
└── tests/               evaluation and VM tests
```

The default module imports only:

- host options, boot, hardening, locale, networking, broker/worker and image
  provenance policy, Nix settings;
- base packages, users, SSH, Tailscale, and operator shell.

Optional capabilities must be imported explicitly from the flake output.
The editor additionally requires the consumer to add the `nvf` input and pass
its `inputs` through `specialArgs`.

## Brokered egress

Configure brokers by exact OCI container name. Provider credentials remain
runtime-only host files and are loaded only into the LLM broker. The agent
receives a random per-boot virtual key; it never receives the provider key.

```nix
tentaflake.broker.agents.hermes-coding = {
  enable = true;
  subnet = "10.203.20.0/30";
  gateway = "10.203.20.1";

  llm = {
    enable = true;
    upstreamBaseUrl =
      "https://api.openai.com/v1/";
    providerCredentialFile =
      "/run/agenix/openai-key";
    allowedModels = [
      {
        name = "gpt-5-mini";
        inputMicrousdPerMillion = 250000;
        outputMicrousdPerMillion = 2000000;
      }
    ];
  };

  fetch = {
    enable = true;
    allowedHosts = [ "platform.openai.com" ];
  };
};
```

Every enabled agent needs a unique `/30`. See
[brokered egress](docs/12-brokered-egress.md) for the full trust boundary,
failure behavior, budgets, and verification steps.

## Disposable tool worker

Enable the worker by exact OCI container name and match its workspace and
numeric user to the corresponding builder:

```nix
tentaflake.worker.agents.hermes-coding = {
  enable = true;
  workspace =
    "/var/lib/hermes-coding/workspace";
  containerUid = 10000;
  containerGid = 10000;
  stateVolumeMiB = 8192;
};
```

Jobs enter through `.tentaflake-worker/inbox/<id>.json`. The boot-enabled
worker drains pre-existing requests once, while later directory changes wake it;
preserved ignored entries do not continuously reactivate the service. Only
`local-reversible` runs automatically. External, financial, productive,
communicative, irreversible, or privileged classes remain pending until a
host operator approves the exact privately captured job; `forbidden` never
runs. Approval never grants network, secrets, host mounts, capabilities, or a
runtime socket. Results appear read-only below
`/run/tentaflake-worker/results/<id>/` in the controller. See
[the disposable-worker guide](docs/13-disposable-worker.md).
Tentaflake creates the matching host group automatically; deployments that
already own a custom container GID can select its existing group with
`hostGroup`. The worker state image is mounted with `noexec` and is created
only for an empty state mountpoint; changing its size or adopting an existing
worker requires the explicit offline migration in the
[disposable-worker guide](docs/13-disposable-worker.md).

The secure example also declares `tentaflake.workspaceQuota.agents` for each
controller. First activation creates and formats a sparse fixed-size ext4 image
for an empty workspace. Existing data is never hidden or migrated implicitly.
Read [the workspace quota guide](docs/14-workspace-quota.md) before enabling it
on an existing host.

## Optional observability

Import `nixosModules.observability`, enable the profile, and provide runtime
credential files for Grafana:

```nix
imports = [
  inputs.tentaflake.nixosModules.observability
];

tentaflake.profiles.observability = {
  enable = true;
  grafanaSecretKeyFile =
    "/run/agenix/grafana-secret-key";
  grafanaAdminPasswordFile =
    "/run/agenix/grafana-admin-password";
};
```

Prometheus, Grafana, Loki, Alloy, and the node exporter bind to loopback. The
profile does not open firewall ports or publish a dashboard.

## Optional runtime detection

Falco is separate because it needs host eBPF visibility and powerful kernel
capabilities. The pinned nixpkgs revision does not package Falco, so consumers
must supply a reviewed, pinned package:

```nix
imports = [
  inputs.tentaflake.nixosModules.falco
];

tentaflake.profiles.falco = {
  enable = true;
  package = myPinnedFalcoPackage;
};
```

The service uses Falco's modern eBPF engine. See
[observability and detection](docs/09-observability.md) for prerequisites and
trust boundaries.

## Optional integrations

These modules are exported but not imported by the core:

| Output | Source |
|---|---|
| `nixosModules.editor` | `modules/optional/editor.nix` |
| `nixosModules.hiveResearch` | `modules/optional/hive-research.nix` |
| `nixosModules.piperTts` | `modules/optional/piper-tts-server.nix` |
| `nixosModules.observability` | `modules/profiles/observability.nix` |
| `nixosModules.falco` | `modules/profiles/falco.nix` |

Piper voice assets remain available as `packages.x86_64-linux.piper-voices`.

### Experimental Sui attestation

[integrations/sui-agent-attestation](integrations/sui-agent-attestation) is an
unpublished Move reference package for agent-evidence commitments. It is not
imported by the core module set and does not give agents RPC access, a wallet,
a signing key, or an automatic publisher. The intended issuer and relayer stay
host-side and explicitly scoped. A consumer must pin its registry, record, and
commitment values rather than accepting a proof type alone. See the
[Sui attestation guide](docs/17-sui-agent-attestation.md) before evaluating a
testnet deployment.

## Build and test

```bash
cargo fmt --all -- --check
cargo clippy --workspace \
  --all-targets -- -D warnings
cargo test --workspace
nix fmt -- --ci
nix flake check
nix build .#installer-iso
```

The full flake check evaluates the installed host, builds the Rust CLI, runs
module assertions, and boots the VM integration test. That VM gate includes the
versioned [Golden host-policy evaluation set](docs/16-golden-evals.md). Keep
evaluation, build, activation, and live-runtime proof separate.

## Consume as a flake input

```nix
{
  inputs.tentaflake.url =
    "github:timfewi/tentaflake";

  outputs = { nixpkgs, tentaflake, ... }: {
    nixosConfigurations.host =
      nixpkgs.lib.nixosSystem {
        system = "x86_64-linux";
        modules = [
          tentaflake.nixosModules.default
          ./configuration.nix
        ];
      };
  };
}
```

The helper functions are exported under `lib.x86_64-linux`.

## Security boundaries

- The repository is a generic template. Deployment identities, real hosts,
  private agent context, and secrets belong in forks.
- Docker group membership is root-equivalent. Podman avoids that group but
  has different root/rootless store semantics.
- Optional dashboards stay loopback-only. Publishing them is a deployment
  decision and should add authentication.
- Falco detects suspicious runtime behavior; it is not container isolation.
- No source change, check, or build implicitly activates NixOS or mutates a VM.

Read [SECURITY.md](SECURITY.md) and the
[operations guide](docs/07-operations.md) before deployment.

## Contributing

Use Conventional Commits and add the DCO sign-off with `git commit -s`. Keep
behavior changes, verification, and documentation synchronized. See
[CONTRIBUTING.md](CONTRIBUTING.md).

Tentaflake is MIT-licensed. The name and logo are covered separately by
[TRADEMARK.md](TRADEMARK.md).
