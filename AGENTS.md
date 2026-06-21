# Agent Instructions

## HARD RULE — Template Only

This repo is a **generic template** — it stays clean of domain-specific code.

**NEVER** commit domain-specific, company-specific, or project-specific code. This includes:
- Company names, brands, or product names
- Real hardware configurations (LUKS, specific disk layouts, real hostnames)
- Real API keys, secrets, or env files
- Agent definitions for specific companies or projects
- Custom agent SOUL.md, AGENTS.md, or skill files written for a specific business
- Any file referencing a real organization, person, or deployment

All such content belongs in a **fork** or **a private project repo**. This template must stay generic.

## Package Manager
Use **nix**: `nix flake check`, `nix build .#installer-iso`

## Commit Attribution
AI commits MUST include:
```
Co-Authored-By: (the agent model's name and attribution byline)
```

## File-Scoped Commands
| Task | Command |
|------|---------|
| Check flake | `nix flake check` |
| Build ISO | `nix build .#installer-iso` |
| Format Nix | `nix fmt` |
| Update lock | `nix flake update` |

## Key Conventions
- `lib/constants.nix` holds template-level constants (stateVersion, locale defaults)
- `lib/mkHermesAgent.nix` creates isolated Docker-based Hermes agents
- `configuration.nix` imports modules and agent definitions
- User-specific config goes in `my-agents.nix` (gitignored)
- Encrypted secrets via agenix go in `secrets/` (.age files tracked, keys never)
- All modules are in `modules/`, reusable across hosts
- `installer/` builds a bootable ISO with TUI installer
