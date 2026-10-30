# Declarative agent configuration

Use `agents.json` as the only public instance-definition source, with non-secret
schema-v1 data. Runtime-native settings belong to the selected adapter; broker,
worker, quota and Research authority stays in Nix host modules. Host policy may
consume instance metadata, but must not define additional instances.

The installer initializes an empty live `agents.json` when absent. The populated
`agents.json.example` is an inactive reference, never enabled automatically.
A missing live file in the template means an empty host.

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

Import atomically adds to the same live `agents.json` schema-v1 document,
preserving optional `_securityNote`. It rejects legacy arrays and duplicate
names/identities across JSON, active inventory and host declarations, including
unmanaged OCI containers.
Inputs/destination must be regular files. Failure preserves existing bytes;
imports share a lock and check for changes before publication. Administrators
can still edit after that
check. A leftover `agents.json.pending` is refused, not overwritten.

`--json` and `--hide` apply to validate/plan/import. Hidden plans redact names,
paths, image references and errors. Raw Nix source diagnostics are not printed.
Plans prove source validation only, not vendor acceptance or live readiness.
Scaffolds remain non-runnable, and secure startup gates still apply.

Import does not stage Git files, rebuild, activate or start anything. Review and
track inputs before a deliberate host update: Git-backed flakes only include
tracked sources. A failed update does not replace the active generation.

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
arguments fail. Adapter arguments are subject to the shared validation.
Never include literal credentials: JSON, settings, environment values and seed
paths become Nix-store inputs. [Secrets](04-agenix-secrets.md) belong in runtime
files outside the capsule.

Names are globally unique across adapters; the IDs above are `agent-assistant`
and `agent-experiment`. Host state defaults to `/var/lib/tentaflake/agents/<name>`.
The reference file includes stopped Hermes/ZeroClaw native settings with a
public `example-model` and broker-only URL, plus stopped OpenClaw/Pi entries
without native settings. These samples require matching Nix broker model
allowlists, network and pricing policy, plus worker, quota and Research policy.
They are not turnkey integrations or evidence of vendor acceptance.

## Consumer flakes

Import the same JSON file into a NixOS module:

```nix
{ agentsFromData, ... }:
{
  imports = agentsFromData { file = ./agents.json; };
}
```

Pass `tentaflake.lib.${system}.agentsFromData` through `specialArgs`. This
public helper is **file-only**: no inline `data` or injectable builder.
`mkAgent` and adapter builds are internal implementation details, not public
instance-definition APIs. See [adapters](agent-adapters.md) for supported
fields and the [breaking migration](agent-adapters.md#breaking-migration).

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
