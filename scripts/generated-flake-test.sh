#!/usr/bin/env bash
# Evaluate the flake.nix that installer.sh generates for the installed system.
#
# Why this exists: the generated flake is a hand-written copy of the repo's
# specialArgs, and configuration.nix consumes those helpers inside `imports`.
# A helper the generated flake forgets to pass therefore does NOT fail with
# "called without required argument" — Nix falls back to config._module.args,
# which needs config, which needs imports, and the rebuild dies with
# "infinite recursion encountered". Exercise nonempty declarations so lazy
# evaluation cannot hide a missing helper behind the safe empty inventory.
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

# Exercise the installer's actual guarded initialization, not a copied recipe.
# Keep refusal ahead of all target copies, not merely ahead of inventory writes.
python3 - "$REPO_DIR/installer/installer.sh" <<'PY'
from pathlib import Path
import sys

source = Path(sys.argv[1]).read_text()
guard = source.index("# Never enable bundled examples")
copy = source.index("# Copy the declarative system")
assert guard < copy
assert source.index('if [ -e "$TARGET_NIXOS/agents.json" ]', guard) < copy
assert source.index('if [ -L "$TARGET_NIXOS/agents.toml" ]', guard) < copy
PY
sed -n '/^# Never enable bundled examples/,/^# Copy the declarative system/p' \
  "$REPO_DIR/installer/installer.sh" >"$WORK/initialize-agents.sh"
test -s "$WORK/initialize-agents.sh"
export TARGET_NIXOS
die() { printf '%s\n' "$*" >&2; exit 1; }
export -f die
bash "$WORK/initialize-agents.sh"
python3 -c 'import pathlib, sys, tomllib; assert tomllib.loads(pathlib.Path(sys.argv[1]).read_text()) == {"schemaVersion": 1, "agents": []}' "$TARGET_NIXOS/agents.toml"
test ! -e "$TARGET_NIXOS/my-agents.nix"
test ! -e "$TARGET_NIXOS/agents.json.example"
test ! -e "$TARGET_NIXOS/agents.toml.example"

# Unsafe paths must refuse without touching external files or enabling samples.
mkdir "$WORK/unsafe-inventory"
printf '%s\n' 'outside inventory sentinel' >"$WORK/outside-inventory"
cp "$WORK/outside-inventory" "$WORK/outside-before"
ln -s "$WORK/outside-inventory" "$WORK/unsafe-inventory/agents.toml"
if TARGET_NIXOS="$WORK/unsafe-inventory" bash "$WORK/initialize-agents.sh"; then
  echo "FAIL: accepted inventory symlink" >&2
  exit 1
fi
cmp "$WORK/outside-before" "$WORK/outside-inventory"
rm "$WORK/unsafe-inventory/agents.toml"
ln -s "$WORK/missing-outside" "$WORK/unsafe-inventory/agents.toml"
if TARGET_NIXOS="$WORK/unsafe-inventory" bash "$WORK/initialize-agents.sh"; then
  echo "FAIL: accepted dangling inventory symlink" >&2
  exit 1
fi
test ! -e "$WORK/missing-outside"
rm "$WORK/unsafe-inventory/agents.toml"
mkdir "$WORK/unsafe-inventory/agents.toml"
if TARGET_NIXOS="$WORK/unsafe-inventory" bash "$WORK/initialize-agents.sh"; then
  echo "FAIL: accepted inventory directory" >&2
  exit 1
fi
test -d "$WORK/unsafe-inventory/agents.toml"

# A legacy inventory must never be ignored, even beside an existing TOML file.
mkdir "$WORK/legacy-inventory"
printf '%s\n' '{"schemaVersion":1,"agents":[]}' >"$WORK/legacy-inventory/agents.json"
cp "$WORK/legacy-inventory/agents.json" "$WORK/legacy-before"
for existing_toml in no yes; do
  if [ "$existing_toml" = yes ]; then
    cp "$TARGET_NIXOS/agents.toml" "$WORK/legacy-inventory/agents.toml"
  fi
  if TARGET_NIXOS="$WORK/legacy-inventory" bash "$WORK/initialize-agents.sh"; then
    echo "FAIL: ignored legacy JSON inventory" >&2
    exit 1
  fi
  cmp "$WORK/legacy-before" "$WORK/legacy-inventory/agents.json"
  if [ "$existing_toml" = yes ]; then
    cmp "$TARGET_NIXOS/agents.toml" "$WORK/legacy-inventory/agents.toml"
  else
    test ! -e "$WORK/legacy-inventory/agents.toml"
  fi
done
rm "$WORK/legacy-inventory/agents.json" "$WORK/legacy-inventory/agents.toml"
for legacy_kind in symlink dangling directory; do
  case "$legacy_kind" in
  symlink) ln -s "$WORK/outside-inventory" "$WORK/legacy-inventory/agents.json" ;;
  dangling) ln -s "$WORK/missing-outside" "$WORK/legacy-inventory/agents.json" ;;
  directory) mkdir "$WORK/legacy-inventory/agents.json" ;;
  esac
  if TARGET_NIXOS="$WORK/legacy-inventory" bash "$WORK/initialize-agents.sh"; then
    echo "FAIL: ignored legacy inventory $legacy_kind" >&2
    exit 1
  fi
  test ! -e "$WORK/legacy-inventory/agents.toml"
  if [ "$legacy_kind" = directory ]; then
    rmdir "$WORK/legacy-inventory/agents.json"
  else
    test -L "$WORK/legacy-inventory/agents.json"
    rm "$WORK/legacy-inventory/agents.json"
  fi
done
cmp "$WORK/outside-before" "$WORK/outside-inventory"
test ! -e "$WORK/missing-outside"

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
  # Host-owned unmanaged OCI fixture: this is not a supported agent input.
  # Its name must still conflict with incoming TOML when active TSV is empty.
  virtualisation.oci-containers.containers.agent-unmanaged-fixture = {
    image = "example.invalid/unmanaged@sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    autoStart = false;
  };
}
EOF

# All multi-runtime declarations come from the single public TOML inventory.
cat >"$TARGET_NIXOS/agents.toml" <<'EOF'
schemaVersion = 1

[[agents]]
adapter = "hermes"
name = "generic-toml"
autoStart = false

[[agents]]
adapter = "openclaw"
name = "assistant"
autoStart = false

[[agents]]
adapter = "zeroclaw"
name = "ops"
autoStart = false
EOF
cp "$TARGET_NIXOS/agents.toml" "$WORK/existing-agents.toml"
bash "$WORK/initialize-agents.sh"
cmp "$WORK/existing-agents.toml" "$TARGET_NIXOS/agents.toml"

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

echo "Evaluating generated flake (versioned agents.toml present) ..."
if nix eval --no-write-lock-file \
  "$TARGET_NIXOS#nixosConfigurations.$HOSTNAME_T.config.system.build.toplevel.drvPath" \
  >"$WORK/out" 2>"$WORK/err"; then
  echo "PASS: $(cat "$WORK/out")"
else
  echo "FAIL: generated flake does not evaluate with an agent configured." >&2
  tail -30 "$WORK/err" >&2
  exit 1
fi

# Unexpected function arguments are Nix evaluation errors, not catchable
# tryEval throws. Exercise rejected public/programmatic calls out of process.
for rejected in public-data public-builder internal-builder; do
  if nix eval --impure --json --no-write-lock-file --expr '
    { root, rejected }:
    let
      flake = builtins.getFlake root;
      pkgs = flake.inputs.nixpkgs.legacyPackages.x86_64-linux;
      public = (import (root + "/lib") { inherit pkgs; inherit (pkgs) lib; }).agentsFromData;
      internal = import (root + "/lib/agentsFromData.nix") { inherit pkgs; inherit (pkgs) lib; };
      file = root + "/agents.toml";
      data = { schemaVersion = 1; agents = []; };
    in { result =
      if rejected == "public-data" then public { inherit file data; }
      else if rejected == "public-builder" then public { inherit file; mkAgent = _: {}; }
      else internal { inherit data; mkAgent = _: {}; };
    }' result --argstr root "$TARGET_NIXOS" --argstr rejected "$rejected" \
    >"$WORK/rejected.out" 2>"$WORK/rejected.err"; then
    echo "FAIL: accepted removed instance API: $rejected" >&2
    exit 1
  fi
  grep -q 'unexpected argument' "$WORK/rejected.err"
done

# Exercise the real operator CLI against an installed, balanced source fixture.
# No VM, host activation, OCI image or Research closure is involved.
printf '%s\n' 'schemaVersion = 1' 'agents = []' >"$TARGET_NIXOS/agents.toml"
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
