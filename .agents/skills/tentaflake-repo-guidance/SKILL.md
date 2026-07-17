---
name: tentaflake-repo-guidance
description: Comprehensive reference for the tentaflake repository — all modules, options, lib functions, build targets, agent config, ISOs, and architecture. Use when working with the template itself, adding features, configuring hosts, or debugging the build.
version: 1.0.0
---

# Tentaflake — Repo Guidance

## Overview

Tentaflake is a **generic NixOS flake template** for running isolated AI agent containers — Hermes and ZeroClaw — on a single headless machine. It is NOT domain-specific — no company config, real hostnames, API keys, or secrets belong here. Domain-specific work goes in forks.

## Repo Layout

```
tentaflake/
├── flake.nix                     # Flake entry: 3 nixosConfigurations, exports
├── flake.lock
├── configuration.nix             # Shared config: OCI backend, docker, admin groups
├── hardware-configuration.nix    # Minimal fallback HW config
├── my-agents.nix.example         # Agent definitions (Hermes + ZeroClaw) reference
├── hermes.env.example            # Hermes agent env file reference
├── zeroclaw.env.example          # ZeroClaw agent env file reference
├── AGENTS.md                     # Agent instructions for this repo
│
├── modules/                      # Reusable NixOS modules (tentaflake.*)
│   ├── default.nix               # Module import hub
│   ├── options.nix               # ★ Most tentaflake.* options (core + shell + toggles)
│   ├── boot.nix                  # systemd-boot EFI bootloader
│   ├── hardening.nix             # Kernel sysctl, sudo rules, AppArmor, journald
│   ├── locale.nix                # Timezone, locale, console keymap
│   ├── networking.nix            # Hostname, NetworkManager, nftables firewall
│   ├── nix-settings.nix          # Nix daemon: flakes, GC, optimisation, trusted users
│   ├── packages.nix              # System packages (curl, git)
│   ├── users.nix                 # Admin user creation (wheel + networkmanager groups)
│   ├── tailscale.nix             # Tailscale VPN with extraUpFlags
│   ├── shell.nix                 # ★ Interactive shell: tentaflake CLI, banner, zsh, tools
│   ├── hermes-auditd.nix         # ★ Filesystem audit daemon + hermes-top TUI (watches all runtimes)
│   ├── piper-tts-server.nix      # ★ Piper TTS HTTP server (OpenAI-compatible)
│   └── editor.nix                # Neovim (nvf) module (separate import)
│
├── lib/
│   ├── default.nix               # Exports: mkHermesAgent, mkZeroClawAgent, constants
│   ├── constants.nix             # Template-wide defaults (hostName, adminUser, stateVersion...)
│   ├── mkHermesAgent.nix         # ★ Builds one isolated Hermes agent as a NixOS module
│   └── mkZeroClawAgent.nix       # ★ Builds one isolated ZeroClaw agent as a NixOS module
│
├── pkgs/
│   ├── hermes-auditd/            # Go daemon: filesystem watcher + SQLite + HTTP/WS
│   │   ├── cmd/hermes-auditd/    # Daemon binary
│   │   ├── cmd/hermes-top/       # TUI dashboard
│   │   └── internal/             # config, watcher, store, hermes event types
│   └── piper-voices/             # Bundled Piper voice ONNX models
│
├── installer/
│   ├── iso.nix                   # Installer ISO config (TUI wizard → full install)
│   ├── live-iso.nix              # Live Agent ISO config (RAM, no disk touch)
│   ├── live-profile.nix          # Live ISO profile: agents, Piper, Tailscale, SSH
│   ├── live-agents.nix           # Pre-configured agents for live ISO (default + research)
│   ├── hermes-firstboot.nix      # USB env detection, data persistence, TTY1 wizard
│   ├── installer.sh              # ★ Interactive TUI installer (partitions, installs, reboots)
│   └── firstboot.sh              # Live ISO first-boot wizard (env file entry)
│
├── examples/
│   └── consumer-flake.nix        # ★ RECOMMENDED: consumer pattern using tentaflake as input
│
├── docs/                         # User-facing documentation
│   ├── 00-install.md             # ISO build + install instructions
│   ├── 01-quickstart.md          # Post-boot 10-minute checklist
│   ├── 02-agent-tips.md          # Agent operation tips
│   ├── 03-skill-index.md         # Bundled Hermes skills index
│   ├── 04-agenix-secrets.md      # Agenix secrets guide
│   ├── 05-fork-checklist.md      # What to change when forking
│   └── 06-shell.md               # Shell/operator experience
│
├── scripts/
│   └── build-iso.sh              # Convenience script for ISO builds
│
├── .agents/skills/                # Bundled skills (development agent + Hermes container)
│   ├── handle-the-host/           # Remote host operations (Tailscale SSH, rebuild)
│   ├── hermes-config-manager/     # Hermes config management
│   ├── hermes-memory-personality/ # Memory & personality management
│   ├── hermes-provider-setup/     # LLM provider configuration
│   ├── hermes-tools-config/       # Tool/toolset configuration
│   └── tentaflake-repo-guidance/  # This file — repo reference
│
├── .github/workflows/check.yml   # CI: nix flake check + Go tests
├── .golangci.yaml                # Go lint config
├── CONTRIBUTING.md               # PR process
└── SECURITY.md                   # Security policy
```

## NixOS Configurations (flake outputs)

Three machines are defined in `flake.nix`:

| Configuration | Profile | Purpose |
|---|---|---|
| `agent-host` | `installed` | Default target machine. Consumes `my-agents.nix` for custom agents. Built-in host in the template. |
| `installer-iso` | `installer` | Bootable ISO → TUI wizard → partitions disk → installs NixOS. |
| `live-agent` | `live` | Bootable ISO → runs agents in RAM + Piper TTS. Ephemeral. |

## All `tentaflake.*` Options

Most options are defined in `modules/options.nix`. Exceptions:
- `tentaflake.hermes-auditd.*` — defined in `modules/hermes-auditd.nix`
- `tentaflake.editor.nvf.*` — defined in `modules/editor.nix`

These are the knobs you turn in your host config.

### Core Identity

| Option | Type | Default | Description |
|---|---|---|---|
| `tentaflake.hostName` | `str` | `"agent-host"` | System hostname |
| `tentaflake.adminUser` | `str` | `"admin"` | Primary admin username |
| `tentaflake.adminDescription` | `str` | `"System Administrator"` | Description for the admin user |
| `tentaflake.adminShell` | `str` | (bash) | Shell for admin user |
| `tentaflake.adminAuthorizedKeys` | `list of str` | `[]` | SSH public keys for admin |
| `tentaflake.timeZone` | `str` | `"UTC"` | System timezone |
| `tentaflake.defaultLocale` | `str` | `"en_US.UTF-8"` | Default locale |
| `tentaflake.consoleKeyMap` | `str` | `"us"` | Console keymap |
| `tentaflake.stateVersion` | `str` | `"26.05"` | NixOS state version |
| `tentaflake.profile` | enum | `"installed"` | `installed` / `installer` / `live` |
| `tentaflake.allowUnfree` | `bool` | `false` | Allow unfree packages |
| `tentaflake.containerBackend` | enum | `"docker"` | `docker` or `podman` |
| `tentaflake.editor.nvf.enable` | bool | `false` | Neovim via nvf (LSP, treesitter, telescope) — requires `nvf` flake input |

### Module Enable/Disable Toggles

| Option | Default | What it enables |
|---|---|---|
| `tentaflake.boot.enable` | `true` | systemd-boot EFI bootloader |
| `tentaflake.hardening.enable` | `true` | Kernel sysctl hardening, AppArmor, sudo rules, journald rate limits |
| `tentaflake.locale.enable` | `true` | Timezone, locale, console keymap |
| `tentaflake.networking.enable` | `true` | Hostname, NetworkManager, nftables firewall |
| `tentaflake.nixSettings.enable` | `true` | Nix flakes, GC (weekly, 14d), optimisation, trusted-users |
| `tentaflake.packages.enable` | `true` | curl + git system packages |
| `tentaflake.users.enable` | `true` | Admin user creation (wheel + networkmanager) |
| `tentaflake.tailscale.enable` | `true` | Tailscale VPN with `--advertise-tags=tag:agent --ssh` |

### Shell Sub-options (`tentaflake.shell.*`)

| Option | Type | Default | Description |
|---|---|---|---|
| `tentaflake.shell.enable` | bool | `true` | Interactive shell improvements (tentaflake CLI, prompt, tools) |
| `tentaflake.shell.motd.enable` | bool | `true` | Dynamic login banner (`tentaflake-status`) on SSH/console |
| `tentaflake.shell.tools.enable` | bool | `true` | Modern CLI tools (eza, bat, fd, ripgrep, fzf, htop, btop, ...) |
| `tentaflake.shell.starship.enable` | bool | `true` | Starship prompt |
| `tentaflake.shell.zsh.enable` | bool | `false` | Zsh + Oh My Zsh + autosuggestions + syntax highlighting + fzf-tab |
| `tentaflake.shell.zoxide.enable` | bool | `true` | Smart dir jumping |
| `tentaflake.shell.lazygit.enable` | bool | `false` | lazygit terminal UI + `lg` alias |
| `tentaflake.shell.tmux.enable` | bool | `false` | tmux multiplexer with system config |
| `tentaflake.shell.tentaflakeCli.enable` | bool | `true` | `tentaflake` CLI for multi-runtime agent management (renamed from `hermesCli`; old name still accepted via `mkRenamedOptionModule`) |

### hermes-auditd Sub-options (`tentaflake.hermes-auditd.*`)

| Option | Type | Default | Description |
|---|---|---|---|
| `tentaflake.hermes-auditd.enable` | bool | — | Enable audit daemon |
| `tentaflake.hermes-auditd.watchDirs` | list of str | `[]` | Auto-discovers from every declarative agent container (any runtime) |
| `tentaflake.hermes-auditd.port` | port | `9090` | HTTP/WebSocket listen port |
| `tentaflake.hermes-auditd.dbPath` | str | `/var/lib/hermes-audit/events.db` | SQLite DB path |
| `tentaflake.hermes-auditd.retentionHours` | int | `24` | Event retention window |

### Piper TTS (`services.piper-tts-server.*`)

This is a `services.*` option (not `tentaflake.*`), defined in `modules/piper-tts-server.nix`.

| Option | Type | Default | Description |
|---|---|---|---|
| `services.piper-tts-server.enable` | bool | — | Enable Piper TTS HTTP server |
| `services.piper-tts-server.voiceName` | str | `"voice"` | Display name |
| `services.piper-tts-server.voiceModel` | path | **required** | Path to .onnx model file |
| `services.piper-tts-server.voiceConfig` | path | **required** | Path to .onnx.json config |
| `services.piper-tts-server.port` | port | `5001` | HTTP listen port |
| `services.piper-tts-server.host` | str | `127.0.0.1` | Bind address |

## Lib Functions

### `mkHermesAgent`

Defined in `lib/mkHermesAgent.nix`. Creates one Hermes agent as a NixOS module.

**Usage in `my-agents.nix`:**

```nix
{ mkHermesAgent }:
[
  (mkHermesAgent {
    name = "coding";
    envFile = "/run/secrets/hermes-coding.env";
  })
]
```

**All parameters:**

| Parameter | Type | Default | Description |
|---|---|---|---|
| `name` | str | **required** | Agent name (becomes `hermes-<name>` container + `hermes-<name>` system user) |
| `stateDir` | str | `/var/lib/hermes-${name}` | Agent state directory |
| `user` | str | `hermes-${name}` | System user name |
| `group` | str | `hermes-${name}` | System group name |
| `uid` | int? | auto | Explicit UID for system user |
| `gid` | int? | auto | Explicit GID for system group |
| `image` | str | `nousresearch/hermes-agent:latest` | OCI container image |
| `envFile` | path? | `null` | Path to plaintext .env file on host |
| `agenixFile` | path? | `null` | Path to agenix-decrypted .env file |
| `seedDir` | path? | `null` | Directory of base files (SOUL.md, skills/) for first-boot seed |
| `extraVolumes` | list of str | `[]` | Extra Docker volumes |
| `extraEnvironment` | attrset | `{}` | Extra env vars |
| `cmd` | list of str? | `null` | Container command (null → `gateway run --replace`) |
| `networkMode` | str | `"host"` | Docker network mode |
| `autoStart` | bool | `true` | Auto-start with system |
| `createUser` | bool | `true` | Create system user/group |
| `extraContainerConfig` | attrset | `{}` | Merged into OCI container config (use for `memory`, `cpus`, port maps) |
| `settings` | YAML attrset? | `null` | Hermes YAML/JSON config serialized to `config.yaml` |
| `containerUid` / `containerGid` | int | `10000` | UID/GID the container runs as; state owned by it (fixes `PermissionError`) |
| `healDataDirs` | list of str | `[]` | Extra data dirs to `chown` to the container uid each boot |
| `providerHealthcheck` | attrset? | `null` | Fail-loud boot preflight (`{ url; model; apiKeyEnv; }`) |
| `gitIdentity` | attrset? | `null` | Git identity inside the container (`{ name; email; }`), re-applied each boot |
| `gitAutoPush` | attrset? | `null` | Host-side secret-safe push (`{ tokenEnvFile; reposRoot?; tokenEnvVar?; interval?; }`) |
| `dashboard` | attrset? | `null` | Launch + optional tailnet serve (`{ port; tailnetPort?; }`) |
| `services` | attrset | `{}` | Durable agent-built web apps (`<name> = { startCommand; port?; tailnetPort?; }`) |

Operational-hardening args (`containerUid` … `services`) are all optional and
default-off. See `docs/07-operations.md` for the rationale and gotchas.

**What each agent gets:**

- System user `hermes-<name>` + group
- State dir `/var/lib/hermes-<name>` (0700, owned by the container uid, default 10000)
- OCI container `hermes-<name>` (host networking, auto-started via systemd)
- `HERMES_HOME` pointing to its isolated state dir
- Optional `config.yaml` from `settings` attrset (mounted read-only)
- Optional `envFile` or `agenixFile` for secrets

### `mkZeroClawAgent`

Defined in `lib/mkZeroClawAgent.nix`. Creates one ZeroClaw agent as a NixOS module — the second supported agent runtime, alongside Hermes.

**Usage in `my-agents.nix`:**

```nix
{ mkZeroClawAgent }:
[
  (mkZeroClawAgent {
    name = "assistant";
    agenixFile = "/run/agenix/zeroclaw-assistant-env";
    hostPort = 9246;
    servePort = 9145;
    settings = {
      schema_version = 3; # required — omitting it silently drops v3-only fields
      providers.models.openrouter.default.model = "anthropic/claude-haiku-4.5";
    };
  })
]
```

**All parameters:**

| Parameter | Type | Default | Description |
|---|---|---|---|
| `name` | str | **required** | Agent name (becomes `zeroclaw-<name>` container) |
| `agenixFile` | path | **required** | Path to an agenix-decrypted `.env` file, passed via `--env-file` (no plaintext `envFile` option) |
| `image` | str | `ghcr.io/zeroclaw-labs/zeroclaw:v0.8.2` | OCI container image |
| `stateDir` | str | `/var/lib/zeroclaw-${name}` | Agent state directory |
| `seedDir` | path? | `null` | Directory of base files copied into the workspace on first boot |
| `gatewayPort` | int | `42617` | In-container gateway port |
| `hostPort` | int | **required** | Host loopback port forwarded to the gateway |
| `servePort` | int | **required** | Tailnet HTTPS port (published via `tailscale serve`) |
| `autoStart` | bool | `true` | Auto-start with system |
| `settings` | TOML attrset | `{}` | ZeroClaw config serialized to `config.toml` |
| `extraEnvironment` | attrset | `{}` | Extra env vars |
| `extraVolumes` | list of str | `[]` | Extra Docker volumes |

**What each agent gets:**

- State dir `/var/lib/zeroclaw-<name>` (0700, owned by uid/gid 65534)
- OCI container `zeroclaw-<name>` with `config.toml` mounted read-only
- Secrets via `agenixFile` → `--env-file`; overrides use `ZEROCLAW_<section>__<sub>__<key>` env vars (double underscores between path segments)
- A `tailscale serve` unit publishing `hostPort` on the tailnet at `servePort`

### `constants`

Defined in `lib/constants.nix`. Single source of truth for template-wide defaults:

```nix
constants = {
  stateVersion     = "26.05";
  defaultLocale    = "en_US.UTF-8";
  consoleKeyMap    = "us";
  hostName         = "agent-host";
  adminUser        = "admin";
  adminShell       = "/run/current-system/sw/bin/bash";
  adminDescription = "System Administrator";
};
```

Usage from consumer flake:
```nix
constants = tentaflake.lib.${system}.constants;
```

## Module Reference

### `boot.nix`
Enables systemd-boot UEFI bootloader.
- `boot.loader.systemd-boot.enable = true`
- `boot.loader.efi.canTouchEfiVariables = true`
- Guarded by `tentaflake.boot.enable`

### `hardening.nix`
Kernel hardening and security controls:
- `kernel.kptr_restrict = 2`, `kernel.dmesg_restrict = 1`
- `kernel.unprivileged_bpf_disabled = 1`, `kernel.perf_event_paranoid = 3`
- `vm.unprivileged_userfaultfd = 0`
- Protected symlinks, hardlinks, FIFOs, regular files
- TCP syncookies, no accept_redirects, no accept_source_route
- `user.max_user_namespaces = 1000`
- `security.sudo.wheelNeedsPassword = true`
- AppArmor enabled
- Journald rate limiting + compression + 500M max
- Guarded by `tentaflake.hardening.enable`

### `locale.nix`
- `time.timeZone`, `i18n.defaultLocale`, `console.keyMap`
- Guarded by `tentaflake.locale.enable`

### `networking.nix`
- Hostname, NetworkManager, nftables firewall (default-deny, no open ports, no ping)
- Guarded by `tentaflake.networking.enable`

### `nix-settings.nix`
- `nix.settings.experimental-features = ["nix-command" "flakes"]`
- `trusted-users = ["root" adminUser]`
- Auto GC: weekly, `--delete-older-than 14d`
- Auto optimise: weekly, persistent
- `allowUnfree` via `tentaflake.allowUnfree`
- Guarded by `tentaflake.nixSettings.enable`

### `packages.nix`
- Installs `curl` + `git` system-wide
- Guarded by `tentaflake.packages.enable`

### `users.nix`
- Creates admin user with `wheel` + `networkmanager` groups
- Configurable shell, description, SSH authorized keys
- Guarded by `tentaflake.users.enable`

### `tailscale.nix`
- Enables Tailscale with `--advertise-tags=tag:agent --hostname=<name> --ssh`
- Opens firewall for Tailscale (WireGuard port)
- Guarded by `tentaflake.tailscale.enable`

### `shell.nix`
Interactive shell experience for SSH/console operators. Features:
- **`tentaflake` CLI** — agent management across every runtime (status, logs, restart, shell, exec)
- **`tentaflake-status`** — dynamic login banner (host info + agent health, all runtimes)
- **`tentaflake top`** — live filesystem activity TUI (requires audit daemon)
- **bash** — completion, large deduped history, colored prompt (or Starship)
- **zsh** — Oh My Zsh, autosuggestions, syntax highlight, fzf-tab
- **Modern tools** — eza, bat, fd, ripgrep, fzf, htop, btop, jq, tree, ncdu
- **Aliases** — `rebuild` → `sudo nixos-rebuild switch --flake /etc/nixos#<hostname>`
- **tmux** — mouse support, renumber-windows, 10K history
- Guarded by `tentaflake.shell.enable`

**The `tentaflake` CLI** is a shell script that auto-discovers agents from the
declared OCI containers, across both runtimes:

```bash
tentaflake              # List agents and their state (default)
tentaflake logs <name>  # Follow agent journal logs
tentaflake restart <name>
tentaflake start <name>
tentaflake stop <name>
tentaflake shell <name> # Interactive container shell
tentaflake exec <name> -- <cmd>
tentaflake ps           # Raw docker ps for agent containers
tentaflake top          # Live TUI (hermes-top, needs audit daemon)
tentaflake backup <name>  # Snapshot an agent's state dir to a .tar.gz (0600)
tentaflake doctor       # Host health check (exits nonzero on problems)
tentaflake console      # Agent Console URL + tailnet publish hint
tentaflake rebuild      # Apply the system config (nixos-rebuild switch)
tentaflake update       # Update flake inputs, review, then rebuild
```

A deprecated `hermes` shim still works — it prints a deprecation note to
stderr and execs `tentaflake "$@"`.

### `hermes-auditd.nix`
Filesystem audit daemon for agent activity tracking (all runtimes):
- Go daemon watches agent state dirs via inotify
- Records events to SQLite DB
- Serves HTTP/WS on `tentaflake.hermes-auditd.port` (default 9090)
- `hermes-top` TUI reads the DB for live dashboard
- Admin added to `hermes-audit` group for sudo-less access
- Auto-discovers watch dirs from every declarative agent container — Hermes
  and ZeroClaw alike (module/package name unchanged: still `hermes-auditd`)

### `piper-tts-server.nix`
Local TTS HTTP server (OpenAI-compatible `/v1/audio/speech`):
- Written in pure Python stdlib (no deps)
- Serves WAV via Piper ONNX models
- Hermes config: `tts.provider = "openai"` with `base_url = "http://localhost:5001/v1"`
- Guarded by `services.piper-tts-server.enable`

## Agent Configuration (`my-agents.nix`)

Create a `my-agents.nix` in the repo root. It now takes both builders and
returns two lists mapped through their respective builder — `hermesAgents`
through `mkHermesAgent`, `zeroclawAgents` through `mkZeroClawAgent`:

```nix
{ mkHermesAgent, mkZeroClawAgent }:
let
  hermesAgents = [
    {
      name = "coding";
      envFile = "/run/secrets/hermes-coding.env";
      settings = {
        model.default = "deepseek/deepseek-v4-flash";
        model.provider = "openrouter";
        # ... full Hermes YAML config as Nix attrs
      };
      extraContainerConfig = {
        extraOptions = [ "--memory=2g" ];
      };
    }
  ];
  zeroclawAgents = [ ];
in
map mkHermesAgent hermesAgents ++ map mkZeroClawAgent zeroclawAgents
```

The `settings` attrset is serialized to `config.yaml` (Hermes) or `config.toml`
(ZeroClaw), mounted read-only into the respective container.

Old single-argument `{ mkHermesAgent }: ...` files still work — `configuration.nix`
passes only the args each `my-agents.nix` declares (`lib.intersectAttrs (lib.functionArgs f)`).

The file is git-tracked. Check `my-agents.nix.example` for a complete reference.

## ISO Builds

### Installer ISO (`nix build .#installer-iso`)
- Boots into TUI wizard (`dialog`-based)
- Asks 6 questions: hostname, username, password, disk, timezone, optional features
- Partitions disk (1GB ESP + rest ext4), generates HW config, creates flake
- Runs `nixos-install` (10-15 min), sets password, reboots
- Embeds full repo at `/etc/tentaflake/` on the ISO

### Live Agent ISO (`nix build .#live-agent-iso`)
- Boots directly into Hermes agents + Piper TTS in RAM
- Ephemeral — pull the USB and everything is gone
- First boot: TUI wizard or USB `HERMES_ENV` auto-detection
- USB `HERMES_DATA` auto-mount for persistent agent state across reboots
- Pre-configured with two agents: `default` (general) and `research` (web-focused)
- `sudo tailscale up` for connectivity
- Can also install to disk via `/etc/tentaflake/installer/installer.sh`

### Build script
```bash
./scripts/build-iso.sh              # live-agent-iso (default)
./scripts/build-iso.sh installer    # installer-iso
```

## How to Use as a Consumer

The recommended pattern (from `examples/consumer-flake.nix`):

1. Import `tentaflake` as a flake input (follows `nixpkgs`)
2. Import `tentaflake.lib.${system}.mkHermesAgent` / `.mkZeroClawAgent` for agent creation
3. Use `tentaflake.nixosModules.default` for the module set
4. Set `tentaflake.*` options in your host config
5. Define agents in a separate file or inline

```nix
# flake.nix inputs
tentaflake = {
  url = "github:timfewi/tentaflake";
  inputs.nixpkgs.follows = "nixpkgs";
};

# NixOS configuration
nixosModules.default  # Enable all base modules
tentaflake.lib.x86_64-linux.mkHermesAgent    # Hermes agent helper
tentaflake.lib.x86_64-linux.mkZeroClawAgent  # ZeroClaw agent helper
tentaflake.lib.x86_64-linux.constants        # Default constants
```

## Secrets Management

Two approaches for agent secrets:

1. **Plain env files** — `envFile = "/run/hermes/<name>.env"` (for live ISO / tmpfs)
2. **Agenix** — `agenixFile = "/run/agenix/<name>-env"` (for installed systems)

Both are passed to Docker via `--env-file`. Never commit secrets to the repo.

ZeroClaw agents (`mkZeroClawAgent`) only accept `agenixFile` — there is no
plain `envFile` option for that runtime.

## Build & Test Commands

```bash
nix flake check                   # Validate flake + build toplevel + run Go tests
nix build .#installer-iso         # Build installer ISO
nix build .#live-agent-iso        # Build live agent ISO
nix build .#hermes-auditd         # Build audit daemon package
nix fmt                           # Format Nix files (nixfmt-tree)
cd pkgs/hermes-auditd && go test ./...  # Run Go tests
golangci-lint run                 # Go lint (in pkgs/hermes-auditd/)
```

## Check (CI)

The `checks.${system}.agent-host` target validates that the full toplevel builds. This runs in CI on every push via `.github/workflows/check.yml`.

## Nix Conventions

- Nix formatting: `nix fmt` (uses `nixfmt-tree`), 2-space indent
- Go formatting: `gofmt`, tabs, run `golangci-lint run` before push
- Commits: Conventional Commits (`feat:`, `fix:`, `docs:`)

## Template Rules (from AGENTS.md)

- This is a **generic template**. NEVER commit domain-specific code (company config, real hostnames, hardware configs, API keys, secrets, agent SOUL.md/skills written for specific deployments).
- Domain-specific work belongs in **forks**.
- See `CONTRIBUTING.md` for PR process.
- See `docs/05-fork-checklist.md` for what to change when forking.

## Packaging

### `pkgs/hermes-auditd`
Go application with:
- `cmd/hermes-auditd/` — daemon binary (filesystem watcher + SQLite + HTTP/WS)
- `cmd/hermes-top/` — TUI dashboard (reads audit DB)
- `internal/config/` — configuration loading
- `internal/watcher/` — inotify-based directory watcher
- `internal/store/` — SQLite event store
- `internal/hermes/` — event types

Build via `pkgs.callPackage ./pkgs/hermes-auditd { }`.

### `pkgs/piper-voices`
Bundled Piper ONNX voice models for TTS. Provides voice files under `/share/piper-voices/`.
