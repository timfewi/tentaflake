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
- **Ownership:** environment endpoints see only the caller's environments;
  another user's environment answers `404`.

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
| `POST` | `/api/v1/environments` | Create and start one `{ name, networkMode? }` (`model-only`, the default, or `open`) | session, CSRF, Origin |
| `GET` | `/api/v1/environments/{id}` | One environment | session |
| `POST` | `/api/v1/environments/{id}/start` | Start a stopped environment | session, CSRF, Origin |
| `POST` | `/api/v1/environments/{id}/stop` | Stop a running environment | session, CSRF, Origin |
| `DELETE` | `/api/v1/environments/{id}` | Remove container, workspace volume and record | session, CSRF, Origin |
