# Documentation ownership and release synchronization

The public repository owns behavior, examples, support facts and guides. The
website mirrors a pinned release; contributors need no private website access.
Never describe an unreleased checkout as already published.

## What belongs in `docs/`

`docs/` contains only maintained product documentation: what Tentaflake is,
how to install, configure, operate and secure it, and its released direction.
Every guide is written for users and operators, is accurate for the current
source and belongs to the website's release-synchronized mirror. A guide added
after the pinned release is not yet published there. Write guides as descriptions
of the product, not of the work that produced it.

These never belong in `docs/` (or anywhere else in Git):

- implementation plans, backlogs, goals, task lists and agent prompts;
- review, investigation, session or archive-comparison notes;
- private deployment facts: consumer or private repository names and
  revisions, host names, infrastructure, provider accounts, local paths and
  workspace state.

Agents and contributors write such working notes to the repository-local
`.scratch/` directory. It is ignored by Git, is never committed or published
and is never a source of truth. Durable outcomes go into code, the owning
guide, `CHANGELOG.md` or the pull request description.

## Guide inventory

Each file in `docs/` must appear exactly once below, with its website route.
A new, renamed or removed guide changes this table in the same pull request
and is named in the PR and `CHANGELOG.md`; the website adds or removes the
route at its next release sync. `python3 scripts/test_docs_inventory.py` (part
of `just fast` and every CI run) fails when `docs/` and this table differ or
when anything below `.scratch/` is tracked.

<!-- guide-inventory:start -->
| Source | Website route |
|---|---|
| `docs/00-install.md` | `getting-started/install` |
| `docs/01-quickstart.md` | `getting-started/quickstart` |
| `docs/02-agent-tips.md` | `operations/agent-management` |
| `docs/03-skill-index.md` | `reference/skills` |
| `docs/04-agenix-secrets.md` | `configuration/secrets` |
| `docs/05-fork-checklist.md` | `reference/fork-checklist` |
| `docs/06-shell.md` | `operations/cli-shell` |
| `docs/07-operations.md` | `operations/recovery` |
| `docs/08-agent-cli.md` | `configuration/agents` |
| `docs/09-observability.md` | `operations/observability` |
| `docs/10-security-profiles.md` | `security/profiles` |
| `docs/11-tailscale-management.md` | `operations/tailscale` |
| `docs/12-brokered-egress.md` | `configuration/brokered-egress` |
| `docs/13-disposable-worker.md` | `configuration/disposable-worker` |
| `docs/14-workspace-quota.md` | `configuration/workspace-quota` |
| `docs/15-threat-model.md` | `security/threat-model` |
| `docs/16-research.md` | `configuration/research` |
| `docs/17-builds.md` | `reference/builds` |
| `docs/18-documentation.md` | `reference/documentation` |
| `docs/19-web-ui.md` | new; route added at next release sync |
| `docs/20-system-control.md` | new; route added at next release sync |
| `docs/21-enterprise-connectors.md` | new; route added at next release sync |
| `docs/agent-adapters.md` | `configuration/agent-adapters` |
| `docs/roadmap.md` | product roadmap, synchronized separately |
| `docs/tailscale-policy.example.json` | example linked from `operations/tailscale` |
<!-- guide-inventory:end -->

`README.md`, `SECURITY.md` and `CHANGELOG.md` are mirrored as
`concepts/architecture`, `security/reporting` and `reference/changelog`.

## Source ownership

| Content | Owner |
|---|---|
| Scope/onboarding | `README.md`, `docs/01-quickstart.md`, `docs/08-agent-cli.md` |
| Runtime implementation and pure preset facts | `adapters/<id>.nix` |
| Generated CLI/support projections | `adapters/catalog.json`, marked table in `docs/agent-adapters.md` |
| Host policy and operational procedures | Owning guides in `docs/` and linked modules |
| Development contracts | `AGENTS.md` (`CLAUDE.md` symlink), `CONTRIBUTING.md`, shared skills |
| Released history/direction | `CHANGELOG.md` / `docs/roadmap.md`, kept distinct |
| Working notes | `.scratch/` (ignored, never published) |
| Routes/search/presentation | Website maintainer |

Regenerate adapter projections with `python3 scripts/runtime-catalog-docs.py`;
`just catalog-check` detects drift. Keep generated table markers intact and
never edit the catalog as a second runtime source.

## Contributors

Update implementation, authoritative guide and affected examples/skills together.
Check names/defaults, local links and support claims against source. Distinguish
policy declarations, synthetic evidence and actual vendor acceptance. Link
policy details instead of copying security essays across guides. Record actual
checks and unknown evidence in the pull request, not in a guide.

## Website maintainers

Pin every mirrored guide, source link, version label, Markdown endpoint and
agent-readable index to one release tag and resolved commit. After a release:

1. Compare source changes and account for every added/removed guide route,
   using the guide inventory above.
2. Regenerate guides/indexes; fix content upstream, not in mirrored copies.
3. Review handwritten summaries, FAQ and landing copy against the same diff.
4. Check coverage, generation drift, links and affected pages before publishing.

Publication and tagging are explicit maintainer actions. Keep tags immutable;
a merge or dirty checkout is not a release. A scheduled release check needs
neither cross-repository write credentials nor automatic deployment authority.

Compare a page's source commit with your deployed revision. The
[website](https://tentaflake.dev/) and [docs site](https://docs.tentaflake.dev/)
may lag a release; `main`, a deployment and a published site are separate
evidence boundaries. Old changelog entries are historical, not current
acceptance claims.
