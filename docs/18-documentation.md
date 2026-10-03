# Documentation ownership and release synchronization

The public Tentaflake repository owns product behavior, configuration examples,
support status, and operational guides. The public website and documentation
site present a pinned release of that material. Website maintenance can remain
separate without making contributors maintain two copies of every guide.

## Source ownership

| Content | Authoritative source | Maintenance |
|---|---|---|
| Scope and entry points | `README.md` | Update with behavior and support changes |
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

## Review checkpoint — 2026-10-02

The README and current guides were reviewed against the source, examples and
adapter contracts. Corrections cover secure startup, inventory, lifecycle,
skills and host-held credentials. Local Markdown links and public/private
references passed inspection across 41 Markdown documents. `just fast` passed,
including 75 Rust tests, policy/adapter evaluation and the generated installer
flake. This documentation-only change ran no VM, ISO or host activation.

Live website samples identified release v0.4.20 and source `5acbc4796e9d`.
The documentation corrections and adapter foundation are newer than that pin;
their website route and release update remains a maintainer step after release.
External provider behavior, vendor CLI commands and actual runtime acceptance
were not newly verified by this documentation review.
