---
name: tentaflake-change-review
description: Review a Tentaflake change for a concrete requirement, proportionate verification, synchronized docs and a complete PR.
version: 1.1.7
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
| Admission schema/projection | `just admission-plan` checks offline/model; separate `just research-admission-plan` checks prepared Research plans through the actual loader |
| Runtime/security | Identify affected host/Research VMs; run when explicitly authorized |
| Installer | Focused disk/generated-flake checks; ISO/VM builds when explicitly authorized |

`just` loads pinned tools; direct Cargo/lint commands need `nix develop`.
PR/push CI and `just fast` cover static/unit checks; evaluation and builds are
explicit gates. Before merging, identify and obtain relevant build/runtime
evidence for the current revision. A skipped build is not runtime acceptance.
See [build boundaries](../../../docs/17-builds.md) for affected workflow dispatch.
Full `just ci`/`just e2e`, VM, ISO and system builds are larger workloads.
Evaluation does not prove runtime behavior or authorize activation.

For broker firewall changes, inspect hook ordering and exercise unrelated open
host ports and DNAT paths in the VM. Broker drops in NixOS's late allow chains
were bypassed by earlier global accepts; matching rule text did not prove the
boundary. Keep authenticated local readiness separate from provider acceptance.

For Unix listeners, exercise an accepted connection in the real kernel table:
accepted server sockets retain the listener pathname. Match the unique listening
row and its inherited FD, excluding established rows; duplicate listeners remain
ambiguous. A path inode or socket creator UID alone does not identify the relay
serving a connection. Preserve the held peer and process identity through probes.

For filesystem ownership tests, obtain the focused Nix package test result as
well as Cargo evidence. A real sandbox mapped `/` to an unowned overflow UID;
host Cargo tests passed while the package tests failed. Anchor private fixtures
at checked directory descriptors, retain descendant ownership/mode checks, and
independently assert that the public absolute-path API still refuses foreign
root ownership. Never relax production ownership policy to fit a sandbox.

VM lifecycle fixtures must inspect container readiness after a unit becomes
active. A successful `systemctl stop` may unload the unit; assert inactive state
and no queued job after a successful run instead of requiring a later
`reset-failed`. After an intentional refusal, allow its known failed state to
remain, but reject new failures during cleanup. Keep refusal and container-absence
evidence independent of cleanup.
When a VM deliberately declares missing grants, assert the doctor's exact severe
findings and nonzero exit; healthy subsystem checks must retain that host posture.
After extracting service scripts into immutable helpers, migrate every inline-script
assertion: verify the wrapper's selected path, then inspect or execute that helper
in the build check. Do not add evaluation-time reads of helper derivations.

- Keep the template generic and secret-free; deployment policy belongs in forks.
- Preserve identified consumers and state. Document intentional API changes and
  migrations without silent security downgrades. Registry images stay digest-pinned
  except for the explicit dev-only escape hatch. Local images require immutable
  Nix descriptors and exact image-ID checks; do not imply registry provenance.
- Keep unaccepted runtimes described as unsupported/stopped.
- Add relevant changes under `Unreleased` in `CHANGELOG.md`; preserve history.
- When committing, use Conventional Commits and `git commit -s` for DCO.
- When opening a PR, fill `.github/PULL_REQUEST_TEMPLATE.md`: explain what and
  why, select the type, tick applicable items and retain unchecked ones.
  Record the requirement, verification and synchronized docs there.
- Identify guide additions/removals for the website maintainer. Publication is
  separate; see [documentation ownership](../../../docs/18-documentation.md).
