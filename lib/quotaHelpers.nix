# Shared immutable quota helpers. The owning module validates all inputs.
{ pkgs, lib }:
{
  name,
  kind,
  bytes,
  ownerUid,
  ownerGid,
  stateDirectories,
  imagePath,
  mountPath,
}:
let
  directoryGuard = ''
    check_directory() {
      directory="$1"
      while [ "$directory" != /var/lib ]; do
        if [ -L "$directory" ] || { [ -e "$directory" ] && [ ! -d "$directory" ]; }; then
          echo "tentaflake: refusing symlink or non-directory quota source: $directory" >&2
          exit 1
        fi
        directory=$(${pkgs.coreutils}/bin/dirname "$directory")
      done
    }
  '';
  imageMountGuard = ''
    image_mounted_here() {
      # ReadWritePaths creates a bind mount even when the host quota is absent.
      # Check the actual loop backing file, not merely a mount point or fs type.
      mounted_source=$(${pkgs.util-linux}/bin/findmnt --noheadings --raw \
        --mountpoint "$workspace" --output SOURCE) || return 1
      source_device="''${mounted_source%%\[*}"
      case "$source_device" in
        /dev/loop[0-9]*)
          backing=$(${pkgs.util-linux}/bin/losetup --noheadings --list --raw \
            --output BACK-FILE "$source_device") || {
            echo "tentaflake: cannot identify quota mount backing" >&2
            exit 1
          }
          [ "$backing" = "$image" ]
          ;;
        *) return 1 ;;
      esac
    }
  '';
  writeQuotaHelper =
    name: text:
    pkgs.writeShellScript name (
      ''
        set -euo pipefail
        export PATH=${
          lib.makeBinPath [
            pkgs.coreutils
            pkgs.btrfs-progs
            pkgs.findutils
            pkgs.util-linux
          ]
        }
        export LC_ALL=C
        umask 0077
      ''
      + text
    );

  target = mountPath;
  directories = map (directory: "${target}/${directory}") stateDirectories;
  preparePrelude =
    directoryGuard
    + imageMountGuard
    + ''
      image=${lib.escapeShellArg imagePath}
      image_tmp=${lib.escapeShellArg "${imagePath}.new"}
      image_directory=${lib.escapeShellArg (builtins.dirOf imagePath)}
      workspace=${lib.escapeShellArg target}
      expected=${toString bytes}
      check_directory "$workspace"
      check_directory "$image_directory"

      # The admission lock fences cooperating host processes, not an agent
      # that can replace entries in a writable backing-image ancestor.
      image_parent="$image_directory"
      while :; do
        if [ -L "$image_parent" ] || { [ -e "$image_parent" ] && [ ! -d "$image_parent" ]; }; then
          echo "tentaflake: unsafe image directory: $image_parent" >&2
          exit 1
        fi
        if [ -d "$image_parent" ]; then
          image_metadata=$(stat -c '%u:%a' "$image_parent")
          IFS=: read -r image_owner image_mode <<< "$image_metadata"
          if [ "$image_owner" -ne 0 ] || (( (8#$image_mode & 0022) != 0 )); then
            echo "tentaflake: image directory must be root-owned and not group/world writable: $image_parent" >&2
            exit 1
          fi
        fi
        [ "$image_parent" = / ] && break
        image_parent=$(dirname "$image_parent")
      done
    '';
  refuseStaging = ''
    if [ -e "$image_tmp" ] || [ -L "$image_tmp" ]; then
      echo "tentaflake: incomplete image exists: $image_tmp" >&2
      echo "inspect that exact file before explicit recovery" >&2
      exit 1
    fi
  '';
  hidingGuard = ''
    # A missing mount directory contains nothing to hide. Verification must
    # leave its creation to the mount unit instead of changing host paths.
    if [ -d "$workspace" ] && ! image_mounted_here; then
      unmounted_content=$(find "$workspace" -mindepth 1 \
        ${
          lib.optionalString (
            kind == "workspace"
          ) ''! -path "$workspace/.tentaflake-worker" ! -path "$workspace/.tentaflake-worker/inbox"''
        } \
        -print -quit)
      if [ -n "$unmounted_content" ]; then
        echo "tentaflake: refusing to hide non-empty ${kind} $workspace" >&2
        echo "migrate it explicitly before enabling the fixed-size volume" >&2
        exit 1
      fi
    fi
  '';
  verify = writeQuotaHelper "tentaflake-${kind}-quota-verify-${name}" (
    ''
      require_mounted=false
      case "$#:$*" in
        0:) ;;
        1:--mounted) require_mounted=true ;;
        *) echo "tentaflake: invalid quota verification arguments" >&2; exit 1 ;;
      esac
    ''
    + preparePrelude
    + refuseStaging
    + ''
      if [ -L "$image" ] || { [ -e "$image" ] && [ ! -f "$image" ]; }; then
        echo "tentaflake: ${kind} image is not a regular file: $image" >&2
        exit 1
      fi
      if [ ! -f "$image" ]; then
        echo "tentaflake: ${kind} image is missing: $image" >&2
        exit 1
      fi
    ''
    + hidingGuard
    + ''
      actual=$(stat -c %s "$image")
      if [ "$actual" -ne "$expected" ]; then
        echo "tentaflake: ${kind} image size drift for ${name}" >&2
        echo "configured=$expected actual=$actual; use an explicit offline resize migration" >&2
        exit 1
      fi

      if [ "$(blkid -p -s TYPE -o value "$image")" != btrfs ]; then
        echo "tentaflake: ${kind} image is not Btrfs: $image" >&2
        echo "back up and migrate existing ext4 images explicitly; no automatic reformat" >&2
        exit 1
      fi
      if ! image_mounted_here; then
        if "$require_mounted"; then
          echo "tentaflake: expected quota image is not mounted" >&2
          exit 1
        fi
        # The bounded read-only observer must own the actual checker PID.
        exec btrfs check --readonly "$image"
      fi
      if "$require_mounted"; then
        if [ "$(findmnt --noheadings --raw --mountpoint "$workspace" --output FSTYPE)" != btrfs ]; then
          echo "tentaflake: quota mount is not Btrfs" >&2
          exit 1
        fi
        mount_options=$(findmnt --noheadings --raw --mountpoint "$workspace" --output OPTIONS)
        for required_option in nodev nosuid noatime; do
          case ",$mount_options," in
            *,"$required_option",*) ;;
            *) echo "tentaflake: quota mount options drifted" >&2; exit 1 ;;
          esac
        done
      fi
    ''
  );
in
{
  inherit verify;
  mount = writeQuotaHelper "tentaflake-${kind}-quota-mount-${name}" (
    preparePrelude
    + ''
      ${verify}
      if image_mounted_here; then
        exec ${verify} --mounted
      fi
      if findmnt --noheadings --raw --mountpoint "$workspace" --output TARGET >/dev/null; then
        echo "tentaflake: refusing to replace a different quota mount" >&2
        exit 1
      fi
      install -d -m 0700 "$workspace"
      check_directory "$workspace"
      mount --types btrfs --options loop,nodev,nosuid,noatime -- "$image" "$workspace"
      exec ${verify} --mounted
    ''
  );
  unmount = writeQuotaHelper "tentaflake-${kind}-quota-unmount-${name}" (
    preparePrelude
    + refuseStaging
    + ''
      if image_mounted_here; then
        ${verify} --mounted
        umount -- "$workspace"
      elif findmnt --noheadings --raw --mountpoint "$workspace" --output TARGET >/dev/null; then
        echo "tentaflake: refusing to unmount a different quota image" >&2
        exit 1
      fi
      if findmnt --noheadings --raw --mountpoint "$workspace" --output TARGET >/dev/null; then
        echo "tentaflake: quota mount remains after unmount" >&2
        exit 1
      fi
    ''
  );
  create = writeQuotaHelper "tentaflake-${kind}-quota-create-${name}" (
    preparePrelude
    + refuseStaging
    + ''
      if [ -e "$image" ] || [ -L "$image" ]; then
        echo "tentaflake: refusing to recreate existing ${kind} image: $image" >&2
        exit 1
      fi
      install -d -m 0700 "$image_directory" "$workspace"
    ''
    + hidingGuard
    + ''
      # No cleanup trap: an interrupted creation remains charged and needs
      # explicit recovery. Noclobber creates the staging file exclusively.
      set -o noclobber
      : > "$image_tmp"
      set +o noclobber
      truncate --no-create --size "$expected" "$image_tmp"
      mkfs.btrfs -q "$image_tmp"
      chmod 0600 "$image_tmp"
      sync "$image_tmp"

      # Publish without replacing a final path created after the first
      # absence check. Sync both directory transitions before success.
      ln --no-target-directory "$image_tmp" "$image"
      sync "$image_directory"
      rm -- "$image_tmp"
      sync "$image_directory"
      exec ${verify}
    ''
  );
  owner = writeQuotaHelper "tentaflake-${kind}-quota-owner-${name}" (
    directoryGuard
    + imageMountGuard
    + ''
      workspace=${lib.escapeShellArg target}
      image=${lib.escapeShellArg imagePath}
      expected=${toString bytes}
      check_directory "$workspace"
      for path in ${lib.escapeShellArgs directories}; do
        check_directory "$path"
      done
      if ! image_mounted_here; then
        echo "tentaflake: expected quota image is not mounted" >&2
        exit 1
      fi
      actual=$(${pkgs.coreutils}/bin/stat -c %s "$image")
      if [ "$actual" -ne "$expected" ]; then
        echo "tentaflake: ${kind} image size drift for ${name}; use an explicit offline resize migration" >&2
        exit 1
      fi
    ''
    + lib.optionalString (kind == "workspace") ''
      control="$workspace/.tentaflake-worker"
      inbox="$control/inbox"

      for path in "$control" "$inbox"; do
        if [ -L "$path" ] || { [ -e "$path" ] && [ ! -d "$path" ]; }; then
          echo "tentaflake: refusing unsafe worker control path: $path" >&2
          exit 1
        fi
      done

      ${pkgs.coreutils}/bin/install -d -m 0700 \
        -o ${toString ownerUid} -g ${toString ownerGid} \
        "$control"
      ${pkgs.coreutils}/bin/install -d -m 0770 \
        -o ${toString ownerUid} -g ${toString ownerGid} \
        "$inbox"
    ''
    + ''
      ${pkgs.coreutils}/bin/chown ${toString ownerUid}:${toString ownerGid} "$workspace"
      ${pkgs.coreutils}/bin/chmod 0700 "$workspace"
      ${lib.optionalString (directories != [ ]) ''
        ${pkgs.coreutils}/bin/install -d -m 0700 \
          -o ${toString ownerUid} -g ${toString ownerGid} \
          ${lib.escapeShellArgs directories}
      ''}
    ''
  );
}
