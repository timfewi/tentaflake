# Agent Instructions — tentaflake

NixOS flake template for running isolated AI agents (Hermes and ZeroClaw) in Docker containers on a single machine.

## Build & Test

```bash
nix flake check
nix build .#installer-iso
nix build \
  .#checks.x86_64-linux.vm-integration \
  -L
cargo fmt --all -- --check
cargo clippy --workspace \
  --all-targets -- -D warnings
cargo test --workspace
just e2e
just e2e-devcontainer
just e2e-installer
just e2e-run-vm
just security
```

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
- A balanced agent with `autoStart = true` requires its exact broker, worker,
  and workspace-quota declarations; stopped scaffolds remain `network=none`.
- `tests/` — NixOS VM test backing `checks.<system>.vm-integration`
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
