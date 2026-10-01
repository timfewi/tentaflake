#!/usr/bin/env bash
# Shared by the installer wizard and its disposable VM fixture. Sourcing this
# file performs no disk operations. The caller provides dialog and fatal die.

mount_retry() {
  local source="$1" destination="$2" log="$3" attempt
  for attempt in {1..10}; do
    if mount "$source" "$destination" >>"$log" 2>&1; then
      return 0
    fi
    echo "mount attempt $attempt: $source -> $destination failed" >>"$log"
    udevadm settle 2>/dev/null || true
    sleep 1
  done
  return 1
}

wait_for_partitions() {
  local root="$1" efi="$2" attempt
  for attempt in {1..10}; do
    udevadm settle 2>/dev/null || true
    [ -b "$root" ] && [ -b "$efi" ] && return 0
    sleep 1
  done
  return 1
}

disk_contains() {
  local candidate device
  candidate=$(readlink -f -- "$1") || return 1
  shift
  for device in "$@"; do
    [ "$candidate" = "$device" ] && return 0
  done
  return 1
}

prepare_disk() {
  local DISK INSTALL_LOG="$2" stack volumes device kind canonical pv group other_pv other_group index
  local EFI_PART ROOT_PART prefix
  local -a devices=() paths=() kinds=()
  DISK=$(readlink -f -- "$1") || die "Cannot resolve selected disk '$1'."
  [ -b "$DISK" ] || die "Selected target '$DISK' is not a block device."

  # Discover the selected stack before mutation; do not stop unrelated swap or
  # VGs. A shared VG needs explicit operator migration, not partial erasure.
  stack=$(lsblk --raw --paths --noheadings --output NAME,TYPE "$DISK") ||
    die "Cannot inspect selected disk $DISK."
  while read -r device kind; do
    canonical=$(readlink -f -- "$device") || die "Cannot resolve target device $device."
    [ -b "$canonical" ] || die "Target device $device disappeared."
    devices+=("$canonical"); paths+=("$device"); kinds+=("$kind")
  done <<< "$stack"
  volumes=$(pvs --readonly --noheadings --options pv_name,vg_name 2>>"$INSTALL_LOG") ||
    die "Cannot inspect LVM before partitioning."
  while read -r pv group; do
    [ -n "$group" ] || continue
    disk_contains "$pv" "${devices[@]}" || continue
    while read -r other_pv other_group; do
      [ "$other_group" = "$group" ] || continue
      disk_contains "$other_pv" "${devices[@]}" ||
        die "LVM group $group spans another disk; migrate it before installing."
    done <<< "$volumes"
  done <<< "$volumes"

  dialog --infobox "Partitioning $DISK ..." 4 50
  umount -R /mnt 2>/dev/null || true
  for ((index = ${#devices[@]} - 1; index >= 0; index--)); do
    umount "${devices[index]}" 2>/dev/null || true
    swapoff "${devices[index]}" 2>/dev/null || true
  done
  while read -r pv group; do
    [ -n "$group" ] || continue
    disk_contains "$pv" "${devices[@]}" || continue
    vgchange -an "$group" >>"$INSTALL_LOG" 2>&1 || die "Cannot deactivate target LVM group $group."
  done <<< "$volumes"
  for ((index = ${#paths[@]} - 1; index >= 0; index--)); do
    if [ "${kinds[index]}" = crypt ]; then
      cryptsetup close "${paths[index]}" >>"$INSTALL_LOG" 2>&1 ||
        die "Cannot close encrypted target ${paths[index]}."
    fi
  done
  wipefs -a "$DISK" >>"$INSTALL_LOG" 2>&1 || die "Cannot wipe target signatures."

  # Numeric device names (NVMe, MMC, loop) use a p before the partition number.
  prefix="$DISK"
  [[ "$prefix" == *[0-9] ]] && prefix+=p
  EFI_PART="${prefix}1"; ROOT_PART="${prefix}2"
  sgdisk --zap-all "$DISK" >>"$INSTALL_LOG" 2>&1 || die "Failed to zap partition table."
  # Next-free-sector placement avoids inclusive-sector overlap at the ESP end.
  sgdisk -o -n 1:0:+1024M -t 1:ef00 -c 1:EFI -n 2:0:0 -t 2:8300 -c 2:NixOS \
    "$DISK" >>"$INSTALL_LOG" 2>&1 || die "Failed to create target partitions."
  blockdev --rereadpt "$DISK" >>"$INSTALL_LOG" 2>&1 || die "Failed to re-read partition table."
  wait_for_partitions "$ROOT_PART" "$EFI_PART" || die "Target partitions never appeared."

  dialog --infobox "Formatting partitions ..." 4 50
  mkfs.fat -F 32 -n BOOT "$EFI_PART" >>"$INSTALL_LOG" 2>&1 || die "Failed to format EFI partition."
  mkfs.btrfs -f -L nixos "$ROOT_PART" >>"$INSTALL_LOG" 2>&1 || die "Failed to format Btrfs root."
  # Formatting can temporarily remove/re-add device nodes on NVMe.
  wait_for_partitions "$ROOT_PART" "$EFI_PART" || die "Target partitions disappeared after formatting."
  dialog --infobox "Mounting partitions ..." 4 50
  mkdir -p /mnt || die "Cannot create installation mount point."
  mount_retry "$ROOT_PART" /mnt "$INSTALL_LOG" || die "Failed to mount root partition."
  mkdir -p /mnt/boot || die "Cannot create ESP mount point."
  mount_retry "$EFI_PART" /mnt/boot "$INSTALL_LOG" || die "Failed to mount boot partition."
}
