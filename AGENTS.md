# Agent Instructions — tentaflake

NixOS flake template for running Hermes AI agents in isolated Docker containers on a single machine.

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

## Module Boundaries

- `modules/` — reusable NixOS modules (generic, composable via `tentaflake.*.enable` options)
- `lib/` — helpers (`mkHermesAgent`, `constants`)
- `pkgs/` — standalone packages (`hermes-auditd`)
- `installer/` — ISO installer and firstboot scripts
- `examples/` — consumer-flake reference
- `docs/` — user-facing documentation
- `.agents/skills/` — bundled Hermes skills (also development agent instructions for this repo)

See CONTRIBUTING.md for PR process.
