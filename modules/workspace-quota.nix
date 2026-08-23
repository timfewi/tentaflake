{
  config,
  lib,
  pkgs,
  ...
}:
let
  cfg = config.tentaflake.workspaceQuota;
  utils = import (pkgs.path + "/nixos/lib/utils.nix") { inherit lib config pkgs; };
  enabledAgents = lib.filterAttrs (_: agent: agent.enable) cfg.agents;
  containerNames = lib.attrNames config.virtualisation.oci-containers.containers;
  volumeRoot = "/var/lib/tentaflake-workspace-volumes";
  imagePath = name: "${volumeRoot}/${name}.img";
  prepareUnit = name: "tentaflake-workspace-quota-prepare-${name}.service";
  ownerUnit = name: "tentaflake-workspace-quota-${name}.service";
  mountUnit = agent: "${utils.escapeSystemdPath agent.workspace}.mount";
  safePath = value: lib.match "^/var/lib/[A-Za-z0-9._+/-]+$" value != null;

  agentType = lib.types.submodule (
    { name, ... }:
    {
      options = {
        enable = lib.mkEnableOption "fixed-size persistent workspace filesystem for ${name}";
        workspace = lib.mkOption {
          type = lib.types.str;
          default = "";
          example = "/var/lib/hermes-coding/workspace";
          description = "Exact controller workspace used as the ext4 mount point.";
        };
        sizeMiB = lib.mkOption {
          type = lib.types.ints.between 32 1048576;
          default = 8192;
          description = "Immutable ext4 image size in MiB; resizing requires an explicit migration.";
        };
        ownerUid = lib.mkOption {
          type = lib.types.ints.unsigned;
          default = 10000;
        };
        ownerGid = lib.mkOption {
          type = lib.types.ints.unsigned;
          default = 10000;
        };
      };
    }
  );

  prepareService = name: agent: {
    description = "Prepare fixed-size workspace filesystem for ${name}";
    requires = [ "systemd-tmpfiles-setup.service" ];
    after = [ "systemd-tmpfiles-setup.service" ];
    before = [ (mountUnit agent) ];
    serviceConfig = {
      Type = "oneshot";
      UMask = "0077";
      NoNewPrivileges = true;
      PrivateDevices = false;
      PrivateTmp = true;
      ProtectHome = true;
      ProtectSystem = "strict";
      ReadWritePaths = [
        volumeRoot
        agent.workspace
      ];
    };
    path = [
      pkgs.coreutils
      pkgs.e2fsprogs
      pkgs.findutils
      pkgs.util-linux
    ];
    script = ''
      image=${lib.escapeShellArg (imagePath name)}
      image_tmp=${lib.escapeShellArg "${imagePath name}.new"}
      workspace=${lib.escapeShellArg agent.workspace}
      expected=$(( ${toString agent.sizeMiB} * 1024 * 1024 ))

      install -d -m 0700 ${lib.escapeShellArg volumeRoot}
      install -d -m 0700 "$workspace"

      if [ -L "$image" ] || { [ -e "$image" ] && [ ! -f "$image" ]; }; then
        echo "tentaflake: workspace image is not a regular file: $image" >&2
        exit 1
      fi

      if [ ! -e "$image" ]; then
        if find "$workspace" -mindepth 1 \
          ! -path "$workspace/.tentaflake-worker" \
          ! -path "$workspace/.tentaflake-worker/inbox" \
          -print -quit | grep -q .; then
          echo "tentaflake: refusing to hide non-empty workspace $workspace" >&2
          echo "migrate it explicitly before enabling the fixed-size volume" >&2
          exit 1
        fi
        if [ -e "$image_tmp" ]; then
          echo "tentaflake: incomplete image exists: $image_tmp" >&2
          echo "inspect and remove that exact file before retrying" >&2
          exit 1
        fi
        truncate --size "$expected" "$image_tmp"
        mkfs.ext4 -F -q -m 0 "$image_tmp"
        chmod 0600 "$image_tmp"
        mv -T "$image_tmp" "$image"
      fi

      actual=$(stat -c %s "$image")
      if [ "$actual" -ne "$expected" ]; then
        echo "tentaflake: workspace image size drift for ${name}" >&2
        echo "configured=$expected actual=$actual; use an explicit offline resize migration" >&2
        exit 1
      fi

      if ! findmnt --noheadings --mountpoint "$workspace" >/dev/null; then
        rc=0
        e2fsck -p "$image" || rc=$?
        if [ "$rc" -gt 1 ]; then
          echo "tentaflake: e2fsck failed for $image with status $rc" >&2
          exit "$rc"
        fi
      fi
    '';
  };

  ownerService = name: agent: {
    description = "Apply workspace ownership after mounting ${name}";
    requires = [ (mountUnit agent) ];
    after = [ (mountUnit agent) ];
    wantedBy = [ "multi-user.target" ];
    script = ''
      workspace=${lib.escapeShellArg agent.workspace}
      control="$workspace/.tentaflake-worker"
      inbox="$control/inbox"

      for path in "$control" "$inbox"; do
        if [ -L "$path" ] || { [ -e "$path" ] && [ ! -d "$path" ]; }; then
          echo "tentaflake: refusing unsafe worker control path: $path" >&2
          exit 1
        fi
      done

      ${pkgs.coreutils}/bin/chown ${toString agent.ownerUid}:${toString agent.ownerGid} "$workspace"
      ${pkgs.coreutils}/bin/chmod 0700 "$workspace"
      ${pkgs.coreutils}/bin/install -d -m 0700 \
        -o ${toString agent.ownerUid} -g ${toString agent.ownerGid} \
        "$control"
      ${pkgs.coreutils}/bin/install -d -m 0770 \
        -o ${toString agent.ownerUid} -g ${toString agent.ownerGid} \
        "$inbox"
    '';
    serviceConfig = {
      Type = "oneshot";
      RemainAfterExit = true;
      NoNewPrivileges = true;
      ProtectSystem = "strict";
      ReadWritePaths = [ agent.workspace ];
      CapabilityBoundingSet = [
        "CAP_CHOWN"
        "CAP_DAC_OVERRIDE"
        "CAP_FOWNER"
      ];
    };
  };
in
{
  options.tentaflake.workspaceQuota.agents = lib.mkOption {
    type = lib.types.attrsOf agentType;
    default = { };
    description = "Per-controller fixed-size persistent workspace filesystems.";
  };

  config = {
    assertions = [
      {
        assertion = enabledAgents == { } || config.tentaflake.security.profile != "dev";
        message = "tentaflake managed workspace quota is intended for balanced/strict profiles.";
      }
    ]
    ++ lib.concatLists (
      lib.mapAttrsToList (name: agent: [
        {
          assertion = lib.elem name containerNames;
          message = "tentaflake workspaceQuota key ${name} must exactly match an OCI agent container name.";
        }
        {
          assertion = lib.match "^[a-z0-9][a-z0-9-]{0,62}$" name != null;
          message = "tentaflake workspaceQuota names must be safe OCI identifiers.";
        }
        {
          assertion = safePath agent.workspace && !(lib.elem ".." (lib.splitString "/" agent.workspace));
          message = "tentaflake workspaceQuota ${name} requires an explicit simple workspace below /var/lib.";
        }
        {
          assertion = agent.workspace != volumeRoot && !(lib.hasPrefix "${volumeRoot}/" agent.workspace);
          message = "tentaflake workspaceQuota ${name} may not mount over its backing-image directory.";
        }
      ]) enabledAgents
    );

    systemd = {
      tmpfiles.rules = [ "d ${volumeRoot} 0700 root root -" ];

      services = lib.foldlAttrs (
        services: name: agent:
        services
        // {
          "tentaflake-workspace-quota-prepare-${name}" = prepareService name agent;
          "tentaflake-workspace-quota-${name}" = ownerService name agent;
        }
      ) { } enabledAgents;

      mounts = lib.mapAttrsToList (name: agent: {
        description = "Fixed-size persistent workspace for ${name}";
        what = imagePath name;
        where = agent.workspace;
        type = "ext4";
        options = "loop,nodev,nosuid,noatime";
        requires = [ (prepareUnit name) ];
        after = [
          "local-fs.target"
          (prepareUnit name)
        ];
        before = [
          (ownerUnit name)
          "umount.target"
        ];
        conflicts = [ "umount.target" ];
        wantedBy = [ "multi-user.target" ];
        # This managed image is intentionally mounted after the ordinary local
        # filesystems. Disable mount-unit defaults that would otherwise force it
        # back before local-fs.target and create an ordering cycle with prepare.
        unitConfig.DefaultDependencies = false;
      }) enabledAgents;
    };
  };
}
