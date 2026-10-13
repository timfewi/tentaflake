# Build boundaries and contributor checks

Start with `just fast`. It runs formatting, Nix lint, ShellCheck, Rust
formatting/Clippy/unit tests, CI routing/evaluation-planning regressions and actionlint.
It does not prepare Research closures, evaluate configurations/generated flakes,
or build Nix packages, images, systems or VMs. Loading the pinned development
environment can still realize contributor tools.

Use `just eval` explicitly for read-only flake evaluation, including focused
adapter evaluation and the required non-VM Research closure preparation.

CI and `just eval` use `scripts/eval_flake.py` after that preparation. It discovers
the full output/check inventory without evaluating derivations, then runs
`nix flake check --no-build` in sequential projections: at most four checks,
one host with its matching check/package aliases, or one remaining output group.
The installed-host check and installer ISO package stay with their host, avoiding
duplicate host evaluation. Source inputs retain Git filtering and existing pins;
only the temporary wrapper writes a lock. Each evaluator exits before the next.
This bounds the amount of evaluation retained together, not an individual
fixture's memory or duration. CI run `38463440894` spent over 19 minutes in the
previous whole-flake evaluation after Research closure preparation; this was
observed elapsed time, without evidence of a hang or failure.

`just generated-flake` validates the installer-generated flake separately.
`checks.*.agent-adapters` covers compatible mappings, JSON, inventory, lazy
selection, stopped OpenClaw, quota declarations and generated quota-helper refusal
regressions without mounts. `just ci`/`just e2e` run the complete gate, both VM
suites and the installer ISO. Run an affected VM suite when changing runtime
or security behavior; use the ISO gate when changing installation.

## Rust cache boundaries

Cargo remains the Rust build definition. `[workspace.package]` in `Cargo.toml`
supplies the shared version; each crate supplies its package description.
Nix packages and the worker image read those manifests. A replaced worker
image supplies its default local reference through `imageName`/`imageTag`;
images without those attributes require an explicit `worker.imageReference`.

Each Rust Nix source contains root Cargo files, all member manifests, and the
selected crate's complete source. Cargo can resolve the workspace while an
unrelated crate source edit leaves the other package derivations unchanged.
Shared manifests, dependencies or toolchain changes still invalidate affected
builds. Declare additional root build inputs explicitly; add shared/path
dependencies to the fileset when introducing them.

A comparison of actual Nix derivation paths with a temporary CLI source edit
invalidated all three packages before this boundary and only the CLI after it.
All three focused-source packages built successfully. This establishes cache
isolation, not a fixed wall-clock speedup. `checks.*.rust-package-sources`
guards manifest availability, source isolation and the worker image version.

## GitHub check selection

The imported policy is `.github/ci-paths.json`; its consumer is
`scripts/ci_changes.py`. Both modes use this ordered policy. Component paths
take precedence over Markdown suffixes, and the selector reads NUL-delimited
Git paths, including deletions and both sides of renames.

Automatic PR/push runs always check real-Git routing regressions and whitespace.
Affected paths add formatting/lint, including actionlint, and Rust crate
Clippy/unit tests with workspace formatting. Unknown paths, invalid lock data or
unavailable comparison history select all fast checks. No automatic path selects Research
closure preparation, configuration/generated-flake evaluation, Nix package/image/
system builds or VM suites. DCO and secret scanning remain separate checks.

Use the check workflow's **Run workflow** action (`workflow_dispatch`) when a
change needs build or runtime evidence. Select the revision to verify and set
optional `base_sha` to the full 40-character SHA to compare against that revision.
An empty, malformed or unavailable comparison SHA selects the full inventory;
unknown paths and invalid lock data do the same. A dispatch with a usable
comparison adds the affected expensive gates below to its fast checks.

| Change | Automatic checks beyond routing/whitespace | Additional explicit gates | VM suite |
| --- | --- | --- | --- |
| Docs and listed presentation metadata | None | None | None |
| CI routing/workflow/test definition or contributor tooling | Static | None | None |
| One Rust crate/package | Static and affected crate | Affected package and source-boundary check; worker includes its image | Runtime |
| Worker module | Static and worker crate | Worker package/image and source-boundary check | Runtime |
| Shared Cargo inputs | Static and all Rust crates | All Rust packages/image and source-boundary check | Runtime |
| Installer scripts or host VM fixture | Static | Generated installed flake | Runtime |
| Installer Nix | Static | Configuration evaluation, generated installed flake and host build | Runtime |
| Research module/client/fixture or research-only input pin | Static | Research closures, configuration evaluation and generated installed flake | Research |
| Dev Container inputs | Static | Dev Container package | None |
| Generated-flake script or module-evaluation fixture | Static | Corresponding generated-flake or configuration gate | None |
| Onboarding validator/fixture | Static | Installed preset/generic CLI fixture | None |
| Operator onboarding CLI module | Static and CLI crate | CLI package, source-boundary and installed fixture | None |
| Shared JSON agent parser | Static | Installed fixture | Runtime |
| Adapters, shared modules/helpers, other lock updates, unknown paths/history | All fast checks | Full inventory | Both |

The routing regression runs directly on every runner without Nix setup;
`checks.*.ci-vm-selection` also packages it for the full local flake gate.
Documentation starts no Nix installation. Fast checks use the pinned binary
cache for contributor tools; only explicit builds upload a GitHub Nix cache.
A Rust-only dispatch
does not build the separate Research service. Worker-module dispatch uses the
same focused package/runtime gates; the Research fixture enables no workers.
Mixing these changes with shared security paths selects their combined gates.
Non-VM package builds run in groups of at most four. Configuration evaluation
prepares the same exact Research closure checks sequentially before validating
the full flake/module inventory. Evaluation-helper changes select this explicit
gate; automatic checks exercise its coverage and failure handling without Nix.
Onboarding validation uses the installed fixture and a selected CLI package or
an explicitly supplied local binary. The standalone host build is selected when
both configuration evaluation and runtime checks are affected.

Before merging, record which build/runtime evidence the change needs and obtain
it for the current revision. Green fast checks and skipped builds do not prove
runtime acceptance. Full VM/ISO/system workloads remain separate from local
fast checks and require their own scope. Automatic runs cancel superseded runs
for the same PR or branch; dispatch has a separate concurrency group so a push
does not cancel explicitly requested verification.

## Nix and Bazel

[Nix + Bazel](https://nix-bazel.build/) and the linked
[rules_nixpkgs guide](https://github.com/tweag/rules_nixpkgs/blob/master/guide.md)
describe providing Nix toolchains/dependencies to Bazel's finer build actions.
That approach can help a large code graph with reusable remote actions.

Tentaflake currently has three independent Rust binaries, plus NixOS modules
and VM tests. Cargo already provides incremental development builds. Narrower
Nix sources retain the other binaries' cached packages, and VM selection avoids
irrelevant guest boots. Bazel would require another target/dependency graph and
cache configuration while the NixOS/VM work remains. No Bazel speedup has been
measured here. Keep Cargo/Nix as the build definition for now; reconsider a
bounded Bazel prototype if measurements show compilation dominates after these
changes or the workspace grows into a larger shared code graph.
