---
name: tentaflake-repo-guidance
description: Locate the owning source, guide and checks before changing Tentaflake modules, adapters, CLI or installer.
version: 2.6.1
---

# Tentaflake repository guidance

Read `AGENTS.md` and `CONTRIBUTING.md`, then use this map to inspect the
relevant source and guide. Avoid loading unrelated references.

| Task | Owning source | Guide |
|---|---|---|
| Host options and exports | `modules/options.nix`, `modules/default.nix`, `flake.nix` | [Quickstart](../../../docs/01-quickstart.md) |
| Builders and contracts | `adapters/catalog.json`, `adapters/`, `lib/mkAgent.nix`, `lib/runtimeContract.nix` | [Adapters](../../../docs/agent-adapters.md) |
| Declarative input, onboarding and inventory | `configuration.nix`, `lib/agentsFromData.nix`, `lib/agentPlan.nix`, CLI onboarding | [Configuration](../../../docs/08-agent-cli.md) |
| CLI and diagnostics | `crates/tentaflake-cli/`, `modules/shell.nix`, `modules/security.nix` | [CLI](../../../docs/06-shell.md) |
| LLM broker and streaming | `modules/broker.nix`, `crates/tentaflake-broker/` | [Broker](../../../docs/12-brokered-egress.md) |
| Research projection | `modules/research.nix`, `lib/researchClient.nix`, adapter hooks | [Research](../../../docs/16-research.md) |
| Offline execution | `modules/worker.nix`, `crates/tentaflake-worker/` | [Worker](../../../docs/13-disposable-worker.md) |
| Workspace/state limits | `modules/workspace-quota.nix`, adapter metadata, `modules/backup.nix` | [Quota](../../../docs/14-workspace-quota.md) |
| Containment and provenance | `lib/containerSecurity.nix`, `lib/pinnedImage.nix`, `modules/image-provenance.nix` | [Security](../../../docs/10-security-profiles.md) |
| Recovery and backups | `lib/serviceRecovery.nix`, `modules/backup.nix` | [Operations](../../../docs/07-operations.md) |
| Metrics and detection | `modules/profiles/` | [Observability](../../../docs/09-observability.md) |
| Rust packaging and identities | `lib/mkRustPackage.nix`, `lib/constants.nix`, `pkgs/` | [Builds](../../../docs/17-builds.md) |
| Installation | `installer/installer.sh`, `installer/disk.sh`, `scripts/generated-flake-test.sh` | [Install](../../../docs/00-install.md) |

## Preserve these boundaries

- Keep the template generic. Deployment identities, private context and secrets
  belong in forks. Settings, JSON and seed paths are Nix-store inputs.
- Generate support facts with `python3 scripts/runtime-catalog-docs.py`;
  catalog declarations and source fixtures never prove vendor acceptance.
  Generic definitions reject extra authority and preserve current startup gates.
  Onboarding imports stopped additions, preserves existing data on failure and
  uses ordinary flake source filtering. Never publish or activate from an import.
- Preserve Hermes/ZeroClaw APIs and state. OpenClaw is a stopped refusal scaffold
  without an OCI artifact or operational acceptance. The
  [roadmap](../../../docs/roadmap.md) describes plans, not current support.
- Prioritize the planned common isolated runtime and installed-host onboarding
  before employee/company features. Keep native agent hooks separate from common
  containment; adding a preset must not create another security implementation.
- Balanced effective `autoStart = true` requires the exact LLM broker, worker,
  quota and Research relay. Incomplete stopped capsules may remain offline.
  `strict` fails closed; `dev` is a compatibility profile.
- Model calls use the LLM broker; web uses only `secure-research-tool`.
  Preserve host-held keys, exact read-only socket/closure mounts, no remote MCP
  or provider-hosted tools, and no dispatched-operation replay.
- A worker declaration does not prove vendor execution mediation. Synthetic
  fixtures do not establish vendor startup or tool discovery.
- Worker queue changes preserve one operator/drain lock and durable running
  claims before OCI dispatch. Never retry abandoned claims; verify real binary
  crash/write-failure and overload cases. Sync directories through readable
  descriptors; guarded `O_PATH` handles cannot themselves be fsynced.
  Migration and limits belong in the [worker guide](../../../docs/13-disposable-worker.md).
- New installs/quota images use Btrfs; quota images require at least 128 MiB.
  Never convert ext4 automatically. Installer cleanup stays inside the selected
  disk stack and refuses cross-disk VGs.
  Optional state quotas match adapter `stateStorage`/ownership; initialize
  directories after mounting and keep seeds/healing behind ownership units.
  Check source/parent symlinks before privileged permission changes. Include
  selected state/workspace mounts separately in Restic and verify both restores.
  `ReadWritePaths` creates bind mounts: identify the exact loop backing image,
  never infer quota readiness from a mount point or filesystem type alone.
- Keep exact runtime resource/mount/network expectations and unknown evidence.
  Reject bind sources below writable mounts; generic source parents remain
  unmounted, root-owned and private, with startup checks for symlinks.
  Read the [threat model](../../../docs/15-threat-model.md) for authority changes.
- Editor, Hive Research and Piper integrations belong in consumer flakes.

## Check and synchronize

Start with `just fast`; `just list` lists recipes. Recipes load pinned tools;
direct Cargo/lint commands need `nix develop`. Use focused checks from
`tests/` and the build guide. Identify affected checks via `.github/ci-paths.json`;
docs and CI metadata skip VM/Research builds; unknown paths retain the full gate.
Automatic GitHub VM runs are temporarily paused; see the build guide for manual
opt-in and restoration. Report skipped runtime evidence explicitly.
Heavy VM, system and ISO builds require explicit workload authorization.
Source checks never authorize activation.

Review the final diff, synchronize affected guides/examples/instructions/skills,
and report actual checks and evidence gaps. The public repository owns docs;
the website mirrors a pinned release. See
[documentation ownership](../../../docs/18-documentation.md).
