# Changelog

All notable changes to tentaflake are documented here.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### ⚠ Breaking
- The fleet-generic infrastructure drops its `hermes-` branding (#52); "Hermes" now only names the Hermes agent runtime. What renames, and what bridges the transition for one release:
  - **systemd units**: `hermes-auditd` → `tentaflake-auditd`, and on the live ISO `hermes-env-detect`/`hermes-data-mount` → `tentaflake-env-detect`/`tentaflake-data-mount`. The old unit names are **gone** — update `systemctl`/`journalctl` scripts. (The console unit was already `tentaflake-console`.)
  - **module options**: `tentaflake.hermes-auditd.*` → `tentaflake.auditd.*`. Old names still evaluate with a deprecation warning via `lib.mkRenamedOptionModule` and will be removed in a future release.
  - **flake package attr**: `packages.hermes-auditd` → `packages.tentaflake-auditd`. The old attr remains as a deprecated alias to the same derivation.
  - **binaries**: `hermes-auditd` → `tentaflake-auditd`, `hermes-top` → `tentaflake-top` (`tentaflake top` now execs the new name). The package installs a deprecated `hermes-top` symlink for one release.
  - **USB labels** (live ISO): `TENTAFLAKE_ENV`/`TENTAFLAKE_DATA` are the primary labels; the legacy `HERMES_ENV`/`HERMES_DATA` labels are still accepted, so existing sticks keep working.
  - **live-ISO env dir**: `/run/hermes` → `/run/tentaflake`, with a `/run/hermes` compat symlink for configs that hardcode the old `envFile` path.
  - **Go module** (fork-relevant only): `tentaflake/hermes-auditd` → `tentaflake/tentaflake-auditd`; `internal/hermes` → `internal/event`.
  - **NOT renamed** (deliberately, so existing deployments keep their audit history): the audit DB state dir `/var/lib/hermes-audit`, the default `dbPath` `/var/lib/hermes-audit/events.db`, and the `hermes-audit` user/group. These rename in a future major with a migration.
  - Also: the `hermes-<name>`/`zeroclaw-<name>` container/user/state-dir prefixes, the `HERMES_*` env contract, and `mkHermesAgent` are unchanged — they name the Hermes runtime, not the infrastructure.

### Security
- `tentaflake-auditd` hardening (#8): the audit DB now sets `secure_delete` and is capped at 10000 pages (~40 MB) so an agent flooding filesystem events cannot fill the host disk; inotify directory watches are capped at 10000 per daemon (with a one-time warning when hit); the console clamps `?limit=` to 1000 and every store read is bounded by a 5 s query timeout. (No `fs.inotify.max_user_watches` sysctl needed — nixpkgs already defaults it to 524288.)

### Added
- `tentaflake` CLI host-management subcommands (#51): `rebuild` (nixos-rebuild switch on the system flake — same command as the `rebuild` alias), `update` (`nix flake update` on `/etc/nixos`, shows the `flake.lock` diff, asks y/N, then rebuilds), `doctor` (deep health check — failed systemd units, root disk ≥90%, Tailscale, `tentaflake-auditd`/`tentaflake-console` service state when enabled, per-agent unit state — every problem paired with its exact fix command; nonzero exit when problems found), `console` (Agent Console URL + the `tailscale serve` publish one-liner, or how to enable the console when it's off), and `backup <name>` (one-shot `sudo tar` snapshot of the agent's state dir to `./tentaflake-<name>-<UTC timestamp>.tar.gz`, with an active-agent consistency warning and the matching restore one-liner). Agent records now carry each agent's state dir, derived from the container's first volume mount (so custom `stateDir`s are honored).
- Opt-in egress filtering (`tentaflake.networking.egress.enable`, **default off**): an nftables output chain (table `tentaflake-egress`, policy drop) allowing loopback, established/related, ICMP/ICMPv6, and configurable outbound ports (`allowedTCPPorts` default `[443]`, `allowedUDPPorts` default `[53 67 123 547 41641]` — DNS, DHCP, NTP, DHCPv6, tailscale). Because agent containers use host networking, the host rules cover the whole fleet. (#9, [docs](docs/07-operations.md#egress-filtering-opt-in))
- `modules/ssh.nix` — opt-in hardened OpenSSH server (`tentaflake.ssh.enable`, default off): key-only auth (no passwords, no keyboard-interactive, no root login, max 3 auth tries), fail2ban, and TCP 22 opened in the otherwise deny-all firewall. Tailscale SSH remains the primary access path; admin keys come from `tentaflake.adminAuthorizedKeys`.
- CI security workflows: CodeQL analysis of the Go code (`.github/workflows/codeql.yml`, push/PR to main + weekly), gitleaks full-history secret scanning (`.github/workflows/gitleaks.yml`), and a weekly `flake.lock` update PR via DeterminateSystems/update-flake-lock (`.github/workflows/update-flake-lock.yml`).
- `.pre-commit-config.yaml` — optional local hooks mirroring the CI gates (gitleaks, shellcheck, gofmt, `go vet`, `nix fmt`); setup documented in CONTRIBUTING.md.
- `scripts/banner-test.sh` (also `just banner`) — renders the `tentaflake-status` banner with a stubbed `systemctl` and a fake mixed-runtime fleet (active/inactive/failed) so the banner can be previewed and regression-checked on any dev machine; self-checks cover fleet counters, duration formatting, logo loading, and logo/info-column alignment.

### Changed
- Live agent ISO: the debug sshd no longer accepts password auth (`PasswordAuthentication = false`) — Tailscale SSH covers remote debugging on the live ISO.
- `pidsLimit` parameter on `mkHermesAgent` and `mkZeroClawAgent` — renders `--pids-limit=<n>` on the agent container (default `512`, generous because agents compile code; `null` skips the flag). Hermes containers additionally gain `--security-opt=no-new-privileges:true` by default (ZeroClaw containers already had it). Opt-in `--cap-drop=ALL` recipe and the docker-group-is-root-equivalent tradeoff (podman as the rootless alternative) documented in `docs/02-agent-tips.md` and `docs/07-operations.md`. (#3)
- Kernel hardening (#4): `hardening.nix` gains sysctls (kexec/sysrq off, BPF JIT hardening, TCP RFC 1337, ICMP broadcast/bogus-error ignore, loose rp_filter — Docker-compatible, martian logging, ARP ignore/announce) , `boot.kernelParams` (`slab_nomerge`, `init_on_alloc/free`, `pti=on`, `vsyscall=none`, `debugfs=off`, `randomize_kstack_offset=on`), and an explicit LSM order via `security.lsm` (`landlock,yama,apparmor,bpf`); `boot.nix` disables the systemd-boot menu editor (blocks `init=/bin/sh`).
- Git hygiene: `.gitignore` now covers `age.key` and `*.log`; `CONTRIBUTING.md` documents SSH commit/tag signing and signed release tags (`git tag -s`). (#10)
- `docs/04-agenix-secrets.md`: "Key Rotation & Recovery" section — rotating secret values, rekeying out a compromised recipient (old ciphertext in Git history stays exposed), recovering from a lost host SSH key via any surviving recipient, and an example rotation cadence for forks. (#6)
- `SECURITY.md`: operator **incident response** runbook — isolate the container, revoke provider keys, inspect the `tentaflake-auditd` audit trail (UTC timestamps), preserve the state dir, rotate secrets and rebuild. (#11)
- `docs/07-operations.md`: **Backup & restore** (state dirs, audit DB via `sqlite3 .backup` — never `cp` a live WAL db — off-host age identity, restic example for forks) and **Log forwarding** (`services.journald.upload` snippet) sections. (#11)
- `modules/piper-tts-server.nix`: systemd unit hardened to parity with `tentaflake-auditd`/`hive-research` (`ProtectSystem=strict`, `ProtectHome`, `PrivateDevices`, `RestrictNamespaces`, `LockPersonality`, empty capability bounding set, `RestrictAddressFamilies`, `SystemCallFilter=@system-service`) plus resource limits: `TasksMax=64` and a new `services.piper-tts-server.memoryMax` option (default `2G` — raise for larger voice models). (#12)
- `modules/nix-settings.nix`: Nix daemon hardening — `allowed-users` restricted to `root` and `@wheel` (agents live in containers and never talk to the host daemon), explicit `sandbox = true` with `sandbox-fallback = false` (no silent downgrade to unsandboxed builds), and `min-free`/`max-free` (2 GiB / 8 GiB) so builds cannot fill the disk. (#5)
- Live-ISO firstboot env detection genericized (#52): agent env-file placeholders are now created for `docker-`/`podman-` units of **both** runtimes (`hermes-*` and `zeroclaw-*`, previously `hermes-*` only), and the USB data-mount persists `/var/lib/zeroclaw-*` state dirs too. Hermes env files keep their bare names (`coding.env`); other runtimes keep the runtime prefix (`zeroclaw-scout.env`).
- `.golangci.yaml` migrated to the golangci-lint v2 config format — the v1 file no longer loaded under golangci-lint 2.x, so lint was silently broken; the errcheck findings that had accumulated unseen are fixed.
- `tentaflake-status` login banner redesigned: braille-art octopus-snowflake logo in cyan (embedded at build time from `public/tentaflake-shell-logo.txt`, the single source of truth) with the header and host facts rendered as a column to its right, and the container backend in the tagline; `AGENTS` header now cyan with a fleet count (`total · active · inactive`, plus `failed` in red when present); each runtime gets its own color (hermes yellow, zeroclaw blue, other magenta) on dot/runtime/status; agents sorted by name; active agents show their uptime (`active 2d 4h`); inactive agents render dimmed; a failed agent adds a red `⚠ failed: <name> — tentaflake logs <name>` hint. Also: memory/disk lines gain usage-colored percentages (green/yellow ≥75%/red ≥90%), host uptime is read from `/proc/uptime` (fixes the duplicated load average in the old fallback), and a separator rule divides host facts from the agent list.

### Fixed
- Installer: `nixos-install` failed evaluating the target flake with a `readFile` error — the redesigned `tentaflake-status` banner reads `public/tentaflake-shell-logo.txt` from `modules/shell.nix`, but the installer never copied `public/` into `/mnt/etc/nixos`. The installer now copies the logo file (just that file, not the multi-MB imagery in `public/`).

## [0.2.0] — 2026-07-11

### ⚠ Breaking
- The host operator CLI is now **`tentaflake`**. The `hermes` host command remains in this release as a deprecated shim (prints a warning to stderr, execs `tentaflake`) and **will be removed in a future release** — update scripts and habits. (The `hermes` CLI *inside* Hermes agent containers is unrelated and unchanged.)
- `tentaflake.shell.hermesCli.enable` → `tentaflake.shell.tentaflakeCli.enable`. The old option still evaluates via `lib.mkRenamedOptionModule` with a deprecation warning and will be removed together with the shim.
- `my-agents.nix` is now expected to accept `{ mkHermesAgent, mkZeroClawAgent }`. Inside this template, `configuration.nix` only passes the arguments your file declares, so old `{ mkHermesAgent }:`-only files keep evaluating — but flake-input consumers that call `import ./my-agents.nix { inherit mkHermesAgent mkZeroClawAgent; }` directly must update old files to the new signature (or add `...`).
- `tentaflake ps` behaves differently from the old `hermes ps`: it lists agent containers of **all runtimes including stopped ones** (`--all` + anchored name filters) instead of `--filter name=hermes-` on running containers only.
- Agent Console / `tentaflake top` label non-Hermes agents with a runtime prefix (`zeroclaw-<name>`). Hermes agents keep their bare labels, so existing audit DBs and dashboards are unaffected.

### Added
- `lib/mkZeroClawAgent.nix` — second agent runtime alongside Hermes: OCI container (`ghcr.io/zeroclaw-labs/zeroclaw`), TOML `settings` attrset generated into a read-only `config.toml`, a `tailscale serve` unit for tailnet HTTPS access, and an optional `seedDir` copied into the state dir on first boot. State dir `/var/lib/zeroclaw-<name>`, secrets via `agenixFile` (`--env-file`).
- `zeroclaw.env.example` — mirrors `hermes.env.example`; documents the `ZEROCLAW_<section>__<sub>__<key>` env-var convention ZeroClaw uses for config overrides, with an OpenRouter `api_key` example.
- `my-agents.nix.example` now takes `{ mkHermesAgent, mkZeroClawAgent }` and defines both a `hermesAgents` list and a `zeroclawAgents` list (`map mkHermesAgent hermesAgents ++ map mkZeroClawAgent zeroclawAgents`), with a fully-commented generic ZeroClaw reference agent (schema_version, model, agentic runtime profile, supervised risk profile).
- `my-agents.nix.example`: rewritten around a per-runtime agent list (now `hermesAgents`, see above) with a fully-commented generic reference agent documenting every `settings` option (model + fallbacks, auxiliary routing, compression, memory, toolsets, approvals, web/terminal backends, provider timeouts, TTS/STT) plus seed/volume/env/container fields. Active agent renamed `default` → `coding` to match the docs. Quickstart and agent-tips docs updated to the new structure.
- `modules/shell.nix`: optional **zsh** (`tentaflake.shell.zsh.enable`) — Oh My Zsh + autosuggestions + syntax-highlighting + fzf-tab, with Starship as the prompt; becomes the admin login shell when enabled. Plus `tentaflake.shell.zoxide.enable` (smart-cd, cross-shell, default on) and `tentaflake.shell.lazygit.enable` (+ `lg` alias). Aliases moved to `environment.shellAliases` so they apply to bash and zsh.
- `modules/shell.nix`: **tmux** (`tentaflake.shell.tmux.enable`, configured). Convenience aliases `rebuild` (→ `nixos-rebuild switch --flake /etc/nixos#<host>`), `reload` (`exec $SHELL`) and `cls`. tmux moved out of the `tools` package set into its own toggle.
- `modules/editor.nix` + `nixosModules.editor`: optional **Neovim via nvf** (`tentaflake.editor.nvf.enable`) — LSP, treesitter, telescope, gitsigns, blink-cmp; lean language set (nix/bash/lua/markdown/yaml). Added the `nvf` flake input (kept out of `nixosModules.default` so external consumers aren't forced to have it).
- Installer: a **feature checklist** (zsh / zoxide / nvf / lazygit / modern tools) whose selections are written into the generated `/etc/nixos/flake.nix` as `tentaflake.*` toggles (nvf also injects the rev-pinned input + editor module import).
- `hermes-top` — live TUI dashboard (bubbletea) of agent filesystem activity, launched via `tentaflake top`. Reads the `hermes-auditd` SQLite DB directly (no network surface — runs over Tailscale SSH). Implements the read side the daemon previously discarded (the `internal/store` already had the query methods).
- `hermes-auditd` is now wired up: enabled by default on `agent-host`, `watchDirs` auto-derives from the agents defined in `my-agents.nix`, runs as an unprivileged `hermes-audit` user with only `CAP_DAC_READ_SEARCH`, and stores its DB group-readable so the admin can run `tentaflake top` without sudo.
- `internal/store`: `Since(afterID, limit)` (incremental tail) and `AgentRows(window)` (per-agent activity summary) read helpers.
- `modules/shell.nix` — operator shell experience for SSH/console: the agent-management CLI (`tentaflake`, see Changed), dynamic `tentaflake-status` login banner, Starship/bash prompt, completion, and a curated modern CLI tool set. Toggle via `tentaflake.shell.*`. See [`docs/06-shell.md`](docs/06-shell.md).
- `modules/piper-tts-server.nix` — local TTS via Piper (OpenAI-compatible `/v1/audio/speech`)
- `modules/hermes-firstboot.nix` — USB env detection + first-boot TUI wizard
- `nixosConfigurations.live-agent` + `nix build .#live-agent-iso` — bootable ISO with Hermes + TTS

### Changed
- Host operator CLI renamed `hermes` → `tentaflake` (same subcommands: `status|logs|restart|start|stop|shell|exec|ps|top|help`), now multi-runtime aware — `tentaflake status`/`ps` and the login banner list agents across all runtimes with a runtime column. A deprecated `hermes` shim still works: it prints a deprecation note to stderr and execs `tentaflake`.
- `tentaflake.shell.hermesCli.enable` renamed to `tentaflake.shell.tentaflakeCli.enable` (old name still accepted via `lib.mkRenamedOptionModule`).
- `modules/hermes-auditd.nix`: `watchDirs` auto-discovery widened from `hermes-*` containers to all declarative agent containers (`hermes-*`/`zeroclaw-*` prefixes), so ZeroClaw state dirs are watched too — unrelated oci-containers are still excluded. Auto-derived console roots keep bare names for Hermes (`coding`) and runtime-prefixed names otherwise (`zeroclaw-assistant`).
- `hermes-auditd` 0.1.3: event attribution (`agentNameFromPath`) and `hermes-top` path shortening now recognize `/var/lib/zeroclaw-<name>` state dirs — ZeroClaw activity is labelled `zeroclaw-<name>` instead of lumped into `unknown`; Hermes labels stay bare for existing DBs.
- `flake.nix` now exports `mkZeroClawAgent` alongside `mkHermesAgent` (`lib.${system}`, `specialArgs`); `configuration.nix` passes each builder to `my-agents.nix` only if the file's function asks for it (`lib.functionArgs` intersection), so old `{ mkHermesAgent }`-only files keep working unmodified.

### Fixed
- `store.go`: fixed event time-window comparisons (`Stats`, `Prune`) — stored RFC3339 timestamps were string-compared against SQLite's `datetime('now', …)` (space-separated) form, which only agreed when the date differed, so same-day events outside the window were mis-counted and never pruned. Now normalized via `datetime(timestamp)`.
- `cmd/hermes-auditd/main.go`: removed the misleading "HTTP/WebSocket server not implemented — notify channel discarded" warning; the notify channel is now drained intentionally (data is read back via `hermes-top`).
- `watcher.go`: added `Close()` method to fix fsnotify file descriptor leak
- `flake.nix`: fixed `mkHermesAgent` import (was attrset, now unwrapped function)
- `watcher.go`: fixed `FlushAll` timer race with `entry.timer = nil` guard after `Stop()`
- CI: added `go test` step to GitHub Actions workflow
- `installer.sh`: removed dead `spinner()` function
- `store.go`: removed unused `done` channel
- Docs: fixed nftables attribution (`hardening.nix` → `networking.nix`)

## [0.1.0] — 2026-06-19

### Added
- Initial release: NixOS flake template for multi-agent Hermes orchestration
- `lib/mkHermesAgent.nix` — declarative Docker-based Hermes agent creation
- `lib/constants.nix` — template-level defaults (stateVersion, locale, hostname)
- `modules/` — reusable NixOS modules (boot, locale, networking, users, hardening, tailscale, etc.)
- `pkgs/hermes-auditd/` — Go daemon for filesystem event auditing with SQLite
- `installer/` — interactive TUI installer ISO (`nix build .#installer-iso`)
- `docs/` — quickstart guide, agent tips, skill index, and 4 bundled Hermes skills
- GitHub Actions CI: `nix flake check` on PR and push to main

[Unreleased]: https://github.com/timfewi/tentaflake/compare/v0.2.0...main
[0.2.0]: https://github.com/timfewi/tentaflake/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/timfewi/tentaflake/releases/tag/v0.1.0
