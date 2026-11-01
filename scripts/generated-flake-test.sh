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

# ── Run installer.sh's own source-copy block (STEP 10) ──
# Extracted from the real installer so a new required directory cannot be
# omitted from installed systems while this fixture still passes.
sed -n '/^# Copy the declarative system and Rust CLI workspace/,/^# Generate user-config.nix$/p' \
  "$REPO_DIR/installer/installer.sh" | sed '$d' >"$WORK/copy-source.sh"
grep -q 'cp ' "$WORK/copy-source.sh" ||
  { echo "FAIL: installer source-copy block not found — did installer.sh move?" >&2; exit 1; }
REPO_DIR="$REPO_DIR" TARGET_NIXOS="$TARGET_NIXOS" bash -eu "$WORK/copy-source.sh"

# Exercise the installer's actual guarded initialization, not a copied recipe.
# Keep refusal ahead of all target copies, not merely ahead of inventory writes.
python3 - "$REPO_DIR/installer/installer.sh" <<'PY'
from pathlib import Path
import sys

source = Path(sys.argv[1]).read_text()
guard = source.index("# Never enable bundled examples")
copy = source.index("# Copy the declarative system")
assert guard < copy
assert source.index('if [ -e "$TARGET_NIXOS/agents.toml" ]', guard) < copy
assert source.index('if [ -e "$TARGET_NIXOS/agents.json" ]', guard) < copy
assert source.index('if [ -L "$TARGET_NIXOS/tentaflake.toml" ]', guard) < copy
PY
sed -n '/^# Never enable bundled examples/,/^# Copy the declarative system/p' \
  "$REPO_DIR/installer/installer.sh" >"$WORK/initialize-agents.sh"
test -s "$WORK/initialize-agents.sh"
export TARGET_NIXOS
die() { printf '%s\n' "$*" >&2; exit 1; }
export -f die
bash "$WORK/initialize-agents.sh"
python3 -c 'import pathlib, sys, tomllib; assert tomllib.loads(pathlib.Path(sys.argv[1]).read_text()) == {"schemaVersion": 1, "agents": []}' "$TARGET_NIXOS/tentaflake.toml"
# Comments/examples in the public template must never enable installation data.
python3 -c 'import pathlib, sys, tomllib; assert tomllib.loads(pathlib.Path(sys.argv[1]).read_text()) == {"schemaVersion": 1, "agents": []}' "$REPO_DIR/tentaflake.toml"
test ! -e "$TARGET_NIXOS/my-agents.nix"
test ! -e "$TARGET_NIXOS/agents.json.example"
test ! -e "$TARGET_NIXOS/tentaflake.toml.example"

# Unsafe paths must refuse without touching external files or enabling samples.
mkdir "$WORK/unsafe-inventory"
printf '%s\n' 'outside inventory sentinel' >"$WORK/outside-inventory"
cp "$WORK/outside-inventory" "$WORK/outside-before"
ln -s "$WORK/outside-inventory" "$WORK/unsafe-inventory/tentaflake.toml"
if TARGET_NIXOS="$WORK/unsafe-inventory" bash "$WORK/initialize-agents.sh"; then
  echo "FAIL: accepted inventory symlink" >&2
  exit 1
fi
cmp "$WORK/outside-before" "$WORK/outside-inventory"
rm "$WORK/unsafe-inventory/tentaflake.toml"
ln -s "$WORK/missing-outside" "$WORK/unsafe-inventory/tentaflake.toml"
if TARGET_NIXOS="$WORK/unsafe-inventory" bash "$WORK/initialize-agents.sh"; then
  echo "FAIL: accepted dangling inventory symlink" >&2
  exit 1
fi
test ! -e "$WORK/missing-outside"
rm "$WORK/unsafe-inventory/tentaflake.toml"
mkdir "$WORK/unsafe-inventory/tentaflake.toml"
if TARGET_NIXOS="$WORK/unsafe-inventory" bash "$WORK/initialize-agents.sh"; then
  echo "FAIL: accepted inventory directory" >&2
  exit 1
fi
test -d "$WORK/unsafe-inventory/tentaflake.toml"

# A legacy inventory must never be ignored, even beside an existing TOML file.
mkdir "$WORK/legacy-inventory"
printf '%s\n' '{"schemaVersion":1,"agents":[]}' >"$WORK/legacy-inventory/agents.json"
cp "$WORK/legacy-inventory/agents.json" "$WORK/legacy-before"
for existing_toml in no yes; do
  if [ "$existing_toml" = yes ]; then
    cp "$TARGET_NIXOS/tentaflake.toml" "$WORK/legacy-inventory/tentaflake.toml"
  fi
  if TARGET_NIXOS="$WORK/legacy-inventory" bash "$WORK/initialize-agents.sh"; then
    echo "FAIL: ignored legacy JSON inventory" >&2
    exit 1
  fi
  cmp "$WORK/legacy-before" "$WORK/legacy-inventory/agents.json"
  if [ "$existing_toml" = yes ]; then
    cmp "$TARGET_NIXOS/tentaflake.toml" "$WORK/legacy-inventory/tentaflake.toml"
  else
    test ! -e "$WORK/legacy-inventory/tentaflake.toml"
  fi
done
rm "$WORK/legacy-inventory/agents.json" "$WORK/legacy-inventory/tentaflake.toml"
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
  test ! -e "$WORK/legacy-inventory/tentaflake.toml"
  if [ "$legacy_kind" = directory ]; then
    rmdir "$WORK/legacy-inventory/agents.json"
  else
    test -L "$WORK/legacy-inventory/agents.json"
    rm "$WORK/legacy-inventory/agents.json"
  fi
done
cmp "$WORK/outside-before" "$WORK/outside-inventory"
test ! -e "$WORK/missing-outside"

# The former TOML source is equally forbidden, even alongside the new source.
for legacy_kind in regular symlink dangling directory; do
  for existing_system in no yes; do
    case "$legacy_kind" in
    regular) cp "$TARGET_NIXOS/tentaflake.toml" "$WORK/legacy-inventory/agents.toml" ;;
    symlink) ln -s "$WORK/outside-inventory" "$WORK/legacy-inventory/agents.toml" ;;
    dangling) ln -s "$WORK/missing-outside" "$WORK/legacy-inventory/agents.toml" ;;
    directory) mkdir "$WORK/legacy-inventory/agents.toml" ;;
    esac
    if [ "$existing_system" = yes ]; then
      cp "$TARGET_NIXOS/tentaflake.toml" "$WORK/legacy-inventory/tentaflake.toml"
    fi
    if TARGET_NIXOS="$WORK/legacy-inventory" bash "$WORK/initialize-agents.sh"; then
      echo "FAIL: ignored legacy TOML $legacy_kind ($existing_system)" >&2
      exit 1
    fi
    if [ "$existing_system" = yes ]; then
      cmp "$TARGET_NIXOS/tentaflake.toml" "$WORK/legacy-inventory/tentaflake.toml"
      rm "$WORK/legacy-inventory/tentaflake.toml"
    else
      test ! -e "$WORK/legacy-inventory/tentaflake.toml"
    fi
    if [ "$legacy_kind" = directory ]; then
      rmdir "$WORK/legacy-inventory/agents.toml"
    else
      if [ "$legacy_kind" = regular ]; then
        cmp "$TARGET_NIXOS/tentaflake.toml" "$WORK/legacy-inventory/agents.toml"
      else
        test -L "$WORK/legacy-inventory/agents.toml"
      fi
      rm "$WORK/legacy-inventory/agents.toml"
    fi
  done
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
cat >"$TARGET_NIXOS/tentaflake.toml" <<'EOF'
schemaVersion = 1

[system]
hostName = "desired-host"
timeZone = "Europe/Berlin"
stateVersion = "26.05"
defaultLocale = "en_US.UTF-8"
consoleKeyMap = "us"

# Synthetic declarations only: no enrollment, remote grants or SSH access claim.
[network]
managementEnable = true
transport = "tailscale"
sshPolicy = "tailnet-policy"

[sandbox]
profile = "balanced"

[shell]
enable = false
tools = true

# The installed source must contain every package an enabled option needs.
[hostApi]
enable = true
peers = []

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
cp "$TARGET_NIXOS/tentaflake.toml" "$WORK/existing-tentaflake.toml"
bash "$WORK/initialize-agents.sh"
cmp "$WORK/existing-tentaflake.toml" "$TARGET_NIXOS/tentaflake.toml"

# ── Generate flake.nix from installer.sh's own heredoc ──
# Extracted from the real source (not a copy) so this test cannot go stale.
export NIXPKGS_REV
NIXPKGS_REV=$(jq -r '.nodes.nixpkgs.locked.rev' "$REPO_DIR/flake.lock")
export RESEARCH_REV
RESEARCH_REV=$(jq -r '.nodes["tentaflake-research"].locked.rev' "$REPO_DIR/flake.lock")
# Exercise the real feature generator with tools unchecked; explicit desired
# tools=true above must still win over the installer's bootstrap answer.
{
  sed -n "/^has_feature() {/,/^if \\[\\[ -n \"\\\$FEATURES\" \\]\\]; then/p" \
    "$REPO_DIR/installer/installer.sh" | sed '$d'
  printf '\nexport ADMIN_SHELL TF_TOGGLES\nprintenv -0 ADMIN_SHELL TF_TOGGLES\n'
} >"$WORK/features.sh"
mapfile -d '' -t feature_vars < <(FEATURES="" bash "$WORK/features.sh")
[[ ${#feature_vars[@]} -eq 2 ]] ||
  { echo "FAIL: feature generator did not return both declarations" >&2; exit 1; }
ADMIN_SHELL=${feature_vars[0]}
TF_TOGGLES=${feature_vars[1]}
export ADMIN_SHELL TF_TOGGLES
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

echo "Evaluating generated flake (versioned tentaflake.toml present) ..."
if nix eval --no-write-lock-file \
  "$TARGET_NIXOS#nixosConfigurations.$HOSTNAME_T.config.system.build.toplevel.drvPath" \
  >"$WORK/out" 2>"$WORK/err"; then
  echo "PASS: $(cat "$WORK/out")"
else
  echo "FAIL: generated flake does not evaluate with an agent configured." >&2
  tail -30 "$WORK/err" >&2
  exit 1
fi

# Assert effective NixOS values, not just freeform TOML projection attributes.
nix eval --json --no-write-lock-file \
  "$TARGET_NIXOS#nixosConfigurations.$HOSTNAME_T.config" \
  --apply 'c: {
    hostName = c.networking.hostName;
    configurationName = c.tentaflake.configurationName;
    timeZone = c.time.timeZone;
    stateVersion = c.system.stateVersion;
    shell = c.tentaflake.shell.enable;
    tools = c.tentaflake.shell.tools.enable;
    management = c.tentaflake.management.enable;
    tailscale = c.services.tailscale.enable;
    transport = c.tentaflake.management.transport;
    privateConnectivity = c.tentaflake.management.capabilities.privateConnectivity;
    sshAuthorization = c.tentaflake.management.capabilities.sshAuthorization;
    securityProfile = c.tentaflake.security.profile;
    shellConfig = c.environment.etc ? "tentaflake/cli.conf";
    hostApi = c.tentaflake.hostApi.enable;
    hostApiPackage = builtins.isString c.tentaflake.hostApi.package.drvPath;
    hostApiService = c.systemd.services ? tentaflake-api;
  }' >"$WORK/projections.json"
jq -e --arg selector "$HOSTNAME_T" '. == {
  hostName: "desired-host", timeZone: "Europe/Berlin", stateVersion: "26.05",
  configurationName: $selector,
  shell: false, tools: true, management: true, tailscale: true, shellConfig: false,
  hostApi: true, hostApiPackage: true, hostApiService: true,
  transport: "tailscale", privateConnectivity: true,
  sshAuthorization: "tailnet-policy", securityProfile: "balanced"
}' "$WORK/projections.json"

# Compose the actual published consumer example through an ordinary Git source.
# Only private integration modules are inert fixtures; Nixpkgs, Tentaflake
# modules, bootstrap defaults and the mandatory local TOML loader are real.
CONSUMER="$WORK/consumer"
CONSUMER_VIEW="$WORK/consumer-view"
mkdir -p "$CONSUMER/machines/agent-box" "$CONSUMER/secrets" "$CONSUMER_VIEW"
cp "$REPO_DIR/examples/consumer-flake.nix" "$CONSUMER/flake.nix"
for file in home.nix machines/agent-box/default.nix \
  machines/agent-box/hardware-configuration.nix \
  machines/agent-box/host-policy.nix secrets/secrets.nix; do
  printf '%s\n' '{}' >"$CONSUMER/$file"
done
printf '%s\n' 'ignored-private-canary' >"$CONSUMER/.gitignore"
git -C "$CONSUMER" init -q
git -C "$CONSUMER" add flake.nix home.nix machines secrets .gitignore
git -C "$CONSUMER" -c user.email=t@t -c user.name=t commit -q -m consumer
cat >"$CONSUMER/tentaflake.toml" <<'EOF'
schemaVersion = 1

[system]
hostName = "consumer-desired"
timeZone = "Europe/Berlin"
defaultLocale = "de_DE.UTF-8"
consoleKeyMap = "de"
stateVersion = "25.11"

[[agents]]
name = "consumer-only"
adapter = "hermes"
autoStart = false
EOF
cp "$CONSUMER/tentaflake.toml" "$WORK/consumer-desired.toml"
printf '%s\n' 'synthetic private canary' >"$CONSUMER/ignored-private-canary"
printf '%s\n' 'synthetic untracked canary' >"$CONSUMER/untracked-private-canary"
# flake=false obtains only the Git-filtered source. Inject already pinned
# dependencies below; never resolve the example's moving integration inputs.
CONSUMER_URL=$(python3 -c 'from pathlib import Path; import sys; print("git+" + Path(sys.argv[1]).resolve().as_uri())' "$CONSUMER")
cat >"$CONSUMER_VIEW/flake.nix" <<EOF
{
  inputs.consumer = { url = "$CONSUMER_URL"; flake = false; };
  outputs = { consumer, ... }: { source = consumer.outPath; };
}
EOF
consumer_projection() {
  local source
  source=$(nix eval --raw --no-write-lock-file "$CONSUMER_VIEW#source") || return
  nix eval --impure --json --no-write-lock-file --expr '
    { root, consumer }:
    let
      flake = builtins.getFlake root;
      inherit (flake.inputs.nixpkgs) lib;
      outputs = (import (consumer + "/flake.nix")).outputs {
        self = {};
        nixpkgs = flake.inputs.nixpkgs;
        tentaflake = flake;
        agenix.nixosModules.age = {};
        home-manager.nixosModules.home-manager = {
          options.home-manager = lib.mkOption {
            type = lib.types.attrsOf lib.types.anything;
          };
        };
      };
      c = outputs.nixosConfigurations.agent-box.config;
    in { result =
      assert !(builtins.pathExists (consumer + "/ignored-private-canary"));
      assert !(builtins.pathExists (consumer + "/untracked-private-canary"));
      {
        configurationName = c.tentaflake.configurationName;
        hostName = c.networking.hostName;
        timeZone = c.time.timeZone;
        defaultLocale = c.i18n.defaultLocale;
        consoleKeyMap = c.console.keyMap;
        stateVersion = c.system.stateVersion;
        declaredStateVersion = c.tentaflake.stateVersion;
        agents = builtins.attrNames c.tentaflake.agentInstances;
      };
    }' result --argstr root "$REPO_DIR" --argstr consumer "$source"
}
consumer_refuses_missing_source() {
  if consumer_projection >"$WORK/consumer-missing.out" 2>"$WORK/consumer-missing.err"; then
    echo "FAIL: consumer used the empty upstream template instead of required local TOML" >&2
    exit 1
  fi
  python3 - "$WORK/consumer-missing.err" <<'PY'
from pathlib import Path
import sys

message = " ".join(Path(sys.argv[1]).read_text().split())
assert "tentaflake.toml is required" in message, "consumer refused for an unrelated reason"
PY
}
# An on-disk untracked/ignored source must not silently evaluate an empty host.
consumer_refuses_missing_source
cmp "$WORK/consumer-desired.toml" "$CONSUMER/tentaflake.toml"
printf '%s\n' 'ignored-private-canary' 'tentaflake.toml' >"$CONSUMER/.gitignore"
consumer_refuses_missing_source
cmp "$WORK/consumer-desired.toml" "$CONSUMER/tentaflake.toml"
printf '%s\n' 'ignored-private-canary' >"$CONSUMER/.gitignore"
# Index tracking is enough for local Git flake evaluation; a commit is not needed.
git -C "$CONSUMER" add .gitignore tentaflake.toml
consumer_projection >"$WORK/consumer-projections.json"
jq -e '. == {
  configurationName: "agent-box", hostName: "consumer-desired",
  timeZone: "Europe/Berlin", defaultLocale: "de_DE.UTF-8",
  consoleKeyMap: "de", stateVersion: "25.11", declaredStateVersion: "25.11",
  agents: ["agent-consumer-only"]
}' "$WORK/consumer-projections.json"
git -C "$CONSUMER" -c user.email=t@t -c user.name=t commit -q -m desired
# Dirty tracked TOML must select its local values, not committed/upstream defaults.
sed 's@Europe/Berlin@Europe/Paris@' "$WORK/consumer-desired.toml" >"$CONSUMER/tentaflake.toml"
consumer_projection >"$WORK/consumer-dirty.json"
jq -e '.timeZone == "Europe/Paris" and .agents == ["agent-consumer-only"]' "$WORK/consumer-dirty.json"
rm "$CONSUMER/tentaflake.toml"
consumer_refuses_missing_source
cp "$WORK/consumer-desired.toml" "$CONSUMER/tentaflake.toml"
consumer_projection >"$WORK/consumer-restored.json"
cmp "$WORK/consumer-projections.json" "$WORK/consumer-restored.json"

# Exercise the generated composition's fail-closed policy, not remote access.
# Each refusal must preserve its input; keep the original inventory intact.
for missing_policy in transport authorization; do
  case "$missing_policy" in
  transport)
    sed 's/^managementEnable = true$/managementEnable = false/' \
      "$WORK/existing-tentaflake.toml" >"$WORK/rejected-policy.toml"
    expected_private=false
    ;;
  authorization)
    sed 's/^sshPolicy = "tailnet-policy"$/sshPolicy = "disabled"/' \
      "$WORK/existing-tentaflake.toml" >"$WORK/rejected-policy.toml"
    expected_private=true
    ;;
  esac
  cp "$WORK/rejected-policy.toml" "$TARGET_NIXOS/tentaflake.toml"
  # Inspect semantic assertion messages, not terminal-rendered Nix errors whose
  # wrapping can split an otherwise correct refusal across stderr lines.
  nix eval --json --no-write-lock-file \
    "$TARGET_NIXOS#nixosConfigurations.$HOSTNAME_T.config" \
    --apply 'c: {
      privateConnectivity = c.tentaflake.management.capabilities.privateConnectivity;
      sshAuthorization = c.tentaflake.management.capabilities.sshAuthorization;
      securityProfile = c.tentaflake.security.profile;
      failures = builtins.map (a: a.message)
        (builtins.filter (a: !a.assertion) c.assertions);
    }' >"$WORK/rejected-policy.json"
  if ! jq -e --argjson private "$expected_private" \
    --arg transport 'tentaflake: balanced/strict requires a configured private management transport.' \
    --arg ssh 'tentaflake: balanced/strict requires a configured private SSH authorization policy.' \
    '.privateConnectivity == $private and .sshAuthorization == "disabled"
      and .securityProfile == "balanced"
      and (.failures | sort) == (([$ssh] + (if $private then [] else [$transport] end)) | sort)' \
    "$WORK/rejected-policy.json"; then
    echo "FAIL: unexpected capabilities or failed assertions for missing $missing_policy" >&2
    exit 1
  fi
  if nix eval --no-write-lock-file \
    "$TARGET_NIXOS#nixosConfigurations.$HOSTNAME_T.config.system.build.toplevel.drvPath" \
    >"$WORK/rejected-policy.out" 2>"$WORK/rejected-policy.err"; then
    echo "FAIL: accepted missing private management $missing_policy" >&2
    exit 1
  fi
  cmp "$WORK/rejected-policy.toml" "$TARGET_NIXOS/tentaflake.toml"
  cp "$WORK/existing-tentaflake.toml" "$TARGET_NIXOS/tentaflake.toml"
done
cmp "$WORK/existing-tentaflake.toml" "$TARGET_NIXOS/tentaflake.toml"

# Unexpected function arguments are Nix evaluation errors, not catchable
# tryEval throws. Exercise rejected public/programmatic calls out of process.
for rejected in public-data public-builder internal-builder; do
  if nix eval --impure --json --no-write-lock-file --expr '
    { root, rejected }:
    let
      flake = builtins.getFlake root;
      pkgs = flake.inputs.nixpkgs.legacyPackages.x86_64-linux;
      public = (import (root + "/lib") { inherit pkgs; inherit (pkgs) lib; }).systemFromData;
      internal = import (root + "/lib/systemFromData.nix") { inherit pkgs; inherit (pkgs) lib; };
      file = root + "/tentaflake.toml";
      data = { schemaVersion = 1; agents = []; };
    in { result =
      if rejected == "public-data" then public { inherit file data; }
      else if rejected == "public-builder" then public { inherit file; mkAgent = _: {}; }
      else internal { inherit file; mkAgent = _: {}; };
    }' result --argstr root "$TARGET_NIXOS" --argstr rejected "$rejected" \
    >"$WORK/rejected.out" 2>"$WORK/rejected.err"; then
    echo "FAIL: accepted removed instance API: $rejected" >&2
    exit 1
  fi
  if ! grep -q 'unexpected argument' "$WORK/rejected.err"; then
    echo "FAIL: removed instance API $rejected failed for an unrelated reason" >&2
    exit 1
  fi
done

# Exercise the real operator CLI against an installed, balanced source fixture.
# No VM, host activation, OCI image or Research closure is involved.
printf '%s\n' 'schemaVersion = 1' 'agents = []' >"$TARGET_NIXOS/tentaflake.toml"
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
