#!/usr/bin/env bash
# ────────────────────────────────────────────────────────────
# Build the Tentaflake installer ISO
# ────────────────────────────────────────────────────────────
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_DIR="$(dirname "$SCRIPT_DIR")"

cd "$REPO_DIR"

# `installer` remains accepted for compatibility with `just iso-installer`.
TARGET="${1:-installer}"
case "$TARGET" in
installer)
	FLAKE_REF=".#installer-iso"
	ISO_PREFIX="tentaflake"
	DESC="Installer ISO (minimal, for installing to disk)"
	;;
*)
	echo "Usage: $0 [installer] [nix build args...]"
	exit 1
	;;
esac
if [ "$#" -gt 0 ]; then
	shift
fi

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
