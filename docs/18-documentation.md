# Documentation ownership and release synchronization

The public repository owns behavior, examples, support facts and guides. The
website mirrors a pinned release; contributors need no private website access.
Never describe an unreleased checkout as already published.

## Source ownership

| Content | Owner |
|---|---|
| Scope/onboarding | `README.md`, `docs/01-quickstart.md`, `docs/08-agent-cli.md` |
| Runtime implementation and pure preset facts | `adapters/<id>.nix` |
| Generated CLI/support projections | `adapters/catalog.json`, marked table in `docs/agent-adapters.md` |
| Host policy and operational procedures | Owning guides in `docs/` and linked modules |
| Development contracts | `AGENTS.md` (`CLAUDE.md` symlink), `CONTRIBUTING.md`, shared skills |
| Released history/plans | `CHANGELOG.md` / `docs/roadmap.md`, kept distinct |
| Routes/search/presentation | Website maintainer |

Regenerate adapter projections with `python3 scripts/runtime-catalog-docs.py`;
`just catalog-check` detects drift. Keep generated table markers intact and
never edit the catalog as a second runtime source.

## Contributors

Update implementation, authoritative guide and affected examples/skills together.
Check names/defaults, local links and support claims against source. Distinguish
policy declarations, synthetic evidence and actual vendor acceptance. Link
policy details instead of copying security essays across guides. Record actual
checks and unknown evidence; identify new/renamed/removed guides in the PR.

The next release sync must replace dual Nix/JSON onboarding and direct builder
examples with the JSON-only workflow, including file-only consumer imports,
inactive samples, empty installer defaults and explicit manual migration.
Keep broker/worker/quota/Research policy in Nix and vendor acceptance pending.
This source change does not claim website publication.

## Website maintainers

Pin every mirrored guide, source link, version label, Markdown endpoint and
agent-readable index to one release tag and resolved commit. After a release:

1. Compare source changes and account for every added/removed guide route.
2. Regenerate guides/indexes; fix content upstream, not in mirrored copies.
3. Review handwritten summaries, FAQ and landing copy against the same diff.
4. Check coverage, generation drift, links and affected pages before publishing.

Publication and tagging are explicit maintainer actions. Keep tags immutable;
a merge or dirty checkout is not a release. A scheduled release check needs
neither cross-repository write credentials nor automatic deployment authority.

Compare a page's source commit with your deployed revision. The
[website](https://tentaflake.dev/) and [docs site](https://docs.tentaflake.dev/)
may lag a release; `main`, a deployment and a published site are separate
evidence boundaries. [Archive notes](archive-comparison.md) and old changelog
entries are historical, not current acceptance claims.
