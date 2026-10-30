# Declarative agent configuration

Use `my-agents.nix` for the complete `mkAgent` API or `agents.json` for non-secret
schema-v1 data. Both feed the same parser/adapter contracts. Runtime-native
settings belong to the selected adapter; host authority stays in host modules.

## Add an agent on an installed host

```bash
tentaflake runtimes --json
tentaflake agent template hermes assistant > incoming.json
tentaflake agent validate incoming.json
tentaflake agent plan incoming.json --json
tentaflake agent import incoming.json
```

For a custom OCI workload, supply your reviewed digest and command:

```bash
tentaflake agent template generic coding \
  --image "$REVIEWED_AGENT_IMAGE" -- agent-command --workspace /workspace \
  > incoming.json
tentaflake agent plan incoming.json
tentaflake agent import incoming.json
```

Templates require no host configuration. Validation/plans/import require the
generated CLI inputs and installed pinned Nix inputs. Evaluation is offline,
bounded to 60 seconds and bounded output; it does not download images. Incoming
files are limited to 1 MiB and 128 stopped additions. Alternate host mounts,
operational services and credential paths remain administrator-owned. Generic
onboarding requires balanced and has no accepted model/Research hooks.

Import atomically adds to a schema-v1 `agents` document, preserving optional
`_securityNote`. It rejects legacy arrays and duplicate names/identities across
JSON, active inventory and current flake declarations. Inputs/destination must
be regular files. Failure preserves existing bytes; imports share a lock and
check for changes before publication. Administrators can still edit after that
check. A leftover `agents.json.pending` is refused, not overwritten.

`--json` and `--hide` apply to validate/plan/import. Hidden plans redact names,
paths, image references and errors. Raw Nix source diagnostics are not printed.
Plans prove source validation only, not vendor acceptance or live readiness.
Scaffolds remain non-runnable, and secure startup gates still apply.

Import does not stage Git files, rebuild, activate or start anything. Review and
track inputs before a deliberate host update: Git-backed flakes only include
tracked sources. A failed update does not replace the active generation.

## Nix definitions

```nix
{ mkAgent, ... }:
[
  (mkAgent { adapter = "hermes"; name = "assistant"; autoStart = false; })
  (mkAgent { adapter = "zeroclaw"; name = "research"; autoStart = false; })
]
```

Names are globally unique across adapters. These IDs are `agent-assistant` and
`agent-research`; host state defaults to `/var/lib/tentaflake/agents/<name>`.
See [adapters](agent-adapters.md) for supported fields, generic definitions,
native layout and migration. No runtime-specific wrapper builders are exported.

## JSON definitions

```json
{
  "schemaVersion": 1,
  "agents": [
    { "adapter": "hermes", "name": "assistant", "autoStart": false },
    { "adapter": "pi", "name": "experiment", "autoStart": false }
  ]
}
```

Both root fields are required. `_securityNote` is optional metadata; unknown
root/entry fields, legacy runtime arrays, unsupported versions and malformed
arguments fail. Adapter arguments are subject to the same Nix validation.
Never include literal credentials: JSON, settings, environment values and seed
paths become Nix-store inputs. [Secrets](04-agenix-secrets.md) belong in runtime
files outside the capsule.

## Apply deliberately

Evaluate and build the complete selected host, review the diff/assertions, and
activate only with operator authorization. Balanced automatic startup requires
the same ID's broker, worker, quota and Research declarations. `autoStart = false`
does not prevent activation from creating declared worker services or quota
filesystems. OpenClaw/Pi create only refusal units, not those runtime resources.

After authorized activation, check `tentaflake status`, `tentaflake doctor
--security`, the exact inventory unit and actual application behavior. For
example, a Docker instance uses `docker-agent-assistant.service`; Podman uses
`podman-`. A build or synthetic test does not prove live vendor acceptance.

## Remove an agent

Remove its definition and matching broker, Research, worker, quota and provenance
declarations; review backup paths and stop old writers deliberately. State,
quota images, budget state and external secrets are not automatic deletion
targets. See [operations](07-operations.md) and the
[breaking migration](agent-adapters.md#breaking-migration).
