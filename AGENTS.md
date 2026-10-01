# Agent Instructions — tentaflake

NixOS flake template for running isolated AI agents (Hermes and ZeroClaw) in Docker containers on a single machine.

## Build & Test

`just` recipes load the pinned contributor tools automatically; `just list`
lists them. Enter `nix develop` before running Cargo or lint tools directly.
Start with `just fast`; use affected VM suites for runtime/security changes.
GitHub VM selection imports `.github/vm-paths.json`; unknown paths require both
runtime and research suites. Keep its real-Git regression check passing.

```bash
nix flake check
nix build .#installer-iso
nix build \
  .#checks.x86_64-linux.vm-integration \
  -L
nix build --no-link .#checks.x86_64-linux.research-policy
nix build --no-link -L .#checks.x86_64-linux.research-integration
cargo fmt --all -- --check
cargo clippy --workspace \
  --all-targets -- -D warnings
cargo test --workspace
just fast
just e2e
just e2e-devcontainer
just e2e-installer
just e2e-run-vm
just security
```

New host installs and managed workspace images use Btrfs. Workspace images
require at least 128 MiB; never automatically reformat or convert existing ext4
images. Follow `docs/14-workspace-quota.md` for explicit migration.

## Conventions

- Nix: `nix fmt` (nixfmt), 2-space indent
- Rust: `cargo fmt`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`
- Commits: Conventional Commits (`feat:`, `fix:`, `docs:`)
- DCO: every non-merge PR commit needs `Signed-off-by:`; use `git commit -s`

## Template Rule

This repo is a GENERIC template. NEVER commit domain-specific code (company config, real hostnames,
hardware configs, API keys, secrets, agent SOUL.md/skills written for specific deployments).
Domain-specific work belongs in FORKS, not here.

## Keep Docs In Sync

After any change that alters behavior, options, or usage, verify the docs are
still accurate before finishing — and update them in the same change:

- `README.md` and `docs/` — user-facing docs
- this `AGENTS.md` / `CLAUDE.md` — agent instructions
- relevant `.agents/skills/` — bundled skill docs

## Module Boundaries

- `modules/` — reusable NixOS modules, including security, brokered egress,
  image-provenance gates, disposable workers, workspace quotas, encrypted
  backup, and generic options
- `lib/` — helpers (`mkHermesAgent`, `mkZeroClawAgent`, `agentsFromData`, `pinnedImage`, `constants`, `devshell`)
- `crates/` and `pkgs/` — Rust CLI/broker/worker workspace and Nix packages
- CLI help is configuration-free; management commands require generated
  host configuration and inventory.
- Host diagnostics share checked systemd/disk evidence; failed or unknown
  agent states are problems, while stopped agents remain valid. `--hide`
  must redact host and agent names in both text and JSON.
- `lib/mkRustPackage.nix` shares Rust packaging: root Cargo files, all member
  manifests, and only the selected crate's source. Add required root inputs
  explicitly. Versions come from the root Cargo workspace; descriptions from
  crate manifests. Derive worker image references from the image metadata.
  Shared container identities live in `lib/constants.nix`.
- A balanced agent with `autoStart = true` requires its exact LLM broker, worker,
  workspace-quota, and research relay declarations; stopped scaffolds remain `network=none`.
- Brokers retry with `RestartMode=direct` so temporary failures preserve the
  controller process. Explicit broker stops must still stop `Requires` controllers.
  Direct retries skip systemd failure hooks; verify health/restart evidence in a VM.
- Web/research uses only the pinned `tentaflake-research` stdio MCP server
  `secure-research-tool`; model calls use the LLM broker. Do not add a second
  web transport, re-enable legacy fetch, expose other agent sockets, or enable
  provider-hosted network tools. See `docs/16-research.md` and its evidence limits.
  Research client recovery must never replay dispatched operations or reopen
  after explicit close; every new session repeats negotiation and UID checks.
- Secure controllers and brokers share `lib/serviceRecovery.nix`: bounded
  restart backoff without permanent start-limit exhaustion. Explicit stops
  must still prevent automatic retries.
- `tests/` — NixOS VM test backing `checks.<system>.vm-integration`
- Restic backups retain filesystem boundaries; selected managed quota mounts
  are separate sources and required mounts. Verify workspace restore as well
  as ordinary state when changing backup or quota behavior.
- `installer/` — installer ISO and disk-install scripts
- `examples/` — consumer-flake reference
- `docs/` — user-facing documentation
- `.agents/skills/` — bundled Hermes skills (also development agent instructions for this repo)

## PR Description

Fill out the full template at `.github/PULL_REQUEST_TEMPLATE.md` — a title alone
is not enough. Write a real `## Description` (what changed and why), check
the applicable `Type of Change` box, and tick every checklist item that
applies (leave inapplicable ones unchecked, don't delete them).

See CONTRIBUTING.md for PR process.
