---
name: tentaflake-repo-guidance
description: Locate owning sources, guides and checks before changing Tentaflake.
version: 3.0.0
---

# Repository guidance

Read `AGENTS.md` and `CONTRIBUTING.md`, then inspect only the relevant owners:

| Area | Source | Guide |
|---|---|---|
| Runtime definition | `adapters/<id>.nix`, `lib/mkAgent.nix`, `lib/runtimeContract.nix` | [Adapters](../../../docs/agent-adapters.md) |
| Onboarding/inventory | `lib/agentsFromData.nix`, `lib/agentPlan.nix`, CLI | [Configuration](../../../docs/08-agent-cli.md) |
| Containment | `lib/containerSecurity.nix`, `modules/security.nix` | [Profiles](../../../docs/10-security-profiles.md) |
| Host services | `modules/{broker,research,worker,workspace-quota,backup}.nix` | [Operations](../../../docs/07-operations.md) and linked policy guides |
| Management | `modules/management.nix`, `modules/tailscale.nix` | [Management](../../../docs/11-tailscale-management.md) |
| CLI/API | `crates/tentaflake-cli/`, `host-api/` | [CLI](../../../docs/06-shell.md), [API](../../../docs/19-web-ui.md) |
| Installer/builds | `installer/disk.sh`, `lib/mkRustPackage.nix`, `scripts/eval_flake.py` | [Install](../../../docs/00-install.md), [Builds](../../../docs/17-builds.md) |

Use only `mkAgent`; keep native behavior inside the selected adapter and common
host policy outside it. Adapter `preset` facts are the editable source;
`python3 scripts/runtime-catalog-docs.py` regenerates the JSON/docs projections.
Names are globally unique and host IDs are `agent-<name>`. Use metadata for
paths/owners; never move existing data or reset budgets implicitly.

Preserve balanced startup gates, host-held secrets, exact read-only Research
mounts, offline workers and quota/backup ownership. No native tool mediation or
vendor acceptance follows from a declaration or synthetic fixture. Scaffolds
stay non-runnable without OCI/state/credentials. Generic custom OCI grants no
accepted model/Research integration. For authority changes read the
[threat model](../../../docs/15-threat-model.md).

Choose checks through the build guide: `just fast` includes pure catalog drift
checks; host evaluation/build/runtime evidence remains separate. Update owning
guides and examples with behavior, keep skills short, and report actual checks
and remaining evidence gaps. Evaluation does not authorize activation.
