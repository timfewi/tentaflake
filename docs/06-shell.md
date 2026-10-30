# Operator shell and Rust CLI

`modules/shell.nix` installs a small Rust CLI and optional operator comforts.
It does not contain the CLI implementation; the workspace source lives in
`crates/tentaflake-cli`.

## Generated inputs

Nix writes two non-secret files:

- `/etc/tentaflake/cli.conf` selects the backend, host, flake directory, and
  agent inventory path.
- `/etc/tentaflake/agents.tsv` uses explicit `tentaflake.agentInstances`
  metadata for adapters and retains a fallback for unmanaged OCI containers.
  Its six fields are adapter, instance name, container name, systemd unit,
  state directory and the instance's `runnable` flag. Stopped scaffolds appear
  with `false` and no OCI container; `stats`, `shell` and `exec` skip or refuse
  them by that flag, not by runtime name.

Do not hand-edit them. Change the Nix configuration and rebuild explicitly.

## Commands

```text
tentaflake help
tentaflake runtimes [--json]
tentaflake agent template <preset> <name>
tentaflake agent validate|plan|import <file> [--json] [--hide]
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

JSON status entries include a canonical container `id` and `runnable` flag.
Use that ID for automation: exact container IDs take precedence over display
names, and ambiguous short names are rejected. `--hide` redacts both the name
and ID. The optional [local host API](19-web-ui.md) uses canonical IDs only and
can expose this inventory to an independently installed web interface.

`runtimes` lists the pinned preset catalog and its support/evidence levels;
`--json` emits the complete schema-v1 catalog. It performs no host reads or
image downloads. See [generic workloads](agent-adapters.md#generic-isolated-workload).

`help`, `--help`, and `-h` display usage even when the generated host
configuration or inventory is missing or invalid. `agent template` is also
configuration-free; other management commands require the generated inputs.
See [installed-host onboarding](08-agent-cli.md#add-an-agent-on-an-installed-host)
for generic image/command templates, additive imports and explicit host updates.

`health` adds host load to the same diagnosis used by `doctor`. Both check
failed systemd units, root-disk usage of at least 90%, and failed or unknown
agent states. They return `0` for a healthy diagnosis, `1` for detected
problems, and `2` when systemd or disk evidence cannot be obtained. An
intentionally stopped agent is valid. JSON reports include `failed_agents`
and `unknown_agents`; `--hide` redacts names in both text and JSON. A
successful diagnosis is host evidence only; it does not prove external
provider access.

`doctor --security` compares the generated desired-state manifest with narrow,
bounded live probes. Stable `TFSEC-*` findings include remediation; critical/high
findings return non-zero. Missing/legacy evidence remains unknown, never green,
and raw OCI environments never enter reports. See
[posture checks](10-security-profiles.md#posture-check) for exact fields, probe
limits and evidence gaps. Provider access, native-tool mediation and remote
management grants are not implied by a healthy declaration.

`backup` exclusively creates a mode-`0600`, caller-owned archive of one state
directory in the current directory before starting privileged reads. Existing
files and symlinks at the timestamped archive name are refused; retry in a later
second or use another directory. Privileged `tar` writes to the already-open
file descriptor, never reopening or changing ownership of an output pathname.
The source remains the state directory in the trusted host inventory; this
command does not back up secret-provider identities.

On failure, a private empty or partial archive is retained and reported. Inspect
and remove it manually before retrying with the same name. The CLI deliberately
does not unlink the output pathname: another process could have replaced it
while the backup was running. Use an operator-controlled current directory so
other users cannot rename or replace the archive; descriptor-based writes do
not prevent such directory-entry changes.

`rebuild` and the apply step offered by `update` activate NixOS. Invoke them
only when the target is resolved and activation is intended.


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
