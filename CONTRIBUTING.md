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

`just ci` mirrors the local gate and adds the installer build. GitHub-only
security services may add checks that cannot be reproduced locally.

The GitHub VM integration job is skipped when a pull request or push changes
only Markdown documentation. It continues to run conservatively for every
other path, including Nix, Rust, tests, installer, and workflow changes.

`just security` runs the pinned Semgrep CLI and rule snapshot against tracked
source, then checks both `Cargo.lock` and the patched Dev Containers CLI
`yarn.lock` against OSV's current advisory database. Semgrep does not contact
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

## Adding modules

Core modules belong in `modules/` and must be imported by
`modules/default.nix`. Optional integrations belong in `modules/optional/`;
profiles belong in `modules/profiles/`. Export optional modules explicitly from
`flake.nix` and document their trust boundary.

## Signing and licensing

Every non-merge commit needs a `Signed-off-by:` line under the Developer
Certificate of Origin in [DCO.txt](DCO.txt). Contributions are provided under
the MIT license. The name and logo are governed separately by
[TRADEMARK.md](TRADEMARK.md).

Report vulnerabilities through GitHub Security Advisories, not public issues.
