# tentaflake dev commands — run `just` to list them.
# Fast checks only; building the host or a VM is an explicit, manual step.

# Use the pinned contributor tools even from an ordinary shell.
set shell := ["nix", "develop", "--no-write-lock-file", "--command", "bash", "-euo", "pipefail", "-c"]
export TENTAFLAKE_NO_BANNER := "1"

# List recipes
default:
    @just --list

alias list := default

# ── The gates ────────────────────────────────────────────────

# Fast source checks: formatting, lint, Rust, offline catalog checks
fast: formatter-test lint rust image-catalog
    actionlint .github/workflows/*.yml

# Fast checks plus flake evaluation without building (the CI gate)
check: fast eval

# Unit tests
test:
    cargo test --workspace
    python3 scripts/test_image_catalog.py

# Evaluate every flake output without building; force all shared constants,
# which the lazy flake check would not touch
eval:
    nix flake check --no-build
    nix eval --json --file lib/constants.nix > /dev/null

# Build the host system closure (explicit; not part of CI)
build:
    nix build .#nixosConfigurations.tentaflake.config.system.build.toplevel --no-link

# Verify base-image pins against checked-in registry metadata without acquisition
image-catalog:
    python3 scripts/test_image_catalog.py

# ── Runtime image (needs Docker; not part of `check`) ────────

# Build the runtime image
runtime-image:
    docker build --tag tentaflake-runtime:dev images/runtime

# Build it and smoke-test it, under gVisor when Docker has runsc (CI runs it when the image changes)
runtime-smoke:
    scripts/runtime-smoke.sh

# ── Formatting & lint ────────────────────────────────────────

# Format the tree in place (nixfmt via `nix fmt`)
fmt:
    nix fmt -- --tree-root .
    cargo fmt --all

# Read-only check of tracked and non-ignored untracked regular Nix files
fmt-check:
    git ls-files -z --cached --others --exclude-standard -- '*.nix' | while IFS= read -r -d '' file; do if [[ -f "$file" && ! -L "$file" ]]; then nixfmt --check "./$file"; fi; done
    cargo fmt --all -- --check

# Prove the formatter gate fails without rewriting its own fixture
formatter-test: fmt-check
    python3 scripts/test_format_check.py

# Nix lint: statix (anti-patterns) + deadnix (dead bindings)
lint:
    statix check .
    deadnix --fail .

# ── Rust workspace ───────────────────────────────────────────

rust:
    cargo clippy --workspace --all-targets -- -D warnings
    cargo test --workspace

# ── Release ──────────────────────────────────────────────────

# Cut a release tag. Update CHANGELOG.md FIRST, then: just tag v0.5.0
# The git tag is the source of truth for the repo version.
tag VERSION:
    @test -z "$(git status --porcelain)" || { echo "working tree dirty (staged, unstaged or untracked) — commit first"; exit 1; }
    @grep -q "## \[{{ replace(VERSION, 'v', '') }}\]" CHANGELOG.md \
        || { echo "no CHANGELOG.md section for {{ VERSION }} — write it first"; exit 1; }
    git tag -a {{ VERSION }} -m "{{ VERSION }}"
    @echo "==> tagged {{ VERSION }}. Push with: git push origin {{ VERSION }}"
