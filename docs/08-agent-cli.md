# Declarative agent configuration

The interactive `tentaflake agent` wizard was removed. Agent definitions are
reviewable inputs: use `my-agents.nix` for the full builder API or
`agents.json` for versioned adapter arguments or the legacy data schema.

## Add an agent on an installed host

Discover presets with `tentaflake runtimes --json`, then create a stopped
definition, review it, and use the same Nix parser/builders as the installer:

```bash
tentaflake agent template hermes assistant > incoming.json
tentaflake agent validate incoming.json
tentaflake agent plan incoming.json --json
tentaflake agent import incoming.json
```

For an operator-reviewed OCI image, use the `generic` preset. Supply a real
digest reference and argument vector; this example deliberately needs your image:

```bash
tentaflake agent template generic coding \
  --image "$REVIEWED_AGENT_IMAGE" -- agent-command --workspace /workspace \
  > incoming.json
tentaflake agent plan incoming.json
tentaflake agent import incoming.json
```

Templates work without host configuration. Validation, plans and imports require
the generated CLI configuration/inventory and installed pinned Nix inputs.
Evaluation runs offline with a 60-second deadline and bounded output; it neither
downloads OCI images nor installs software. Definitions must be non-secret.
Incoming files are limited to 1 MiB and 128 additions. Import accepts only a
schema-v1 `agents` envelope and ordinary stopped workload fields; alternate host
mounts, operational services and credential paths remain administrator-owned.
Generic onboarding requires a balanced host profile.

Import adds entries to `agents.json`, preserves legacy arrays and operator notes,
and refuses duplicate identities in JSON, active inventory or the flake's current
declared OCI/adapter inventory. Inputs and destination must be regular files.
Validation failure leaves existing bytes intact. CLI imports share a file lock;
atomic publication checks that `agents.json` did not change during validation.
This detects observed manual edits; it cannot lock administrators out of changes
after the final check. A leftover `agents.json.pending` is refused rather than
overwritten: review it before removing it. The running host remains unchanged.

`plan`, `validate` and `import` accept `--json` and `--hide`. Hidden plans redact
names, paths, image references and failure details. Nix source diagnostics are
not printed because source excerpts may contain private operator data. Plans
report source validation and pending vendor evidence, not live readiness or
vendor acceptance. OpenClaw remains a non-runnable scaffold. Unsupported generic
model/Research capabilities are refused; current secure startup gates still apply.

Review and track `agents.json` before an explicit host update. Git-backed flakes
only evaluate tracked files, including source-only `my-agents.nix` instances:

```bash
git add agents.json
tentaflake rebuild
```

The import does not git-add, rebuild, activate or start services. A host update
still validates the complete configuration and its dependencies; a failed update
does not replace the last active system revision. Review any concurrent Nix
configuration edits before activation.

## Nix definitions

Start from `my-agents.nix.example`:

```bash
cp my-agents.nix.example my-agents.nix
```

Each builder returns a NixOS module. The repository auto-imports the resulting
list when `my-agents.nix` exists.

```nix
{ mkHermesAgent, mkZeroClawAgent, ... }:
[
  (mkHermesAgent {
    name = "assistant";
    autoStart = false;
  })
  (mkZeroClawAgent {
    name = "research";
    autoStart = false;
    settings.schema_version = 3;
  })
]
```

This evaluates under the default `balanced` profile. The capsules have no
direct network and receive no real credentials. Keep them stopped until a
reviewed broker path exists.

Use `mkAgent { adapter = "hermes"; ... }` or the compatible runtime-specific
builders according to the runtime contract. See [agent adapters](agent-adapters.md)
for the lazy registry and OpenClaw's stopped scaffold and acceptance limits.
See the adapter implementations and example file for runtime-specific options.

## JSON definitions

Copy `agents.json.example`, keep only the agents you need, and validate it:

```bash
cp agents.json.example agents.json
jq -e . agents.json
```

`agentsFromData` accepts `schemaVersion: 1` and a generic `agents` array of
builder arguments including `adapter`, `name`, and `autoStart`. Stopped generic
entries can use balanced policy. The legacy `hermes` and `zeroclaw` arrays retain
their fields and defaults. Both forms are additive; unknown fields/adapters and
effective container collisions fail. The informational `_securityNote` is
accepted for compatibility. Input may name a runtime credential path, but must
never contain a credential value.

Legacy entries with direct environment files or ZeroClaw ports require the
host to deliberately select:

```nix
tentaflake.security.profile = "dev";
```

This is a migration bridge, not a secure 24/7 configuration. Prefer the Nix
builder API or generic `agents` entries for balanced capsules.

## Apply deliberately

First evaluate or build the selected host:

```bash
nix build \
  .#nixosConfigurations.tentaflake.config.\
system.build.toplevel \
  --no-link
```

Activation is a separate runtime operation. Review the build result and target
before using `nixos-rebuild switch` or `tentaflake rebuild`.

## Verify

After an explicitly authorized activation:

```bash
tentaflake status
tentaflake doctor
tentaflake doctor --security
systemctl status \
  docker-hermes-assistant.service
```

For Podman, unit names start with `podman-`.

## Remove an agent

Remove its declarative entry and matching broker, Research, worker, quota and
provenance declarations, then review backup paths before activation. See
[agent removal](02-agent-tips.md#add-or-remove-an-agent). State directories,
backing images and secret files are not deleted automatically; decide separately
whether they must be archived, retained or removed.
