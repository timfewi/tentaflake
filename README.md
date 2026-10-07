# Tentaflake

![Tentaflake octopus and snowflake logo with agent mascots](public/tentaflake-readme.jpg)

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
Secure controllers and policy brokers recover from crashes with capped
systemd restart backoff, including after prolonged transient failures.
Transient broker crashes preserve the controller process. Explicit broker stops
still stop dependent controllers; model requests may fail while a broker recovers.

The core is intentionally small. It contains the host modules, agent builders,
an installer ISO, and a Rust operator CLI. Observability and runtime detection
are separate opt-in profiles.

The [roadmap](docs/roadmap.md) prioritizes a common isolated runtime and simple
agent onboarding on installed hosts, before employee-profile and company
features. This is planned work; the current support matrix below still applies.

Discover presets with `tentaflake runtimes --json`. A digest-pinned image and
command can use the [common isolated workload definition](docs/agent-adapters.md#generic-isolated-workload)
without a new adapter; generic startup remains gated pending capability admission.

## Current scope

| Component | Status |
|---|---|
| Installed NixOS host | Core |
| Adapter registry and Hermes/ZeroClaw builders | Core |
| Rust `tentaflake` CLI | Core |
| Rust LLM/fetch policy broker | Core, opt-in per agent |
| `secure-research-tool` web research | Pinned public service; required for balanced auto-start |
| Disposable no-egress tool worker | Core; required for balanced auto-start |
| Fixed-size persistent workspace and optional private state | Core; workspace required for balanced auto-start |
| Cosign image start gate | Core, opt-in per agent |
| Encrypted Restic backup including selected state/workspace quota mounts | Core, opt-in |
| Installer ISO | Core |
| Prometheus, Grafana, Loki, Alloy | Optional profile |
| Falco runtime detection | Optional profile |

There is no live-agent ISO, bundled audit daemon, SQLite event store, custom
web console, or Go workspace.

## Quick start

Run the contributor gate, which loads the pinned development tools itself:

```bash
just fast
```

Enter `nix develop` before invoking Cargo or lint tools directly. The fast gate
includes formatting, lint, Rust checks, policy and adapter evaluation, and the
installer-generated flake. For changes that need runtime evidence, run the
affected VM suite explicitly; the full flake check includes both VM suites:

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

New installations use Btrfs for the host root and fixed-size agent workspaces.
Existing ext4 hosts and workspace images require an explicit migration; see
[installation](docs/00-install.md) and [workspace migration](docs/14-workspace-quota.md).
Installer cleanup preserves other disks' swap, LVM and encrypted mappings;
LVM groups spanning the selected and another disk require migration first.

The result is written below `result/iso/`. Writing it to a block device is
destructive; follow [the install guide](docs/00-install.md) and resolve the
target device explicitly.

Contributor end-to-end entry points keep the pinned tool and VM setup behind
short recipes. They load the Nix development tools automatically, so `just lint`,
`just rust` and `just security` also work from an ordinary shell. `just list`
lists the available recipes:

```bash
just fast                # contributor checks without VM or ISO builds
just e2e                 # complete automated local gate
just e2e-devcontainer    # rebuild and smoke-test the locked Dev Container
just security            # Semgrep source scan + OSV dependency scan
just e2e-installer       # install into one isolated UEFI/QCOW2 VM
just e2e-run-vm          # boot that installed VM again
```

The installer VM never receives a host block device. Its persistent test disk
and UEFI variables live below `/var/tmp/tentaflake-e2e-<user>/`.
See [build boundaries](docs/17-builds.md) for fast checks, GitHub check selection
and the Nix/Bazel assessment.

## Define agents

Copy the example and keep it generic in this repository:

```bash
cp my-agents.nix.example my-agents.nix
```

`my-agents.nix` may use `mkAgent` or the compatible runtime-specific builders:

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
The versioned generic `agents` array accepts the same adapter arguments.
See [agent adapters](docs/agent-adapters.md) for the contract, complete secure
consumer example and evidence limits. OpenClaw v2026.9.7 is a stopped scaffold;
it creates no runtime container and is not operationally supported yet.
`agents.json` is declarative input; the removed interactive wizard no longer
edits it.

Installed systems default to `tentaflake.security.profile = "balanced"`.
Balanced agents are fail-closed capsules: no host network, published ports,
direct egress, real provider credentials, or mutable images. Without a
per-agent broker declaration they remain at `network=none`. With one, they
join exactly one internal network and can reach only their configured host
broker endpoints. Research agents use only the LLM broker on that network;
web access goes through the separate Unix-socket relay. `autoStart = false`
keeps the controller stopped at boot; activating the host can still create
declared workers and workspace filesystems.
Setting `autoStart = true` under `balanced` is accepted only after the exact
container also has an enabled LLM broker, disposable worker, fixed-size
workspace quota, and research relay. Web access uses the pinned public
[tentaflake-research](https://github.com/timfewi/tentaflake-research) service;
native web tools and the legacy fetch broker are disabled for research agents.
See [configuration and verification](docs/16-research.md). This prevents an
incomplete 24/7 declaration from silently starting with missing policy boundaries.

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
tentaflake help
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

`help`, `--help`, and `-h` work before the host configuration is installed.
Create stopped definitions with `tentaflake agent template <preset> <name>`.
On installed hosts, `agent validate|plan|import <file>` uses the shared Nix
parser and adds entries without activation. See
[onboarding](docs/08-agent-cli.md#add-an-agent-on-an-installed-host).

`health` and `doctor` return non-zero for failed units, unknown agent states,
or root-disk usage of at least 90%; unavailable host queries are errors.
Stopped agents remain valid. `--hide` redacts host and agent names in both
text and JSON reports.

The `rebuild` and `update` subcommands are explicit runtime operations. Source
evaluation or a successful build does not authorize activation.

See [the CLI guide](docs/06-shell.md) and
[agent configuration guide](docs/08-agent-cli.md).
Management-plane policy is covered by the
[private management and Tailscale policy guide](docs/11-tailscale-management.md).

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
Balanced also requires configured private management and SSH authorization through
`tentaflake.management`. Defaults retain the currently reviewed `tailscale`
transport, `ssh.policy = "tailnet-policy"`, and `tag:agent-host`; public OpenSSH
remains forbidden. `tentaflake.tailscale.enable` aliases `management.enable` for
existing consumers. Version-2 security manifests distinguish this configured
contract from unknown legacy evidence. Enrollment, effective tailnet grants and
operator access still require separate evidence; remote policy remains an
operator-owned control installed separately. See the [contract and
migration](docs/11-tailscale-management.md#host-contract-and-migration).

Phase B adds per-agent LLM credential and SSRF-safe fetch brokers, model/host
allowlists, request and daily budgets, prompt-free JSONL audit, DNS pinning,
redirect revalidation, quarantine, and host/FORWARD firewall rules. Agents
without that explicit declaration stay at `network=none`. Phase C adds an
opt-in disposable worker with bounded FD-safe snapshots, gVisor, no network or
secrets, runtime/resource/tmpfs limits, host-side action approval, cleanup, and
a read-only result path. An opt-in fixed-size Btrfs volume places a hard ceiling
on each persistent controller workspace and, when configured, its private state.
Other host state and aggregate sparse-image allocation still need capacity
monitoring. The broker marks web material as untrusted;
it does not claim that prompt injection is solved. Digest pinning is mandatory
in secure profiles. Optional per-agent Cosign policies add a fail-closed
publisher-signature gate before controller start; they do not prove that signed
software is harmless.

## Architecture

```text
flake.nix
├── modules/             core NixOS modules, brokers, worker
├── modules/profiles/    observability, Falco
├── adapters/            runtime implementations and schema-v1 contracts
├── lib/                 common builder, compatible wrappers, helpers
├── crates/              Rust workspace
├── pkgs/                Nix package wrappers
├── installer/           installer ISO
└── tests/               evaluation and VM tests
```

The default module imports only:

- host options, boot, hardening, locale, networking, broker/worker and image
  provenance policy, agent-instance inventory, research integration, Nix settings;
- base packages, users, the private management contract, SSH, Tailscale,
  and operator shell.

Optional profiles must be imported explicitly from the flake output.

## Brokered egress

Configure brokers by exact OCI container name. Provider credentials remain
runtime-only host files and are loaded only into the LLM broker. The agent
receives a random per-boot virtual key; it never receives the provider key.

Chat Completions and Responses support bounded SSE when the exact broker's
`llm.streaming.enable = true`. It defaults to false and preserves conservative
budget accounting. See [streaming limits and failure behavior](docs/12-brokered-egress.md#streaming-model-responses).
Both routes preserve optional, bounded `x-opencode-session` conversation metadata
for JSON and SSE. The client supplies the value; see the
[header contract](docs/12-brokered-egress.md#declaration).

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

  fetch.enable = false;
};
```

Every enabled agent needs a unique `/30`. This is the LLM boundary only; declare
the [research relay](docs/16-research.md) separately. Legacy fetch cannot be
enabled for a research agent. See
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
};
```

Jobs enter through `.tentaflake-worker/inbox/<id>.json`. Only
`local-reversible` runs automatically. External, financial, productive,
communicative, irreversible, or privileged classes remain pending until a
host operator approves the exact privately captured job; `forbidden` never
runs. Approval never grants network, secrets, host mounts, capabilities, or a
runtime socket. Results appear read-only below
`/run/tentaflake-worker/results/<id>/` in the controller. The queue shares one
operator/worker lock, bounds aggregate entries and bytes, and retires interrupted
claims without automatic dispatch replay. See
[the disposable-worker guide](docs/13-disposable-worker.md).
Tentaflake creates the matching host group automatically; deployments that
already own a custom container GID can select its existing group with
`hostGroup`.

The secure example also declares `tentaflake.workspaceQuota.agents` for each
controller. First activation creates and formats a sparse fixed-size Btrfs image
for an empty workspace. Existing data is never hidden or migrated implicitly.
Read [the workspace quota guide](docs/14-workspace-quota.md) before enabling it
on an existing host.
The same declaration accepts `state = { path = "/var/lib/hermes-coding"; sizeMiB = 1024; };`
to bound private state separately. Its path/owner must match adapter metadata;
existing state requires an explicit offline migration.

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

## Optional profiles

These modules are exported but not imported by the core:

| Output | Source |
|---|---|
| `nixosModules.observability` | `modules/profiles/observability.nix` |
| `nixosModules.falco` | `modules/profiles/falco.nix` |

Editor, Hive Research, and Piper TTS modules and voice assets have been removed.
Remove their imports, options, and package references when updating a consumer;
deployment-specific integrations belong in the consumer flake.

See the [archive comparison](docs/archive-comparison.md) for selected backports,
preserved current fixes, and the next useful archive changes.

## Build and test

```bash
just fast
```

See [contributor checks](CONTRIBUTING.md) for focused recipes. `just e2e`
includes the full flake check, both VM suites, and the installer ISO. Run these
larger gates deliberately. Keep evaluation, build, activation, and live-runtime
proof separate.

## Documentation

The public repository is the source of truth. The [website](https://tentaflake.dev/)
and [documentation site](https://docs.tentaflake.dev/) present a pinned release;
their source lines identify the exact commit. Unreleased checkout changes can
therefore be newer than the website.

| Topic | Guide |
|---|---|
| Product direction and planned milestones | [Roadmap](docs/roadmap.md) (planned work, not current support) |
| Installation and first host | [Install](docs/00-install.md), [quickstart](docs/01-quickstart.md) |
| Agent definitions and runtimes | [Declarative configuration](docs/08-agent-cli.md), [adapters](docs/agent-adapters.md) |
| Day-to-day management | [Agent management](docs/02-agent-tips.md), [operator CLI](docs/06-shell.md), [recovery](docs/07-operations.md) |
| Skills and secrets | [Skill index](docs/03-skill-index.md), [Agenix](docs/04-agenix-secrets.md) |
| Forks and builds | [Fork checklist](docs/05-fork-checklist.md), [build boundaries](docs/17-builds.md) |
| Security and management access | [Profiles](docs/10-security-profiles.md), [Tailscale](docs/11-tailscale-management.md), [threat model](docs/15-threat-model.md) |
| Model, research and execution | [Broker](docs/12-brokered-egress.md), [research](docs/16-research.md), [worker](docs/13-disposable-worker.md) |
| Storage and monitoring | [Workspace quota](docs/14-workspace-quota.md), [observability](docs/09-observability.md) |
| Documentation maintenance | [Source ownership and release synchronization](docs/18-documentation.md) |

[Archive comparison](docs/archive-comparison.md) records a historical review;
use the current guides for implementation and support status.

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
The [artwork overview](public/README.md) links the logo, illustrations, and
wallpaper downloads.
