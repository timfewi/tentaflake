# tentaflake dev commands — run `just` to list them.
# Fast source checks and explicitly selected build/runtime gates.

# Use the pinned contributor tools even from an ordinary shell.
set shell := ["nix", "develop", "--no-write-lock-file", "--command", "bash", "-euo", "pipefail", "-c"]
export TENTAFLAKE_NO_BANNER := "1"

# List recipes
default:
    @just --list

alias list := default

# ── The gates ────────────────────────────────────────────────

# Full local gate: source checks, all flake checks and the installer ISO
ci: fmt-check lint shellcheck rust ci-policy api-contract check generated-flake iso-installer
    @echo "==> all green"

# Fast source checks; evaluation and all Nix package/runtime builds are explicit
fast: fmt-check lint shellcheck rust ci-policy api-contract

# Validate the versioned host API and all offline contract exchanges
api-contract:
    python3 scripts/test_api_contract.py

# Test routing with real Git histories and validate workflow syntax
ci-policy:
    python3 scripts/test_ci_changes.py
    python3 scripts/test_eval_flake.py
    actionlint .github/workflows/*.yml

# Materialize exact research closures before read-only flake evaluation
eval:
    for check in research-policy module-evaluation agent-adapters; do nix build --no-link ".#checks.x86_64-linux.$check"; done
    python3 scripts/eval_flake.py

# Automated end-to-end gate (alias for the complete local CI path)
e2e: ci

# Rebuild the locked Dev Container and run its contributor-shell smoke test
e2e-devcontainer:
    ./scripts/e2e-devcontainer.sh

# Scan tracked source and locked dependencies for security findings
security:
    ./scripts/security-scan.sh

# Build and boot the installer ISO with one isolated QCOW2 disk
e2e-installer:
    ./scripts/e2e-installer-vm.sh install

# Boot the disposable VM created by `just e2e-installer`
e2e-run-vm:
    ./scripts/e2e-installer-vm.sh boot

# nix flake check (eval + host + Rust packages + VM test)
check:
    nix flake check

# Build just the tentaflake system (the toplevel CI builds), no symlink
build:
    nix build .#nixosConfigurations.tentaflake.config.system.build.toplevel --no-link

# ── Formatting & lint ────────────────────────────────────────

# Format the tree in place (nixfmt via `nix fmt`)
fmt:
    nix fmt -- --tree-root .

# Format check only — fails if anything is unformatted (CI mode)
fmt-check:
    nix fmt -- --tree-root . --ci

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
        || { echo "no CHANGELOG.md section for {{ VERSION }} — write it first"; exit 1; }
    git tag -a {{ VERSION }} -m "{{ VERSION }}"
    @echo "==> tagged {{ VERSION }}. Push with: git push origin {{ VERSION }}"
