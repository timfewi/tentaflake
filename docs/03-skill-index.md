# Bundled skills

Skills are procedural reference files. Tentaflake bundles them under
`.agents/skills/` for development agents and for optional seeding into Hermes
state. They do not grant tool access, credentials, network paths, or operator
authorization.

## Inventory

| Skill | Purpose |
|---|---|
| [handle-the-host](../.agents/skills/handle-the-host/SKILL.md) | Tailscale SSH, host inspection, maintenance and recovery |
| [tentaflake-change-review](../.agents/skills/tentaflake-change-review/SKILL.md) | Requirement, focused verification and documentation review |
| [tentaflake-repo-guidance](../.agents/skills/tentaflake-repo-guidance/SKILL.md) | Modules, adapters, builders, CLI, installer and checks |
| [hermes-config-manager](../.agents/skills/hermes-config-manager/SKILL.md) | Declarative Hermes settings and read-only configuration |
| [hermes-provider-setup](../.agents/skills/hermes-provider-setup/SKILL.md) | Provider and model configuration |
| [hermes-memory-personality](../.agents/skills/hermes-memory-personality/SKILL.md) | Memory, user context and personality files |
| [hermes-tools-config](../.agents/skills/hermes-tools-config/SKILL.md) | Tool selection, output limits and Research/worker routing |

Each skill is a short procedure with links to the owning guides and source.
The Hermes references follow Tentaflake's declarative configuration and host
policy. Native commands, option names and limits depend on the pinned image;
check matching help/source instead of assuming the latest upstream behavior.
The host's deprecated `hermes` command is a Tentaflake CLI shim; inspect native
help inside the selected container. A skill does not establish vendor startup,
MCP discovery or execution acceptance.

## Seed skills deliberately

The Hermes builder accepts `seedDir`, copied into the agent's state without
overwriting existing files. To seed this repository's skill collection, use a
consumer-owned directory shaped like this:

```text
seed/
  skills/
    handle-the-host/
      SKILL.md
    hermes-config-manager/
      SKILL.md
```

Point `seedDir` at `seed/`, not directly at `.agents/skills/`; the contents are
copied into the state root, which needs a `skills/` child. The path becomes a
Nix input, so include only public, non-secret material. Private context needs
runtime provisioning even in a fork. Deployment-specific content
belongs in the consumer fork, never in the generic template.

Seeding is not automatic synchronization. Changing the seed does not overwrite
an existing skill. Review and update existing state explicitly while preserving
runtime changes. See [agent management](02-agent-tips.md).

Guide/source links in the bundled files refer to this checkout. Before seeding,
adapt them to the deployment's pinned documentation location; copying skills
alone does not make repository guides available inside the capsule. Preserve
links between skills when selecting only part of the collection.

## Configuration and security limits

Generated Hermes `config.yaml` is mounted read-only when effective `settings`
is non-empty; enabling Research adds settings even when callers supply none.
Use the declarative configuration rather than an interactive save command.
Provider credentials and provider selection instructions in generic Hermes
skills must respect the host profile: balanced capsules receive only a virtual
LLM-broker key. Direct provider credentials and alternate terminal/network
backends are dev-only compatibility instructions.

Balanced web access uses only the pinned `secure-research-tool` stdio relay.
Do not install native web fetch tools, remote MCP servers, provider-hosted web
tools, or alternate browser transports to work around this boundary. See
[web research](16-research.md) and [the threat model](15-threat-model.md).

Installing additional skills from a community tap requires source review and a
permitted acquisition path. The template does not bundle or verify those taps;
a balanced capsule has no general GitHub download route. Prepare reviewed skill
files outside the capsule and seed them deliberately instead of assuming an
in-container install command can access the Internet.

## Author and maintain a skill

A skill needs a `SKILL.md` with its name, description, and bounded instructions:

```markdown
---
name: example-workflow
description: Inspect one declared agent and report evidence gaps.
---

# Example workflow

1. Resolve the target from the generated inventory.
2. Read the relevant guide and inspect the declared policy.
3. Report what was verified and which runtime evidence is missing.
```

Keep reusable procedures separate from deployment identities and secret values.
Keep the skill to its trigger, actionable steps, boundaries and verification;
link detailed option tables and examples in the owning guide instead of copying
them. Validate local links and frontmatter (`name` matches the directory and
`description` names the task). Verify version-dependent claims against the
pinned runtime, and keep secrets out of diagnostic commands.
When behavior changes, update the relevant skill and its authoritative guide in
the same change. [Documentation maintenance](18-documentation.md) describes
source ownership and the website's release boundary.
