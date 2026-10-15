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

# Fast source checks: formatting, lint, Rust, control service, UI, offline catalog checks
fast: formatter-test lint rust control ui image-catalog
    actionlint .github/workflows/*.yml

# Fast checks plus flake evaluation without building (the CI gate)
check: fast eval

# Unit tests
test:
    cargo test --workspace
    cd control && bun install --frozen-lockfile && bun test
    cd ui && bun install --frozen-lockfile && bun test ./src
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

# ── UI (ui/) ─────────────────────────────────────────────────

# Biome lint, TypeScript type check, unit tests and a production build (ui/dist, not committed)
ui:
    cd ui && bun install --frozen-lockfile && biome lint . && bun run typecheck && bun test ./src && bun run build

# Browser journeys in headless Chromium: the built UI on the real control service with a fake Docker and OpenCode (no containers; not part of CI).
# playwright-core in ui/package.json must match the pinned nixpkgs' playwright-driver; set PLAYWRIGHT_BROWSERS_PATH to use browsers of that revision from elsewhere.
ui-e2e:
    cd ui && bun install --frozen-lockfile && bun run build
    export PLAYWRIGHT_BROWSERS_PATH="${PLAYWRIGHT_BROWSERS_PATH:-$(nix build --inputs-from . --no-link --print-out-paths nixpkgs#playwright-driver.browsers)}" && cd ui && bun test ./e2e

# ── Runtime image (needs Docker; not part of `check`) ────────

# Build the runtime image
runtime-image:
    docker build --tag tentaflake-runtime:dev images/runtime

# Build it and smoke-test it, under gVisor when Docker has runsc (CI runs it when the image changes)
runtime-smoke:
    scripts/runtime-smoke.sh

# ── Formatting & lint ────────────────────────────────────────

# Format the tree in place (nixfmt via `nix fmt`, Biome for the control service and the UI)
fmt:
    nix fmt -- --tree-root .
    cargo fmt --all
    cd control && biome format --write .
    cd ui && biome format --write .

# Read-only formatting check: tracked and non-ignored untracked regular Nix files, Rust, control service, UI
fmt-check:
    git ls-files -z --cached --others --exclude-standard -- '*.nix' | while IFS= read -r -d '' file; do if [[ -f "$file" && ! -L "$file" ]]; then nixfmt --check "./$file"; fi; done
    cargo fmt --all -- --check
    cd control && biome format .
    cd ui && biome format .

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

# ── Control service (control/) ───────────────────────────────

# Biome lint, TypeScript type check and unit tests
control:
    cd control && bun install --frozen-lockfile && biome lint . && bun run typecheck && bun test

# Lifecycle and model-access tests against real Docker with runsc, a built runtime image and the broker (CI runs it after the smoke test)
control-integration image="tentaflake-runtime:smoke":
    cargo build -p tentaflake-broker
    cd control && bun install --frozen-lockfile && TENTAFLAKE_INTEGRATION_IMAGE={{ image }} TENTAFLAKE_INTEGRATION_BROKER={{ justfile_directory() }}/target/debug/tentaflake-broker bun test ./test/integration.test.ts

# ── Release ──────────────────────────────────────────────────

# Cut a release tag. Update CHANGELOG.md FIRST, then: just tag v0.5.0
# The git tag is the source of truth for the repo version.
tag VERSION:
    @test -z "$(git status --porcelain)" || { echo "working tree dirty (staged, unstaged or untracked) — commit first"; exit 1; }
    @grep -q "## \[{{ replace(VERSION, 'v', '') }}\]" CHANGELOG.md \
        || { echo "no CHANGELOG.md section for {{ VERSION }} — write it first"; exit 1; }
    git tag -a {{ VERSION }} -m "{{ VERSION }}"
    @echo "==> tagged {{ VERSION }}. Push with: git push origin {{ VERSION }}"
