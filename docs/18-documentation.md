# Documentation ownership and release synchronization

The public Tentaflake repository owns product behavior, configuration examples,
support status, and operational guides. The public website and documentation
site present a pinned release of that material. Website maintenance can remain
separate without making contributors maintain two copies of every guide.

## Source ownership

| Content | Authoritative source | Maintenance |
|---|---|---|
| Scope and entry points | `README.md` | Update with behavior and support changes |
| Product direction and planned milestones | `docs/roadmap.md` | Keep plans separate from implemented and released support |
| Configuration and operational detail | `docs/` | Keep examples consistent with modules and builders |
| Development workflow | `CONTRIBUTING.md`, `AGENTS.md` / `CLAUDE.md` | Keep commands and verification boundaries accurate |
| Bundled operational skills | `.agents/skills/` | Update the skill and its related guide together |
| Release history | `CHANGELOG.md` | Preserve historical entries; describe new changes separately |
| Navigation, search, summaries and presentation | Website | Generate guides from the public source; review summaries |

Private deployment details and website implementation belong outside this
generic repository. Public contributor workflows must not depend on access to
a separately maintained repository or its credentials.

## For public contributors

1. Change the implementation and its authoritative guide together. Update the
   README, agent instructions, examples and affected skills where needed.
2. Check option names, defaults, CLI examples, local links, and support claims
   against source. Keep configured policy, fixture evidence and actual runtime
   acceptance distinct.
3. Run `just fast` and any explicitly selected checks required for the change;
   record what passed and what remains unverified.
4. Identify added, renamed or removed guides in the PR so the website maintainer
   can update navigation. Public contributors need only the public repository.

## For the website maintainer

Use one exact release tag and its resolved commit for every generated guide,
source link, version label, Markdown endpoint and agent-readable index. Preserve
that pin until a newer release is ready; a dirty checkout is not a release.

After a release:

1. Compare the pinned source with the new tag, including guide and skill changes.
2. Map every new guide to a stable route or record why it is deliberately excluded.
   Remove obsolete routes when upstream removes a guide.
3. Regenerate mirrored guides and indexes from that commit. Fix generated
   content upstream; avoid hand edits to synchronized copies.
4. Review landing copy, FAQ, handwritten summaries and the published skill
   against the same source diff. Generation cannot validate those claims by itself.
5. Verify source coverage, generation drift, links and the affected pages before
   publishing. Deployment remains a separate maintainer action.

A scheduled check can report a newer release without public-repository write
access or a cross-repository credential. Keep release following and website
deployment under the website maintainer's control. Local status should separately
report unreleased commits and working-tree changes instead of calling them synced.

## Reading the right version

The [website](https://tentaflake.dev/) and
[documentation site](https://docs.tentaflake.dev/) can lag a new release until
the maintainer updates them. Compare a page's source commit with the release
used by your deployment. Repository `main`, an unreleased checkout and a
released website are different evidence boundaries.

The [archive comparison](archive-comparison.md) and older changelog entries are
historical records. The current [adapter guide](agent-adapters.md) defines
runtime support; a stopped scaffold does not imply operational support.

## Review checkpoint — 2026-10-04

The seven bundled skills and current guides were reviewed against adapter,
CLI, host-policy and example source. Skills now use short procedures and
source/guide links instead of copied upstream catalogs. Corrections cover
read-only Hermes/ZeroClaw configuration, Agenix recipient/runtime separation,
host identities, environment-file verification and complete agent removal.
The adapter status reflects implemented opt-in SSE while retaining the actual
vendor acceptance gaps. Historical changelog links use their matching release.

Local Markdown paths/anchors, skill YAML frontmatter and the Agenix example's
Nix syntax/metadata/formatting checks passed. Fast-gate checks passed, including all
94 Rust tests, lint/formatting, policy/adapter evaluation, read-only flake
validation and the generated installer flake. No VM/ISO build or host activation
was run. Live website, provider behavior, secret decryption and actual vendor
workloads were not exercised; publication and deployment acceptance remain
separate operations.
