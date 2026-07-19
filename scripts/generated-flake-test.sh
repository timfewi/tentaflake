#!/usr/bin/env bash
# Evaluate the flake.nix that installer.sh generates for the installed system.
#
# Why this exists: the generated flake is a hand-written copy of the repo's
# specialArgs, and configuration.nix consumes those helpers inside `imports`.
# A helper the generated flake forgets to pass therefore does NOT fail with
# "called without required argument" — Nix falls back to config._module.args,
# which needs config, which needs imports, and the rebuild dies with
# "infinite recursion encountered". That is invisible until an agent exists,
# because `lib.optionals (pathExists ./agents.json)` keeps agentsFromData
# unforced on a fresh install. So this test writes an agents.json first.
#
# Usage: ./scripts/generated-flake-test.sh
set -euo pipefail

REPO_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT
TARGET_NIXOS="$WORK/nixos"
mkdir -p "$TARGET_NIXOS"

# ── Mirror installer.sh's file layout (STEP 10) ──
cp -r "$REPO_DIR/lib" "$REPO_DIR/modules" "$TARGET_NIXOS/"
mkdir -p "$TARGET_NIXOS/public"
cp "$REPO_DIR/public/tentaflake-shell-logo.txt" "$TARGET_NIXOS/public/"
cp "$REPO_DIR/configuration.nix" "$TARGET_NIXOS/configuration.nix"
echo '{ mkHermesAgent }: [ ]' >"$TARGET_NIXOS/my-agents.nix"

HOSTNAME_T="tentaflake"
cat >"$TARGET_NIXOS/user-config.nix" <<EOF
{
  hostName   = "$HOSTNAME_T";
  userName   = "user";
  timeZone   = "UTC";
}
EOF

# profile = "installed" makes configuration.nix import this.
cat >"$TARGET_NIXOS/hardware-configuration.nix" <<'EOF'
{ lib, ... }:
{
  fileSystems."/" = { device = "/dev/disk/by-label/nixos"; fsType = "ext4"; };
  boot.loader.systemd-boot.enable = true;
  nixpkgs.hostPlatform = lib.mkDefault "x86_64-linux";
}
EOF

# The trigger: one agent, shaped exactly like `tentaflake agent add` writes it.
cat >"$TARGET_NIXOS/agents.json" <<'EOF'
{
  "hermes": [
    { "name": "coding", "provider": "openrouter", "model": "anthropic/claude-opus-4",
      "base_url": null, "envFile": "/etc/tentaflake/secrets/hermes-coding.env" }
  ],
  "zeroclaw": [
    { "name": "ops", "provider": "openrouter", "model": "anthropic/claude-opus-4",
      "base_url": null, "hostPort": 8080, "servePort": 8081,
      "envFile": "/etc/tentaflake/secrets/zeroclaw-ops.env" }
  ]
}
EOF

# ── Generate flake.nix from installer.sh's own heredoc ──
# Extracted from the real source (not a copy) so this test cannot go stale.
export NIXPKGS_REV
NIXPKGS_REV=$(jq -r '.nodes.nixpkgs.locked.rev' "$REPO_DIR/flake.lock")
# Both single-quoted strings below are deliberate: the first is literal Nix the
# heredoc must emit verbatim, the second is a sed script. Neither may expand.
# shellcheck disable=SC2016
export ADMIN_SHELL='"${pkgs.bash}/bin/bash"'
export TF_TOGGLES=""
export NVF_INPUT=""
export NVF_MODULE_LINE=""
export HOSTNAME="$HOSTNAME_T"

{
  echo 'cat <<FLAKEEOF'
  # shellcheck disable=SC2016
  sed -n '/^cat >"\$TARGET_NIXOS\/flake.nix" <<FLAKEEOF$/,/^FLAKEEOF$/p' \
    "$REPO_DIR/installer/installer.sh" | sed '1d;$d'
  echo 'FLAKEEOF'
} >"$WORK/gen.sh"
# shellcheck disable=SC1090
bash "$WORK/gen.sh" >"$TARGET_NIXOS/flake.nix"

grep -q 'nixosConfigurations' "$TARGET_NIXOS/flake.nix" ||
  { echo "FAIL: heredoc extraction produced no flake — did installer.sh move?" >&2; exit 1; }

# Path flakes snapshot their NAR hash, so commit before evaluating.
git -C "$TARGET_NIXOS" init -q
git -C "$TARGET_NIXOS" add -A
git -C "$TARGET_NIXOS" -c user.email=t@t -c user.name=t commit -q -m generated

echo "Evaluating generated flake (agents.json present) ..."
if nix eval --no-write-lock-file \
  "$TARGET_NIXOS#nixosConfigurations.$HOSTNAME_T.config.system.build.toplevel.drvPath" \
  >"$WORK/out" 2>"$WORK/err"; then
  echo "PASS: $(cat "$WORK/out")"
else
  echo "FAIL: generated flake does not evaluate with an agent configured." >&2
  tail -30 "$WORK/err" >&2
  exit 1
fi
