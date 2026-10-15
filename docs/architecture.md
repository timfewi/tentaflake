# Tentaflake 0.5 architecture

Tentaflake turns a rented VPS into a private agent cloud. The user boots the
Tentaflake image on a VPS provider that supports custom images, opens the web
UI, and creates isolated agent environments: an Ubuntu container in a gVisor
sandbox with an agent inside, the user's own model API keys, MCP servers and
tools. The UI is the control plane. The user brings the VPS and the keys;
Tentaflake brings the environment and the simplicity.

> [!NOTE]
> 0.5 is being built. This page describes the target. The
> [status table](#status) says what exists in the repository today.

## Model

| Term | Meaning in 0.5 |
|---|---|
| Runtime image | Exactly one: Ubuntu 26.04 LTS, a pinned [OpenCode](https://github.com/anomalyco/opencode) and standard tools (git, curl, Python, Node). |
| Environment | One running container of that image in gVisor, with a persistent workspace volume, a network mode and a generated `opencode.json`. |
| Session | An OpenCode session inside an environment: chat, tool calls, approvals, files, terminal. |

OpenCode is the single runtime in 0.5 because it runs headless with an HTTP
API and event stream (`opencode serve`), works with any provider through a
configurable base URL and key, supports local and remote MCP servers, and has
per-tool permissions (`allow`, `ask`, `deny`). Tentaflake builds no agent of
its own. Other runtimes and user-built images come after 0.5.

The generated OpenCode configuration turns off session sharing and automatic
updates. The OpenCode server inside an environment is reachable only from the
host and uses a random per-environment password.

## Components

```text
browser ── HTTPS (Caddy, ACME) ── control service (Bun/TypeScript, SQLite)
                                   ├─ serves the UI (ui/)
                                   ├─ Docker + runsc: create/start/stop environments
                                   ├─ proxies OpenCode sessions, events, approvals, terminal, files
                                   └─ keys (encrypted), routes ─→ broker: forwards model calls, budgets
environment container (gVisor) ── opencode serve ── model calls → broker only
```

- **Host:** NixOS modules in `modules/`, Docker with the gVisor `runsc`
  runtime, the `balanced` security profile.
- **Control service:** TypeScript on Bun, sharing types with the UI and using
  OpenCode's TypeScript SDK. State lives in one SQLite file. It talks to Docker
  and is therefore privileged: it stays small, listens on loopback behind
  Caddy, and never hands the Docker socket to an environment.
- **UI:** in `ui/` of this repository. It uses only the documented, versioned
  HTTP API of the control service, so other UIs can use the same API.
- **Broker:** `crates/tentaflake-broker`. The control service stores the
  users' API keys encrypted on the host; the broker forwards model calls with
  them. Environments get a broker URL and a per-environment token, never a
  provider key.
- **Runtime image:** built from a Dockerfile `FROM ubuntu@sha256:…` with apt
  from a dated snapshot and the OpenCode binary pinned by checksum, referenced
  by digest from the host configuration. The reviewed Ubuntu pin lives in
  `images/catalog.json`.

## Runtime image

`images/runtime/Dockerfile` builds it. Everything it depends on is pinned in
that file: the base is the `ubuntu-26.04` digest from `images/catalog.json`
(never a tag), apt reads a dated Ubuntu snapshot, and OpenCode 2 is downloaded
as its npm platform package by exact version and checked against a SHA-256. A
test in `just fast` keeps the `FROM` line equal to the catalog pin. The
container runs as user `agent` (UID 1000) with `/workspace` as its working
directory and refuses to start without `OPENCODE_SERVER_PASSWORD`; it then
serves `opencode serve` on port 4096.

`images/runtime/opencode.json` is the base OpenCode configuration (found
through `OPENCODE_CONFIG`): sharing and updates off, `shell` and `webfetch` on
`ask`, and the model base URL and key read from `TENTAFLAKE_MODEL_BASE_URL`
and `TENTAFLAKE_MODEL_API_KEY`. The generated per-environment configuration
layers on top of it.

`scripts/runtime-smoke.sh` (`just runtime-smoke`) builds the image, runs it
with the isolation an environment gets (gVisor `runsc` when Docker has it,
dropped capabilities, read-only root, memory and PID limits) and checks the
HTTP API: password required, a session can be created, the effective
configuration matches. It makes no model call. It needs Docker, so CI runs it
only when `images/runtime/`, the script, `control/` or the broker changes
(`runtime-smoke.yml`), followed by the control service's tests against the
image it built and the broker (`just control-integration`): the environment
lifecycle, and model calls from `model only` and `open internet` environments
through the broker to a fake provider, which receives the provider key while
the containers never hold it; a `model only` environment reaches nothing else.

## Control service

`control/` is TypeScript on Bun using its built-ins (`Bun.serve`, `bun:sqlite`,
`Bun.password`) and no runtime dependencies. `bun run control/src/main.ts
<config.json>` starts it with one JSON file like `control/config.example.json`:
listen address (loopback by default), the exact browser origin, the paths of
the database, host key and setup token, the Docker socket, the runtime image
and the environment limits. The file holds no secrets. The host key file holds
32 random bytes (mode `0600`) that encrypt the TOTP secrets and OpenCode
passwords in the database.

One SQLite file with numbered migrations holds users (role `admin` or
`member`, Argon2id password hash, TOTP secret), sessions (hashed token, CSRF
token, expiry), model keys (provider, label, encrypted secret), environments
and their MCP servers. Every environment and key has an owner, and every
query for one is scoped to it. The API and its rules are in [api.md](api.md).

An environment is a container of the runtime image, created through the Docker
Engine API on its Unix socket: `runsc`, user 1000, all capabilities dropped,
`no-new-privileges`, a read-only root with tmpfs for `/tmp` and the home
directory, the configured memory, CPU and PID limits, a named workspace volume
on `/workspace` and the label `io.tentaflake.environment=<id>`. It gets a random
`OPENCODE_SERVER_PASSWORD` and the generated OpenCode configuration as
`OPENCODE_CONFIG_CONTENT`, which OpenCode ranks above an `opencode.json` in the
workspace. The network mode picks one of two networks the service owns:
`tentaflake-model-only` (internal, no route out) or `tentaflake-open`. A
record exists while Docker may hold resources for the environment, and a
failed Docker call is an error, never a silent success.

An environment uses one of its owner's keys and a list of allowed models. It
gets a random broker token, and the container gets the broker URL and that
token as `TENTAFLAKE_MODEL_BASE_URL` and `TENTAFLAKE_MODEL_API_KEY`. The
generated configuration names the AI SDK package of the key's provider
(`@ai-sdk/anthropic`, `@ai-sdk/openai`, or `@ai-sdk/openai-compatible` for
OpenRouter and compatible APIs), the allowed models (the first is the
default) and the environment's MCP servers; it never contains a provider key.
MCP secrets reach it only when the owner assigns them. Changing the MCP
servers replaces the container, since its configuration is part of the
container's environment; the workspace volume stays.

## Broker

`tentaflake-broker --config broker.json` serves every environment from one
process. Its configuration holds the listen address, the paths of the routes
file, audit log and budget state, and the limits (request, header and
response sizes, timeouts, concurrency, request rate, streaming deadlines).
The control service owns the routes: on every change of an environment or
key it rewrites, in its broker directory (a tmpfs runtime directory), one
credential file per environment with the decrypted provider key (mode
`0600`), then `routes.json`, then removes stale credential files. It does so
in the database transaction of the change, which rolls back when the files
cannot be written, and rebuilds them at startup. A route maps the SHA-256 of an environment token to
its API family (`openai` or `anthropic`), upstream base URL, credential file,
models, completion-token limit and daily budgets. Each request reads the
routes file and reparses it when its content changed, so a new route works on
the next request, a removed one stops working, and requests in flight finish
with the route they started with. An unreadable or invalid file refuses every
request.

The broker listens on the gateway address of `tentaflake-model-only`: a host
address that `model only` environments reach although their network has no
route out, and that `open internet` environments reach through their own
gateway. It accepts the paths and headers OpenCode 2.0.29 sends (observed
against a recording server): `POST /v1/chat/completions` (OpenRouter and
compatible APIs) and `POST /v1/responses` (OpenAI) with the token as a bearer
token, and `POST /v1/messages` (Anthropic) with the token as `x-api-key`,
forwarding `anthropic-version` and `anthropic-beta`. Each request is
admitted only for an allowed model, without provider-side tools or
extensions, within the route's request rate and daily request, token and cost
budgets (input counted conservatively as request bytes, output as the
completion-token limit). The provider key replaces the token, streaming
responses are relayed event by event with their deadlines, nothing is
replayed, and the audit log records routes, models and usage but no prompt,
response, token or key. Upstreams must be HTTPS on public addresses; that is
checked per request, so a route that breaks it fails alone.

## Users and access

- Public HTTPS on the VPS domain with password + TOTP login. Without an own
  domain, a wildcard DNS name derived from the IP can serve as the first
  domain. Tailscale stays an optional extra.
- 0.5 ships with one admin, but the data model is multi-user from the start:
  users are records with a role; environments, keys and MCP definitions belong
  to an owner; every API call is authorised against the calling user. Teams,
  invitations and passkeys come later without rewriting ownership.

## Security boundaries

- **Keys** are entered in the UI, stored encrypted on the host, and used only
  by the broker. No provider key enters a container.
- **MCP servers** are defined per environment and written into
  `opencode.json`. In 0.5 a secret for an MCP server reaches an environment
  only if its owner explicitly assigns it, with a warning that the agent can
  read it. Host-side MCP proxies that keep such secrets out of containers come
  later.
- **External effects** use OpenCode's permissions per tool: `allow`, `ask`
  (shown as an approval in the UI) or `deny`.
- **Network per environment:** `model only` (default) or `open internet`.
  Allowlists come later.
- **Isolation:** every environment runs in gVisor with dropped capabilities,
  a non-root user and CPU, memory, process and disk limits. The `strict`
  separate-kernel profile is not implemented; most VPS hosts offer no nested
  virtualisation.

## Lifecycle

- Create → running → idle-stopped after N minutes without an open session or
  running prompt (default 15) → started again on demand with the workspace
  intact → deleted by its owner.
- Per environment: CPU, memory and disk limits and a token budget enforced by
  the broker.
- Ephemeral one-task environments, schedules and webhooks come later.

## Definition of done for 0.5

On a fresh VPS booted from the Tentaflake image:

1. Open `https://<domain>`; the setup wizard creates the admin (password + TOTP).
2. Add one model API key.
3. Create an environment (Ubuntu + OpenCode, `model only`).
4. In the chat, ask for a hello-world script and its output. The answer, the
   file and the terminal output are visible; an `ask` permission appears as an
   approval in the UI.
5. After the idle timeout the environment is stopped; starting it again keeps
   the workspace. Deleting it removes container and workspace.
6. Update the host to a new revision with rollback.

The image is provider-agnostic. It is tested on real VPS providers once this
works, and the README then lists where it was verified.

## Status

| Part | State |
|---|---|
| Host modules, Docker with `runsc`, security profile | In the repository |
| Broker crate and package | In the repository; one process serves every environment by token (multi-route), tested with a fake provider under `runsc` in CI when it changes |
| Base-image catalog (Ubuntu 26.04 pin) | In the repository |
| Runtime image (Ubuntu + OpenCode) | In the repository (smoke-tested in CI when the image changes) |
| Control service and its API: users, setup, login, environment lifecycle, model keys, MCP definitions | In the repository; no session proxy yet (tested under `runsc` in CI when it changes) |
| Host wiring of the broker (service, firewall) | Planned |
| UI in `ui/` | Planned |
| Setup wizard, HTTPS | Planned |
| VPS image and updates | Planned |

## Out of scope for 0.5

Company and employee features, business connectors, multiple runtimes,
user-built images, schedules, teams, and the separate-kernel `strict`
profile. The pre-0.5 features (declared Hermes/ZeroClaw agents, admission
ledger, Research service, disposable worker, operator CLI) remain at tag
`v0.4-final` and are not maintained.
