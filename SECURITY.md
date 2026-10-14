# Security policy

## Supported versions

Security updates apply to the latest commit on `main`. The pre-0.5 line
(`v0.4-final`, `legacy/0.4`) is no longer maintained.

## Report a vulnerability

Do not open a public issue. Use the repository's GitHub Security Advisory
workflow and include the affected component, reproduction, impact, and any
suggested mitigation. The project aims to acknowledge reports within 72 hours.

## Scope

| Area | Examples |
|---|---|
| Secrets | Nix-store, log, environment, or Git leakage of user API keys |
| Environments | gVisor sandbox breakout, mounts, users, capabilities |
| Agent policy | Files, tools, MCP servers, network, model access |
| Control plane | Authentication, authorisation, API input handling |
| Broker | Key handling, request forwarding, budgets |
| Nix | Supply chain, evaluation, and pinning |

## Deployment practices

- Never commit environment files or literal secret values.
- Treat Docker group membership as root-equivalent; never give an environment
  the Docker socket.
- Keep installed hosts on the `balanced` profile. `dev` is a deliberate
  compatibility path, and `strict` is not implemented.
- Environments never receive a provider API key; model calls go through the
  broker.

The security model is described in [the architecture](docs/architecture.md).
