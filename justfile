# tentaflake dev commands — run `just` to list them.
# Everything here mirrors CI plus the installer ISO build CI does not run.

set shell := ["bash", "-euo", "pipefail", "-c"]

# List recipes
default:
    @just --list

# ── The gates ────────────────────────────────────────────────

# Full local gate: everything CI runs + the installer ISO
ci: fmt-check lint shellcheck rust check generated-flake iso-installer
    @echo "==> all green"

# nix flake check (eval + host + Rust packages + VM test)
check:
    nix flake check

# Build just the tentaflake system (the toplevel CI builds), no symlink
build:
    nix build .#nixosConfigurations.tentaflake.config.system.build.toplevel --no-link

# ── Formatting & lint ────────────────────────────────────────

# Format the tree in place (nixfmt via `nix fmt`)
fmt:
    nix fmt

# Format check only — fails if anything is unformatted (CI mode)
fmt-check:
    nix fmt -- --ci

# Nix lint: statix (anti-patterns) + deadnix (dead bindings)
lint:
    statix check .
    deadnix --fail .

# ── Rust workspace ───────────────────────────────────────────

rust:
    cargo fmt --all -- --check
    cargo clippy --workspace --all-targets -- -D warnings
    cargo test --workspace

# ── Shell scripts ────────────────────────────────────────────

# ShellCheck the installer + helper scripts (CI parity)
shellcheck:
    shellcheck installer/*.sh scripts/*.sh

# Evaluate the flake installer.sh generates for an installed machine with a
# declarative JSON agent fixture.
generated-flake:
    ./scripts/generated-flake-test.sh

# ── ISOs ─────────────────────────────────────────────────────

# Build the installer ISO (minimal, installs to disk)
iso-installer:
    ./scripts/build-iso.sh installer

# ── Release ──────────────────────────────────────────────────

# Cut a release tag. Update CHANGELOG.md FIRST, then: just tag v0.4.0
# The git tag is the source of truth for the repo version.
tag VERSION:
    @test -z "$(git status --porcelain)" || { echo "working tree dirty (staged, unstaged or untracked) — commit first"; exit 1; }
    @grep -q "## \[{{ replace(VERSION, 'v', '') }}\]" CHANGELOG.md \
        || { echo "no CHANGELOG.md section for {{VERSION}} — write it first"; exit 1; }
    git tag -a {{VERSION}} -m "{{VERSION}}"
    @echo "==> tagged {{VERSION}}. Push with: git push origin {{VERSION}}"
