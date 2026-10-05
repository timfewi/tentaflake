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
the selected crate's complete source. Editing CLI source does not rebuild the
broker or worker. Package versions come from `[workspace.package]` in
`Cargo.toml`; descriptions come from each crate manifest. The worker image tag
uses that same version, and its default reference follows the image metadata.
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
nix flake check
```

Installer image when its path changes:

```bash
nix build .#installer-iso
```

Start with `just fast`: formatting, lint, ShellCheck, Rust checks, CI selection
regressions, read-only flake evaluation and the generated installed flake.
It builds the necessary non-VM policy closures but runs no VM or ISO builds.
Run affected VM suites for runtime/security changes; `just ci` runs the full
local gate and adds the installer build. GitHub-only
security services may add checks that cannot be reproduced locally.

The isolated research module computes exact package closures for its read-only
mounts during evaluation. A cold `nix flake check --no-build` evaluates read-only
and cannot instantiate those derivations. CI first runs these non-VM gates:

```bash
nix build --no-link \
  .#checks.x86_64-linux.research-policy \
  .#checks.x86_64-linux.module-evaluation
nix flake check --no-build
```

The normal `nix flake check` permits the required builds. Preparation retains
the exact closure boundary; it does not mount the entire Nix store into agents.

The non-VM CI build includes the pinned Dev Containers CLI package so source,
lockfile, and offline-cache hash drift fail before a change lands.

GitHub selects builds, evaluation, static checks and runtime/research VM suites
through `.github/ci-paths.json` and `scripts/ci_changes.py`. Documentation skips
Nix setup, Research preparation, package/system builds and VMs. CI routing changes
run real-Git regressions and static checks without configuration or VM builds.
Rust changes run the workspace checks and affected package/image builds without
Research preparation; applicable runtime changes still select the host VM.
Research-specific changes and a research-only lock pin select its own suite.
Shared Nix modules, other lock changes, unknown paths and missing comparison
history retain the full gate. Renames include the old path. Routing regressions
run on every event; other checks run when their owning inputs change.
See [build boundaries](docs/17-builds.md) for cache guarantees and the
Nix/Bazel assessment.

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
