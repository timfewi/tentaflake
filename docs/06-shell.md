# Shell Experience — Operating a Tentaflake Host over SSH

When you SSH into a freshly-installed Tentaflake machine (over Tailscale SSH),
`modules/shell.nix` makes the landing useful instead of a bare prompt. Everything
here is generic and toggleable via `tentaflake.shell.*` — it never hardcodes any
agent; it reflects whatever you defined in `my-agents.nix`.

## What you get

| Feature | Description |
|---|---|
| **Login banner** | `tentaflake-status` runs once per SSH/console login: host facts (kernel, uptime, load, memory/disk with usage-colored percentages), Tailscale IP, an agent count (total · active · inactive · failed), and a health line per agent colored by runtime — with per-agent uptime for active agents, dimmed inactive agents, and a `tentaflake logs` hint when one has failed. |
| **`tentaflake` CLI** | One command to drive agent containers across every runtime (Hermes, ZeroClaw) — backend-aware (works for `docker` or `podman`). A deprecated `hermes` shim still works, with a warning. |
| **Bash QoL** | Completion, large deduped history, a colored prompt, and sensible aliases. |
| **Modern CLI tools** | `eza`, `bat`, `fd`, `ripgrep`, `fzf`, `htop`, `btop`, `jq`, `tree`, `ncdu`, `tmux`, `dnsutils`. |

## The `tentaflake` command

```
tentaflake [status]            Show all agents and their state (default)
tentaflake logs <name> [args]  Follow an agent's logs (extra journalctl args ok)
tentaflake restart <name>      Restart an agent
tentaflake start <name>        Start an agent
tentaflake stop <name>         Stop an agent
tentaflake shell <name>        Open a shell inside an agent container
tentaflake exec <name> -- cmd  Run a command inside an agent container
tentaflake ps                  Show all declarative agent containers
tentaflake top                 Live filesystem-activity TUI
tentaflake help                Show this help
```

A deprecated `hermes` shim still works for old scripts/muscle memory: it
prints `note: host command 'hermes' is deprecated; use 'tentaflake'` to
stderr and execs `tentaflake "$@"`. Update to `tentaflake` when convenient —
the shim may be removed in a future release. (This is the **host** CLI; it's
unrelated to the `hermes` command *inside* a Hermes agent container, which is
unchanged.)

Examples:

```bash
tentaflake                       # health table for every agent, every runtime
tentaflake logs coding           # tail -f the coding (hermes) agent's journal
tentaflake restart research      # sudo systemctl restart podman-hermes-research
tentaflake shell coding          # drop into a shell in the container
tentaflake shell assistant       # works the same for a ZeroClaw agent
tentaflake exec coding -- hermes chat
```

`tentaflake` enumerates every container under
`virtualisation.oci-containers.containers` — `hermes-<name>` (from
`mkHermesAgent`) and `zeroclaw-<name>` (from `mkZeroClawAgent`) alike — so it
automatically tracks whatever agents your `my-agents.nix` defines, across both
runtimes. Add or remove an agent and `tentaflake` reflects it after the next
rebuild. State-changing actions (`restart`/`start`/`stop`) shell out to `sudo
systemctl`, so the admin user needs its usual `wheel` membership (the
default).

### Status output has a runtime column

`tentaflake status` (and the login banner) list every agent with its runtime,
so a mixed Hermes + ZeroClaw fleet is legible at a glance. The header counts
the fleet (total · active · inactive · failed), each runtime has its own color
(hermes yellow, zeroclaw blue, other magenta), agents are sorted by name,
active agents show how long they have been up, inactive agents are dimmed,
and a failed agent adds a `tentaflake logs <name>` hint below the list:

```
  AGENTS (3 · 2 active · 1 inactive)
    ● assistant             zeroclaw   active   3h 12m
    ● coding                hermes     active   2d 4h
    ○ research              hermes     inactive
```

## `tentaflake top` — live activity dashboard

`tentaflake top` (execs the `hermes-top` binary — that name is unchanged) is a
full-screen TUI showing, in real time, what files your agents are touching,
across every runtime. It reads the `hermes-auditd` SQLite database directly
and refreshes once a second — **no network port is opened**, so it fits the
Tailscale-only, firewall-closed posture: you run it inside your SSH session.

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
changes* in every declarative agent's state dir (`/var/lib/hermes-<name>/` and
`/var/lib/zeroclaw-<name>/` alike) — a strong "is the agent working, and on
what" signal. It is **not** the agent's conversation or tool-call stream; for
that use `tentaflake logs <name>`. The two are complementary.

**Access:** the daemon runs as the unprivileged `hermes-audit` user and stores
its DB group-readable by the `hermes-audit` group; the admin user is added to
that group, so `tentaflake top` works **without sudo**. The daemon itself holds
only `CAP_DAC_READ_SEARCH` (a read-only bypass to watch the agents' `0700`
state dirs).

Enable it (on by default for `agent-host`):

```nix
tentaflake.hermes-auditd.enable = true;
# watchDirs auto-derives from ALL your agents, both runtimes (hermes-<name>
# and zeroclaw-<name> state dirs); override only for custom stateDirs:
# tentaflake.hermes-auditd.watchDirs = [ "/var/lib/hermes-coding" ];
# tentaflake.hermes-auditd.retentionHours = 24;
```

## Agent Console — web file explorer + live monitor

`tentaflake top` is local-only (a TUI over SSH). The **Agent Console** (`tentaflake-console`
binary, same package) is its web counterpart: one fast page, served on the tailnet,
that replaces logging into each agent's kanban/dashboard. It has two panes:

- **Files** — a read-only, Google-Drive-style explorer over every agent's state dir.
  Browse, preview text, and download. **Secrets are always hidden** (`.env*`,
  `auth.json`, `config.yaml*`, `*.key`/`*.pem`/`*.age`, ssh/aws creds, …) at every
  depth, alongside caches (`.cache`, `.npm`, `.venv`, `node_modules`, …). The surface
  is **GET-only** — no edit, delete, or upload.
- **Activity** — "`tentaflake top`, but more advanced": per-agent op-rate cards plus a
  live event feed (create/write/remove/rename/chmod) streamed over SSE from the same
  `events.db`.

It reuses the daemon's exact security model — unprivileged `hermes-audit` user with a
**read-only** `CAP_DAC_READ_SEARCH` bypass — so it can read the agents' `0700` dirs
but write nowhere. Bind it to loopback and publish on the tailnet with
`tailscale serve` (see [operations](07-operations.md#exposing-dashboards--agent-built-apps-on-the-tailnet)).

```nix
tentaflake.hermes-auditd.console.enable = true;
# Roots auto-derive from your agents across both runtimes (one folder per
# /var/lib/hermes-<name> or /var/lib/zeroclaw-<name>).
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
tentaflake.shell.enable = true;                # master toggle for everything below
tentaflake.shell.motd.enable = true;           # the tentaflake-status login banner
tentaflake.shell.tools.enable = true;          # eza/bat/fd/ripgrep/fzf/... package set
tentaflake.shell.starship.enable = true;       # starship prompt (off → colored bash PS1)
tentaflake.shell.tentaflakeCli.enable = true;  # the `tentaflake` CLI (+ deprecated `hermes` shim)
```

`tentaflake.shell.tentaflakeCli.enable` was renamed from
`tentaflake.shell.hermesCli.enable`; the old name still works (via
`lib.mkRenamedOptionModule`) but prints an eval-time deprecation warning —
switch to the new name when convenient.

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
- The `tentaflake` CLI enumerates every container declared under
  `virtualisation.oci-containers.containers`. It recognizes the
  `hermes-<name>` prefix (from `mkHermesAgent`) and `zeroclaw-<name>` prefix
  (from `mkZeroClawAgent`) to label the runtime column; anything else is
  labelled generically as `agent`. If you run agents through some other
  mechanism entirely (not via `oci-containers`), the CLI won't see them.
