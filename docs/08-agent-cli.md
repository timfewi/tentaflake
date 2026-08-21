# Declarative agent configuration

The interactive `tentaflake agent` wizard was removed. Agent definitions are
reviewable inputs: use `my-agents.nix` for the full builder API or
`agents.json` for the smaller data schema.

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

Use `mkHermesAgent` or `mkZeroClawAgent` according to the runtime contract.
See the builder comments and example file for runtime-specific options.

## JSON definitions

Copy `agents.json.example`, keep only the agents you need, and validate it:

```bash
cp agents.json.example agents.json
jq -e . agents.json
```

`agentsFromData` turns the committed, non-secret data into Hermes and ZeroClaw
modules. The JSON file may name an `envFile`; it must never contain the secret
value itself.

The current JSON compatibility schema always wires environment files and, for
ZeroClaw, ports. It therefore evaluates only when the host deliberately sets:

```nix
tentaflake.security.profile = "dev";
```

This is a migration bridge, not a secure 24/7 configuration. Prefer the Nix
builder API for balanced capsules.

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
