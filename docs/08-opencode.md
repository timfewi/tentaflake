# OpenCode Agents

`mkOpenCodeAgent` runs [OpenCode](https://opencode.ai) as a first-class
tentaflake agent runtime, alongside Hermes and ZeroClaw. OpenCode is a coding
agent that ships a headless HTTP server (`opencode serve`) exposing a documented
OpenAPI 3.1 interface — which makes it the cleanest runtime to drive from an
external orchestrator such as **n8n**, CI, or cron.

## Why OpenCode for orchestrated work

Unlike the Hermes/ZeroClaw gateways, OpenCode's task API is public and stable:

```
POST http://127.0.0.1:<hostPort>/session
  -> { "id": "<sessionID>", ... }

POST http://127.0.0.1:<hostPort>/session/<sessionID>/message
  { "parts": [ { "type": "text", "text": "<the task>" } ] }
  -> { "info": { ... }, "parts": [ ... ] }   # the agent's result
```

`GET /global/health` returns `{ healthy: true, version }` for readiness checks.
Full spec is served at `http://127.0.0.1:<hostPort>/doc`.

Each agent keeps the tentaflake isolation contract: its own container, uid, a
`0700` state dir (`/var/lib/opencode-<name>`), a `/workspace` project dir, and
secrets injected via `--env-file`.

## Defining agents

In `my-agents.nix` (see `my-agents.nix.example`):

```nix
{ mkOpenCodeAgent }:
let
  opencodeAgents = [
    {
      name = "code";
      hostPort = 4096;                 # http://127.0.0.1:4096
      envFile = "/run/secrets/opencode-code.env";
      settings = {
        model = "anthropic/claude-haiku-4.5";
        provider.anthropic.options.baseURL = "http://proxy.host:4000";
      };
    }
    { name = "ops"; hostPort = 4097; envFile = "/run/secrets/opencode-ops.env"; }
  ];
in
map mkOpenCodeAgent opencodeAgents
```

### Parameters

| Parameter | Type | Default | Description |
|---|---|---|---|
| `name` | str | **required** | Agent name (`opencode-<name>` container + state dir) |
| `hostPort` | int | **required** | Host loopback port forwarded to the gateway |
| `image` | str | `ghcr.io/anomalyco/opencode:latest` | OCI image |
| `stateDir` | str | `/var/lib/opencode-<name>` | Host state dir (XDG config/data) |
| `workspaceDir` | str | `<stateDir>/workspace` | Project dir mounted at `/workspace` |
| `seedDir` | path? | `null` | Files copied into the workspace on first boot (no-clobber) |
| `gatewayPort` | int | `4096` | In-container `opencode serve` port |
| `servePort` | int? | `null` | Tailnet HTTPS port via `tailscale serve` (must differ from `hostPort`) |
| `envFile` | path? | `null` | Plaintext env file (`--env-file`) |
| `agenixFile` | path? | `null` | agenix-decrypted env file (`--env-file`) |
| `authFile` | path? | `null` | Provider `auth.json` mounted read-only into the data dir |
| `settings` | JSON attrset | `{}` | `opencode.json` config, mounted read-only |
| `containerUid`/`containerGid` | int | `65534` | uid/gid the container runs as; owns state |
| `autoStart` | bool | `true` | Auto-start with the system |
| `pidsLimit` | int? | `512` | `--pids-limit` (fork-bomb ceiling); `null` disables |
| `extraEnvironment` | attrset | `{}` | Extra env vars |
| `extraVolumes` | list of str | `[]` | Extra Docker volumes |

## Credentials — two patterns

OpenCode needs LLM credentials to do work. Choose one:

### 1. Local LLM proxy (recommended)

Point each agent's `settings` at a proxy (LiteLLM, or any OpenAI/Anthropic-
compatible endpoint) and put the proxy key in the env file. This is
**host-portable** (works on the deployed NixOS host, not just a dev laptop),
keeps agents isolated, and centralizes the one real upstream key.

```nix
settings = {
  model = "anthropic/claude-haiku-4.5";
  provider.anthropic.options.baseURL = "http://proxy.host:4000";
};
```

`opencode-code.env`:
```
OPENCODE_SERVER_PASSWORD=<pick-a-strong-password>
ANTHROPIC_API_KEY=<proxy-key>
```

### 2. Reuse an existing `auth.json`

If you already have OpenCode credentials, set `authFile` to a provider
`auth.json`; it is mounted read-only at the container's
`~/.local/share/opencode/auth.json`. The file **must exist on the target host**
(a laptop's `~/.local/share/opencode/auth.json` does not transfer to a server),
and all agents pointed at it share that one credential — so prefer the proxy
pattern for anything beyond local experimentation.

> **Never** mount the whole OpenCode data dir: `opencode.db` can grow to many
> GB. Mount only `auth.json`.

## Server authentication

Always set `OPENCODE_SERVER_PASSWORD` in the env file — it protects the HTTP
server with basic auth (username `opencode`, or `OPENCODE_SERVER_USERNAME`).
n8n's HTTP Request node sends it via Basic Auth credentials.

## Driving it from n8n

1. **Create a session** — HTTP Request node:
   - `POST http://<host>:<hostPort>/session`
   - Basic Auth: `opencode` / `$OPENCODE_SERVER_PASSWORD`
   - capture `{{ $json.id }}`
2. **Submit the task** — HTTP Request node:
   - `POST http://<host>:<hostPort>/session/{{ $json.id }}/message`
   - JSON body: `{ "parts": [ { "type": "text", "text": "{{ $json.task }}" } ] }`
   - the response contains the agent's result parts.

For fire-and-forget, use `POST /session/<id>/prompt_async` (returns `204`) and
poll `GET /session/<id>/message` for the result.

## Operating it

The `tentaflake` CLI discovers OpenCode agents like any other runtime:

```bash
tentaflake                 # lists opencode-<name> with the others
tentaflake logs code
tentaflake restart code
tentaflake shell code
```
