# Tentaflake product direction

The current development line returns to the declarative v0.4.37 architecture.
It is not a new published release. The release tag remains the authority for
released behavior; the README and operational guides describe this checkout.

## Declarative agents first

Declare Hermes and ZeroClaw through `my-agents.nix` or `agents.json`, validate
with the CLI and apply the NixOS configuration explicitly. Keep the existing
builders, agent state and operational commands. An agent does not require a
web control service, a company model or a mandatory shared agent runtime.

The opt-in [local host API](19-web-ui.md) lets a separately supplied web UI
inspect inventory/health and, when enabled, control fixed systemd actions.
This is an alternative operator interface over declarative instances, not a
replacement for the builders or a requirement to adopt HQ task ownership.
Native runtime chat remains a separate integration and acceptance step.

The existing security boundaries remain: balanced agents need their broker,
Research relay, worker and workspace quota; `dev` is for trusted compatibility
use and `strict` remains unavailable. A configuration that evaluates is not
proof that its model provider and tools work end to end.

## Priorities

1. Maintain the declarative configuration and installer path. Keep examples
   executable and validate mixed Nix/JSON definitions, refusal paths and imports.
2. Verify Hermes and ZeroClaw startup, provider calls, tools, explicit stops,
   crash recovery, persistent state and backup restoration before claiming
   runtime acceptance. Prefer fixes within the existing modules.
3. Simplify onboarding and diagnostics without hiding configuration or changing
   security defaults. Additional adapters need their own runtime evidence.
4. Keep documentation and release versions aligned. Publish a new maintenance
   release only after its applicable checks; do not relabel an old tag.

## Preserved development

The later admission ledger, host API design, local images and native-reference
work remain in [v0.4-final](https://github.com/timfewi/tentaflake/tree/v0.4-final).
The [0.5 development](https://github.com/timfewi/tentaflake/tree/ad28cc0e890df7b6653bfe99e03749237613fdd5)
remains in Git history as well. Reuse individual pieces when they solve a
concrete need in the declarative product; these branches are not dependencies
of this line.

The previous [milestone roadmap](https://github.com/timfewi/tentaflake/blob/e21be475a3afc6e95c5087003edd0b7f780e4df4/docs/roadmap.md)
is historical planning. Its employee/company milestones and the separate 0.5
runtime direction are not current delivery commitments. Existing issue status
alone establishes neither acceptance nor released support.
