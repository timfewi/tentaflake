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

# ── Helper: mount with retries ──
# Immediately after mkfs, udev emits a "change" event and re-probes the
# new filesystem; on NVMe this can briefly hold the device open or remove
# and re-add the device node. A single mount can race that window and fail
# even though the filesystem is valid. Retry a few times, settling udev
# between attempts, before giving up.
mount_retry() {
  local src="$1" dst="$2"
  local i
  for i in 1 2 3 4 5 6 7 8 9 10; do
    if mount "$src" "$dst" >>"$INSTALL_LOG" 2>&1; then
      return 0
    fi
    echo "mount attempt $i: $src -> $dst failed, settling and retrying ..." >>"$INSTALL_LOG"
    udevadm settle 2>/dev/null || true
    sleep 1
  done
  return 1
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
    --inputbox "Enter the primary admin username" 8 50 "agent")
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
# STEP 6b: Optional shell / editor features
# ════════════════════════════════════════════════════════════
# A checklist of opt-in extras; each maps to a tentaflake.* toggle written into
# the generated flake further down. Defaults are all "on" — uncheck to skip.
# `|| FEATURES=""` keeps `set -e` from aborting if the user cancels the dialog.
FEATURES=$(dialog --stdout --title "Optional Features" --checklist \
  "Choose extras to install.\nSPACE toggles an item, ENTER confirms." 18 78 6 \
  zsh "Zsh + Oh My Zsh (autosuggestions, syntax highlight, fzf-tab)" on \
  zoxide "zoxide — smart 'cd' that learns your frequent directories" on \
  nvf "Neovim (nvf) — LSP, treesitter, telescope, git, completion" on \
  lazygit "lazygit — a fast terminal UI for git" on \
  tmux "tmux — terminal multiplexer (persistent sessions over SSH)" on \
  tools "Modern CLI tools (eza, bat, fd, ripgrep, fzf, htop, btop)" on) || FEATURES=""
# Some dialog builds wrap each tag in quotes; strip them so matching is simple.
FEATURES=${FEATURES//\"/}

has_feature() { case " $FEATURES " in *" $1 "*) return 0 ;; *) return 1 ;; esac }

# ── Translate selections into fragments injected into the generated flake ──
# ADMIN_SHELL/TF_TOGGLES hold literal Nix; the heredoc expands the bash var once
# and does NOT re-scan the result, so embedded ${pkgs...} survive verbatim.
# shellcheck disable=SC2016  # ${pkgs...} is literal Nix, must NOT expand in bash
ADMIN_SHELL='"${pkgs.bash}/bin/bash"'
TF_TOGGLES=""
NVF_INPUT=""
NVF_MODULE_LINE=""

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
if has_feature nvf; then
  TF_TOGGLES+="            tentaflake.editor.nvf.enable = true;"$'\n'
  # Pin nvf to the rev this ISO was built from (guaranteed to exist + cached).
  NVF_REV=$(jq -r '.nodes.nvf.locked.rev' "$REPO_DIR/flake.lock" 2>/dev/null)
  if [ -n "$NVF_REV" ] && [ "$NVF_REV" != "null" ]; then
    NVF_INPUT="    nvf = { url = \"github:NotAShelf/nvf/${NVF_REV}\"; inputs.nixpkgs.follows = \"nixpkgs\"; };"$'\n'
  else
    NVF_INPUT="    nvf = { url = \"github:NotAShelf/nvf\"; inputs.nixpkgs.follows = \"nixpkgs\"; };"$'\n'
  fi
  NVF_MODULE_LINE=$'\n          ./modules/editor.nix'
fi

FEATURE_SUMMARY="${FEATURES:-(none)}"

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
# Under the hood:
#   We create two partitions on the target disk using sgdisk:
#     1. EFI System Partition (ESP): 1 GB, FAT32, type ef00
#        - Required for UEFI boot. The bootloader (systemd-boot)
#          lives here. Kernel images, initrd, and EFI drivers
#          are stored on this partition.
#     2. Root partition: rest of the disk, ext4, type 8300
#        - Contains the entire NixOS system: /nix/store, /etc,
#          /var, /home, everything. NixOS uses a read-only
#          /nix/store with symlinks from /etc and /run.
#
#   We use pure sgdisk (from gptfdisk) instead of mixing sgdisk
#   with parted because:
#     - sgdisk uses BLKRRPART ioctl (full partition table re-read)
#     - parted uses BLKPG ioctl (per-partition add/remove)
#     - Mixing both can confuse the kernel on NVMe drives:
#       sgdisk says "table wiped", then parted says "add partition",
#       kernel gets conflicting signals and silently ignores parted
#     - partprobe (which we'd need after parted) has a known bug
#       where it silently does nothing on NVMe
#     - One tool, one notification mechanism = reliable
#
#   We also wait for the partition device nodes (/dev/nvme0n1p1,
#   /dev/nvme0n1p2) to appear before formatting. After sgdisk
#   writes the partition table, the kernel needs time to create
#   the block devices. Without this wait, mkfs or mount would
#   fail with "special device does not exist".
#
#   After mkfs, we wait AGAIN because writing a new filesystem
#   can trigger a udev "change" event that briefly removes and
#   re-creates the partition device node. If mount races this
#   window, it fails even though the filesystem is valid.
# ────────────────────────────────────────────────────────────
dialog --infobox "Partitioning $DISK ..." 4 50

# Defensive guard: $DISK is sourced from a dialog menu built off lsblk, so it
# should always be a real block device — but never run the destructive steps
# below against anything else.
[ -b "$DISK" ] || die "Selected target '$DISK' is not a block device."

# Release and wipe the target first, so a previously-used disk can't block
# partitioning (busy) or confuse mount later with stale signatures.
umount -R /mnt 2>/dev/null || true
swapoff -a 2>/dev/null || true
for part in "${DISK}"*[0-9]; do umount "$part" 2>/dev/null || true; done

# Deactivate any LVM volume groups first. An active VG holds its underlying
# partitions open, so sgdisk/blockdev would fail to re-read the partition
# table ("device busy") on a previously-LVM disk. Do this BEFORE closing
# LUKS, since LVM may sit on top of a LUKS container.
vgchange -an >>"$INSTALL_LOG" 2>&1 || true

# Close any LUKS containers on the target — they'll be wiped anyway.
# Without this, sgdisk --zap-all fails with "device busy" on a
# previously-encrypted disk.
for mapper in /dev/mapper/luks-*; do
  [ -b "$mapper" ] || continue
  # Only close containers that sit on our disk's partitions
  src=$(dmsetup table "${mapper#/dev/mapper/}" 2>/dev/null | grep -o "${DISK##*/}[^ ]*" || true)
  if [ -n "$src" ]; then
    cryptsetup close "${mapper#/dev/mapper/}" >>"$INSTALL_LOG" 2>&1 || true
  fi
done

wipefs -a "$DISK" >>"$INSTALL_LOG" 2>&1 || true

# Determine partition names early (handle NVMe: /dev/nvme0n1p1 vs /dev/sda1)
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

# ── Partition using pure sgdisk ──
# sgdisk is more reliable than mixing sgdisk + parted:
#   • Uses BLKRRPART ioctl (full table re-read) not BLKPG (per-partition add)
#   • partprobe is known to silently no-op on NVMe
#   • single tool, single notification mechanism → no kernel confusion

# ── Partitioning strategy ──
# Under the hood, sgdisk is a CLI wrapper around libgpt (GPT fdisk).
# Each -n flag creates a partition entry in the GPT table on disk:
#
#   sgdisk -n <partnum>:<start>:<end> -t <partnum>:<typecode> <device>
#
# We use start=0 ("the next free aligned sector") and +SIZE lengths so
# sgdisk computes the boundaries itself. Hand-picking absolute MiB offsets
# is error-prone: the end of one partition and the start of the next must
# differ by at least one sector, or sgdisk rejects the overlap. Letting
# sgdisk track the cursor (start=0) guarantees back-to-back, non-overlapping,
# 1 MiB-aligned partitions. (1 MiB alignment avoids read-modify-write on
# 4K-sector drives; the first 1 MiB also reserves room for the GPT header
# + protective MBR.)

# Wipe any existing GPT/MBR structures so a previously-used disk starts clean.
sgdisk --zap-all "$DISK" >>"$INSTALL_LOG" 2>&1 ||
  die "Failed to zap partition table on $DISK"

# Create a fresh GPT and BOTH partitions in a SINGLE sgdisk invocation.
#
# Why one call: sgdisk lays partitions out in command order, tracking the
# next free aligned sector itself. Splitting the ESP and root into separate
# calls with absolute MiB offsets is what broke the old version — the ESP
# was created as 1MiB:1025MiB (ending at sector 2099200, inclusive) and the
# root was then asked to START at 1025MiB (sector 2099200 too). Those
# overlap on exactly one sector, so sgdisk refused partition 2 with
# "Could not create partition 2 ..." → the "Failed to create root
# partition" error. Using start=0 ("next free aligned sector") and a
# +SIZE length lets sgdisk place them back-to-back with no overlap.
#
#   -o                 create a fresh empty GPT
#   -n 1:0:+1024M      partition 1: next free sector, 1 GiB long (ESP)
#   -t 1:ef00          EFI System Partition type (UEFI scans for this)
#   -c 1:EFI           GPT partition name
#   -n 2:0:0           partition 2: next free sector → last usable sector
#   -t 2:8300          Linux filesystem type
#   -c 2:NixOS         GPT partition name
#
# ESP is 1 GiB (generous — most distros use 512 MB) to leave room for
# multiple kernels and systemd-boot backups across NixOS generations.
sgdisk -o \
  -n 1:0:+1024M -t 1:ef00 -c 1:"EFI" \
  -n 2:0:0 -t 2:8300 -c 2:"NixOS" \
  "$DISK" >>"$INSTALL_LOG" 2>&1 ||
  die "Failed to create partitions on $DISK"

# Force kernel to re-read the partition table
# blockdev --rereadpt sends BLKRRPART ioctl to the kernel block device
# driver. This makes the kernel re-scan the partition table from disk
# and create/remove device nodes under /dev/ accordingly.
# More reliable than partprobe (which uses BLKPG and can silently
# no-op on NVMe — known kernel bug).
blockdev --rereadpt "$DISK" >>"$INSTALL_LOG" 2>&1 ||
  die "Failed to re-read partition table"

# Wait for partition device nodes to appear under /dev/
# After blockdev --rereadpt, the kernel asynchronously creates block
# devices like /dev/nvme0n1p1 and /dev/nvme0n1p2.  The udev daemon
# processes these events with a small delay.  We poll for the actual
# device files (checking with [ -b ]) rather than sleeping a fixed
# amount, because NVMe drives can take variable time depending on
# the controller firmware and kernel version.  10 seconds max.
for _ in 1 2 3 4 5 6 7 8 9 10; do
  [ -b "$ROOT_PART" ] && [ -b "$EFI_PART" ] && break
  sleep 1
  udevadm settle 2>/dev/null || true
done
[ -b "$ROOT_PART" ] || die "Root partition $ROOT_PART never appeared after partitioning"
[ -b "$EFI_PART" ] || die "EFI partition $EFI_PART never appeared after partitioning"

# ── Format ──
# mkfs.fat creates the FAT32 filesystem required by UEFI for the ESP.
# The -n BOOT flag sets the filesystem label (shows in lsblk, useful
# for identifying the partition later).
dialog --infobox "Formatting partitions ..." 4 50
mkfs.fat -F 32 -n BOOT "$EFI_PART" >>"$INSTALL_LOG" 2>&1 ||
  die "Failed to format EFI partition"

# mkfs.ext4 -F -L nixos creates the root ext4 filesystem.
# -F forces creation even if there are leftover superblock signatures.
# -L nixos sets the volume label (visible in /dev/disk/by-label/nixos).
# The kernel's ext4 driver will read this superblock on mount.
mkfs.ext4 -F -L nixos "$ROOT_PART" >>"$INSTALL_LOG" 2>&1 ||
  die "Failed to format root partition"

# ── Wait after mkfs ──
# mkfs.ext4 writes a new ext4 superblock and block group descriptors.
# This changes the partition's content, which triggers a udev "change"
# event. On NVMe, some udev rules can briefly remove and re-add the
# partition device node during processing. If mount runs in this window,
# it gets ENXIO (no such device) even though the filesystem is valid.
# We wait for the device to stabilize before mounting.
udevadm settle 2>/dev/null || true
for _ in 1 2 3 4 5 6 7 8 9 10; do
  [ -b "$ROOT_PART" ] && [ -b "$EFI_PART" ] && break
  sleep 1
  udevadm settle 2>/dev/null || true
done
[ -b "$ROOT_PART" ] || die "Root partition $ROOT_PART disappeared after mkfs"

# ── Mount ──
# mount attaches the filesystem to the directory tree at /mnt.
# The kernel reads the ext4 superblock from $ROOT_PART, validates it,
# and makes the filesystem accessible under /mnt.
# We mount root first at /mnt, then the ESP at /mnt/boot (so the
# bootloader files end up on the ESP when nixos-install writes them).
dialog --infobox "Mounting partitions ..." 4 50
# /mnt is not guaranteed to exist on this minimal ISO (it imports only
# cd-dvd/iso-image.nix, not the full installation-cd profile that ships a
# /mnt directory). Create it before mounting, or mount fails with
# "mount point does not exist".
mkdir -p /mnt
mount_retry "$ROOT_PART" /mnt ||
  die "Failed to mount root partition ($ROOT_PART)"
mkdir -p /mnt/boot
mount_retry "$EFI_PART" /mnt/boot ||
  die "Failed to mount boot partition ($EFI_PART)"

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
# modules/shell.nix reads ../public/tentaflake-shell-logo.txt at eval time.
# Copy just that file — the rest of public/ is multi-MB imagery that would
# bloat every installed system's config repo and nix store.
mkdir -p "$TARGET_NIXOS/public"
cp "$REPO_DIR/public/tentaflake-shell-logo.txt" "$TARGET_NIXOS/public/" ||
  die "Failed to copy shell logo (modules/shell.nix needs it)"
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

# Generate flake.nix for the installed system
cat >"$TARGET_NIXOS/flake.nix" <<FLAKEEOF
{
  description = "NixOS Agent Machine — ${HOSTNAME}";
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/${NIXPKGS_REV}";
${NVF_INPUT}  };
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
            tentaflake.adminShell = ${ADMIN_SHELL};
            tentaflake.timeZone   = uc.timeZone;
${TF_TOGGLES}          }
          ./modules
          ./configuration.nix${NVF_MODULE_LINE}
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
