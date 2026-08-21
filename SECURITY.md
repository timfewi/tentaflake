# Security policy

## Supported versions

Security updates apply to the latest commit on `main`.

## Report a vulnerability

Do not open a public issue. Use the repository's GitHub Security Advisory
workflow and include the affected component, reproduction, impact, and any
suggested mitigation. The project aims to acknowledge reports within 72 hours.

## Scope

Relevant areas include:

| Area | Examples |
|---|---|
| Secrets | Nix-store, log, environment, or Git leakage |
| Containers | Breakout, mounts, users, capabilities |
| Agent policy | Files, tools, network, repository access |
| Installer | Target resolution and disk-wipe safety |
| Rust CLI | Argument handling and privileged commands |
| Observability | Dashboard exposure and log sensitivity |
| Falco | eBPF privileges, rules, event handling |
| Nix | Supply chain, evaluation, and pinning |

## Deployment practices

- Never commit environment files or literal secret values.
- Use runtime secret files such as agenix outputs.
- Enable only the tools, mounts, and network access an agent needs.
- Treat Docker group membership as root-equivalent.
- Keep Grafana, Prometheus, Loki, and Alloy loopback-only unless an
  authenticated private publishing path is deliberately configured.
- Treat Falco as detection, not prevention or container isolation.
- Keep installed agent hosts on the `balanced` profile. `dev` is a deliberate
  compatibility path, and `strict` is not implemented.
- Install a restrictive tailnet grants/SSH policy for `tag:agent-host` before
  relying on Tailscale; private transport does not replace authorization.
- Do not work around balanced `network=none` by injecting provider credentials.
  Use the per-agent broker path; the agent receives only a virtual key and a
  network with host/FORWARD firewall restrictions.

The detailed security model is split between the
[threat model](docs/15-threat-model.md) and
[security profiles and broker migration](docs/10-security-profiles.md).

## Incident response

1. Stop the exact affected unit after resolving its backend and name:

   ```bash
   tentaflake stop <agent>
   ```

2. Revoke provider and repository credentials. Stopping a container does not
   invalidate an already leaked key.
3. Preserve relevant journald logs, Falco output when enabled, and a read-only
   copy of the agent state directory before cleanup.
4. Inspect the declared mounts, tools, network policy, and recent changes.
5. Rotate runtime secrets and build the repaired configuration.
6. Activate only after reviewing the target and build result.

Tentaflake does not maintain a private SQLite audit database or custom web
console. Host and container-unit evidence is available through journald;
the optional Loki/Alloy profile can retain and query it.
