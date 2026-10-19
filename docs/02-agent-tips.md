# Agent management

Manage agents through the operator CLI and declarative configuration. Installed
hosts default to `balanced`; direct provider files, host networking, published
ports, dashboards, and mutable images require an explicit `dev` exception.
See [security profiles](10-security-profiles.md) before migrating an old setup.

## Identify the agent

```text
tentaflake status
tentaflake ps
tentaflake doctor
tentaflake doctor --security
```

Use the exact identifier shown by the inventory. Adapter metadata supplies the
runtime, instance, container, unit, and state path; unmanaged OCI containers
retain a fallback. The [adapter guide](agent-adapters.md) distinguishes existing
Hermes/ZeroClaw integrations from OpenClaw's stopped scaffold. A scaffold can
appear in the inventory without a container and refuses an explicit start.

## State and configuration

| Runtime | Default host state | Managed workspace | Container identity |
|---|---|---|---|
| Hermes | `/var/lib/hermes-<name>` | `workspace/` | UID/GID 10000 |
| ZeroClaw | `/var/lib/zeroclaw-<name>` | `data/` | UID/GID 65534 |

Hermes mounts its state at the same path inside the capsule and sets
`HERMES_HOME` there. ZeroClaw mounts its state at `/zeroclaw-data`; its generated
TOML configuration lives at `/zeroclaw-data/.zeroclaw/config.toml`. State paths
and permitted ownership overrides come from the selected builder. Confirm them
before copying data; do not infer a path from an instance name alone.

Hermes generates read-only configuration when effective `settings` is non-empty;
Research adds settings even when callers supply none. ZeroClaw always generates
and mounts its configuration read-only. Edit the declarative input and rebuild
deliberately. Interactive configuration writes can fail against that mount;
they are not a persistent configuration workflow. Seed files are copied without
overwriting existing
state, so changing `seedDir` is not an automatic update of existing skills or
personality files.

[Workspace quotas](14-workspace-quota.md) cap the declared workspace, not every
session, cache, log, or broker directory. Monitor host free space separately.
New managed images use Btrfs and require at least 128 MiB; existing ext4 images
need the documented explicit backup/restore migration.

## Start, stop, restart and logs

```text
tentaflake logs <agent>
tentaflake stop <agent>
tentaflake start <agent>
tentaflake restart <agent>
tentaflake shell <agent>
tentaflake exec <agent> -- <command>
```

Resolve the target before a lifecycle mutation. `stop` stops the controller and
its loaded LLM/fetch brokers together. A direct container-runtime restart skips
that coordinated systemd operation; use the CLI. See [operations and recovery](07-operations.md)
for crash backoff, broker readiness, and the combined kill switch.

For host-side unit evidence, use the exact unit from the inventory:

```bash
sudo systemctl status docker-hermes-coding.service
sudo journalctl -u docker-hermes-coding.service --since '1 hour ago'
```

Podman units use the `podman-` prefix. NixOS manages these containers as root;
selecting Podman does not make this a rootless deployment. The balanced
operator is not placed in the root-equivalent Docker group.

Do not print raw OCI environment data: compatibility containers can contain
real credentials, and balanced capsules contain virtual keys. The security
doctor provides bounded, redacted runtime evidence. `--hide` removes host and
agent names from diagnostic text and JSON; review other operational logs before
sharing them. Optional [observability](09-observability.md) adds local metrics
and journal queries.

## Add or remove an agent

Use the module list in `my-agents.nix` or the versioned generic JSON shape in
[declarative configuration](08-agent-cli.md). For example:

```nix
{ mkHermesAgent, ... }:
[
  (mkHermesAgent {
    name = "assistant";
    autoStart = false;
  })
]
```

This creates a stopped capsule. Under balanced, automatic start requires its
exact LLM broker, disposable worker, workspace quota, and research relay. Model
access follows [brokered egress](12-brokered-egress.md); web access follows
[secure research](16-research.md). The legacy fetch broker must be disabled for
a research-enabled agent. [The secure adapter example](../examples/adapter-secure.nix)
combines these declarations while retaining the documented runtime evidence gaps.

First evaluate and build the intended host, review assertions and the diff,
then activate in a planned window. `autoStart = false` does not prevent an
activation from creating declared quota filesystems or worker services.

Remove the agent's builder or JSON entry and its matching broker, research,
worker, quota, and provenance declarations. Review backup paths before
activation. State, backing images, and external secret files are not an
automatic deletion target; retain or archive them deliberately. Do not manually
remove Nix-managed users or groups as part of routine agent removal.

## Rotate credentials

For balanced agents, rotate the broker's provider file through the host's
runtime secret mechanism and restart only the exact LLM broker. Verify its
`/healthz` readiness and audit persistence without printing credentials. The
controller never receives the real provider value; the virtual key survives
broker unit restarts and changes on host reboot.

Research credentials belong to the separate Research service. Git auto-push,
backup, and observability credentials also stay host-side. Direct environment
files in an agent are a dev-only compatibility path; consult
[Agenix](04-agenix-secrets.md). Runtime files below `/run` must be provisioned
again at boot by the deployment's secret mechanism.

## Update images

Default controller image digests live in `adapters/catalog.json`;
`lib/constants.nix` retains the existing exported aliases. They are independent
of `flake.lock`:

```bash
./scripts/update-agent-images.sh
```

The script reports upstream digests; it does not rewrite pins. Review the image
source and provenance, update the exact digest and regenerate catalog docs with
`python3 scripts/runtime-catalog-docs.py`, run the appropriate checks, and
build before activation. Use `registry/repository@sha256:<digest>`; references
combining a tag and digest are rejected for backend compatibility. The
`allowMutableImage` escape hatch is dev-only.

A successful source evaluation or fixture test does not prove real vendor
startup, model compatibility, MCP discovery, or routing through the worker.
Validate those against the exact deployed image.

## Tune resources and execution

Balanced resource limits are enforced after caller configuration:

```nix
tentaflake.security.resources = {
  memory = "4g";
  memorySwap = "4g";
  cpus = "0.5";
  nofile = 4096;
};
```

Both current runtime builders default to `pidsLimit = 512`. Secure controllers
require a positive ceiling; `null` is dev-only. `extraContainerConfig` cannot
weaken the final limits or restore capabilities. See
[security profiles](10-security-profiles.md) for tmpfs and other bounds.

Use the [offline disposable worker](13-disposable-worker.md) for untrusted code.
Declaring a worker does not prove that every native runtime tool submits work to
its queue. Approval of a worker job never grants network, credentials, or a
writable host bind.

## Back up and recover

Use the opt-in encrypted Restic policy in [operations and recovery](07-operations.md)
for unattended backups. Selected quota mounts are added as separate sources
and must be mounted before backup. Keep configuration, runtime credentials,
recipient identities, and state recovery plans distinct.

`tentaflake backup <agent>` instead creates a mode-0600, caller-owned state
archive in the current directory. It is not encrypted, does not back up secret
identities, and does not replace a real restore drill. Plan space and access
before creating an archive. Restore with the controller stopped, verify numeric
ownership and quota mounts, then test application behavior.
