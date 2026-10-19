#!/usr/bin/env bash
# Rebuild the repository's Dev Container from its frozen lock and run a
# contributor-shell smoke test inside it.
set -euo pipefail

repo_dir=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)
cd "$repo_dir"

if [ -n "${TENTAFLAKE_CONTAINER_RUNTIME:-}" ]; then
  runtime=$(command -v "$TENTAFLAKE_CONTAINER_RUNTIME" 2>/dev/null || true)
elif command -v podman >/dev/null 2>&1; then
  runtime=$(command -v podman)
elif command -v docker >/dev/null 2>&1; then
  runtime=$(command -v docker)
else
  echo "error: Podman or Docker is required for the Dev Container E2E test" >&2
  exit 1
fi

if [ -z "$runtime" ]; then
  echo "error: TENTAFLAKE_CONTAINER_RUNTIME is not an executable command" >&2
  exit 1
fi

devcontainer_root=$(nix build \
  --no-link \
  --print-out-paths \
  path:.#devcontainer-cli)
devcontainer="$devcontainer_root/bin/devcontainer"

echo "==> Rebuilding Dev Container with the frozen feature lock"
"$devcontainer" up \
  --workspace-folder "$repo_dir" \
  --docker-path "$runtime" \
  --remove-existing-container \
  --frozen-lockfile

echo "==> Running contributor-shell smoke test inside the Dev Container"
"$devcontainer" exec \
  --workspace-folder "$repo_dir" \
  --docker-path "$runtime" \
  bash -lc \
  'cd /workspaces/tentaflake && nix develop --command just lint'

echo "==> Dev Container E2E passed"
