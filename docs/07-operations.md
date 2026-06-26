# Operating a Hermes fleet — persistence, UIDs, secrets & exposure

This page covers the operational model behind `mkHermesAgent` and the optional
hardening arguments it exposes. Everything here is generic — no host-specific
config belongs in the template (see `CLAUDE.md` → Template Rule).

## The persistence model (read this first)

A running agent sees **two** very different storage locations:

| Inside the container | Host path | Lifetime | Use for |
|---|---|---|---|
| `$HERMES_HOME` (e.g. `/var/lib/hermes-<name>`) | bind-mount of `stateDir` | **Persistent** — survives restart, recreate, reboot | All real work |
| `$HOME` (e.g. `/opt/data`) | anonymous Docker volume | **Ephemeral** — wiped when the container is *recreated* (any rebuild that changes the container) | Throwaway dotfiles only |

> **The `/opt/data` trap.** The agent's UNIX `$HOME` is *not* persistent. Work
> built there (a venv, a web app, a database) **disappears on the next
> `nixos-rebuild switch` that recreates the container**. Durable work — and
> anything you want to keep — must live under `$HERMES_HOME/workspace`.

Practical consequences:

- Tell the agent (in its SOUL/AGENTS seed) to build under `$HERMES_HOME/workspace`.
- A restart (`systemctl restart docker-hermes-<name>`) preserves `$HOME`; a
  **recreate** (config/mount/image change) does not. When in doubt, assume recreate.
- Tool configs the agent relies on (e.g. `~/.config/...`) live in the ephemeral
  `$HOME` unless you seed/symlink them into `$HERMES_HOME`.

## UID alignment (why writes used to fail)

The official `nousresearch/hermes-agent` image runs its `hermes` user as **uid
10000**. The container writes `$HERMES_HOME` as that uid. If the host `stateDir`
is owned by a *different* uid (e.g. an auto-allocated NixOS system user), every
write fails with `PermissionError` — pairing, kanban, sessions, the lot.

`mkHermesAgent` fixes this generically:

- `containerUid` / `containerGid` (default `10000`, from `lib/constants.nix`) —
  state dirs are owned by this numeric uid via `systemd.tmpfiles`.
- A `hermes-<name>-heal-uid` oneshot `chown -R`s `stateDir` (plus any
  `healDataDirs`) to that uid **before the container starts**, so rebuilds *heal*
  ownership instead of breaking it.

```nix
healDataDirs = [ "/srv/agent-data/<name>" ];   # extra mounted data slices to keep aligned
```

## `config.yaml` is read-only by design

When you pass `settings`, they are serialized to a `config.yaml` mounted
**read-only** at `$HERMES_HOME/config.yaml`. Config is declarative: edit
`my-agents.nix` and rebuild.

> The dashboard's **"save config" button returns a 500** against this read-only
> file. That is expected — don't edit config via the dashboard. (If you truly
> need dashboard-editable config, drop `settings` and manage `config.yaml`
> inside the state dir yourself; you lose declarative reproducibility.)

## Secrets never reach the agent terminal

Hermes **strips secret env vars from the agent's terminal tool** by design, so an
agent cannot read `GH_TOKEN` and push on its own. The generic pattern:

- The agent **commits** locally (set its identity with `gitIdentity`).
- A trusted **host** unit **pushes**, holding the token the agent never sees.

```nix
gitIdentity = { name = "<name>-bot"; email = "<name>@example.com"; };
gitAutoPush = { tokenEnvFile = "/run/agenix/hermes-<name>-env"; };  # GH_TOKEN read here
```

`gitAutoPush` finds git repos under `<stateDir>/workspace`, and pushes GitHub
`https` remotes on a timer using an `x-access-token` credential helper. Optional
fields: `reposRoot`, `tokenEnvVar` (default `GH_TOKEN`), `interval` (default
`2min`).

## Fail-loud provider preflight

A wrong/absent `model.base_url` or a missing `*_API_KEY` produces an HTTP `401`
that Hermes surfaces *downstream* as "worker exited without completing —
protocol violation" or "agent crashed" — easy to misdiagnose as a model/agent
bug. `providerHealthcheck` turns it into an obvious boot-time error:

```nix
providerHealthcheck = {
  url       = "https://api.example.com/v1";   # the model.base_url
  model     = "my-model";
  apiKeyEnv = "MY_API_KEY";                    # value stays in the container
};
```

At boot it POSTs a 1-token completion and logs
`[provider-healthcheck] <name>: OK` or `... FAIL HTTP <code> ... — check
model.base_url and <APIKEYENV>` to the journal. Non-fatal (won't block the
container) but loud.

> Reminder for OpenAI-compatible providers: set `model.base_url` **explicitly**.
> A subtly wrong path (e.g. `/v1` vs `/go/v1`) with an otherwise-valid key is the
> classic silent 401.

## Exposing dashboards & agent-built apps on the tailnet

Containers use host networking, so a service bound to the host's `127.0.0.1:<port>`
is reachable by host-level `tailscale serve` (TLS, tailnet-only, no firewall port).

```nix
dashboard = { port = 9219; tailnetPort = 9119; };   # hermes dashboard → https://<host>:9119

services.knowledge-base = {                          # any agent-built web app, durable
  startCommand = "cd $HERMES_HOME/workspace/kb && exec ./.venv/bin/python app.py";
  port = 9191; tailnetPort = 9122;
};
```

> **Host-networking port gotcha.** Under host networking the in-container bind
> port and the external `tailscale serve` port must **differ**, or `serve`'s
> listener collides with the app's `0.0.0.0` bind (`[Errno 98]`). Convention:
> internal port = external + 100.

`dashboard`/`services` run as auto-restarting host units (a foreground
`docker exec` under `Type=simple`): when the container restarts they die and are
restarted once it's back, so the exposure is durable across reboots and recreates.

## Quick reference

| Argument | Tier | What it does |
|---|---|---|
| `containerUid` / `containerGid` | fix | Own state to the image's uid (default 10000) |
| `healDataDirs` | fix | Chown extra data dirs to the container uid each boot |
| `providerHealthcheck` | fix | Boot-time fail-loud auth/endpoint check |
| `gitIdentity` | build-block | Git identity inside the container, re-applied each boot |
| `gitAutoPush` | build-block | Host-side secret-safe push on a timer |
| `dashboard` | build-block | Launch + (optionally) tailnet-publish the dashboard |
| `services` | build-block | Durable agent-built web apps, optional tailnet publish |

All default to off; existing agents are unaffected.
