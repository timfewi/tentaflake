# Fork Checklist — What to Change

Find and replace these before deploying your fork:

| What | Find | Replace with |
|---|---|---|
| GitHub handle | `timfewi` | your handle |
| Repo name / Go module | `tentaflake` | your repo name |
| Hostname | `tentaflake` | your hostname |
| Admin user | `user` | your username |
| Timezone | `UTC` | your tz |
| Badges (README) | `timfewi/tentaflake` | your repo |
| Tailscale auth key | (set at runtime) | your key |
| Agenix recipients | (`secrets.nix`) | your SSH pubkeys |

A global `tentaflake` → `<your-repo>` replace also renames the Go module:
`pkgs/tentaflake-auditd/go.mod` declares `module tentaflake/tentaflake-auditd` and every
import uses that prefix, so they stay consistent. (Equivalently, run
`cd pkgs/tentaflake-auditd && go mod edit -module <new>/tentaflake-auditd` and update the
`import` lines.)

## nixpkgs channel

The template tracks `nixos-unstable`, pinned to an exact revision by the committed
`flake.lock` — so builds are reproducible; bump deliberately with `nix flake update`.
Unstable is required: it is currently the only channel that provides BOTH a
non-vulnerable `docker` (29.x) and Go ≥ 1.25 (needed by `tentaflake-auditd`'s
`modernc.org/sqlite`). The current stable (25.11) ships `docker` 28.5.2, which
nixpkgs flags as insecure, and only Go 1.24.

## Files to customize in your fork

- `lib/constants.nix` — default hostname, admin user, locale, `stateVersion`
- `flake.nix` — the `nixosConfigurations.<your-host>` block
- `configuration.nix` — agent wiring (`myAgents`) and container backend

## Files to create in your fork

- `my-agents.nix` — define your agents with `mkHermesAgent` and/or `mkZeroClawAgent`
  (see `my-agents.nix.example`)
- `secrets/hermes-<name>.env` / `secrets/zeroclaw-<name>.env` — API keys for each agent

## Don't forget

- Set `tentaflake.adminAuthorizedKeys = [ "ssh-ed25519 ..." ]` (or a
  `hashedPassword`) — a fresh `tentaflake` has no console password; remote access
  is via Tailscale SSH (`services.tailscale` with `--ssh`).
- `tentaflake.containerBackend` defaults to `"docker"`; set `"podman"` for a
  rootless, daemonless setup.
