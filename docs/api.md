# Control service API

The control service (`control/`) serves a JSON API under `/api/v1`. The UI uses
only this API, and so can other clients. Request and response types live in
[`control/src/api.ts`](../control/src/api.ts).

## Rules

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
| `GET` | `/api/v1/keys` | `{ keys }`: the caller's model keys, without secrets | session |
| `POST` | `/api/v1/keys` | Add one `{ provider, label, secret, baseUrl? }`; `provider` is `anthropic`, `openai`, `openrouter` or `openai-compatible` | session, CSRF, Origin |
| `DELETE` | `/api/v1/keys/{id}` | Delete one; environments using it lose model access | session, CSRF, Origin |
