#!/usr/bin/env bash
# ────────────────────────────────────────────────────────────
# Tentaflake — Interactive Installer
# Bootable ISO wizard: asks 4-5 questions, partitions disk,
# generates flake, runs nixos-install, reboots.
# ────────────────────────────────────────────────────────────
set -euo pipefail

REPO_DIR="/etc/tentaflake"
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
die() {
  dialog --title "ERROR" --msgbox "$1" 8 60
  exit 1
}

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
    --inputbox "Enter the hostname for this machine" 8 50 "agent-machine")
  [ -z "$HOSTNAME" ] && dialog --title "Invalid" --msgbox "Hostname cannot be empty." 5 40
done

# ════════════════════════════════════════════════════════════
# STEP 3: Username
# ════════════════════════════════════════════════════════════
USERNAME=""
while [ -z "$USERNAME" ]; do
  USERNAME=$(dialog --stdout --title "Username" \
    --inputbox "Enter the primary admin username" 8 50 "agent")
  [ -z "$USERNAME" ] && dialog --title "Invalid" --msgbox "Username cannot be empty." 5 40
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
# STEP 7: Summary + confirm
# ════════════════════════════════════════════════════════════
dialog --title "Confirm Installation" --yesno \
  "Please verify your choices:

  Hostname:   $HOSTNAME
  Username:   $USERNAME
  Disk:       $DISK
  Timezone:   $TIMEZONE

WARNING: ALL DATA on $DISK will be destroyed!

Proceed?" 14 60 || die "Installation cancelled."

# ════════════════════════════════════════════════════════════
# STEP 8: Partition and mount
# ════════════════════════════════════════════════════════════
dialog --infobox "Partitioning $DISK ..." 4 50

# Release and wipe the target first, so a previously-used disk can't block
# partitioning (busy) or confuse mount later with stale signatures.
umount -R /mnt 2>/dev/null || true
swapoff -a 2>/dev/null || true
for part in "${DISK}"*[0-9]; do umount "$part" 2>/dev/null || true; done
wipefs -a "$DISK" >>"$INSTALL_LOG" 2>&1 || true
sgdisk --zap-all "$DISK" >>"$INSTALL_LOG" 2>&1 || true
udevadm settle 2>/dev/null || true

# Create a fresh GPT partition table
parted -s "$DISK" mklabel gpt >>"$INSTALL_LOG" 2>&1 ||
  die "Failed to create partition table on $DISK"

# EFI partition: 1GB
parted -s "$DISK" mkpart primary fat32 1MiB 1025MiB >>"$INSTALL_LOG" 2>&1 ||
  die "Failed to create EFI partition"
parted -s "$DISK" set 1 esp on >>"$INSTALL_LOG" 2>&1

# Root partition: rest
parted -s "$DISK" mkpart primary ext4 1025MiB 100% >>"$INSTALL_LOG" 2>&1 ||
  die "Failed to create root partition"

# Wait for the kernel + udev to register the new partition nodes
partprobe "$DISK" 2>/dev/null || true
udevadm settle 2>/dev/null || true
sleep 1

# Determine partition names (handle NVMe: /dev/nvme0n1p1 vs /dev/sda1)
if echo "$DISK" | grep -q nvme; then
  EFI_PART="${DISK}p1"
  ROOT_PART="${DISK}p2"
elif echo "$DISK" | grep -q mmcblk; then
  EFI_PART="${DISK}p1"
  ROOT_PART="${DISK}p2"
else
  EFI_PART="${DISK}1"
  ROOT_PART="${DISK}2"
fi

# Format
dialog --infobox "Formatting partitions ..." 4 50
mkfs.fat -F 32 -n BOOT "$EFI_PART" >>"$INSTALL_LOG" 2>&1 ||
  die "Failed to format EFI partition"
mkfs.ext4 -F -L nixos "$ROOT_PART" >>"$INSTALL_LOG" 2>&1 ||
  die "Failed to format root partition"

# Mount — settle first and retry. Right after partprobe/mkfs the kernel can
# briefly drop and recreate the partition node, so a single mount can race
# device creation and fail even though the filesystem is fine.
dialog --infobox "Mounting partitions ..." 4 50
udevadm settle 2>/dev/null || true
mounted=0
for _ in 1 2 3 4 5; do
  mount "$ROOT_PART" /mnt 2>>"$INSTALL_LOG" && {
    mounted=1
    break
  }
  sleep 1
  udevadm settle 2>/dev/null || true
done
[ "$mounted" -eq 1 ] || die "Failed to mount root partition ($ROOT_PART)"
mkdir -p /mnt/boot
mount "$EFI_PART" /mnt/boot || die "Failed to mount boot partition"

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

# Copy modules, lib, pkgs, and configuration.nix from the embedded repo
cp -r "$REPO_DIR/modules" "$TARGET_NIXOS/modules"
cp -r "$REPO_DIR/lib" "$TARGET_NIXOS/lib"
cp -r "$REPO_DIR/pkgs" "$TARGET_NIXOS/pkgs"
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

# Generate flake.nix for the installed system
cat >"$TARGET_NIXOS/flake.nix" <<FLAKEEOF
{
  description = "NixOS Agent Machine — ${HOSTNAME}";
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-26.11";
  };
  outputs = { self, nixpkgs, ... }@inputs:
    let
      system    = "x86_64-linux";
      pkgs      = nixpkgs.legacyPackages.\${system};
      lib       = nixpkgs.lib;
      uc        = import ./user-config.nix;
      constants = import ./lib/constants.nix;
      mkHermesAgent = (import ./lib { inherit pkgs lib; }).mkHermesAgent;
    in {
      nixosConfigurations.\${uc.hostName} = lib.nixosSystem {
        inherit system;
        specialArgs = {
          inherit self inputs mkHermesAgent constants;
          profile = "installed";
        };
        modules = [
          {
            tentaflake.hostName   = uc.hostName;
            tentaflake.adminUser  = uc.userName;
            tentaflake.adminShell = "\${pkgs.bash}/bin/bash";
            tentaflake.timeZone   = uc.timeZone;
          }
          ./modules
          ./configuration.nix
        ];
      };
    };
}
FLAKEEOF

# ════════════════════════════════════════════════════════════
# STEP 11: Run nixos-install
# ════════════════════════════════════════════════════════════
dialog --infobox "Running nixos-install ...\n(This takes 10-15 minutes and may appear frozen)" 6 60

# Use --show-trace for debugging if something goes wrong
if ! nixos-install --flake "$TARGET_NIXOS#$HOSTNAME" --root /mnt \
  --option substituters "https://cache.nixos.org" \
  --log-format internal-json -v 2>"$INSTALL_LOG"; then
  # Show the last 20 lines of the log on failure
  LOG_TAIL=$(tail -20 "$INSTALL_LOG" 2>/dev/null || echo "No log available")
  dialog --title "Installation Failed" --msgbox \
    "nixos-install failed. Check the log:

$LOG_TAIL

Full log: $INSTALL_LOG" 20 70
  exit 1
fi

dialog --infobox "Setting user password ..." 4 50

# Set user password on the installed system
echo "$USERNAME:$PASSWORD" | chpasswd --root /mnt 2>>"$INSTALL_LOG" ||
  dialog --title "Warning" --msgbox "Failed to set password for '$USERNAME'. Set manually after boot." 6 60

# Note: root account not given password — use sudo from admin user

# ════════════════════════════════════════════════════════════
# STEP 12: Copy agent examples
# ════════════════════════════════════════════════════════════
cp "$REPO_DIR/hermes.env.example" "$TARGET_NIXOS/hermes.env.example" 2>/dev/null || true
cp "$REPO_DIR/my-agents.nix.example" "$TARGET_NIXOS/my-agents.nix.example" 2>/dev/null || true
cp -r "$REPO_DIR/docs" "$TARGET_NIXOS/docs" 2>/dev/null || true
cp -r "$REPO_DIR/skills" "$TARGET_NIXOS/skills" 2>/dev/null || true

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
