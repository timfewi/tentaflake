# Agent CLI Wizard — Add & Configure Agents Without Nix

`tentaflake agent add|list|set-model|remove` is an interactive wizard for
declaring Hermes and ZeroClaw agents **without writing Nix**. It's the
non-developer on-ramp: everything `my-agents.nix` can do by hand, the wizard
does by asking questions — including keeping API keys out of both Git and
the Nix store.

It lives in the same `tentaflake` CLI as `status`/`logs`/`restart` — see
[06-shell.md](06-shell.md) for the rest of the command surface. Prefer
hand-writing `my-agents.nix` (see [01-quickstart.md](01-quickstart.md) and
[02-agent-tips.md](02-agent-tips.md))? Nothing here changes that path — the
two coexist (details below).

## The commands

```
tentaflake agent [list]              Show every agent from agents.json (runtime, model, ports)
tentaflake agent add                 Interactive wizard: add a Hermes or ZeroClaw agent
tentaflake agent set-model <name>    Change an existing agent's model interactively
tentaflake agent remove <name>       Remove an agent from agents.json (and, optionally, its secret file)
```

None of these touch `my-agents.nix`. They read and write a single file,
`agents.json`, at the flake root — see [Where config lives](#where-config-lives-agentsjson)
below.

### Example: `tentaflake agent add`

On a real terminal the tentaflake logo is pinned to the top of the screen for
the whole wizard — it uses the terminal's own scroll region (DECSTBM), so only
the prompts below it scroll. It is restored on exit, including on Ctrl-C. The
full logo needs at least 24 rows × 60 columns; from 12–23 rows (and ≥40
columns) you get a compact two-row header instead, and below that no header at
all. A legacy Linux VT also gets the compact header, because it has no braille
glyphs to draw the logo with. In a pipe, under `NO_COLOR`, or on a `dumb`
terminal both the header and the colours are skipped and the wizard runs as
plain sequential output — colour is gated on those three conditions only,
never on window size.

Runtime and provider are picked from a numbered menu (an `fzf` fuzzy-picker
instead, if `fzf` is installed — see [06-shell.md](06-shell.md)); everything
else is a plain prompt:

```
$ tentaflake agent add
runtime
  1) hermes
  2) zeroclaw
runtime [1-2]: 1
agent name (lowercase, digits, dashes): research
provider
  1) openrouter
  2) anthropic
  3) openai
  4) custom
provider [1-4]: 1
model id (concrete id, e.g. anthropic/claude-sonnet-4): anthropic/claude-sonnet-4
API key for research (hidden — paste is fine, blank aborts):
  got sk-or-…mnop (73 chars) · looks like an OpenRouter key — correct? [Y/n] y
✓ added hermes agent 'research' → /etc/nixos/agents.json
  secret: /var/lib/tentaflake/secrets/hermes-research.env (root:root 0600, key not in git)
next: rebuild so the container is created from this config.
Rebuild now? [y/N] y
Applying system configuration — sudo nixos-rebuild switch --flake /etc/nixos#tentaflake
...
```

A ZeroClaw agent asks two more questions — `hostPort` and `servePort` are
required, and each must be an integer (the wizard doesn't auto-suggest a
free pair; pick your own, following the fleet convention of internal =
external + 100 documented in [`my-agents.nix.example`](../my-agents.nix.example)):

```
$ tentaflake agent add
runtime [1-2]: 2
agent name (lowercase, digits, dashes): assistant
provider [1-4]: 1
model id (concrete id, e.g. anthropic/claude-sonnet-4): anthropic/claude-haiku-4.5
hostPort: 9246
servePort: 9146
API key for assistant (hidden — paste is fine, blank aborts):
  got sk-or-…mnop (73 chars) · looks like an OpenRouter key — correct? [Y/n] y
✓ added zeroclaw agent 'assistant' → /etc/nixos/agents.json
  secret: /var/lib/tentaflake/secrets/zeroclaw-assistant.env (root:root 0600, key not in git)
next: rebuild so the container is created from this config.
Rebuild now? [y/N] y
```

Picking `custom` as the provider asks one extra question — the env-var name
to write the key under (e.g. `MY_PROVIDER_API_KEY`), since a custom
OpenAI-compatible endpoint doesn't follow one of the built-in provider
conventions — plus the `base_url` itself. (ZeroClaw ignores that env-var
name: it always writes the key under its own
`ZEROCLAW_providers__models__<provider>__default__api_key` convention,
`custom` included.)

The API key prompt reads with `read -rs` — it's never echoed to the
terminal, never on the shell history, and never appears in `ps`. It's staged
to a `0600` temp file and installed by `sudo install -D -m600 -o root -g
root`, so the plaintext never exists at a predictable path first. The wizard
also runs `git add agents.json` for you (staged, not committed) — review and
commit it alongside the rest of your config the usual way.

Because `read -rs` shows you nothing, the wizard echoes a **masked**
confirmation afterwards — the first 6 and last 4 characters plus the total
length — so you can tell a truncated paste from a good one, and say `n` to
retry. That masked preview is the one thing about the key that ends up in
your scrollback; the key itself never is.

### Importing the key from a USB stick

Nobody wants to type a 73-character OpenRouter key. If a stick is plugged in,
`tentaflake agent add` offers the key it finds on it:

```
model id (concrete id, e.g. anthropic/claude-sonnet-4): anthropic/claude-sonnet-4
Found a key on connected media:
  sk-or-…mnop (73 chars) · looks like an OpenRouter key
  openrouter.txt (on My Stick)
use it? [Y/n]:
```

If it finds several, you get a numbered list — and "type or paste it myself"
is always the last option. With no stick plugged in, nothing changes: the
wizard says so and goes to the key prompt. If you only remember the stick once
you are already there, plug it in and type `r` then Enter to rescan — none of
your earlier answers are lost.

**No filesystem label and no particular filename are required.** Use whatever
stick you already own. The wizard looks for:

- already-mounted media first — `/run/media/<user>/<volume>`, `/media/*`,
  `/mnt/*`. On a desktop your stick is already there, and nothing privileged
  happens at all.
- only if that turns up **no key**, unmounted **removable** partitions (never a
  fixed disk), which it mounts itself, read-only. A stale, key-less mountpoint
  under `/mnt` therefore does not stop it from finding your stick.

On each it searches the top level and one directory down for `.env` files
containing the provider's `KEY=value` line, and for `.txt`/`.key` files
holding a bare token.

Safety properties, deliberately not traded away for the convenience:

- Files on removable media are **parsed, never `source`d, `eval`d or
  executed**. The value is matched with a regex and taken as literal text; a
  file containing `OPENROUTER_API_KEY=$(rm -rf ~)` yields nothing runnable.
- Anything the wizard mounts itself is mounted `ro,nosuid,nodev,noexec` and
  unmounted again the moment the key has been read — including when you abort
  or hit Ctrl-C.
- The imported key takes exactly the same route as a typed one: into a `0600`
  temp file, then `sudo install -D -m600 -o root -g root`. Never in argv,
  never in `agents.json`, never in git, never in the Nix store.

> **This does not replace the `TENTAFLAKE_ENV` label.** That label is still the
> deterministic marker for the live ISO's *unattended* boot (see
> [README.md](../README.md#skip-the-wizard-unattended-boot)), which has nobody
> to ask. It is simply no longer required for the interactive wizard — where a
> labelled volume just gets offered first.

### `tentaflake agent list`

```
$ tentaflake agent list
  RUNTIME   NAME             PROVIDER    MODEL                      HOST   SERVE
  hermes    coding           openrouter  anthropic/claude-sonnet-4  -      -
  hermes    research         openrouter  anthropic/claude-sonnet-4  -      -
  zeroclaw  assistant        openrouter  anthropic/claude-haiku-4.5 9246   9146
```

Reads straight from `agents.json` — no rebuild needed to see current state,
and it never prints secrets (it doesn't even open the env files). Bare
`tentaflake agent` (no subcommand) is the same as `tentaflake agent list`.
With no agents configured yet, it just points you at `agent add`.

### `tentaflake agent set-model <name>`

Changes only the `model` field for an existing agent — provider, `base_url`,
ports, and `envFile` are left exactly as they are:

```
$ tentaflake agent set-model research
current model for 'research': anthropic/claude-sonnet-4
new model id [anthropic/claude-sonnet-4]: anthropic/claude-opus-4.1
✓ set model for 'research' → anthropic/claude-opus-4.1
apply with: tentaflake rebuild
```

Unlike `agent add`, it doesn't offer to rebuild for you — run
`tentaflake rebuild` yourself when ready. Switching providers (not just the
model id within the same provider) currently means `agent remove` +
`agent add` — `set-model` doesn't re-key the secret file or change
`provider`/`base_url`.

### `tentaflake agent remove <name>`

```
$ tentaflake agent remove research
Remove hermes agent research from agents.json? [y/N] y
✓ removed 'research' from /etc/nixos/agents.json
Also delete its secret file /var/lib/tentaflake/secrets/hermes-research.env? [y/N] y
deleted /var/lib/tentaflake/secrets/hermes-research.env
apply with: tentaflake rebuild
```

Like `set-model`, it doesn't rebuild for you — the config change is only
live after `tentaflake rebuild`. And like removing an agent from
`my-agents.nix` by hand, it doesn't clean up the container's system user or
state dir on its own — see
[02-agent-tips.md#adding--removing-agents](02-agent-tips.md#adding--removing-agents)
for the manual `userdel`/`rm -rf` cleanup, which applies here too.

## Where config lives: `agents.json`

`agents.json` sits at the flake root (`/etc/nixos/agents.json` on an
installed host) next to `my-agents.nix`. It is **flat, non-secret, and
git-trackable** — safe to commit, safe to read, safe to paste into an issue.
It holds only what's needed to render the Nix module call: name, runtime,
provider, model, base URL, ports, and the *path* to the env file — never the
key itself.

### Schema v1

```json
{
  "hermes": [
    {
      "name": "research",
      "provider": "openrouter",
      "model": "anthropic/claude-sonnet-4",
      "base_url": null,
      "envFile": "/var/lib/tentaflake/secrets/hermes-research.env"
    }
  ],
  "zeroclaw": [
    {
      "name": "assistant",
      "provider": "openrouter",
      "model": "anthropic/claude-haiku-4.5",
      "base_url": null,
      "hostPort": 9246,
      "servePort": 9146,
      "envFile": "/var/lib/tentaflake/secrets/zeroclaw-assistant.env"
    }
  ]
}
```

A runnable reference lives at [`agents.json.example`](../agents.json.example)
in the repo root — normally you'd let `tentaflake agent add` write
`agents.json` for you rather than copying the example by hand, but it's
there if you want to see (or hand-edit) the shape directly.

| Field | Type | Hermes | ZeroClaw | Notes |
|---|---|:---:|:---:|---|
| `name` | string | required | required | matches `mkHermesAgent`/`mkZeroClawAgent`'s `name` |
| `provider` | string | required | required | `openrouter` \| `anthropic` \| `openai` \| `custom` |
| `model` | string | required | required | concrete model id — no `"auto"` (ZeroClaw rejects catch-all ids) |
| `base_url` | string \| `null` | optional | optional | set for `custom` / self-hosted OpenAI-compatible endpoints |
| `hostPort` | int | — | required | ZeroClaw only: host loopback → container gateway port |
| `servePort` | int | — | required | ZeroClaw only: tailnet HTTPS port |
| `envFile` | string | required | required | **path** to the runtime env file; the key itself never lives here |

[`lib/agentsFromData.nix`](../lib/agentsFromData.nix) loads `agents.json`
(when present) the same way `configuration.nix` auto-imports `my-agents.nix`
— additive, not a replacement — feeding each entry through `mkHermesAgent` /
`mkZeroClawAgent`. For ZeroClaw, `envFile` is passed as that helper's
`agenixFile` argument (the parameter name is `agenixFile` regardless of
whether the file is agenix-decrypted or a plain env file).

### Where the API key actually lives

**Never in `agents.json`.** Keys live in a root-owned, `0600` file at:

```
/var/lib/tentaflake/secrets/<runtime>-<name>.env
```

e.g. `/var/lib/tentaflake/secrets/hermes-research.env` containing:

```
OPENROUTER_API_KEY=sk-or-...
```

The env-var name follows the provider: `OPENROUTER_API_KEY` (openrouter),
`ANTHROPIC_API_KEY` (anthropic), `OPENAI_API_KEY` (openai), or a name you
choose for `custom`. For a ZeroClaw agent the key line instead uses
ZeroClaw's double-underscore config-override convention:

```
ZEROCLAW_providers__models__openrouter__default__api_key=sk-or-...
```

The wizard writes this file with `umask 077` and `install -m600` (or a temp
file + `sudo mv`, owned `root:root`) — the key is never in `agents.json`,
never in shell history, never in `ps`, and never copied into the Nix store.
`agents.json` only records the file's *path* in `envFile`, exactly like a
hand-written `my-agents.nix` records it in `envFile`/`agenixFile` today.

Want the key **encrypted in Git** instead of a plaintext runtime file? That's
what agenix is for — see [04-agenix-secrets.md](04-agenix-secrets.md). The
wizard's env files and agenix's `.age` files serve the same runtime contract
(a path to a file containing `KEY=value` lines); they're just two ways to
get there, and you can mix both across agents on the same host.

## Coexisting with `my-agents.nix`

`agents.json` and `my-agents.nix` are two independent sources feeding the
same two builders (`mkHermesAgent`, `mkZeroClawAgent`):

- **Wizard-managed agents** (`agents.json`) — for non-developers, or for
  quick agents that don't need custom `settings` (toolsets, memory,
  compression, TTS/STT, extra volumes, …).
- **Hand-written agents** (`my-agents.nix`) — for power users who want the
  full `settings` surface documented in
  [`my-agents.nix.example`](../my-agents.nix.example) and
  [02-agent-tips.md](02-agent-tips.md#config-tips-settings-parameter).

Both files can define agents at the same time; give each agent a unique
`name` across the two. An agent added via the wizard can later be
"graduated" to `my-agents.nix` for fuller control — copy its `agents.json`
entry into the matching Nix attrset, add whatever `settings` you need, then
run `tentaflake agent remove <name>` to drop it from `agents.json` so it
isn't declared twice.

## See also

- [06-shell.md](06-shell.md) — the rest of the `tentaflake` CLI (status,
  logs, restart, rebuild, doctor, …)
- [04-agenix-secrets.md](04-agenix-secrets.md) — encrypted alternative to a
  plaintext runtime env file
- [02-agent-tips.md](02-agent-tips.md) — hand-written agent management,
  including the manual cleanup steps `agent remove` doesn't automate
- [01-quickstart.md](01-quickstart.md) — first agent, the hand-written way
