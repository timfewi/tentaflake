# Contributing to tentaflake

Tentaflake is a generic template. Company configuration, real hostnames,
hardware profiles, API keys, private agent context, and deployment-specific
policy belong in forks.

## Development setup

```bash
git clone \
  https://github.com/timfewi/tentaflake
cd tentaflake
nix develop
```

The development shell provides Nix tooling, Rust, Cargo, Clippy, Rustfmt,
ShellCheck, Statix, Deadnix, and `just`.
The `just` recipes enter this pinned environment automatically, including when
called from an ordinary shell. `just list` lists them. Direct Cargo/lint commands
still require the contributor shell.

The three Rust packages share `lib/mkRustPackage.nix`. Each build source
contains the root Cargo manifest and lockfile, all workspace manifests, and only
the selected crate's complete source and local dependencies. The CLI includes
and tests the shared admission library. Editing CLI source does not rebuild the
broker or worker. Package versions come from `[workspace.package]` in
`Cargo.toml`; descriptions come from each crate manifest. The worker image tag
uses that same version, and its default reference follows the image metadata.
The CLI also embeds `adapters/catalog.json`; broker and worker sources exclude
that catalog so preset edits preserve their package caches.
Keep additional build-time files inside their crate, or explicitly add them
to the shared fileset when a build requires a root-level input.

As an alternative to installing Nix on the host, open the repository in a
Dev Container-compatible editor and select **Reopen in Container**. The
container installs Nix and preloads this same `nix develop` environment. Its
base image and Nix feature are digest-locked; update
`.devcontainer/devcontainer-lock.json` together with intentional feature
updates. No container-runtime socket or credentials are mounted automatically.

## Required checks

Rust workspace:

```bash
cargo fmt --all -- --check
cargo clippy --workspace \
  --all-targets -- -D warnings
cargo test --workspace
```

Nix and shell:

```bash
nix fmt -- --ci
statix check .
deadnix --fail .
shellcheck installer/*.sh scripts/*.sh
just ci-policy
```

Installer image when its path changes:

```bash
nix build .#installer-iso
```

Start with `just fast`: formatting, lint, ShellCheck, Rust formatting/Clippy/unit
tests, real-Git CI routing regressions and actionlint. It does not prepare
Research closures, evaluate configurations or generated flakes, or build Nix
packages, images, systems or VMs. The pinned development environment may still
need to realize its tools. Run `just eval` and `just generated-flake` explicitly
for configuration changes, and affected VM suites for runtime/security changes.
`just ci` runs the full local gate, including the installer build. GitHub-only
security services may add checks that cannot be reproduced locally.

The isolated research module computes exact package closures for its read-only
mounts during evaluation. A cold `nix flake check --no-build` evaluates read-only
and cannot instantiate those derivations. The explicit evaluation gate first
prepares the non-VM closures (`just eval` also includes adapter checks):

```bash
nix build --no-link \
  .#checks.x86_64-linux.research-policy \
  .#checks.x86_64-linux.module-evaluation
nix flake check --no-build
```

The normal `nix flake check` permits the required builds. Preparation retains
the exact closure boundary; it does not mount the entire Nix store into agents.

GitHub uses `.github/ci-paths.json` and `scripts/ci_changes.py` for both modes:

- Automatic PR/push runs always check real-Git routing regressions and whitespace,
  then affected formatting/lint (including actionlint), Rust workspace formatting
  and affected crate Clippy/unit tests. Unknown paths or unavailable comparison
  history select all fast checks. These runs never select configuration evaluation,
  Research closure preparation, Nix package/image/system builds or VM suites.
- Explicit `workflow_dispatch` runs select affected build, configuration and
  runtime/Research VM gates as well as fast checks. Set optional `base_sha` to
  a full 40-character comparison SHA. An empty, invalid or unavailable SHA
  selects the full inventory. A known comparison includes deleted paths and
  both sides of renames; unknown paths or invalid lock data also select the
  full inventory. Manual and automatic runs have separate concurrency groups.

Before merging, decide which build/runtime evidence the change requires and
obtain it for the current revision. A green automatic run or skipped build is
not runtime acceptance. Dev Container changes need their pinned package build
to detect source, lockfile and offline-cache hash drift. DCO and secret scanning
remain enabled. See [build boundaries](docs/17-builds.md) for dispatch selection,
cache guarantees and the Nix/Bazel assessment.

`just security` first checks `Cargo.lock` and the patched Dev Containers CLI
`yarn.lock` against OSV's current advisory database, then runs the pinned
Semgrep CLI and rule snapshot against tracked source. Semgrep does not contact
its registry or send metrics; the OSV portion needs network access for current
advisories, so it remains a separate pre-PR gate rather than part of `just ci`.

For end-to-end verification, `just e2e` runs that complete automated gate,
`just e2e-devcontainer` uses the source- and dependency-hash-pinned
`.#devcontainer-cli`, rebuilds the frozen Dev Container, and runs its Nix lint
smoke test. `just e2e-installer` starts the interactive installer with one
isolated QCOW2 disk; afterward, `just e2e-run-vm` boots the same VM without
the ISO. The VM state is contributor-owned below
`/var/tmp/tentaflake-e2e-<user>/`; no host block device is passed to QEMU.

Keep source evaluation, builds, activation, and live-runtime verification
separate. Contributions must not activate NixOS, deploy, or mutate a VM as a
side effect of testing.

## Conventions

| Area | Convention |
|---|---|
| Commits | Conventional Commits |
| Nix | `nix fmt` and two-space indent |
| Rust | Rustfmt, Clippy warnings denied |
| Shell | ShellCheck |
| Sign-off | DCO on every non-merge commit |

Create signed-off commits with:

```bash
git commit -s -m "feat: add capability"
```

## Pull requests

1. Link the change to an issue or concrete requirement.
2. Keep the change focused and generic.
3. Add or update focused verification.
4. Synchronize README, `docs/`, agent instructions, and bundled skills.
5. Fill every applicable part of the PR template.
6. Leave inapplicable checklist entries unchecked.

Do not stage, overwrite, or remove unrelated work in a dirty checkout.

## Documentation and website

Update product behavior and examples in this public repository. The website
mirrors a pinned release; contributors do not need website access. Identify
new, renamed or removed guides in the PR so the maintainer can update routes.
See [documentation ownership](docs/18-documentation.md) for release following,
generated content, summary review and version boundaries.

## Adding modules

Core modules belong in `modules/` and must be imported by
`modules/default.nix`. Optional profiles belong in `modules/profiles/`; export
them explicitly from `flake.nix` and document their trust boundary. Editor,
Hive Research and speech integrations belong in consumer flakes. The core
secure research transport remains in `modules/research.nix`.

## Signing and licensing

Every non-merge commit needs a `Signed-off-by:` line under the Developer
Certificate of Origin in [DCO.txt](DCO.txt). Contributions are provided under
the MIT license. The name and logo are governed separately by
[TRADEMARK.md](TRADEMARK.md).

Report vulnerabilities through GitHub Security Advisories, not public issues.
