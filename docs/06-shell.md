# Shell Experience — Operating a Tentaflake Host over SSH

When you SSH into a freshly-installed Tentaflake machine (over Tailscale SSH),
`modules/shell.nix` makes the landing useful instead of a bare prompt. Everything
here is generic and toggleable via `tentaflake.shell.*` — it never hardcodes any
agent; it reflects whatever you defined in `my-agents.nix`.

## What you get

| Feature | Description |
|---|---|
| **Login banner** | `tentaflake-status` runs once per SSH/console login: host facts (kernel, uptime, load, memory, disk), Tailscale IP, and a colored health line per agent. |
| **`hermes` CLI** | One command to drive the agent containers — backend-aware (works for `docker` or `podman`). |
| **Bash QoL** | Completion, large deduped history, a colored prompt, and sensible aliases. |
| **Modern CLI tools** | `eza`, `bat`, `fd`, `ripgrep`, `fzf`, `htop`, `btop`, `jq`, `tree`, `ncdu`, `tmux`, `dnsutils`. |

## The `hermes` command

```
hermes [status]            Show all agents and their state (default)
hermes logs <name> [args]  Follow an agent's logs (extra journalctl args ok)
hermes restart <name>      Restart an agent
hermes start <name>        Start an agent
hermes stop <name>         Stop an agent
hermes shell <name>        Open an interactive shell inside an agent container
hermes exec <name> -- cmd  Run a command inside an agent container
hermes ps                  Raw docker/podman ps for agent containers
hermes top                 Live TUI dashboard of agent filesystem activity
hermes help                Show this help
```

Examples:

```bash
hermes                       # health table for every agent
hermes logs coding           # tail -f the coding agent's journal
hermes restart research      # sudo systemctl restart podman-hermes-research
hermes shell coding          # drop into a shell in the container
hermes exec coding -- hermes chat
```

`hermes` enumerates the generated `<backend>-hermes-<name>.service` units, so it
automatically tracks whatever agents your `my-agents.nix` defines — add or remove
an agent and `hermes` reflects it after the next rebuild. State-changing actions
(`restart`/`start`/`stop`) shell out to `sudo systemctl`, so the admin user needs
its usual `wheel` membership (the default).

## `hermes top` — live activity dashboard

`hermes top` (the `hermes-top` binary) is a full-screen TUI showing, in real
time, what files your agents are touching. It reads the `hermes-auditd` SQLite
database directly and refreshes once a second — **no network port is opened**,
so it fits the Tailscale-only, firewall-closed posture: you run it inside your
SSH session.

```
hermes-top  agent-host
3 events retained · window 5m · updated 12:04:31

  AGENT                  5m    TOTAL  LAST ACTIVITY
▶ coding                 42      318  write  skills/web.md
  research                7       54  create notes/q3.md

  EVENTS · coding  (44)
  12:04:31 coding       write  skills/web.md
  12:04:30 coding       create out/draft.txt
  12:04:28 research     remove tmp/scratch

q quit · ↑↓/jk scroll · g/G top/bottom · f filter agent · p pause · r refresh
```

Keys: `q`/`Esc` quit · `↑↓`/`jk` scroll · `pgup`/`pgdn` page · `g`/`G` top/bottom ·
`f` cycle the agent filter · `p`/space pause · `r` force refresh. Flags:
`-db <path>`, `-window <dur>` (recent-count window, default 5m), `-interval <dur>`.

**What it shows vs. what it doesn't:** `hermes-auditd` records *filesystem
changes* in each agent's state dir (`/var/lib/hermes-<name>/`) — a strong "is the
agent working, and on what" signal. It is **not** the agent's conversation or
tool-call stream; for that use `hermes logs <name>`. The two are complementary.

**Access:** the daemon runs as the unprivileged `hermes-audit` user and stores
its DB group-readable by the `hermes-audit` group; the admin user is added to
that group, so `hermes top` works **without sudo**. The daemon itself holds only
`CAP_DAC_READ_SEARCH` (a read-only bypass to watch the agents' `0700` state dirs).

Enable it (on by default for `agent-host`):

```nix
tentaflake.hermes-auditd.enable = true;
# watchDirs auto-derives from your agents; override only for custom stateDirs:
# tentaflake.hermes-auditd.watchDirs = [ "/var/lib/hermes-coding" ];
# tentaflake.hermes-auditd.retentionHours = 24;
```

## Agent Console — web file explorer + live monitor

`hermes top` is local-only (a TUI over SSH). The **Agent Console** (`tentaflake-console`
binary, same package) is its web counterpart: one fast page, served on the tailnet,
that replaces logging into each agent's kanban/dashboard. It has two panes:

- **Files** — a read-only, Google-Drive-style explorer over every agent's state dir.
  Browse, preview text, and download. **Secrets are always hidden** (`.env*`,
  `auth.json`, `config.yaml*`, `*.key`/`*.pem`/`*.age`, ssh/aws creds, …) at every
  depth, alongside caches (`.cache`, `.npm`, `.venv`, `node_modules`, …). The surface
  is **GET-only** — no edit, delete, or upload.
- **Activity** — "`hermes top`, but more advanced": per-agent op-rate cards plus a
  live event feed (create/write/remove/rename/chmod) streamed over SSE from the same
  `events.db`.

It reuses the daemon's exact security model — unprivileged `hermes-audit` user with a
**read-only** `CAP_DAC_READ_SEARCH` bypass — so it can read the agents' `0700` dirs
but write nowhere. Bind it to loopback and publish on the tailnet with
`tailscale serve` (see [operations](07-operations.md#exposing-dashboards--agent-built-apps-on-the-tailnet)).

```nix
tentaflake.hermes-auditd.console.enable = true;
# Roots auto-derive from your agents (one folder per /var/lib/hermes-<name>).
# Keep those and ADD data-disk mounts with extraRoots:
# tentaflake.hermes-auditd.console.extraRoots = [
#   { name = "my-agent-data"; path = "/srv/agent-data/my-agent"; }
# ];
# tentaflake.hermes-auditd.console.roots = [ … ];             # override the
#   auto-derived homes entirely (rarely needed; prefer extraRoots)
# tentaflake.hermes-auditd.console.addr = "127.0.0.1:9090";   # loopback bind
# tentaflake.hermes-auditd.console.extraDeny = [ "*.sqlite" ]; # extra hides
```

Then publish it, e.g. `tailscale serve --bg --https=9125 127.0.0.1:9090` →
`https://<host>.<tailnet>.ts.net:9125`.

## zsh, zoxide, lazygit, Neovim

Everything is opt-in — you're never locked into a shell or editor:

```nix
tentaflake.shell.zsh.enable = true;      # zsh + Oh My Zsh + autosuggestions +
                                         #   syntax-highlighting + fzf-tab
tentaflake.shell.zoxide.enable = true;   # `z` smart-cd (bash + zsh) — default on
tentaflake.shell.lazygit.enable = true;  # lazygit + the `lg` alias
tentaflake.shell.tmux.enable = true;     # tmux + sensible system config
tentaflake.editor.nvf.enable = true;     # Neovim via nvf (needs the nvf input)
```

- **tmux** installs a configured tmux (mouse on, `clock24`, 10k scrollback,
  1-based windows). Great for persistent sessions over SSH.
### Handy aliases

Always present with `shell.enable` (bash + zsh):

| Alias | Expands to |
|---|---|
| `rebuild` | `sudo nixos-rebuild switch --flake /etc/nixos#<hostName>` |
| `reload` | `exec $SHELL` (reload the current shell) |
| `cls` | `clear` |
| `lg` | `lazygit` (when `lazygit.enable`) |
| `ls`/`ll`/`la`/`cat`/`tree` | `eza`/`bat` equivalents (when `tools.enable`) |

- **zsh** — when enabled it becomes the admin user's login shell (overriding
  `adminShell`), with Oh My Zsh (`git`, `sudo`, `systemd`, and your container
  backend's plugin), autosuggestions, syntax highlighting, fzf-tab completion,
  and Starship as the prompt. When disabled you stay on bash.
- **Neovim (nvf)** lives in a separate module (`modules/editor.nix`) because it
  needs the `nvf` flake input. The template wires it into its own hosts and
  exports it as `nixosModules.editor`. The config is lean (LSP, treesitter,
  telescope, gitsigns, blink-cmp; languages nix/bash/lua/markdown/yaml) — add
  `languages.<lang>.enable` in a fork for a fuller dev stack. `EDITOR` becomes
  `nvim` when enabled.

These are set at install time by the **installer feature checklist** (see below),
or by hand in your host config.

## Options

The base shell options default to **on** for the installed `agent-host`:

```nix
tentaflake.shell.enable = true;            # master toggle for everything below
tentaflake.shell.motd.enable = true;       # the tentaflake-status login banner
tentaflake.shell.tools.enable = true;      # eza/bat/fd/ripgrep/fzf/... package set
tentaflake.shell.starship.enable = true;   # starship prompt (off → colored bash PS1)
tentaflake.shell.hermesCli.enable = true;  # the `hermes` CLI
```

When `tools.enable` is on, `ls`/`ll`/`la`/`cat`/`tree` are aliased to the modern
equivalents (`eza`, `bat`); turn it off to keep stock coreutils.

When `starship.enable` is off, a hand-rolled colored bash prompt is installed
instead (`user@host:cwd (git-branch)`, red username when root).

## Installer feature checklist

The installer ISO asks which extras to install via a checklist (zsh, zoxide,
nvf, lazygit, tmux, modern tools — pre-checked). Your choices are written straight
into the generated `/etc/nixos/flake.nix` as `tentaflake.*` toggles (and, for
nvf, the `nvf` flake input pinned to the ISO's revision + the editor module
import). Everything stays editable afterward — flip a toggle and
`sudo nixos-rebuild switch`.

## Defaults across the ISOs

| Profile | `shell.enable` | `motd.enable` | Why |
|---|---|---|---|
| `agent-host` (installed) | on | on | the system you SSH into day-to-day |
| `live-agent` (live ISO) | on | **off** | the live profile ships its own static `users.motd`; the dynamic banner is disabled to avoid two stacked banners |
| `installer-iso` | **off** | — | TTY1 only runs `installer.sh`; no agents exist yet |

## Notes

- The banner only prints on interactive SSH or console **login** shells, once per
  session — inner subshells, `tmux` panes, and `ssh host -- cmd` stay quiet. It is
  wired for both **bash and zsh**; aliases come from `environment.shellAliases` so
  they apply to whichever shell is active.
- The `hermes` CLI assumes the container/unit naming produced by `mkHermesAgent`
  (`hermes-<name>` container, `<backend>-hermes-<name>.service` unit). If you run
  agents through some other mechanism, the CLI won't see them.
