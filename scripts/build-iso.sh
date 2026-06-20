#!/usr/bin/env bash
# ────────────────────────────────────────────────────────────
# Build the NixOS Agent Orchestration installer ISO
# ────────────────────────────────────────────────────────────
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_DIR="$(dirname "$SCRIPT_DIR")"

cd "$REPO_DIR"

echo "==> Building installer ISO ..."
echo "    Repo: $REPO_DIR"
echo ""

nix build ".#nixosConfigurations.installer-iso.config.system.build.isoImage" \
  --extra-experimental-features "nix-command flakes" \
  "$@"

echo ""
echo "==> Done! ISO written to:"
ls -lh result/iso/nixos-agent-orchestration.iso 2>/dev/null || {
  find result -name "*.iso" -ls 2>/dev/null || echo "(check result/ for ISO)"
}
