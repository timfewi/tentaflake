# Security Policy

## Supported Versions

This project is currently in early development. Security updates are provided
for the latest commit on `main`.

| Version | Supported          |
| ------- | ------------------ |
| main    | :white_check_mark: |
| < v0.1  | :x:                |

## Reporting a Vulnerability

**Do not open a public issue for security vulnerabilities.**

Instead, report them privately:

1. **GitHub Security Advisories:** Go to the [Security tab](https://github.com/timfewi/tentaflake/security/advisories) and click "Report a vulnerability".

2. **What to include:**
   - Description of the vulnerability
   - Steps to reproduce
   - Affected components (Nix modules, Go daemon, installer, etc.)
   - Potential impact
   - Suggested fix (if any)

You will receive a response within **72 hours**. We will work with you to
understand, validate, and address the issue.

## Scope

Security concerns relevant to this project include:

| Area | Concern |
|------|---------|
| **Secrets handling** | API keys, tokens, env files — leakage through Nix store, logs, or Git |
| **Container isolation** | Docker container breakout, volume mounts, privilege escalation |
| **Agent sandboxing** | File access, network access, and tool restrictions per agent |
| **Installer** | Disk wiping safety, password handling, input validation |
| **Go daemon (hermes-auditd)** | SQLite injection, file descriptor leaks, resource exhaustion |
| **Nix evaluation** | Supply chain, IFD (import from derivation), secrets in store |

## Disclosure Policy

- Reporter will be acknowledged in the advisory (unless they request anonymity)
- Fix will be developed in a private fork
- Once fixed, a GitHub Security Advisory will be published
- CVE will be requested for critical vulnerabilities

## Best Practices for Users

When deploying this template:

1. **Never commit `.env` files** — use `secrets/` with agenix or external secret management
2. **Set `HERMES_SANDBOX=strict`** in agent config for maximum isolation
3. **Audit agent `toolsets`** — only enable what each agent needs
4. **Keep the template generic** — domain-specific config goes in your fork, not here

## Incident Response

Generic runbook for operators who suspect an agent is compromised (leaked
credentials, unexpected pushes, suspicious filesystem activity):

1. **Isolate the agent** — stop its container:

   ```bash
   sudo systemctl stop docker-hermes-<name>   # podman backend: podman-hermes-<name>
   tentaflake stop <name>                     # backend-aware equivalent
   ```

2. **Revoke the agent's provider API keys** at the provider — stopping the
   container does not stop a key that already leaked.

3. **Inspect the audit trail** — `hermes-auditd` logs filesystem activity for
   every watched agent to a SQLite database:

   ```bash
   sqlite3 /var/lib/hermes-audit/events.db \
     "SELECT timestamp, agent, op, file FROM events ORDER BY id DESC LIMIT 200;"
   ```

   Timestamps are stored in **UTC** — account for that when correlating with
   local logs.

4. **Preserve evidence** — copy the agent's `stateDir`
   (default `/var/lib/hermes-<name>`) somewhere safe *before* wiping or
   reseeding anything.

5. **Rotate secrets and rebuild** — re-encrypt affected secrets (see the
   rotation and recovery section of
   [`docs/04-agenix-secrets.md`](docs/04-agenix-secrets.md)), then
   `nixos-rebuild switch` to bring the agent back with fresh credentials.
