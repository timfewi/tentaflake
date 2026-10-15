# Control service API

The control service (`control/`) serves a JSON API under `/api/v1`. The UI uses
only this API, and so can other clients. Request and response types live in
[`control/src/api.ts`](../control/src/api.ts).

## Rules

- **UI:** when the service is configured with the built UI (`uiDirectory`),
  every `GET` and `HEAD` outside `/api/` is answered from its files (`404` for
  any other path), with a
  Content-Security-Policy that allows scripts from the page's own origin only
  (`frame-ancestors 'none'`, `nosniff`, no referrer). Nothing else changes: the
  UI is a client of the endpoints below.
- **Session:** login sets the cookie `__Host-tentaflake` (`Secure`, `HttpOnly`,
  `SameSite=Strict`, `Path=/`). A session ends after the configured idle time
  (default 60 minutes), at its absolute lifetime (default 12 hours) or on
  logout.
- **Mutations** (every method except `GET`) need an `Origin` header equal to the
  configured origin. With a session they also need the session's CSRF token in
  `X-CSRF-Token`. Request bodies are `application/json`.
- **Login** takes username, password and the current TOTP code; each code works
  once. Five failures for a username block it for 15 minutes (`429`).
- **Setup** works only while no user exists, with the token from the configured
  setup token file.
- **Ownership:** environment and key endpoints see only the caller's
  environments and keys; another user's answers `404`. An environment can use
  only one of its owner's keys.
- **Keys:** the secret goes in once, with `POST /api/v1/keys`, and is never
  returned or logged. It is stored encrypted with the host key; the broker
  receives it in a private credential file. `baseUrl` is required for
  `openai-compatible` and refused otherwise; the broker forwards only to HTTPS
  upstreams on public addresses.
- **Model access:** an environment calls the broker with its own token
  (`TENTAFLAKE_MODEL_BASE_URL`, `TENTAFLAKE_MODEL_API_KEY` in the container);
  the broker allows the environment's models within its daily token budget
  and adds the provider key. Deleting the key ends the environment's model
  access; if the broker's files cannot be updated, nothing is deleted (`500`).
- **MCP servers:** a `local` server runs `command` with `environment`; a
  `remote` one is reached at `url` with `headers`. Both are returned as
  stored. `secrets` are environment variables (local) or headers (remote)
  that are stored encrypted and listed by name only; the response that
  assigns them carries a `warning`: the agent can read them. A `PUT` replaces
  the whole server, secrets included. Every change recreates the container
  with the new configuration (workspace kept; a stopped environment stays
  stopped); when the old container cannot be removed, the change is not
  applied (`502`).

- **Agent:** the `sessions`, `events`, `permissions`, `files` and `terminal`
  endpoints of an environment reach the OpenCode server inside it, for its
  owner, while it runs (`409` otherwise). The service calls OpenCode with the
  environment's password and passes on nothing of the browser's request.
  OpenCode's answers come from inside the environment and are checked; a
  missing or unexpected one answers `502` with `opencode`.
- **Events:** a stream of server-sent events whose `data` is one `AgentEvent`:
  sessions created, renamed or deleted, a prompt's start and end
  (`session.execution.*`), the reply's text (`session.text.delta`,
  `session.text.ended`), tool calls (`session.tool.*`) and approvals
  (`permission.asked`, `permission.replied`). Other or invalid OpenCode events
  are dropped, and so is one over 1 MiB. Comments keep the connection open.
  When the environment stops, is deleted or has its container replaced (a
  configuration change), `environment.stopped` is sent and the stream ends;
  the same closes a terminal with `1000` and "the environment stopped".
  Nothing is replayed: after a reconnect, list sessions and permissions.
- **Approvals:** a tool call that OpenCode's permissions set to `ask` waits in
  `permissions` until its owner replies `once` (this call), `always` (calls
  like it from now on; OpenCode saves the rule) or `reject`.
- **Files:** `path` is relative to `/workspace`, without empty, `.` or `..`
  segments (`400`). A read returns at most 1 MiB (`413 too_large` above) as
  `application/octet-stream`. OpenCode refuses to read a file whose real path
  is outside the workspace; a listing follows a symbolic link to a directory.
- **Terminal:** a WebSocket to a login shell in `/workspace`, as the
  environment's user (UID 1000), from OpenCode's PTY API. Its handshake needs
  the configured `Origin`. The browser sends JSON text frames
  `{ "type": "input", "data": "…" }` or `{ "type": "resize", "rows": r, "cols": c }`
  (1-1000); anything else closes it with `1008`. Output arrives as text
  frames. A frame holds at most 64 KiB; a larger one from the browser drops
  the connection. It closes with `1000` and a reason when the shell exits or
  the environment stops, with `1011` when OpenCode sends a frame over 256 KiB,
  and the shell ends when the connection closes.
- **Idle stop:** prompts, approvals, file reads, relayed events and terminal
  input are activity (`lastActivityAt`, to within 10 seconds). An environment
  without activity for `environmentIdleMinutes` (default 15) is stopped,
  unless a terminal is open or a prompt runs. Starting it again keeps its
  workspace and its sessions.

Errors answer `{ "error": "<code>", "message": "…" }`. A failed Docker call
answers `502`; the environment's recorded state stays accurate.

## Endpoints

| Method | Path | Purpose | Auth |
|---|---|---|---|
| `GET` | `/api/v1/setup` | `{ needed }`: whether the first admin can still be created | none |
| `POST` | `/api/v1/setup` | Create the first admin `{ token, username, password }`; returns the user and, once, the TOTP `otpauthUri` | setup token, Origin |
| `POST` | `/api/v1/session` | Log in `{ username, password, code }`; sets the cookie, returns the user and CSRF token | Origin |
| `GET` | `/api/v1/session` | The current user and CSRF token | session |
| `DELETE` | `/api/v1/session` | Log out | session, CSRF, Origin |
| `GET` | `/api/v1/environments` | The caller's environments | session |
| `POST` | `/api/v1/environments` | Create and start one `{ name, networkMode?, keyId, models }`; `networkMode` is `model-only` (default) or `open`, the first model is OpenCode's default | session, CSRF, Origin |
| `GET` | `/api/v1/environments/{id}` | One environment | session |
| `POST` | `/api/v1/environments/{id}/start` | Start the container, also one that exited on its own | session, CSRF, Origin |
| `POST` | `/api/v1/environments/{id}/stop` | Stop the container | session, CSRF, Origin |
| `DELETE` | `/api/v1/environments/{id}` | Remove container, workspace volume and record | session, CSRF, Origin |
| `GET` | `/api/v1/environments/{id}/mcp` | `{ servers }`: the environment's MCP servers | session |
| `PUT` | `/api/v1/environments/{id}/mcp/{name}` | Create or replace one: `{ type: "local", command, environment?, secrets? }` or `{ type: "remote", url, headers?, secrets? }`; returns `{ server, warning? }` | session, CSRF, Origin |
| `DELETE` | `/api/v1/environments/{id}/mcp/{name}` | Remove one | session, CSRF, Origin |
| `GET` | `/api/v1/environments/{id}/sessions` | `{ sessions }`: the newest 50 OpenCode sessions, each with `running` | session |
| `POST` | `/api/v1/environments/{id}/sessions` | Create one | session, CSRF, Origin |
| `POST` | `/api/v1/environments/{id}/sessions/{session}/prompt` | Send `{ text }` (up to 32,768 characters); `202 { messageId }`, the reply follows as events | session, CSRF, Origin |
| `POST` | `/api/v1/environments/{id}/sessions/{session}/abort` | Stop the running prompt; `{ aborted }` | session, CSRF, Origin |
| `GET` | `/api/v1/environments/{id}/events` | Server-sent `AgentEvent`s of all sessions | session |
| `GET` | `/api/v1/environments/{id}/permissions` | `{ permissions }`: tool calls waiting for approval | session |
| `POST` | `/api/v1/environments/{id}/sessions/{session}/permissions/{permission}` | Reply `{ reply }`: `once`, `always` or `reject` | session, CSRF, Origin |
| `GET` | `/api/v1/environments/{id}/files?path=` | `{ entries }` of a directory; without `path`, of `/workspace` | session |
| `GET` | `/api/v1/environments/{id}/files/content?path=` | A file's bytes | session |
| `GET` | `/api/v1/environments/{id}/terminal` | WebSocket: a shell in the environment | session, Origin |
| `GET` | `/api/v1/keys` | `{ keys }`: the caller's model keys, without secrets | session |
| `POST` | `/api/v1/keys` | Add one `{ provider, label, secret, baseUrl? }`; `provider` is `anthropic`, `openai`, `openrouter` or `openai-compatible` | session, CSRF, Origin |
| `DELETE` | `/api/v1/keys/{id}` | Delete one; environments using it lose model access | session, CSRF, Origin |
