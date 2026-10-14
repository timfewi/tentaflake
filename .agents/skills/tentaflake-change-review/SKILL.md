---
name: tentaflake-change-review
description: Review a Tentaflake change for a concrete requirement, proportionate verification, synchronized docs and a complete PR.
version: 1.1.23
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
| Admission schema/projection | `just admission-plan` checks offline/Model plans; after closure preparation, `just research-admission-plan` checks Research/combined plans through the actual loader |
| Runtime/security | Identify affected host/Research VMs; run when explicitly authorized |
| Installer | Focused disk/generated-flake checks; ISO/VM builds when explicitly authorized |

`just` loads pinned tools; direct Cargo/lint commands need `nix develop`.
PR/push CI and `just fast` cover static/unit checks; evaluation and builds are
explicit gates. Before merging, identify and obtain relevant build/runtime
evidence for the current revision. A skipped build is not runtime acceptance.
See [build boundaries](../../../docs/17-builds.md) for affected workflow dispatch.
Full `just ci`/`just e2e`, VM, ISO and system builds are larger workloads.
Evaluation does not prove runtime behavior or authorize activation.
Combined capability collection must retain each authenticated observation's
original time and expiry. Model-before-Research collection previously stamped
the older Model result with the later timestamp, hiding an expired prerequisite.
Exercise delayed later observations and the real combined lifecycle; separate
Model-only and Research-only cases do not establish their composition. Changes
to shared live collectors must retain both explicit VM routes.
When scanning a generated dependency lockfile, realize its source derivation
first. Evaluating the Dev Containers source path alone produced an absent
`yarn.lock` on a cold store, preventing the remaining security checks from running.
Keep Semgrep bounded to two workers and retain strict parsing. Its default worker
count exhausted local io_uring allocations; the pinned parser also rejected a
valid arithmetic Bash `for` loop. An equivalent bounded `while` keeps coverage.

For asynchronous HTTP work, bound queued blocking tasks as well as connections.
A connection deadline can drop its task handle while `spawn_blocking` continues;
hold the work permit inside that closure and test cancellation with real requests.
Thread-count limits alone leave the pending task queue unbounded.
Session-revocation fixtures must prove the saved cookie still succeeds immediately
before restart or process death. The original HTTP fixture saved an already
logged-out cookie, so its restart refusal could not detect missing revocation.
Keep logout coverage separate and preserve durable token consumption on restart.
For closed tagged JSON objects, use an empty struct variant instead of a unit
variant. A planning regression showed Serde accepting extra workspace fields on
the unit variant despite `deny_unknown_fields`; retain that malformed-input check.
For broker firewall changes, inspect hook ordering and exercise unrelated open
host ports and DNAT paths in the VM. Broker drops in NixOS's late allow chains
were bypassed by earlier global accepts; matching rule text did not prove the
boundary. Keep authenticated local readiness separate from provider acceptance.

For Unix listeners, exercise an accepted connection in the real kernel table:
accepted server sockets retain the listener pathname. Match the unique listening
row and its inherited FD, excluding established rows; duplicate listeners remain
ambiguous. Preserve the held peer and process identity through probes.

For filesystem ownership tests, obtain the focused Nix package test result as
well as Cargo evidence. A real sandbox mapped `/` to an unowned overflow UID;
host Cargo tests passed while the package tests failed. Anchor private fixtures
at checked directory descriptors, retain descendant ownership/mode checks, and
independently assert that the public absolute-path API still refuses foreign
root ownership. Never relax production ownership policy to fit a sandbox.
Use a process-wide counter for concurrent fixture names; PID plus a nanosecond
clock reading alone collided in parallel CI diagnostics tests.
Replace immutable directory fixtures within the same parent: an unprivileged
cross-parent rename needs write access to the moved directory and failed before
the identity assertion. Do not chmod away the identity being tested.

Validate mount declarations against their capability owner before comparing live
mounts with those declarations. Worker shape-validation regressions exposed
accepted foreign, writable and unrequested results bindings; matching those
declarations does not establish their authority.
Keep pure plan checks separate from runtime filesystem observation and grants.

VM lifecycle fixtures must inspect container readiness after a unit becomes
active. A successful `systemctl stop` may unload the unit; assert inactive state
and no queued job after a successful run instead of requiring a later
`reset-failed`. After an intentional refusal, allow its known failed state to
remain, but reject new failures during cleanup. Keep refusal and container-absence
evidence independent of cleanup. Repeated refusal fixtures must prove a fresh
CLI invocation and its expected exit status, then clear only those known failed
units after stop/no-job evidence; otherwise systemd's start limit can hide the
intended refusal or block later recovery. Never disable the unit rate limit.
Initial quota owners can already have refused
the missing ledger at boot; verify that invocation's diagnostic and absent images
before accepting its retained failed state in each node's bootstrap setup.
The VM driver's command result contains stdout only. Capture stderr explicitly
when asserting a CLI refusal message, while retaining the nonzero-exit check.
Systemd's loaded inventory can include dependency-only `not-found` mounts (the
VM exposed `home.mount`). Keep required-unit reads strict. Any incidental-mount
exception needs inert properties and successful installed-file absence checks
before and after observation; query failures never prove absence.
For `busctl --json` fixtures, distinguish direct get-property values from wrapped
method arguments. Observe the actual property shape before adding a collector;
a wrapped scalar fixture hid a Worker path parser failure until source review.
Review harness-generated mounts too: the NixOS VM's writable store adds an
overlay. Admission fixtures use its read-only store bind mount and retain real
Nix database/GC-root registration for authority retention. Isolate registration
and the daemon in private mount namespaces; never remount or change ownership
of the host-namespace store. Disabling guest Nix breaks retention-backed
bootstrap. Keep both shared-store mounts available until the final shutdown
ramfs: lazy unmount removed systemd's shutdown executable and froze the guest.
Suppress early shutdown dependencies only for those harness mounts. Persistent
volume and runtime stop checks must remain unchanged.
When a VM deliberately declares missing grants, assert the doctor's exact severe
findings and nonzero exit; healthy subsystem checks must retain that host posture.
After extracting service scripts into immutable helpers, migrate every inline-script
assertion: verify the wrapper's selected path, then inspect or execute that helper
in the build check. Do not add evaluation-time reads of helper derivations.
Build-phase reads need context-bearing Nix paths. A Worker fixture decoded its
paths from context-free JSON, so evaluation passed but sandboxed `jq` lacked the
generated files. Read the original module artifacts in the builder and retain
path-equality assertions against the decoded manifest.

- Keep the template generic and secret-free; deployment policy belongs in forks.
- Preserve identified consumers and state. Document intentional API changes and
  migrations without silent security downgrades. Registry images stay digest-pinned
  except for the explicit dev-only escape hatch. Local images require immutable
  Nix descriptors and exact image-ID checks; do not imply registry provenance.
  Verify the actual Docker store contract: Docker 29's containerd store resolves
  manifest/index IDs, while archive descriptors contain config IDs. Keep the
  supported classic-store setting explicit and verify live driver status; a
  successful load does not prove the descriptor ID is addressable.
  Exercise abnormal supervisor exits: systemd 261 skips ExecStop after a failed
  running main. Bind post-stop uncertainty to its exact invocation; never release
  resources or modify another invocation because a cleanup helper ran.
- Keep unaccepted runtimes described as unsupported/stopped.
- Add relevant changes under `Unreleased` in `CHANGELOG.md`; preserve history.
- When committing, use Conventional Commits and `git commit -s` for DCO.
- When opening a PR, fill `.github/PULL_REQUEST_TEMPLATE.md`: explain what and
  why, select the type, tick applicable items and retain unchecked ones.
  Record the requirement, verification and synchronized docs there.
- Identify guide additions/removals for the website maintainer. Publication is
  separate; see [documentation ownership](../../../docs/18-documentation.md).
