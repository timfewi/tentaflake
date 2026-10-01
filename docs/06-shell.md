# Operator shell and Rust CLI

`modules/shell.nix` installs a small Rust CLI and optional operator comforts.
It does not contain the CLI implementation; the workspace source lives in
`crates/tentaflake-cli`.

## Generated inputs

Nix writes two non-secret files:

- `/etc/tentaflake/cli.conf` selects the backend, host, flake directory, and
  agent inventory path.
- `/etc/tentaflake/agents.tsv` is derived from declared OCI containers.

Do not hand-edit them. Change the Nix configuration and rebuild explicitly.

## Commands

```text
tentaflake help
tentaflake status [--json] [--hide]
tentaflake health [--json] [--hide]
tentaflake doctor [--json] [--hide]
tentaflake doctor --security [--json] [--hide]
tentaflake stats
tentaflake logs <agent>
tentaflake restart <agent>
tentaflake start <agent>
tentaflake stop <agent>
tentaflake shell <agent>
tentaflake exec <agent> -- <command>
tentaflake ps
tentaflake backup <agent>
tentaflake rebuild
tentaflake update
```

`status` is the default command. `--json` provides machine-readable status;
`--hide` redacts the host and agent names. `tentaflake-status` invokes the same
status renderer and is used by the login banner.

`help`, `--help`, and `-h` display usage even when the generated host
configuration or inventory is missing or invalid. Management commands still
require the generated inputs.

`health` adds host load to the same diagnosis used by `doctor`. Both check
failed systemd units, root-disk usage of at least 90%, and failed or unknown
agent states. They return `0` for a healthy diagnosis, `1` for detected
problems, and `2` when systemd or disk evidence cannot be obtained. An
intentionally stopped agent is valid. JSON reports include `failed_agents`
and `unknown_agents`; `--hide` redacts names in both text and JSON. A
successful diagnosis is host evidence only; it does not prove external
provider access.

`doctor --security` reads the Nix-generated desired-state security manifest.
It emits stable `TFSEC-*` findings with severity and remediation; critical or
high findings return non-zero. JSON output is CI-friendly. The desired-state check
covers the declared profile, capsule flags, mounts, images, credential files,
OpenSSH, AppArmor, Docker-group membership, backup declaration, workspace
quota, and missing broker. It also inspects root-disk pressure, the last
successful Restic timestamp, and live Tailscale Serve/Funnel JSON. If a live
command is unavailable or blocked, the result is an explicit warning rather
than green. A non-interactive backend inspect also compares a running Docker or
Podman container's privilege, user, root filesystem, runtime, network,
capabilities, AppArmor, security options, resource limits, and mounts with the
exact manifest values, including every bind mount, hardened tmpfs size, and
CPU/memory/PID/ulimit setting. Brokered networks are separately inspected for
the exact internal bridge. Each OCI inspection captures both output streams
and has a five-second deadline and an 8 MiB combined stdout/stderr limit;
raw Inspect environment data never reaches
the report. A stopped/missing container, incomplete backend schema, or
unavailable approved sudo path is reported as unknown. Configured broker
`/healthz` endpoints are probed directly: connection failures stay unknown,
while an explicit non-ready response is high severity because broker health
checks credentials and audit persistence. Complete controller-tool audit
coverage and cross-agent network denial remain separate runtime evidence.

`backup` creates a mode-`0600`, caller-owned archive of one state directory in
the current directory. It does not back up secret-provider identities.

`rebuild` and the apply step offered by `update` activate NixOS. Invoke them
only when the target is resolved and activation is intended.

The deprecated `hermes` shim remains for command-name compatibility. The old
`top`, `console`, and interactive agent wizard commands were removed with the
Auditd, SQLite, and custom web-console stack.

## Shell options

All options live below `tentaflake.shell`:

| Option | Default | Purpose |
|---|---:|---|
| `enable` | `true` | Shell module |
| `tentaflakeCli.enable` | `true` | Rust CLI |
| `motd.enable` | `true` | Login status |
| `tools.enable` | `true` | Curated terminal tools |
| `starship.enable` | `true` | Prompt |
| `zsh.enable` | `false` | Zsh and completion |
| `zoxide.enable` | `true` | Directory jumping |
| `lazygit.enable` | `false` | Git TUI |
| `tmux.enable` | `false` | Terminal multiplexer |

## Physical console

`tentaflake.modernConsole.enable` selects kmscon for Unicode-capable output.
Set it to `false` when hardware cannot use kmscon; `consoleFont` then controls
the legacy VT font. The installer disables font replacement to avoid display
reconfiguration on sensitive hardware.
