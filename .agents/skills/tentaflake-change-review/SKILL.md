---
name: tentaflake-change-review
description: Review a Tentaflake change for a concrete requirement, proportionate verification, synchronized docs and a complete PR.
version: 1.1.0
---

# Tentaflake change review

Use for features, fixes and substantive reviews. Read `AGENTS.md` and
`CONTRIBUTING.md`; formatting and typo edits need only relevant checks.

1. **Reason:** name the issue, reproduced bug or user requirement before editing.
   Inspect the working tree and preserve unrelated changes.
2. **Verify:** choose a check that exercises the intended behavior. Reproduce
   bugs when practical and add a regression check for the failing path.
3. **Sync:** update the affected guide, example, instructions and skill together.
   Check names, defaults, links and support claims against source.
4. **Review:** inspect the final diff for missed requirements and accidental
   changes. Report commands run, results and unverified behavior.

| Change | Verification |
|---|---|
| Ordinary source/configuration | Start with `just fast`, then focused assertions/tests |
| Docs/skills | Inspect source claims, local links and frontmatter; run `just fast` |
| Image validation | `nix build --no-link .#checks.x86_64-linux.image-pinning` |
| Runtime/security | Identify affected host/Research VMs; run when explicitly authorized |
| Installer | Focused disk/generated-flake checks; ISO/VM builds when explicitly authorized |

`just` loads pinned tools; direct Cargo/lint commands need `nix develop`.
See [build boundaries](../../../docs/17-builds.md). Full `just ci`/`just e2e`,
VM, ISO and system builds are larger workloads. Evaluation does not prove
runtime behavior or authorize activation.

- Keep the template generic and secret-free; deployment policy belongs in forks.
- Preserve identified consumers and state. Document intentional API changes and
  migrations without silent security downgrades. Images stay digest-pinned
  except for the explicit dev-only escape hatch.
- Keep unaccepted runtimes described as unsupported/stopped.
- Add relevant changes under `Unreleased` in `CHANGELOG.md`; preserve history.
- When committing, use Conventional Commits and `git commit -s` for DCO.
- When opening a PR, fill `.github/PULL_REQUEST_TEMPLATE.md`: explain what and
  why, select the type, tick applicable items and retain unchecked ones.
  Record the requirement, verification and synchronized docs there.
- Identify guide additions/removals for the website maintainer. Publication is
  separate; see [documentation ownership](../../../docs/18-documentation.md).
