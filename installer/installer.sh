#!/usr/bin/env bash
# ────────────────────────────────────────────────────────────
# Tentaflake — Interactive Installer
# Bootable ISO wizard: asks 4-5 questions, partitions disk,
# generates flake, runs nixos-install, reboots.
# ────────────────────────────────────────────────────────────
set -euo pipefail

REPO_DIR="/etc/tentaflake/source"
TARGET_NIXOS="/mnt/etc/nixos"
INSTALL_LOG="/tmp/installer.log"

# ── Colors for dialog --infobox / --msgbox ──
export NCURSES_NO_UTF8_ACS=1
export DIALOGOPTS="--backtitle Tentaflake Installer"

# ── Cleanup handler ──
cleanup() {
  local rc=$?
  # If we crashed mid-install, unmount /mnt
  if mountpoint -q /mnt 2>/dev/null; then
    umount -R /mnt 2>/dev/null || true
  fi
  exit $rc
}
trap cleanup EXIT INT TERM

# ── Helper: red error box ──
# Always append the tail of the install log so the operator can see the
# ACTUAL underlying error (mount/mkfs/sgdisk stderr all land in the log).
# Without this the dialog only shows a generic message and the ISO loops
# on tty1 with no shell, leaving no way to find out what really failed.
die() {
  local log_tail=""
  if [ -s "$INSTALL_LOG" ]; then
    log_tail=$(tail -n 15 "$INSTALL_LOG" 2>/dev/null)
  fi
  dialog --title "ERROR" --msgbox "$1

--- last lines of $INSTALL_LOG ---
${log_tail:-（log is empty）}" 22 72
  exit 1
}

# Shared destructive operations; sourced functions do nothing until called.
# shellcheck source=installer/disk.sh
source "$(dirname -- "${BASH_SOURCE[0]}")/disk.sh"

# ── Helper: check if dialog is available ──
if ! command -v dialog &>/dev/null; then
  echo "FATAL: dialog not found. Install dialog or run from the installer ISO."
  exit 1
fi

# ════════════════════════════════════════════════════════════
# STEP 1: Welcome
# ════════════════════════════════════════════════════════════
dialog --title "Welcome" --msgbox \
  "Welcome to the Tentaflake Installer.

This wizard will guide you through installing NixOS with the
agent orchestration framework.

You will need:
  - A disk to install to (WILL BE WIPED)
  - Internet connection (NetworkManager is active)
  - About 10-15 minutes for the build

We'll ask you 5 questions, then go." 14 60

# ════════════════════════════════════════════════════════════
# STEP 2: Hostname
# ════════════════════════════════════════════════════════════
HOSTNAME=""
while [ -z "$HOSTNAME" ]; do
  HOSTNAME=$(dialog --stdout --title "Hostname" \
    --inputbox "Enter the hostname for this machine" 8 50 "tentaflake")
  if [ -z "$HOSTNAME" ]; then
    dialog --title "Invalid" --msgbox "Hostname cannot be empty." 5 40
  # Restrict to RFC-1123 label chars. Anything else (e.g. " or #) would be
  # interpolated raw into the generated Nix config / flake URI and break it.
  elif ! printf '%s' "$HOSTNAME" | grep -qE '^[a-zA-Z0-9]([a-zA-Z0-9-]*[a-zA-Z0-9])?$'; then
    dialog --title "Invalid" --msgbox "Hostname may only contain letters, digits and hyphens (not at the start/end)." 7 50
    HOSTNAME=""
  fi
done

# ════════════════════════════════════════════════════════════
# STEP 3: Username
# ════════════════════════════════════════════════════════════
USERNAME=""
while [ -z "$USERNAME" ]; do
  USERNAME=$(dialog --stdout --title "Username" \
    --inputbox "Enter the primary admin username" 8 50 "user")
  if [ -z "$USERNAME" ]; then
    dialog --title "Invalid" --msgbox "Username cannot be empty." 5 40
  # Restrict to a valid Linux user name (same as useradd's NAME_REGEX).
  elif ! printf '%s' "$USERNAME" | grep -qE '^[a-z_][a-z0-9_-]*$'; then
    dialog --title "Invalid" --msgbox "Username must start with a letter/underscore and contain only lowercase letters, digits, '-' or '_'." 8 50
    USERNAME=""
  fi
done

# ════════════════════════════════════════════════════════════
# STEP 4: Password (with confirmation)
# ════════════════════════════════════════════════════════════
PASSWORD=""
PASSWORD2="x"
while [ "$PASSWORD" != "$PASSWORD2" ] || [ -z "$PASSWORD" ]; do
  PASSWORD=$(dialog --stdout --title "Password" \
    --passwordbox "Enter password for user '$USERNAME'" 8 50)
  [ -z "$PASSWORD" ] && {
    dialog --title "Invalid" --msgbox "Password cannot be empty." 5 40
    continue
  }
  PASSWORD2=$(dialog --stdout --title "Password" \
    --passwordbox "Confirm password" 8 50)
  [ "$PASSWORD" != "$PASSWORD2" ] &&
    dialog --title "Mismatch" --msgbox "Passwords do not match. Try again." 5 50
done
# Clear the confirmation var for safety
PASSWORD2=""

# ════════════════════════════════════════════════════════════
# STEP 5: Disk selection
# ════════════════════════════════════════════════════════════
DISK=""
while [ -z "$DISK" ]; do
  # Build menu from lsblk
  DISK_LIST=$(lsblk -dno NAME,SIZE,MODEL,TYPE 2>/dev/null | grep disk |
    awk '{print "/dev/"$1, $2, $3, $4}')
  [ -z "$DISK_LIST" ] && die "No disks found on this system."

  MENU_ITEMS=()
  while IFS= read -r line; do
    dev=$(echo "$line" | awk '{print $1}')
    info=$(echo "$line" | awk '{$1=""; print $0}' | xargs)
    MENU_ITEMS+=("$dev" "$info")
  done <<<"$DISK_LIST"

  # Show menu; cancel = exit
  DISK=$(dialog --stdout --title "Disk Selection" \
    --menu "Select the disk to install to.\nALL DATA will be WIPED!" 15 60 5 \
    "${MENU_ITEMS[@]}")
  rc=$?
  [ $rc -ne 0 ] && die "Installation cancelled."
done

# ════════════════════════════════════════════════════════════
# STEP 6: Timezone
# ════════════════════════════════════════════════════════════
TIMEZONE=$(dialog --stdout --title "Timezone" \
  --inputbox "Enter timezone (e.g. Europe/Berlin, America/New_York, UTC)" 8 50 "UTC")
[ -z "$TIMEZONE" ] && TIMEZONE="UTC"

# ════════════════════════════════════════════════════════════
# STEP 6b: Optional shell features
# ════════════════════════════════════════════════════════════
# A checklist of opt-in extras; each maps to a tentaflake.* toggle written into
# the generated flake further down. Defaults are all "on" — uncheck to skip.
# `|| FEATURES=""` keeps `set -e` from aborting if the user cancels the dialog.
FEATURES=$(dialog --stdout --title "Optional Features" --checklist \
  "Choose extras to install.\nSPACE toggles an item, ENTER confirms." 18 78 5 \
  zsh "Zsh + Oh My Zsh (autosuggestions, syntax highlight, fzf-tab)" on \
  zoxide "zoxide — smart 'cd' that learns your frequent directories" on \
  lazygit "lazygit — a fast terminal UI for git" on \
  tmux "tmux — terminal multiplexer (persistent sessions over SSH)" on \
  tools "Modern CLI tools (eza, bat, fd, ripgrep, fzf, htop, btop)" on) || FEATURES=""
# Some dialog builds wrap each tag in quotes; strip them so matching is simple.
FEATURES=${FEATURES//\"/}

has_feature() {
  case " $FEATURES " in
    *" $1 "*) return 0 ;;
    *) return 1 ;;
  esac
}

# ── Translate selections into fragments injected into the generated flake ──
# ADMIN_SHELL/TF_TOGGLES hold literal Nix; the heredoc expands the bash var once
# and does NOT re-scan the result, so embedded ${pkgs...} survive verbatim.
# shellcheck disable=SC2016  # ${pkgs...} is literal Nix, must NOT expand in bash
ADMIN_SHELL='"${pkgs.bash}/bin/bash"'
TF_TOGGLES=""

if has_feature zsh; then
  # shellcheck disable=SC2016  # literal Nix interpolation, not bash
  ADMIN_SHELL='"${pkgs.zsh}/bin/zsh"'
  TF_TOGGLES+="            tentaflake.shell.zsh.enable = true;"$'\n'
fi
if ! has_feature zoxide; then
  TF_TOGGLES+="            tentaflake.shell.zoxide.enable = false;"$'\n'
fi
if has_feature lazygit; then
  TF_TOGGLES+="            tentaflake.shell.lazygit.enable = true;"$'\n'
fi
if has_feature tmux; then
  TF_TOGGLES+="            tentaflake.shell.tmux.enable = true;"$'\n'
fi
if ! has_feature tools; then
  TF_TOGGLES+="            tentaflake.shell.tools.enable = false;"$'\n'
fi
if [[ -n "$FEATURES" ]]; then
  FEATURE_SUMMARY=$FEATURES
else
  FEATURE_SUMMARY="(none)"
fi

# ════════════════════════════════════════════════════════════
# STEP 7: Summary + confirm
# ════════════════════════════════════════════════════════════
dialog --title "Confirm Installation" --yesno \
  "Please verify your choices:

  Hostname:   $HOSTNAME
  Username:   $USERNAME
  Disk:       $DISK
  Timezone:   $TIMEZONE
  Features:   $FEATURE_SUMMARY

WARNING: ALL DATA on $DISK will be destroyed!

Proceed?" 16 64 || die "Installation cancelled."

# ════════════════════════════════════════════════════════════
# STEP 8: Partition and mount
# ════════════════════════════════════════════════════════════
# The disk library is also imported directly by the disposable VM fixture.
prepare_disk "$DISK" "$INSTALL_LOG"

# ════════════════════════════════════════════════════════════
# STEP 9: Generate hardware config
# ════════════════════════════════════════════════════════════
mkdir -p /mnt/etc/nixos
dialog --infobox "Generating hardware configuration ..." 4 50
nixos-generate-config --root /mnt --show-hardware-config >/mnt/etc/nixos/hardware-configuration.nix 2>>"$INSTALL_LOG" ||
  die "Failed to generate hardware config"

# ════════════════════════════════════════════════════════════
# STEP 10: Create system configuration on target
# ════════════════════════════════════════════════════════════
dialog --infobox "Creating system configuration ..." 4 50

TARGET_NIXOS="/mnt/etc/nixos"

# Copy the declarative system and Rust CLI workspace from the embedded repo.
cp -r "$REPO_DIR/modules" "$TARGET_NIXOS/modules"
cp -r "$REPO_DIR/lib" "$TARGET_NIXOS/lib"
cp -r "$REPO_DIR/adapters" "$TARGET_NIXOS/adapters"
cp -r "$REPO_DIR/pkgs" "$TARGET_NIXOS/pkgs"
cp -r "$REPO_DIR/crates" "$TARGET_NIXOS/crates"
cp "$REPO_DIR/Cargo.toml" "$TARGET_NIXOS/Cargo.toml"
cp "$REPO_DIR/Cargo.lock" "$TARGET_NIXOS/Cargo.lock"
cp "$REPO_DIR/configuration.nix" "$TARGET_NIXOS/configuration.nix"
cp "$REPO_DIR/my-agents.nix" "$TARGET_NIXOS/my-agents.nix" 2>/dev/null || echo '{ mkHermesAgent }: [ ]' >"$TARGET_NIXOS/my-agents.nix"

# Generate user-config.nix
cat >"$TARGET_NIXOS/user-config.nix" <<EOF
# Generated by Tentaflake Installer
{
  hostName   = "$HOSTNAME";
  userName   = "$USERNAME";
  timeZone   = "$TIMEZONE";
}
EOF

# Pin the installed system's nixpkgs to the EXACT revision this ISO was
# built from, read from the embedded flake.lock. This guarantees the rev
# exists and maximises binary-cache reuse so the install is fast and
# reproducible. (The previous hardcoded "nixos-26.11" branch did not exist
# yet, so nixos-install could not fetch nixpkgs and failed.)
NIXPKGS_REV=$(jq -r '.nodes.nixpkgs.locked.rev' "$REPO_DIR/flake.lock" 2>/dev/null)
if [ -z "$NIXPKGS_REV" ] || [ "$NIXPKGS_REV" = "null" ]; then
  die "Could not read nixpkgs revision from $REPO_DIR/flake.lock"
fi
RESEARCH_REV=$(jq -r '.nodes["tentaflake-research"].locked.rev' "$REPO_DIR/flake.lock" 2>/dev/null)
if [ -z "$RESEARCH_REV" ] || [ "$RESEARCH_REV" = "null" ]; then
  die "Could not read tentaflake-research revision from $REPO_DIR/flake.lock"
fi

# Generate flake.nix for the installed system
cat >"$TARGET_NIXOS/flake.nix" <<FLAKEEOF
{
  description = "NixOS Agent Machine — ${HOSTNAME}";
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/${NIXPKGS_REV}";
    tentaflake-research.url = "github:timfewi/tentaflake-research/${RESEARCH_REV}";
  };
  outputs = { self, nixpkgs, ... }@inputs:
    let
      system    = "x86_64-linux";
      pkgs      = nixpkgs.legacyPackages.\${system};
      lib       = nixpkgs.lib;
      uc        = import ./user-config.nix;
      # Splat the whole helper set (mkAgent, adapters, mkHermesAgent, mkZeroClawAgent,
      # agentsFromData, constants) instead of listing helpers by hand, so this
      # generated flake cannot drift out of sync with what configuration.nix
      # asks for. Drift does not fail loudly: configuration.nix consumes these
      # inside imports, so a missing one sends Nix to config._module.args and
      # the rebuild dies with "infinite recursion encountered" instead.
      tfLib     = import ./lib { inherit pkgs lib; };
    in {
      nixosConfigurations.\${uc.hostName} = lib.nixosSystem {
        inherit system;
        specialArgs = tfLib // {
          inherit self inputs;
          profile = "installed";
        };
        modules = [
          {
            tentaflake.hostName   = uc.hostName;
            tentaflake.adminUser  = uc.userName;
            tentaflake.adminShell = ${ADMIN_SHELL};
            tentaflake.timeZone   = uc.timeZone;
${TF_TOGGLES}          }
          (import ./modules { researchFlake = inputs.tentaflake-research; })
          ./configuration.nix
        ];
      };
    };
}
FLAKEEOF

# ════════════════════════════════════════════════════════════
# STEP 10b: Make the config a git repo
# ════════════════════════════════════════════════════════════
# /mnt/etc/nixos is a plain directory, so Nix treats it as a `path:` flake
# and snapshots its NAR hash before evaluating. nixos-install then writes
# flake.lock INTO that directory, changing its contents mid-evaluation, so
# the snapshot no longer matches → "error: NAR hash mismatch in input
# 'path:/mnt/etc/nixos'". Committing to git first gives Nix an immutable
# source snapshot to build from, so writing flake.lock afterwards is
# harmless. It also leaves the installed config version-controlled.
dialog --infobox "Initialising config git repository ..." 4 50
git -C "$TARGET_NIXOS" init -q >>"$INSTALL_LOG" 2>&1 ||
  die "Failed to git init $TARGET_NIXOS"
git -C "$TARGET_NIXOS" add -A >>"$INSTALL_LOG" 2>&1 ||
  die "Failed to git add config"
git -C "$TARGET_NIXOS" \
  -c user.email=installer@tentaflake -c user.name="Tentaflake Installer" \
  commit -q -m "Initial system configuration (generated by installer)" >>"$INSTALL_LOG" 2>&1 ||
  die "Failed to git commit config"

# ════════════════════════════════════════════════════════════
# STEP 11: Run nixos-install
# ════════════════════════════════════════════════════════════
dialog --infobox "Running nixos-install ...\n(This takes 10-15 minutes and may appear frozen)" 6 60

# Log in plain human-readable format (NOT internal-json — that produces
# unreadable {"action":...,"type":10} lines that hide the real error).
# --show-trace gives full evaluation traces for flake/module errors.
# --no-root-passwd skips the interactive root-password prompt at the end
# (we set the admin user's password ourselves; root stays locked → sudo).
# Both stdout and stderr go to the log so the actual error is captured.
if ! nixos-install --flake "$TARGET_NIXOS#$HOSTNAME" --root /mnt \
  --no-root-passwd \
  --show-trace \
  --option substituters "https://cache.nixos.org" >>"$INSTALL_LOG" 2>&1; then
  # Show the last 25 lines of the log on failure
  LOG_TAIL=$(tail -25 "$INSTALL_LOG" 2>/dev/null || echo "No log available")
  dialog --title "Installation Failed" --msgbox \
    "nixos-install failed. Real error below:

$LOG_TAIL

Full log: $INSTALL_LOG" 24 76
  exit 1
fi

dialog --infobox "Setting user password ..." 4 50

# Set user password on the installed system
echo "$USERNAME:$PASSWORD" | chpasswd --root /mnt 2>>"$INSTALL_LOG" ||
  dialog --title "Warning" --msgbox "Failed to set password for '$USERNAME'. Set manually after boot." 6 60

# Drop the plaintext password from shell memory now that it has been applied.
unset PASSWORD

# Note: root account not given password — use sudo from admin user

# ════════════════════════════════════════════════════════════
# STEP 12: Copy agent examples
# ════════════════════════════════════════════════════════════
cp "$REPO_DIR/hermes.env.example" "$TARGET_NIXOS/hermes.env.example" 2>/dev/null || true
cp "$REPO_DIR/zeroclaw.env.example" "$TARGET_NIXOS/zeroclaw.env.example" 2>/dev/null || true
cp "$REPO_DIR/my-agents.nix.example" "$TARGET_NIXOS/my-agents.nix.example" 2>/dev/null || true
cp -r "$REPO_DIR/docs" "$TARGET_NIXOS/docs" 2>/dev/null || true
cp -r "$REPO_DIR/skills" "$TARGET_NIXOS/skills" 2>/dev/null || true

# Commit everything produced after the initial config commit — the flake.lock
# that nixos-install wrote, plus the bundled examples/docs/skills copied above —
# so /etc/nixos is a clean git tree and the first nixos-rebuild doesn't warn
# about a dirty tree.
git -C "$TARGET_NIXOS" add -A >>"$INSTALL_LOG" 2>&1 || true
git -C "$TARGET_NIXOS" \
  -c user.email=installer@tentaflake -c user.name="Tentaflake Installer" \
  commit -q -m "Add flake.lock and bundled examples" >>"$INSTALL_LOG" 2>&1 || true

# ════════════════════════════════════════════════════════════
# DONE
# ════════════════════════════════════════════════════════════
dialog --title "Installation Complete" --msgbox \
  "NixOS has been installed successfully!

  Hostname: $HOSTNAME
  Username: $USERNAME

AFTER REBOOT:
  1. Log in as '$USERNAME'
  2. Read /etc/nixos/docs/01-quickstart.md to get started
  3. Look at /etc/nixos/my-agents.nix.example for agent examples
  4. Set Hermes API keys:
     sudo -u hermes hermes config set OPENROUTER_API_KEY sk-or-...
  5. Rebuild: sudo nixos-rebuild switch --flake /etc/nixos#$HOSTNAME

The system will now reboot." 16 65

# Unmount and reboot
umount -R /mnt 2>/dev/null || true
dialog --infobox "Rebooting in 5 seconds ..." 4 50
sleep 2
reboot
