# Build boundaries and contributor checks

Start with `just fast`. It runs formatting, Nix lint, ShellCheck, Rust checks,
the CI-selection/source-boundary regressions, read-only flake evaluation and
the generated installed flake, including focused adapter evaluation.
`checks.*.agent-adapters` covers compatible mappings, JSON, inventory, lazy
selection, stopped OpenClaw and negative host policy. Required research closure preparation uses
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

| Change | Non-VM checks | VM suite |
| --- | --- | --- |
| Docs and listed presentation metadata | Routing regression and whitespace; no Nix setup/cache | None |
| CI routing/workflow/test definition | Routing regression, focused selection package and static checks | None |
| One Rust crate/package | Rust workspace checks, that package and source-boundary check; worker includes its image | Runtime |
| Shared Cargo inputs | Rust workspace and all three packages/image; no Research/configuration preparation | Runtime |
| Installer scripts or host VM fixture | Static checks and generated installed flake | Runtime |
| Installer Nix | Configuration evaluation, static checks and generated installed flake | Runtime |
| Research module/client/fixture or research-only input pin | Research/configuration preparation and evaluation, static checks and generated installed flake | Research |
| Dev Container inputs | Dev Container package and static checks | None |
| Generated-flake script or module-evaluation fixture | Its generated-flake or configuration gate and static checks | None |
| Adapters, shared modules/helpers, other lock updates, unknown paths/history | Full gate | Both |

The selector reads NUL-delimited Git paths and includes deleted/old rename
paths. Invalid lock data requires the full gate. The real-Git regression check
runs on every event directly on the runner; `checks.*.ci-vm-selection` also
packages it for the contributor gate. CI metadata changes are covered by these
regressions rather than building unchanged guest systems.

Documentation does not start Nix installation, cache upload, Research closure
materialization, full flake evaluation or Rust/system builds. A Rust-only change
does not build the separate Research service. Non-VM package builds run in
groups of at most four. Configuration evaluation still prepares the exact
Research closures because the full flake/module fixtures need them; shared
configuration changes retain their applicable runtime and Research evidence.
The standalone host build runs only for configuration changes affecting the
runtime, not for a Research-only or documentation change. Full VM/ISO/system
workloads remain separate from local fast checks and require their own scope.
New pushes cancel superseded runs for the same PR or branch.

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
