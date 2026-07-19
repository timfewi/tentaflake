{
  config,
  lib,
  pkgs,
  ...
}:

let
  # The interactive TUI wizard (runs via .bashrc on TTY1)
  firstbootScript = pkgs.writeShellScriptBin "tentaflake-firstboot" ''
    ${builtins.readFile ./firstboot.sh}
  '';

  # USB TENTAFLAKE_ENV auto-detector (runs as systemd service before Docker)
  envDetect = pkgs.writeShellScript "tentaflake-env-detect" ''
    set -euo pipefail

    ENV_DIR="/run/tentaflake"
    mkdir -p "$ENV_DIR"

    # Compat symlink for one release: older configs hardcode
    # /run/hermes/<name>.env as their envFile. If /run/hermes is a real
    # directory (in-place rebuild across the rename on a running system),
    # migrate its env files first so ln doesn't abort the script.
    if [ -d /run/hermes ] && [ ! -L /run/hermes ]; then
      cp /run/hermes/*.env "$ENV_DIR"/ 2>/dev/null || true
      rm -rf /run/hermes
    fi
    ln -sfn "$ENV_DIR" /run/hermes

    # ── Copy env files from a labeled TENTAFLAKE_ENV USB, if present.
    # The legacy HERMES_ENV label is still accepted so existing sticks
    # keep working. ──
    USB_DEV=$(blkid -l -o device -t LABEL=TENTAFLAKE_ENV 2>/dev/null || true)
    [ -n "$USB_DEV" ] || USB_DEV=$(blkid -l -o device -t LABEL=HERMES_ENV 2>/dev/null || true)
    if [ -n "$USB_DEV" ]; then
      mkdir -p /mnt/tentaflake-env
      if mount "$USB_DEV" /mnt/tentaflake-env 2>/dev/null; then
        for f in /mnt/tentaflake-env/*.env; do
          [ -f "$f" ] || continue
          name=$(basename "$f" .env)
          cp "$f" "''${ENV_DIR}/''${name}.env"
          chmod 600 "''${ENV_DIR}/''${name}.env"
          echo "tentaflake-env-detect: loaded ''${name}.env from USB"
        done
        umount /mnt/tentaflake-env 2>/dev/null || true
      fi
      rmdir /mnt/tentaflake-env 2>/dev/null || true
    fi

    # ── Ensure every agent has an env file so `docker run --env-file` never
    # fails on a cold boot. Agents are enumerated from their systemd units,
    # so this stays generic (works for docker- and podman- backends and both
    # agent runtimes). Missing files get an empty placeholder; the firstboot
    # wizard later overwrites them with real keys and restarts the units. ──
    for unit in $(systemctl list-unit-files --no-legend 'docker-hermes-*.service' 'podman-hermes-*.service' 'docker-zeroclaw-*.service' 'podman-zeroclaw-*.service' 'docker-opencode-*.service' 'podman-opencode-*.service' 2>/dev/null | awk '{print $1}'); do
      name=''${unit%.service}
      name=''${name#docker-}
      name=''${name#podman-}
      # Hermes env files keep their historical bare names (hermes-coding →
      # coding); other runtimes keep the runtime prefix (zeroclaw-scout stays
      # zeroclaw-scout, opencode-code stays opencode-code) — same labeling
      # scheme as the audit daemon.
      name=''${name#hermes-}
      envf="''${ENV_DIR}/''${name}.env"
      if [ ! -f "$envf" ]; then
        touch "$envf"
        chmod 600 "$envf"
        echo "tentaflake-env-detect: created placeholder ''${name}.env (awaiting firstboot wizard)"
      fi
    done
  '';

  # USB TENTAFLAKE_DATA auto-mounter (persistent state across reboots)
  dataMount = pkgs.writeShellScript "tentaflake-data-mount" ''
    set -euo pipefail

    # Legacy HERMES_DATA label still accepted so existing sticks keep working.
    USB_DEV=$(blkid -l -o device -t LABEL=TENTAFLAKE_DATA 2>/dev/null || true)
    [ -n "$USB_DEV" ] || USB_DEV=$(blkid -l -o device -t LABEL=HERMES_DATA 2>/dev/null || true)
    [ -z "$USB_DEV" ] && exit 0

    mkdir -p /mnt/tentaflake-data
    mount "$USB_DEV" /mnt/tentaflake-data 2>/dev/null || exit 0

    # Move one agent state dir onto the USB, replacing it with a symlink.
    # Every step is checked explicitly (errexit is disabled inside a function
    # used as an `if` condition) and the original dir is only removed once the
    # copy is verified — a full, read-only or ownership-less stick must degrade
    # to "no persistence", never to "state deleted".
    stage_to_usb() {
      src=$1
      dst=$2
      owner=$(stat -c '%u:%g' "$src") || return 1
      mode=$(stat -c '%a' "$src") || return 1

      mkdir -p "$dst" || return 1
      # mkdir gives root:root 0755; agents run as their own uid (hermes 10000,
      # opencode 65534) with a 0700 state dir, so mirror the original. vfat and
      # exfat carry no Unix ownership at all — the agent could never write
      # there, so skip rather than break it.
      if ! chown "$owner" "$dst" 2>/dev/null || ! chmod "$mode" "$dst" 2>/dev/null \
        || [ "$(stat -c '%u:%g %a' "$dst")" != "$owner $mode" ]; then
        echo "tentaflake-data-mount: ''${dst} has no Unix ownership (vfat/exfat?) — skipping ''${src}"
        return 1
      fi

      # Three cases, and only two of them may touch $src:
      #   USB empty            -> seed it from local state, verify, then swap.
      #   local empty          -> USB state wins (the normal reboot path: tmpfiles
      #                           recreated an empty dir); swap, nothing to lose.
      #   both non-empty       -> REFUSE. Copying either way discards the other,
      #                           and we cannot know which the operator wants.
      if [ -z "$(ls -A "$dst")" ]; then
        cp -a "$src/." "$dst/" || return 1
        [ "$(find "$src" | wc -l)" = "$(find "$dst" | wc -l)" ] || return 1
      elif [ -n "$(ls -A "$src")" ]; then
        echo "tentaflake-data-mount: ''${src} and ''${dst} both hold state — refusing to discard either; resolve by hand"
        return 1
      fi

      # Safe now: $src is either empty or byte-for-byte present on the stick.
      rm -rf "$src" || return 1
      # $src is gone from here on, so a failed symlink is NOT "kept local state".
      # Return 2 so the caller reports it as the data-losing case it is.
      if ! ln -s "$dst" "$src"; then
        echo "tentaflake-data-mount: ERROR: ''${src} removed but symlink to ''${dst} failed — state is on the USB, relink by hand"
        return 2
      fi
    }

    # Symlink agent state dirs to USB for persistence (all runtimes)
    for dir in /var/lib/hermes-* /var/lib/zeroclaw-* /var/lib/opencode-*; do
      [ -d "$dir" ] || continue
      # Only migrate if not already a symlink
      [ -L "$dir" ] && continue
      name=$(basename "$dir")
      usb_dir="/mnt/tentaflake-data/''${name}"
      stage_to_usb "$dir" "$usb_dir" && rc=0 || rc=$?
      if [ "$rc" = 0 ]; then
        echo "tentaflake-data-mount: ''${dir} → ''${usb_dir}"
      elif [ "$rc" != 2 ]; then
        # rc=2 already reported itself accurately; anything else left $dir alone.
        echo "tentaflake-data-mount: WARNING: could not persist ''${dir} — keeping local state, no USB persistence"
      fi
    done
  '';
in
{
  # ── USB env detection (before Docker starts) ──
  systemd.services.tentaflake-env-detect = {
    description = "Detect USB TENTAFLAKE_ENV partition and load agent .env files";
    requires = [ "local-fs.target" ];
    after = [ "local-fs.target" ];
    before = [ "docker.service" ];
    wantedBy = [ "multi-user.target" ];
    # blkid/mount/umount + awk are not in the default service PATH.
    path = [
      pkgs.util-linux
      pkgs.gawk
    ];
    serviceConfig = {
      Type = "oneshot";
      RemainAfterExit = true;
      ExecStart = "${envDetect}";
    };
  };

  # ── USB data persistence (after env + tmpfiles, before Docker) ──
  # MUST run after systemd-tmpfiles-setup: the agent builders create the
  # /var/lib/<runtime>-<name> state dirs via tmpfiles. If this service ran
  # first the glob below would match nothing and persistence would silently
  # no-op.
  systemd.services.tentaflake-data-mount = {
    description = "Mount USB TENTAFLAKE_DATA partition for persistent agent state";
    requires = [ "local-fs.target" ];
    after = [
      "local-fs.target"
      "systemd-tmpfiles-setup.service"
      "tentaflake-env-detect.service"
    ];
    before = [ "docker.service" ];
    wantedBy = [ "multi-user.target" ];
    # blkid/mount are not in the default service PATH.
    path = [ pkgs.util-linux ];
    serviceConfig = {
      Type = "oneshot";
      RemainAfterExit = true;
      ExecStart = "${dataMount}";
    };
  };

  # ── Run firstboot wizard on TTY1 auto-login ──
  programs.bash.interactiveShellInit = ''
    if [ -z ''${TENTAFLAKE_FIRSTBOOT_DONE:-} ] && [ "$(tty 2>/dev/null)" = "/dev/tty1" ]; then
      export TENTAFLAKE_FIRSTBOOT_DONE=1
      tentaflake-firstboot
    fi
  '';

  # ── Expose scripts as packages ──
  environment.systemPackages = [ firstbootScript ];
}
