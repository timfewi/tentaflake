---
name: handle-the-host
description: Inspect or maintain a deployed Tentaflake host through its authorized Tailscale SSH management path.
version: 1.1.0
---

# Handle the host

## Resolve and inspect

Use the operator-supplied host and account. Confirm the target before mutations;
a local Tailscale flag does not prove enrollment or remote grants. Read
[management policy](../../../docs/11-tailscale-management.md) and
[operations](../../../docs/07-operations.md) for the relevant procedure.

SSH/hostname preferences use `tailscale set`; `--advertise-tags` belongs to
`tailscale up`. NixOS supplies `--advertise-tags=tag:agent-host` through
`extraUpFlags` during auth-key enrollment; request it explicitly for manual
enrollment as described in the management guide.

```sh
tailscale ssh <admin>@<host>
```

For interactive sudo use `ssh -t <admin>@<host>` through the authorized tailnet
path. Preserve existing authentication and recovery access. On the target:

```sh
hostname
tailscale status
tentaflake status
tentaflake health
tentaflake doctor --security
```

Resolve container, unit and state path from `/etc/tentaflake/agents.tsv`;
backend and flake target come from `/etc/tentaflake/cli.conf`. Do not assume
Docker, `/etc/nixos` or a runtime prefix for a consumer deployment. Bound logs:

```sh
sudo journalctl -u <exact-unit> --since '1 hour ago' -n 100 --no-pager
```

`tentaflake logs <agent>` follows until interrupted. Never dump OCI environment
data or credentials. Diagnostic `--hide` redacts host/agent names; other logs
still need review before sharing.

An operator may instead choose the optional [web interface](../../../docs/19-web-ui.md).
Its local host API is disabled by default, with separate action/log permissions.
Use canonical container IDs, inspect current state after an unknown action,
and retry an uncertain HTTP response with the original request ID. Never treat
host readiness as native chat or provider acceptance, or assign the API socket
group to an agent. Nix remains authoritative for agent definitions.

## Perform the authorized operation

Prefer `tentaflake start|stop|restart <exact-container>`. `stop` also stops the
controller's loaded LLM/fetch brokers. Stopped scaffolds are valid inventory
entries; OpenClaw refuses a start.

For updates, review input/image changes, run checks and build the exact candidate
before activation. `tentaflake rebuild` activates the configured host;
`tentaflake update` changes its lockfile and offers activation. Rollback,
credential rotation, disk work and publishing services need authorization
covering that action and target. Reuse existing authorization without asking
again. Do not add passwordless sudo, Serve/Funnel or firewall openings as a
routine connection fix.

After a mutation, repeat health/posture and affected application checks.
Report results and unknown evidence. Management connectivity is separate from
Research VPN readiness and provider/vendor acceptance.
