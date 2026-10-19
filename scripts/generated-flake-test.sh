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

# The disposable fixture must not inherit the caller's Git repository or index.
unset GIT_DIR GIT_WORK_TREE GIT_INDEX_FILE GIT_COMMON_DIR

REPO_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT
TARGET_NIXOS="$WORK/nixos"
mkdir -p "$TARGET_NIXOS"

# ── Mirror installer.sh's file layout (STEP 10) ──
cp -r "$REPO_DIR/lib" "$REPO_DIR/adapters" "$REPO_DIR/modules" \
  "$REPO_DIR/pkgs" "$REPO_DIR/crates" \
  "$TARGET_NIXOS/"
cp "$REPO_DIR/Cargo.toml" \
  "$REPO_DIR/Cargo.lock" \
  "$TARGET_NIXOS/"
cp "$REPO_DIR/configuration.nix" "$TARGET_NIXOS/configuration.nix"
cat >"$TARGET_NIXOS/my-agents.nix" <<'EOF'
{ mkAgent, mkHermesAgent }:
[
  # Legacy JSON entries below explicitly exercise dev compatibility.
  { tentaflake.security.profile = "dev"; }
  (mkAgent { adapter = "hermes"; name = "generic-nix"; autoStart = false; })
  (mkHermesAgent { name = "legacy-nix"; autoStart = false; })
]
EOF

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
  fileSystems."/" = { device = "/dev/disk/by-label/nixos"; fsType = "btrfs"; };
  boot.loader.systemd-boot.enable = true;
  nixpkgs.hostPlatform = lib.mkDefault "x86_64-linux";
}
EOF

# The trigger: one declarative JSON agent fixture.
cat >"$TARGET_NIXOS/agents.json" <<'EOF'
{
  "schemaVersion": 1,
  "agents": [
    { "adapter": "hermes", "name": "generic-json", "autoStart": false },
    { "adapter": "openclaw", "name": "assistant", "autoStart": false }
  ],
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
export RESEARCH_REV
RESEARCH_REV=$(jq -r '.nodes["tentaflake-research"].locked.rev' "$REPO_DIR/flake.lock")
# Both single-quoted strings below are deliberate: the first is literal Nix the
# heredoc must emit verbatim, the second is a sed script. Neither may expand.
# shellcheck disable=SC2016
export ADMIN_SHELL='"${pkgs.bash}/bin/bash"'
export TF_TOGGLES=""
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

echo "Evaluating generated flake (dev compatibility agents.json present) ..."
if nix eval --no-write-lock-file \
  "$TARGET_NIXOS#nixosConfigurations.$HOSTNAME_T.config.system.build.toplevel.drvPath" \
  >"$WORK/out" 2>"$WORK/err"; then
  echo "PASS: $(cat "$WORK/out")"
else
  echo "FAIL: generated flake does not evaluate with an agent configured." >&2
  tail -30 "$WORK/err" >&2
  exit 1
fi

# Exercise the real operator CLI against an installed, balanced source fixture.
# No VM, host activation, OCI image or Research closure is involved.
sed -i 's/security.profile = "dev"/security.profile = "balanced"/' "$TARGET_NIXOS/my-agents.nix"
printf '%s\n' '{"schemaVersion":1,"agents":[],"hermes":[],"zeroclaw":[]}' >"$TARGET_NIXOS/agents.json"
nix eval --impure --json --no-write-lock-file --expr \
  '{ root }: let f = builtins.getFlake root; in { result = import (root + "/tests/agent-onboarding.nix") { pkgs = f.inputs.nixpkgs.legacyPackages.x86_64-linux; }; }' result \
  --argstr root "$REPO_DIR" \
  >"$WORK/onboarding-fixtures.json"
if [[ -n "${TENTAFLAKE_CLI_TEST_BIN:-}" ]]; then
  CLI_TEST_BIN="$TENTAFLAKE_CLI_TEST_BIN"
else
  CLI_PACKAGE=$(nix build --no-link --print-out-paths --no-write-lock-file "$REPO_DIR#packages.x86_64-linux.tentaflake-cli")
  CLI_TEST_BIN="$CLI_PACKAGE/bin/tentaflake"
fi
python3 "$REPO_DIR/scripts/agent-onboarding-test.py" "$CLI_TEST_BIN" "$TARGET_NIXOS" "$HOSTNAME_T"
