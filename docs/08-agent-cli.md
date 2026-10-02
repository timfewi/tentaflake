# Declarative agent configuration

The interactive `tentaflake agent` wizard was removed. Agent definitions are
reviewable inputs: use `my-agents.nix` for the full builder API or
`agents.json` for versioned adapter arguments or the legacy data schema.

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

Delete its declarative entry and rebuild deliberately. State directories and
secret files are not deleted automatically. Decide separately whether they
must be archived, retained, or removed.
