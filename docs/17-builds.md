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

## GitHub VM selection

The imported policy is `.github/vm-paths.json`; its consumer is
`scripts/ci_vm_changes.py`. GitHub selects runtime/research independently.

| Change | VM suite |
| --- | --- |
| Explicitly listed docs/contributor-only paths | None |
| Runtime Rust, package, installer or host VM fixture | Runtime |
| Research module/client/fixture or research-only input pin | Research |
| Adapters, shared modules/helpers, other lock updates, unknown paths | Both |
| Selection-policy changes or unavailable comparison history | Both |

The selector reads NUL-delimited Git paths and includes deleted/old rename
paths. Runtime paths take priority over a documentation suffix. Invalid lock
data requires both suites. The real-Git regression check runs on every CI
change, alongside normal static, package, policy and evaluation gates.

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
