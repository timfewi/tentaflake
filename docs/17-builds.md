# Build boundaries and contributor checks

Start with `just fast`. It runs formatting, Nix lint, ShellCheck, Rust checks,
the CI-selection/source-boundary regressions, read-only flake evaluation and
the generated installed flake, including focused adapter evaluation.
`checks.*.agent-adapters` covers compatible mappings, JSON, inventory, lazy
selection, stopped OpenClaw, quota declarations and generated quota-helper refusal
regressions without mounts. Required research closure preparation uses
non-VM builds. `just e2e` still runs the complete gate, both VM suites and the
installer ISO. Run an affected VM suite when changing runtime or security
behavior; use the ISO gate when changing installation.

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
`scripts/ci_changes.py`. One ordered policy selects non-VM checks and the two
VM suites independently. Component paths take precedence over Markdown suffixes.
The table describes affected suites; execution follows the temporary pause below.

| Change | Non-VM checks | VM suite |
| --- | --- | --- |
| Docs and listed presentation metadata | Routing regression and whitespace; no Nix setup/cache | None |
| CI routing/workflow/test definition | Routing regression, focused selection package and static checks | None |
| One Rust crate/package | Rust workspace checks, that package and source-boundary check; worker includes its image | Runtime |
| Worker module | Rust workspace/worker package and image; actual worker configuration and policy in the runtime fixture | Runtime |
| Shared Cargo inputs | Rust workspace and all three packages/image; no Research/configuration preparation | Runtime |
| Installer scripts or host VM fixture | Static checks and generated installed flake | Runtime |
| Installer Nix | Configuration evaluation, static checks and generated installed flake | Runtime |
| Research module/client/fixture or research-only input pin | Research/configuration preparation and evaluation, static checks and generated installed flake | Research |
| Dev Container inputs | Dev Container package and static checks | None |
| Generated-flake script or module-evaluation fixture | Its generated-flake or configuration gate and static checks | None |
| Onboarding validator/fixture | Installed preset/generic CLI fixture and static checks; no Research preparation | None |
| Operator onboarding CLI module | Rust/CLI package, source-boundary and installed fixture checks; no Research preparation | None |
| Shared JSON agent parser | Installed fixture and static checks; no Research preparation | Runtime |
| Adapters, shared modules/helpers, other lock updates, unknown paths/history | Full gate | Both |

The selector reads NUL-delimited Git paths and includes deleted/old rename
paths. Invalid lock data requires the full gate. The real-Git regression check
runs on every event directly on the runner; `checks.*.ci-vm-selection` also
packages it for the contributor gate. CI metadata changes are covered by these
regressions rather than building unchanged guest systems.

Documentation does not start Nix installation, cache upload, Research closure
materialization, full flake evaluation or Rust/system builds. A Rust-only change
does not build the separate Research service. Worker-module changes use the
same focused package/runtime gates: the Research fixture enables no workers.
Mixing these changes with shared security paths retains the full gate.
Non-VM package builds run in
groups of at most four. Configuration evaluation still prepares the exact
Research closures because the full flake/module fixtures need them; shared
configuration changes still select the applicable runtime and Research suites.
Onboarding validation uses the installed fixture rather than full flake checks;
its CLI binary is a small selected package or an explicitly supplied local binary.
The standalone host build runs only for configuration changes affecting the
runtime, not for a Research-only or documentation change. Full VM/ISO/system
workloads remain separate from local fast checks and require their own scope.
New pushes cancel superseded runs for the same PR or branch and event type;
manual runs do not cancel automatic runs.

### Temporary VM pause

Automatic runtime and Research VM jobs are paused by default on PRs and `main`.
Non-VM selection, evaluation, builds, lint and unit checks remain active. The
`changes` job summary states when affected VM suites were skipped. Green non-VM
CI does not establish runtime/security acceptance for an untested change.

To run both suites once, select **Actions → Check → Run workflow**, choose the
branch or ref and enable `run_vm_tests`. Manual dispatch has no comparison base,
so it conservatively selects the complete gate, including both VM suites when
enabled. Without the opt-in, the VM job remains skipped unless automatic runs
have been restored.

To restore automatic affected-suite execution, set the repository Actions
variable `RUN_VM_TESTS` to the exact string `true` under **Settings → Secrets and
variables → Actions → Variables**. Removing it or setting it to `false` pauses
automatic VM execution again; no workflow edit is needed. Before a release or
host activation, obtain the applicable runtime/Research acceptance on the
candidate commit. Keep skipped evidence explicit in PRs and issue delivery.

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
