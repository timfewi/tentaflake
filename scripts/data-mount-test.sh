#!/usr/bin/env bash
# Exercises the stage_to_usb() migration in installer/firstboot.nix against real
# directories. That function replaces an agent's state dir with a symlink to a
# USB dir, so a bug there deletes agent state — the one thing in this repo worth
# a test that actually runs the code instead of reading it.
#
# The function is extracted from the Nix string verbatim (unescaping ''${ -> ${)
# so the test cannot drift from the shipped script.
#
#   ./scripts/data-mount-test.sh
set -euo pipefail

repo=$(cd "$(dirname "$0")/.." && pwd)
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

# Extract stage_to_usb() from the Nix string and undo Nix's ''${ escaping.
sed -n '/^    stage_to_usb() {$/,/^    }$/p' "$repo/installer/firstboot.nix" \
  | sed "s/''\\\${/\${/g" > "$tmp/fn.sh"
[ -s "$tmp/fn.sh" ] || { echo "FAIL: could not extract stage_to_usb from firstboot.nix"; exit 1; }
# shellcheck source=/dev/null
. "$tmp/fn.sh"

fail() { echo "FAIL: $1"; exit 1; }

# ── case 1: USB empty, local has state -> migrate, symlink, data preserved ──
src=$tmp/c1/src dst=$tmp/c1/dst
mkdir -p "$src/sub" "$dst"
echo hello > "$src/sub/a.txt"
chmod 700 "$src"
stage_to_usb "$src" "$dst" || fail "case1: migration returned nonzero"
[ -L "$src" ] || fail "case1: src is not a symlink"
[ "$(cat "$src/sub/a.txt")" = hello ] || fail "case1: data not readable through symlink"
[ "$(cat "$dst/sub/a.txt")" = hello ] || fail "case1: data not on the stick"

# ── case 2: local empty, USB has state -> USB wins, nothing lost ──
src=$tmp/c2/src dst=$tmp/c2/dst
mkdir -p "$src" "$dst"
echo persisted > "$dst/b.txt"
chmod 700 "$src"
stage_to_usb "$src" "$dst" || fail "case2: migration returned nonzero"
[ -L "$src" ] || fail "case2: src is not a symlink"
[ "$(cat "$src/b.txt")" = persisted ] || fail "case2: USB state not visible"

# ── case 3: BOTH non-empty -> refuse, and leave local state untouched ──
# This is the regression guard: the pre-fix code ran `rm -rf "$src"` here with
# nothing copied, silently destroying the agent's state dir.
src=$tmp/c3/src dst=$tmp/c3/dst
mkdir -p "$src" "$dst"
echo local > "$src/c.txt"
echo stick > "$dst/c.txt"
chmod 700 "$src"
stage_to_usb "$src" "$dst" && fail "case3: expected refusal, got success"
[ -d "$src" ] || fail "case3: local state dir was destroyed"
[ ! -L "$src" ] || fail "case3: local state dir was replaced by a symlink"
[ "$(cat "$src/c.txt")" = local ] || fail "case3: local data was lost"

# ── case 3b: resume after a PARTIALLY copied stick -> refuse ──
# A first migration that died inside `cp -a` (full stick, I/O error, power cut)
# leaves a truncated copy on the USB. Completeness must never be inferred from
# "the target is non-empty", or this boot silently swaps the complete local dir
# for the truncated one and reports success.
src=$tmp/c3b/src dst=$tmp/c3b/dst
mkdir -p "$src/workspace" "$dst"
echo a > "$src/IMPORTANT.md"; echo b > "$src/SOUL.md"; echo c > "$src/workspace/code.py"
echo a > "$dst/IMPORTANT.md"   # only the first file made it across
chmod 700 "$src"
stage_to_usb "$src" "$dst" && fail "case3b: accepted a partial copy as authoritative"
[ ! -L "$src" ] || fail "case3b: local dir replaced by a symlink to a truncated copy"
[ -f "$src/SOUL.md" ] && [ -f "$src/workspace/code.py" ] || fail "case3b: local data was lost"

# ── case 4: unwritable USB target -> refuse, leave local state untouched ──
# Stands in for the vfat/exfat and read-only-stick paths: the ownership mirror
# fails, so we must skip rather than strand the agent on a dir it cannot write.
if [ "$(id -u)" -ne 0 ]; then
  src=$tmp/c4/src dst=$tmp/c4/nope/dst
  mkdir -p "$src" "$tmp/c4/nope"
  echo local > "$src/d.txt"
  chmod 700 "$src"; chmod 500 "$tmp/c4/nope"
  stage_to_usb "$src" "$dst" && fail "case4: expected refusal on unwritable target"
  [ -d "$src" ] && [ ! -L "$src" ] || fail "case4: local state dir was touched"
  [ "$(cat "$src/d.txt")" = local ] || fail "case4: local data was lost"
  chmod 700 "$tmp/c4/nope"
else
  echo "  (case4 skipped: running as root, permission checks do not apply)"
fi

echo "OK: stage_to_usb migrates, honors existing USB state, and refuses to discard data"
