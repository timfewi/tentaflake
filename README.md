# Tentaflake

![Tentaflake logo](public/tentaflake-readme.jpg)

A runtime-agnostic NixOS flake template for isolated AI agents on one host.
Declare agents in `agents.toml`, choose an adapter, and manage them through
systemd and the Rust `tentaflake` CLI. No mandatory web UI or control service.

> **Pre-1.0:** interfaces and security policy can break between releases. Pin a
> revision and read the [changelog](CHANGELOG.md) and
> [migration guide](docs/agent-adapters.md#breaking-migration) before updating.

## Start here

1. [Install](docs/00-install.md) or use the [consumer example](examples/).
2. Follow the [secure quickstart](docs/01-quickstart.md).
3. [Import a stopped agent](docs/08-agent-cli.md), then review and explicitly
   build/activate the host. Import never starts services.

The checked-in `agents.toml` and new installations start with an empty inventory:

```toml
schemaVersion = 1
agents = []
```

There is no separate example inventory and no sample starts automatically.
Add reviewed, stopped entries as shown in [configuration](docs/08-agent-cli.md).
Existing `agents.json` inventories require a deliberate
[migration](docs/agent-adapters.md#breaking-migration); they are never silently
ignored in favor of an empty TOML inventory.

All managed container IDs are `agent-<name>`; names are globally unique across
adapters. Default host state is `/var/lib/tentaflake/agents/<name>`.
[Configuration](docs/08-agent-cli.md) uses this file as the only instance source;
host broker, worker, quota and Research policy stays in Nix.

## Scope and support

Hermes and ZeroClaw have native adapters; real vendor acceptance remains
pending. OpenClaw and Pi are **stopped refusal scaffolds**, not runnable
integrations. The generic adapter accepts a reviewed digest-pinned custom OCI
image and command for offline workloads, with no accepted model or Research
integration. Discover presets with `tentaflake runtimes --json`; the
[adapter catalog](docs/agent-adapters.md#support-and-upstream-evidence) records
support and evidence separately. Synthetic tests are not vendor acceptance.

Balanced containment uses gVisor, a read-only root, non-root ownership, bounded
resources and no general egress. Automatic startup requires the exact agent's
[LLM broker](docs/12-brokered-egress.md), [offline worker](docs/13-disposable-worker.md),
[workspace quota](docs/14-workspace-quota.md) and [Research relay](docs/16-research.md).
A worker declaration does not automatically intercept native tools. Provider
keys stay on the host; private data never belongs in Nix inputs or Git.
Read the [security profiles](docs/10-security-profiles.md) and
[threat model](docs/15-threat-model.md) before deployment.

Optional modules provide encrypted backup, image provenance, observability,
runtime detection and a private Unix [operator API](docs/19-web-ui.md).
They do not establish application readiness or native chat support.

## Guides

- [CLI](docs/06-shell.md), [agent management](docs/02-agent-tips.md),
  [operations and restore](docs/07-operations.md)
- [Secrets](docs/04-agenix-secrets.md), [private management](docs/11-tailscale-management.md),
  [observability](docs/09-observability.md)
- [Fork checklist](docs/05-fork-checklist.md), [skills](docs/03-skill-index.md),
  [roadmap](docs/roadmap.md)
- [Contributing](CONTRIBUTING.md), [build/check boundaries](docs/17-builds.md),
  [documentation ownership](docs/18-documentation.md)

Contributors start with `just fast`; evaluation, builds and live acceptance are
separate gates. Enter `nix develop` for direct Cargo/lint commands. No check
implicitly authorizes deployment. Report vulnerabilities through GitHub Security
Advisories. Code is [MIT licensed](LICENSE); [DCO](DCO.txt) and
[trademark terms](TRADEMARK.md) apply separately.
