---
name: hermes-config-manager
description: Change or diagnose declarative Hermes settings while preserving Tentaflake's read-only configuration and broker boundaries.
version: 1.1.0
platforms: [linux]
metadata:
  hermes:
    tags: [config, setup, migration]
    category: devops
    requires_toolsets: [terminal]
---

# Hermes configuration

1. Resolve the instance, image digest and state path from inventory and its
   declaration. `adapters/hermes.nix` owns the builder; image defaults live in
   `lib/constants.nix`.
2. Hermes uses `$HERMES_HOME`, normally `/var/lib/hermes-<name>` inside and
   outside the container, rather than the operator's `~/.hermes`.
3. Non-empty effective `settings` generates a read-only `config.yaml`.
   Research creates effective settings even without caller settings.
   Interactive edits fail; they are not a persistence workflow.
4. Change `settings` in the consumer's `my-agents.nix` with `mkHermesAgent` or
   `mkAgent { adapter = "hermes"; ... }`. Read
   [agent configuration](../../../docs/08-agent-cli.md) and the adapter arguments.
5. Validate native option names against the pinned image's help or matching
   source. Current upstream defaults may differ. The host's deprecated
   `hermes` command is a Tentaflake shim; inspect the native CLI inside the
   selected container instead:

```sh
tentaflake exec hermes-<name> -- hermes --help
```

Keep settings and `extraEnvironment` secret-free: generated values enter the
Nix store. Balanced keys belong in host broker runtime files. Profiles,
alternate terminal backends, OAuth and auxiliary models need separately
reviewed paths and authority; they cannot bypass declared host policy.

Research settings apply after caller settings and disable native web/browser
tools. See [tools](../hermes-tools-config/SKILL.md) and
[providers](../hermes-provider-setup/SKILL.md).

Evaluate/check the consumer configuration and review the generated diff before
an authorized activation. Afterwards check the exact unit, bounded logs and
application behavior. Recheck native options after image changes. Never dump
`.env`, `auth.json` or complete runtime configuration for diagnosis.
See [agent management](../../../docs/02-agent-tips.md) for seeds and ownership.
