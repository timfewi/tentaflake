#!/usr/bin/env bash
# ────────────────────────────────────────────────────────────
# Build the Tentaflake live agent ISO
# ────────────────────────────────────────────────────────────
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_DIR="$(dirname "$SCRIPT_DIR")"

cd "$REPO_DIR"

# Default to live-agent-iso; pass "installer" as arg for installer ISO
TARGET="${1:-live}"
case "$TARGET" in
live)
	FLAKE_REF=".#live-agent-iso"
	ISO_PREFIX="tentaflake-live"
	DESC="Live agent ISO (Hermes + Piper TTS out of the box)"
	;;
installer)
	FLAKE_REF=".#installer-iso"
	ISO_PREFIX="tentaflake"
	DESC="Installer ISO (minimal, for installing to disk)"
	;;
*)
	echo "Usage: $0 [live|installer]"
	exit 1
	;;
esac

echo "==> Building $DESC ..."
echo "    Repo: $REPO_DIR"
echo ""

nix build "$FLAKE_REF" \
	--extra-experimental-features "nix-command flakes" \
	"$@"

echo ""
echo "==> Done! ISO written to:"
ls -lh "result/iso/${ISO_PREFIX}.iso" 2>/dev/null || {
	find result -name "*.iso" -ls 2>/dev/null || echo "(check result/ for ISO)"
}
