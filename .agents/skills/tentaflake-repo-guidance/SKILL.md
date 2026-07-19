---
name: tentaflake-repo-guidance
description: Comprehensive reference for the tentaflake repository — all modules, options, lib functions, build targets, agent config, ISOs, and architecture. Use when working with the template itself, adding features, configuring hosts, or debugging the build.
version: 1.0.0
---

# Tentaflake — Repo Guidance

## Overview

Tentaflake is a **generic NixOS flake template** for running isolated AI agent containers — Hermes, ZeroClaw and OpenCode — on a single headless machine. It is NOT domain-specific — no company config, real hostnames, API keys, or secrets belong here. Domain-specific work goes in forks.

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
│   ├── nix-settings.nix          # Nix daemon: flakes, GC, optimisation, trusted users, hardening
│   ├── packages.nix              # System packages (curl, git)
│   ├── users.nix                 # Admin user creation (wheel + networkmanager groups)
│   ├── tailscale.nix             # Tailscale VPN with extraUpFlags
│   ├── ssh.nix                   # Opt-in hardened OpenSSH + fail2ban (tentaflake.ssh.enable)
│   ├── hive-research.nix         # Opt-in web-research MCP server (package comes from a flake input)
│   ├── shell.nix                 # ★ Interactive shell: tentaflake CLI, banner, zsh, tools
│   ├── tentaflake-auditd.nix     # ★ Filesystem audit daemon + tentaflake-top TUI (watches all runtimes)
│   ├── piper-tts-server.nix      # ★ Piper TTS HTTP server (OpenAI-compatible)
│   └── editor.nix                # Neovim (nvf) module (separate import)
│
├── lib/
│   ├── default.nix               # Exports: mkHermesAgent, mkZeroClawAgent, mkOpenCodeAgent, agentsFromData, constants
│   ├── constants.nix             # Template-wide defaults (hostName, adminUser, stateVersion, pinned images...)
│   ├── mkHermesAgent.nix         # ★ Builds one isolated Hermes agent as a NixOS module
│   ├── mkZeroClawAgent.nix       # ★ Builds one isolated ZeroClaw agent as a NixOS module
│   ├── mkOpenCodeAgent.nix       # ★ Builds one isolated OpenCode agent (opencode serve) as a NixOS module
│   ├── agentsFromData.nix        # Builds agents from the CLI wizard's agents.json
│   ├── pinnedImage.nix           # Enforces digest-pinned + shell-safe image refs
│   └── pinnedImage-test.nix      # Backs checks.<system>.image-pinning
│
├── pkgs/
│   ├── tentaflake-auditd/        # Go daemon: filesystem watcher + SQLite (no socket)
│   │   ├── cmd/tentaflake-auditd/ # Daemon binary
│   │   ├── cmd/tentaflake-console/ # Agent Console HTTP server
│   │   ├── cmd/tentaflake-top/   # TUI dashboard
│   │   └── internal/             # config, watcher, store, event types, web
│   └── piper-voices/             # Bundled Piper voice ONNX models
│
├── installer/
│   ├── iso.nix                   # Installer ISO config (TUI wizard → full install)
│   ├── live-iso.nix              # Live Agent ISO config (RAM, no disk touch)
│   ├── live-profile.nix          # Live ISO profile: agents, Piper, Tailscale, SSH
│   ├── live-agents.nix           # Pre-configured agents for live ISO (default + research)
│   ├── firstboot.nix             # USB env detection, data persistence, TTY1 wizard
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
│   ├── 06-shell.md               # Shell/operator experience
│   ├── 07-operations.md          # Backup/restore, egress filtering, log forwarding
│   ├── 08-agent-cli.md           # `tentaflake agent` wizard guide
│   └── 08-opencode.md            # OpenCode runtime guide
│
├── scripts/
│   ├── build-iso.sh              # Convenience script for ISO builds
│   ├── banner-test.sh            # Renders tentaflake-status against a fake fleet + self-checks
│   ├── generated-flake-test.sh   # Checks the flake installer.sh generates still evaluates
│   └── update-agent-images.sh    # Prints upstream digests for the pinned agent images
│
├── tests/
│   └── integration.nix           # NixOS VM test (checks.vm-integration): boots a VM from nixosModules.default
│
├── .agents/skills/                # Bundled skills (development agent + Hermes container)
│   ├── handle-the-host/           # Remote host operations (Tailscale SSH, rebuild)
│   ├── hermes-config-manager/     # Hermes config management
│   ├── hermes-memory-personality/ # Memory & personality management
│   ├── hermes-provider-setup/     # LLM provider configuration
│   ├── hermes-tools-config/       # Tool/toolset configuration
│   ├── tentaflake-change-review/  # REASON → VERIFY → SYNC gates for any change
│   └── tentaflake-repo-guidance/  # This file — repo reference
│
├── .github/workflows/
│   ├── check.yml                 # CI: nix flake check + fmt + Go build/vet/test + lint + shellcheck
│   ├── codeql.yml                # CodeQL analysis of the Go code
│   ├── gitleaks.yml              # Full-history secret scanning
│   └── update-flake-lock.yml     # Weekly flake.lock update PR
│
├── .golangci.yaml                # Go lint config (v2 format)
├── .pre-commit-config.yaml       # Optional local hooks mirroring the CI gates
├── CHANGELOG.md                  # Keep a Changelog; new entries go under [Unreleased]
├── README.md                     # Project entry point
├── CONTRIBUTING.md               # PR process
└── SECURITY.md                   # Security policy
```

## NixOS Configurations (flake outputs)

Three machines are defined in `flake.nix`:

| Configuration | Profile | Purpose |
|---|---|---|
| `tentaflake` | `installed` | Default target machine. Consumes `my-agents.nix` for custom agents. Built-in host in the template. |
| `installer-iso` | `installer` | Bootable ISO → TUI wizard → partitions disk → installs NixOS. |
| `live-agent` | `live` | Bootable ISO → runs agents in RAM + Piper TTS. Ephemeral. |

## All `tentaflake.*` Options

Most options are defined in `modules/options.nix`. Exceptions:
- `tentaflake.auditd.*` — defined in `modules/tentaflake-auditd.nix`
- `tentaflake.editor.nvf.*` — defined in `modules/editor.nix`

These are the knobs you turn in your host config.

### Core Identity

| Option | Type | Default | Description |
|---|---|---|---|
| `tentaflake.hostName` | `str` | `"tentaflake"` | System hostname |
| `tentaflake.adminUser` | `str` | `"user"` | Primary admin username |
| `tentaflake.adminDescription` | `str` | `"System Administrator"` | Description for the admin user |
| `tentaflake.adminShell` | `str` | (bash) | Shell for admin user |
| `tentaflake.adminAuthorizedKeys` | `list of str` | `[]` | SSH public keys for admin |
| `tentaflake.timeZone` | `str` | `"UTC"` | System timezone |
| `tentaflake.defaultLocale` | `str` | `"en_US.UTF-8"` | Default locale |
| `tentaflake.consoleKeyMap` | `str` | `"us"` | Console keymap |
| `tentaflake.consoleFont` | `str?` | `"ter-v16n"` | Legacy VT font; applies only when `modernConsole.enable = false` (`null` skips `setfont`) |
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
| `tentaflake.ssh.enable` | `false` | Hardened OpenSSH (key-only, no root) + fail2ban, opens TCP 22 |
| `tentaflake.networking.egress.enable` | `false` | nftables outbound allowlist (`allowedTCPPorts`/`allowedUDPPorts`) |
| `tentaflake.modernConsole.enable` | `true` | kmscon on TTY1 (real TTF fonts + Unicode); `fontSize` default 14. Off ⇒ legacy VT with `consoleFont` |

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

### tentaflake-auditd Sub-options (`tentaflake.auditd.*`)

| Option | Type | Default | Description |
|---|---|---|---|
| `tentaflake.auditd.enable` | bool | — | Enable audit daemon |
| `tentaflake.auditd.watchDirs` | list of str | `[]` | Auto-discovers from every declarative agent container (any runtime) |
| `tentaflake.auditd.port` | port | `9090` | Passed as `AUDIT_PORT`; the daemon opens no socket (the console has its own `console.addr`) |
| `tentaflake.auditd.console.enable` | bool | `false` | Agent Console: read-only web file explorer + live activity monitor |
| `tentaflake.auditd.console.addr` | str | `127.0.0.1:9090` | Console listen address (loopback; publish via `tailscale serve`) |
| `tentaflake.auditd.dbPath` | str | `/var/lib/hermes-audit/events.db` | SQLite DB path |
| `tentaflake.auditd.retentionHours` | int | `24` | Event retention window |

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
| `image` | str | `docker.io/nousresearch/hermes-agent@sha256:4a2f23bd3ffaa6ee7b3be8a302a38be43ab0321a2988cd3fb16b7dd472dde812` | Digest-pinned OCI container image |
| `allowMutableImage` | bool | `false` | Accept an unpinned `image`; gives up reproducibility |
| `envFile` | path? | `null` | Path to plaintext .env file on host |
| `agenixFile` | path? | `null` | Path to agenix-decrypted .env file |
| `seedDir` | path? | `null` | Directory of base files (SOUL.md, skills/) for first-boot seed |
| `extraVolumes` | list of str | `[]` | Extra Docker volumes |
| `extraEnvironment` | attrset | `{}` | Extra env vars |
| `cmd` | list of str? | `null` | Container command (null → `gateway run --replace`) |
| `networkMode` | str | `"host"` | Docker network mode |
| `pidsLimit` | int? | `512` | Container `--pids-limit` (fork-bomb ceiling); `null` disables |
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
| `image` | str | `ghcr.io/zeroclaw-labs/zeroclaw@sha256:eae321dac2d314bc282bdfb28b5378c9d527998f7e2fe0dee8315bfdcdf13a0c` | Digest-pinned OCI container image |
| `allowMutableImage` | bool | `false` | Accept an unpinned `image`; gives up reproducibility |
| `stateDir` | str | `/var/lib/zeroclaw-${name}` | Agent state directory |
| `seedDir` | path? | `null` | Directory of base files copied into the workspace on first boot |
| `gatewayPort` | int | `42617` | In-container gateway port |
| `hostPort` | int | **required** | Host loopback port forwarded to the gateway |
| `servePort` | int | **required** | Tailnet HTTPS port (published via `tailscale serve`) |
| `autoStart` | bool | `true` | Auto-start with system |
| `pidsLimit` | int? | `512` | Container `--pids-limit` (fork-bomb ceiling); `null` disables |
| `settings` | TOML attrset | `{}` | ZeroClaw config serialized to `config.toml` |
| `extraEnvironment` | attrset | `{}` | Extra env vars |
| `extraVolumes` | list of str | `[]` | Extra Docker volumes |

**What each agent gets:**

- State dir `/var/lib/zeroclaw-<name>` (0700, owned by uid/gid 65534)
- OCI container `zeroclaw-<name>` with `config.toml` mounted read-only
- Secrets via `agenixFile` → `--env-file`; overrides use `ZEROCLAW_<section>__<sub>__<key>` env vars (double underscores between path segments)
- A `tailscale serve` unit publishing `hostPort` on the tailnet at `servePort`

### `mkOpenCodeAgent`

Defined in `lib/mkOpenCodeAgent.nix`. Creates one [OpenCode](https://opencode.ai) agent as a NixOS module — the third supported runtime. Runs `opencode serve` (headless HTTP, documented OpenAPI 3.1), which makes it the cleanest runtime to drive from external orchestrators (n8n, CI, cron): `POST /session` then `POST /session/<id>/message`.

**Usage in `my-agents.nix`:**

```nix
{ mkOpenCodeAgent }:
[
  (mkOpenCodeAgent {
    name = "code";
    hostPort = 4096;                 # http://127.0.0.1:4096
    envFile = "/run/secrets/opencode-code.env"; # OPENCODE_SERVER_PASSWORD + key
    settings = {
      model = "anthropic/claude-haiku-4.5";
      provider.anthropic.options.baseURL = "http://proxy.host:4000"; # local LLM proxy
    };
  })
]
```

**Key parameters:** `name` (required), `hostPort` (required, host loopback → gateway), `image` (default `constants.opencodeImage`, digest-pinned), `allowMutableImage` (default `false`), `stateDir`, `workspaceDir` (mounted `/workspace`), `seedDir`, `gatewayPort` (default 4096), `servePort` (optional tailnet HTTPS, must differ from `hostPort`), `allowUnauthenticatedServe` (default `false`), `envFile`/`agenixFile` (secrets), `authFile` (opt-in read-only provider `auth.json`), `settings` (→ `opencode.json`, JSON, ro), `containerUid`/`containerGid` (default 65534), `autoStart`, `pidsLimit`, `extraEnvironment`, `extraVolumes`.

**Credentials:** two patterns — a **local LLM proxy** (recommended; `settings` base_url + env-file key; host-portable, keeps agents isolated) or **reuse an `auth.json`** via `authFile` (must exist on the host; never mount the whole data dir — `opencode.db` can be many GB). Always set `OPENCODE_SERVER_PASSWORD` in the env file for HTTP basic auth (username defaults to `opencode`). Setting `servePort` **asserts** that an `envFile` or `agenixFile` is wired up, because the container runs `opencode serve --hostname 0.0.0.0` and would otherwise be published to the tailnet unauthenticated; `allowUnauthenticatedServe = true` is the explicit opt-out. The eval assertion only sees that a file is wired up; the `opencode-<name>-tailscale-serve` unit re-checks the file contents at start, tears down any stale publication, and refuses to publish if no non-empty `OPENCODE_SERVER_PASSWORD` is defined. Never put the password in `extraEnvironment` — that lands in the world-readable Nix store. Full guide: `docs/08-opencode.md`.

**What each agent gets:** state dir `/var/lib/opencode-<name>` (0700, owned by the container uid), a `/workspace` project dir, `opencode.json` mounted read-only, `opencode serve` on `hostPort` (loopback), and an optional `tailscale serve` unit on `servePort` (with an `ExecStop` that removes the mapping when the agent stops).

### `agentsFromData`

Defined in `lib/agentsFromData.nix`. Turns the `tentaflake agent add` wizard's
`agents.json` into agent modules, so CLI-declared agents compose with a
hand-written `my-agents.nix`. Exported alongside the builders; `configuration.nix`
only forces it when an `agents.json` exists.

### `constants`

Defined in `lib/constants.nix`. Single source of truth for template-wide defaults:

```nix
constants = {
  stateVersion     = "26.05";
  defaultLocale    = "en_US.UTF-8";
  consoleKeyMap    = "us";
  consoleFont      = "ter-v16n";   # legacy VT only (kmscon off)
  hostName         = "tentaflake"; # doubles as the built-in host's flake attr
  adminUser        = "user";
  adminShell       = "/run/current-system/sw/bin/bash";
  adminDescription = "System Administrator";
  # Digest-pinned agent images (never tags) — see lib/pinnedImage.nix
  hermesImage      = "docker.io/nousresearch/hermes-agent@sha256:...";
  zeroclawImage    = "ghcr.io/zeroclaw-labs/zeroclaw@sha256:...";
  opencodeImage    = "ghcr.io/anomalyco/opencode@sha256:...";
  containerUid     = 10000;        # uid the hermes-agent image runs as
  containerGid     = 10000;
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
- Daemon hardening: `allowed-users = ["root" "@wheel"]` (agents live in containers, never talk to the host daemon), `sandbox = true` + `sandbox-fallback = false` (no silent downgrade to unsandboxed builds), `min-free`/`max-free` (2 GiB / 8 GiB) so builds cannot fill the disk
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
- **`tentaflake agent add`** — no-Nix agent wizard; logo pinned via the terminal's own scroll region (DECSTBM), API key imported from any USB stick (no label required) or typed with a masked confirm. Hidden check: `tentaflake agent __selftest`
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
tentaflake top          # Live TUI (tentaflake-top, needs audit daemon)
tentaflake backup <name>  # Snapshot an agent's state dir to a .tar.gz (0600)
tentaflake doctor       # Host health check (exits nonzero on problems)
tentaflake console      # Agent Console URL + tailnet publish hint
tentaflake agent list   # List configured agents (agents.json)
tentaflake agent add    # Interactive wizard: new agent, no Nix (pinned logo,
                        #   USB key import, key → root 0600 file)
tentaflake agent set-model <name>
tentaflake agent remove <name>
tentaflake rebuild      # Apply the system config (nixos-rebuild switch)
tentaflake update       # Update flake inputs, review, then rebuild
```

A deprecated `hermes` shim still works — it prints a deprecation note to
stderr and execs `tentaflake "$@"`.

### `tentaflake-auditd.nix`
Filesystem audit daemon for agent activity tracking (all runtimes):
- Go daemon watches agent state dirs via inotify
- Records events to SQLite DB
- The daemon itself **opens no socket** — it only writes to SQLite. The HTTP surface is the
  separate `tentaflake-console` service (`tentaflake.auditd.console.enable`, bound to
  `console.addr`, default loopback `127.0.0.1:9090`)
- `tentaflake-top` TUI reads the DB for live dashboard
- Admin added to `hermes-audit` group for sudo-less access
- Auto-discovers watch dirs from every declarative agent container — Hermes,
  ZeroClaw, and OpenCode alike (state-dir prefixes `hermes-`/`zeroclaw-`/`opencode-`;
  the Go watcher's `agentNameFromPath` labels each accordingly)

### `piper-tts-server.nix`
Local TTS HTTP server (OpenAI-compatible `/v1/audio/speech`):
- Written in pure Python stdlib (no deps)
- Serves WAV via Piper ONNX models
- Hermes config: `tts.provider = "openai"` with `base_url = "http://localhost:5001/v1"`
- Guarded by `services.piper-tts-server.enable`

## Agent Configuration (`my-agents.nix`)

Create a `my-agents.nix` in the repo root. It takes the three builders and
returns lists mapped through their respective builder — `hermesAgents`
through `mkHermesAgent`, `zeroclawAgents` through `mkZeroClawAgent`, and
`opencodeAgents` through `mkOpenCodeAgent`:

```nix
{ mkHermesAgent, mkZeroClawAgent, mkOpenCodeAgent }:
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
  opencodeAgents = [
    { name = "code"; hostPort = 4096; envFile = "/run/secrets/opencode-code.env"; }
  ];
in
map mkHermesAgent hermesAgents
++ map mkZeroClawAgent zeroclawAgents
++ map mkOpenCodeAgent opencodeAgents
```

The `settings` attrset is serialized to `config.yaml` (Hermes), `config.toml`
(ZeroClaw), or `opencode.json` (OpenCode), mounted read-only into the
respective container.

Old files taking fewer builders (e.g. `{ mkHermesAgent }: ...` or
`{ mkHermesAgent, mkZeroClawAgent }: ...`) still work — `configuration.nix`
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
- First boot: TUI wizard or USB `TENTAFLAKE_ENV` auto-detection (legacy `HERMES_ENV` accepted)
- USB `TENTAFLAKE_DATA` auto-mount for persistent agent state across reboots (legacy `HERMES_DATA` accepted)
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
2. Import `tentaflake.lib.${system}.mkHermesAgent` / `.mkZeroClawAgent` / `.mkOpenCodeAgent` for agent creation
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
tentaflake.lib.x86_64-linux.mkOpenCodeAgent  # OpenCode agent helper
tentaflake.lib.x86_64-linux.agentsFromData   # Agents from the CLI wizard's agents.json
tentaflake.lib.x86_64-linux.constants        # Default constants
```

## Secrets Management

Two approaches for agent secrets:

1. **Plain env files** — `envFile = "/run/tentaflake/<name>.env"` (for live ISO / tmpfs)
2. **Agenix** — `agenixFile = "/run/agenix/<name>-env"` (for installed systems)

Both are passed to Docker via `--env-file`. Never commit secrets to the repo.

ZeroClaw agents (`mkZeroClawAgent`) only accept `agenixFile` — there is no
plain `envFile` option for that runtime.

## Build & Test Commands

```bash
nix flake check                   # Validate flake + build toplevel + VM integration test + Go tests
nix build .#installer-iso         # Build installer ISO
nix build .#live-agent-iso        # Build live agent ISO
nix build .#tentaflake-auditd     # Build audit daemon package
nix build .#nixosConfigurations.tentaflake.config.system.build.toplevel --no-link  # Host config only
nix build .#checks.x86_64-linux.vm-integration -L  # Boot a VM from nixosModules.default, assert runtime
nix fmt                           # Format Nix files (nixfmt-tree; CI runs `nix fmt -- --ci`)
cd pkgs/tentaflake-auditd && go test ./...  # Run Go tests
golangci-lint run                 # Go lint (in pkgs/tentaflake-auditd/)
shellcheck installer/*.sh scripts/*.sh     # Shell lint (same invocation as CI)
./scripts/banner-test.sh          # Preview + self-check the tentaflake-status banner
```

## Check (CI)

`checks.${system}` exposes four targets, all run by `nix flake check` and in CI (`.github/workflows/check.yml`):

- `tentaflake` — validates the full toplevel builds. The attr name follows `constants.hostName`, so it moves if the default hostname does.
- `tentaflake-auditd` — builds the audit daemon package.
- `image-pinning` — asserts `lib/pinnedImage.nix` rejects mutable/unsafe image refs.
- `vm-integration` — boots a VM built from `nixosModules.default` (not the `tentaflake` host config; the test node sets its own `hostName`/`adminUser`) and asserts the runtime path: the `tentaflake` CLI runs, the status banner renders and names the host, `tentaflake-auditd` is active and has created its SQLite DB, and each declared agent produces its systemd unit + `0700` state dir (Hermes also its system user). Defined in `tests/integration.nix`.

CI additionally runs `nix fmt -- --ci`, `go build/vet/test`, `golangci-lint`, and `shellcheck` on every push.

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

### `pkgs/tentaflake-auditd`
Go application with:
- `cmd/tentaflake-auditd/` — daemon binary (filesystem watcher + SQLite; opens no socket)
- `cmd/tentaflake-console/` — Agent Console HTTP server (the only network surface)
- `cmd/tentaflake-top/` — TUI dashboard (reads audit DB)
- `internal/config/` — configuration loading
- `internal/watcher/` — inotify-based directory watcher (`agentNameFromPath` attribution)
- `internal/store/` — SQLite event store
- `internal/event/` — event types
- `internal/web/` — console handlers/templates

Build via `pkgs.callPackage ./pkgs/tentaflake-auditd { }`.

### `pkgs/piper-voices`
Bundled Piper ONNX voice models for TTS. Provides voice files under `/share/piper-voices/`.
