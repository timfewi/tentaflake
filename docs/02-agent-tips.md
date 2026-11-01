# Agent management

Resolve the canonical `agent-<name>` ID, unit, state path and `runnable` flag from
`tentaflake status` or `/etc/tentaflake/agents.tsv`. Do not derive ownership or
workspace paths from an adapter name. [Adapter metadata](agent-adapters.md)
owns those details, including native read-only configuration and seed behavior.

## Lifecycle and diagnostics

Use `tentaflake start|stop|restart <id>` rather than restarting through the OCI
backend. `stop` also stops loaded LLM/fetch brokers for the exact controller.
`logs` follows until interrupted; for bounded logs use the inventory unit:

```bash
sudo journalctl -u docker-agent-coding.service --since '1 hour ago' -n 100 --no-pager
```

Podman units use `podman-`; selecting Podman does not make NixOS containers
rootless. Balanced operators are not members of the root-equivalent Docker
group. Scaffolds can appear in inventory without an OCI container and refuse
start/shell/exec operations as appropriate.

Use `tentaflake health` and `tentaflake doctor --security` for bounded evidence.
Never dump raw OCI environments or credentials. Diagnostic `--hide` removes
host/agent names; other logs still require review before sharing. A healthy host
does not prove provider access, tool discovery or real vendor acceptance.

## Configuration and resources

[Configuration](08-agent-cli.md) owns adding/removing agents. Change generated
settings declaratively, not through interactive save commands. Seeds are
public Nix inputs and no-clobber initialization, not synchronization. Private
memory/personality belongs in runtime state, not this template.

The shared [security profile](10-security-profiles.md) enforces resource/mount
limits after caller configuration; adapter settings cannot weaken containment.
Use [workspace/state quotas](14-workspace-quota.md) for hard storage bounds and
monitor aggregate host capacity separately. A [worker](13-disposable-worker.md)
declaration does not automatically mediate native tools; approval never grants
network, secrets or host authority.

## Images and credentials

Edit the exact reviewed image pin in `adapters/<id>.nix`, then regenerate its
catalog projection and docs with `python3 scripts/runtime-catalog-docs.py`.
`./scripts/update-agent-images.sh` reports upstream digests, not automatic pin
updates. Digest identity is not provenance or runtime acceptance. Review the
[image policy](10-security-profiles.md), build, and activate deliberately.

Rotate provider files through the host [secret mechanism](04-agenix-secrets.md)
and restart the exact LLM broker when authorized. Check local readiness/audit
without exposing values; Research credentials stay in the Research service.
Runtime `/run` files need provisioning after reboot. No host Git publication
service is supplied; see [legacy removal](07-operations.md#git-auto-push).

## Backup and recovery

[Operations](07-operations.md) owns encrypted Restic, restore drills, updates
and incident response. Selected quota mounts are separate backup sources.
`tentaflake backup <id>` creates an unencrypted mode-0600 caller-owned archive,
not a backup of secret identities or proof of restore readiness. Keep the agent
stopped during restore and verify numeric ownership and mount boundaries.
