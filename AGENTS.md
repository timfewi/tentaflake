# Agent Instructions — tentaflake

NixOS flake template for running isolated AI agents (Hermes, ZeroClaw) in Docker containers on a single machine.

## Build & Test

```bash
nix flake check              # validate flake and build toplevel
nix build .#installer-iso    # build installer ISO
nix build .#live-agent-iso   # build live agent ISO
cd pkgs/hermes-auditd && go vet ./... && go test ./...
```

## Conventions

- Nix: `nix fmt` (nixfmt), 2-space indent
- Go: `gofmt`, tabs, run `golangci-lint run` before push
- Commits: Conventional Commits (`feat:`, `fix:`, `docs:`)

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

- `modules/` — reusable NixOS modules (generic, composable via `tentaflake.*.enable` options)
- `lib/` — helpers (`mkHermesAgent`, `mkZeroClawAgent`, `constants`)
- `pkgs/` — standalone packages (`hermes-auditd`)
- `installer/` — ISO installer and firstboot scripts
- `examples/` — consumer-flake reference
- `docs/` — user-facing documentation
- `.agents/skills/` — bundled Hermes skills (also development agent instructions for this repo)

## PR Description

Fill out the full template at `.github/PULL_REQUEST_TEMPLATE.md` — a title alone
is not enough. Write a real `## Description` (what changed and why), check
the applicable `Type of Change` box, and tick every checklist item that
applies (leave inapplicable ones unchecked, don't delete them).

See CONTRIBUTING.md for PR process.
