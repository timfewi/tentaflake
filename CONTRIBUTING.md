# Contributing to tentaflake

Thanks for contributing! This is a **generic template** — keep it that way.

## Template Rule

**NEVER** commit domain-specific, company-specific, or project-specific code. This includes:
- Company names, brands, or product names
- Real hardware configurations (LUKS, specific disk layouts, real hostnames)
- Real API keys, secrets, or env files
- Agent definitions for specific companies or projects
- Custom agent SOUL.md, AGENTS.md, or skill files written for a specific business
- Any file referencing a real organization, person, or deployment

All such content belongs in a **fork** or a private project repo.

## Development Setup

```bash
# Clone
git clone https://github.com/timfewi/tentaflake
cd tentaflake
```

All the commands below are wrapped as `just` recipes (`just` is in the dev
shell). Run `just` to list them; `just ci` runs the full local gate — every CI
step **plus the two ISO builds and the statix/deadnix/golangci-lint linters CI
does not do**. The raw commands stay the source of truth and are documented below.

### Nix/NixOS (flake checks, ISO builds)

```bash
# Validate the flake
nix flake check

# Format Nix files
nix fmt

# Build the installer ISO
nix build .#installer-iso

# Build the live agent ISO (Hermes + Piper TTS)
nix build .#live-agent-iso
```

### Go (hermes-auditd)

```bash
cd pkgs/hermes-auditd

# Run tests
go test ./...

# Static analysis
go vet ./...

# All checks
go vet ./... && go test ./...
```

## Conventions

| Area | Convention |
|------|-----------|
| Commits | [Conventional Commits](https://www.conventionalcommits.org/) (`feat:`, `fix:`, `docs:`, `chore:`) |
| AI commits | Must include `Co-Authored-By: model-name (via OpenCode)` |
| Nix formatting | `nix fmt` (nixfmt) |
| Go formatting | Standard `gofmt` |
| Module boundary | Keep template generic — fork for specifics |

## Pull Request Process

1. Fork the repo and create a feature branch:
   ```bash
   git checkout -b feat/my-feature
   ```

2. Make your changes and verify:
   ```bash
   nix flake check
   cd pkgs/hermes-auditd && go vet ./... && go test ./...
   ```

3. Commit using conventional commits:
   ```bash
   git commit -m "feat: add thing"
   ```

4. Push and open a PR against `main`.

5. Ensure CI passes (flake check + Go tests).

6. A maintainer will review. Keep PRs focused — one concern per PR.

## Adding a Module

When adding a new NixOS module under `modules/`:

1. Create `modules/my-feature.nix`
2. Import it in `modules/default.nix`
3. Add it to the module table in `README.md`
4. Document it in `docs/` if it's user-facing

## Adding a Hermes Skill

Skills live under `.agents/skills/<name>/SKILL.md`. Follow the [skill authoring guide](docs/03-skill-index.md) for format.

## Questions?

Open a [discussion](https://github.com/timfewi/tentaflake/discussions) or an issue with the `question` label.
