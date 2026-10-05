{
  config,
  lib,
  pkgs,
  ...
}:
let
  constants = import ../lib/constants.nix;
  cfg = config.tentaflake.workspaceQuota;
  utils = import (pkgs.path + "/nixos/lib/utils.nix") { inherit lib config pkgs; };
  enabledAgents = lib.filterAttrs (_: agent: agent.enable) cfg.agents;
  containerNames = lib.attrNames config.virtualisation.oci-containers.containers;
  volumeRoot = "/var/lib/tentaflake-workspace-volumes";
  imageRoot = kind: volumeRoot + lib.optionalString (kind == "state") "/state";
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
  imagePath = name: kind: "${imageRoot kind}/${name}.img";
  prepareUnit = name: kind: "tentaflake-${kind}-quota-prepare-${name}.service";
  ownerUnit = name: kind: "tentaflake-${kind}-quota-${name}.service";
  mountPath = agent: kind: if kind == "state" then agent.state.path else agent.workspace;
  mountUnit = agent: kind: "${utils.escapeSystemdPath (mountPath agent kind)}.mount";
  pathWithin = root: path: path == root || lib.hasPrefix "${root}/" path;
  safePath =
    value:
    lib.match "^/var/lib/[A-Za-z0-9._+/-]+$" value != null
    && lib.all (part: part != "" && part != "." && part != "..") (lib.tail (lib.splitString "/" value));
  volumes = lib.concatLists (
    lib.mapAttrsToList (
      name: agent:
      map (kind: { inherit name agent kind; }) (
        [ "workspace" ] ++ lib.optional (agent.state != null) "state"
      )
    ) enabledAgents
  );

  agentType = lib.types.submodule (
    { name, ... }:
    {
      options = {
        enable = lib.mkEnableOption "fixed-size persistent agent filesystems for ${name}";
        workspace = lib.mkOption {
          type = lib.types.str;
          default = "";
          example = "/var/lib/hermes-coding/workspace";
          description = "Exact controller workspace used as the Btrfs mount point.";
        };
        sizeMiB = lib.mkOption {
          type = lib.types.ints.between 128 1048576;
          default = 8192;
          description = "Immutable Btrfs image size in MiB (at least 128); resizing requires an explicit migration.";
        };
        ownerUid = lib.mkOption {
          type = lib.types.ints.unsigned;
          default = constants.containerUid;
        };
        ownerGid = lib.mkOption {
          type = lib.types.ints.unsigned;
          default = constants.containerGid;
        };
        state = lib.mkOption {
          default = null;
          description = "Optional fixed-size private runtime state; existing state requires explicit offline migration.";
          type = lib.types.nullOr (
            lib.types.submodule {
              options = {
                path = lib.mkOption {
                  type = lib.types.str;
                  description = "Exact stateStorage mount source declared by the adapter.";
                };
                sizeMiB = lib.mkOption {
                  type = lib.types.ints.between 128 1048576;
                  default = 1024;
                  description = "Immutable Btrfs state image size in MiB; resizing requires explicit migration.";
                };
              };
            }
          );
        };
      };
    }
  );

  prepareService =
    name: agent: kind:
    let
      target = mountPath agent kind;
      stateParent = lib.optional (
        kind == "workspace" && agent.state != null && pathWithin agent.state.path agent.workspace
      ) (ownerUnit name "state");
      sizeMiB = if kind == "state" then agent.state.sizeMiB else agent.sizeMiB;
    in
    {
      description = "Prepare fixed-size ${kind} filesystem for ${name}";
      requires = [ "systemd-tmpfiles-setup.service" ] ++ stateParent;
      after = [ "systemd-tmpfiles-setup.service" ] ++ stateParent;
      before = [ (mountUnit agent kind) ];
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
          target
        ];
      };
      path = [
        pkgs.coreutils
        pkgs.btrfs-progs
        pkgs.findutils
        pkgs.util-linux
      ];
      script = directoryGuard + ''
        image=${lib.escapeShellArg (imagePath name kind)}
        image_tmp=${lib.escapeShellArg "${imagePath name kind}.new"}
        workspace=${lib.escapeShellArg target}
        expected=$(( ${toString sizeMiB} * 1024 * 1024 ))

        # Check before install/chmod/mount: following a persisted agent symlink
        # could change or mount another instance's directory as root.
        check_directory "$workspace"

        install -d -m 0700 ${lib.escapeShellArg (imageRoot kind)}
        install -d -m 0700 "$workspace"

        if [ -L "$image" ] || { [ -e "$image" ] && [ ! -f "$image" ]; }; then
          echo "tentaflake: ${kind} image is not a regular file: $image" >&2
          exit 1
        fi

        # An existing image must not hide files written below a stopped mount.
        if ! findmnt --noheadings --mountpoint "$workspace" >/dev/null; then
          if find "$workspace" -mindepth 1 \
            ${
              lib.optionalString (
                kind == "workspace"
              ) ''! -path "$workspace/.tentaflake-worker" ! -path "$workspace/.tentaflake-worker/inbox"''
            } \
            -print -quit | grep -q .; then
            echo "tentaflake: refusing to hide non-empty ${kind} $workspace" >&2
            echo "migrate it explicitly before enabling the fixed-size volume" >&2
            exit 1
          fi
        fi

        if [ ! -e "$image" ]; then
          if [ -e "$image_tmp" ] || [ -L "$image_tmp" ]; then
            echo "tentaflake: incomplete image exists: $image_tmp" >&2
            echo "inspect and remove that exact file before retrying" >&2
            exit 1
          fi
          truncate --size "$expected" "$image_tmp"
          mkfs.btrfs -q "$image_tmp"
          chmod 0600 "$image_tmp"
          mv -T "$image_tmp" "$image"
        fi

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

        if ! findmnt --noheadings --mountpoint "$workspace" >/dev/null; then
          btrfs check --readonly "$image"
        fi
      '';
    };

  ownerService =
    name: agent: kind:
    let
      target = mountPath agent kind;
      stateDependency = lib.optional (kind == "workspace" && agent.state != null) (
        ownerUnit name "state"
      );
      stateDirectories = lib.optionals (kind == "state") (
        map (directory: "${target}/${directory}") config.tentaflake.agentInstances.${name}.stateDirectories
      );
    in
    {
      description = "Apply ${kind} ownership after mounting ${name}";
      requires = [ (mountUnit agent kind) ] ++ stateDependency;
      after = [ (mountUnit agent kind) ] ++ stateDependency;
      wantedBy = [ "multi-user.target" ];
      script =
        directoryGuard
        + ''
          workspace=${lib.escapeShellArg target}
          check_directory "$workspace"
          for path in ${lib.escapeShellArgs stateDirectories}; do
            check_directory "$path"
          done
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
            -o ${toString agent.ownerUid} -g ${toString agent.ownerGid} \
            "$control"
          ${pkgs.coreutils}/bin/install -d -m 0770 \
            -o ${toString agent.ownerUid} -g ${toString agent.ownerGid} \
            "$inbox"
        ''
        + ''
          ${pkgs.coreutils}/bin/chown ${toString agent.ownerUid}:${toString agent.ownerGid} "$workspace"
          ${pkgs.coreutils}/bin/chmod 0700 "$workspace"
          ${lib.optionalString (stateDirectories != [ ]) ''
            ${pkgs.coreutils}/bin/install -d -m 0700 \
              -o ${toString agent.ownerUid} -g ${toString agent.ownerGid} \
              ${lib.escapeShellArgs stateDirectories}
          ''}
        '';
      serviceConfig = {
        Type = "oneshot";
        RemainAfterExit = true;
        NoNewPrivileges = true;
        ProtectSystem = "strict";
        ReadWritePaths = [ target ];
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
    description = "Per-controller fixed-size persistent workspace and optional private state filesystems.";
  };

  config = {
    boot.supportedFilesystems = lib.mkIf (enabledAgents != { }) [ "btrfs" ];

    assertions = [
      {
        assertion = enabledAgents == { } || config.tentaflake.security.profile != "dev";
        message = "tentaflake managed workspace quota is intended for balanced/strict profiles.";
      }
      {
        assertion = lib.all (
          first:
          lib.all (
            second:
            if first.name == second.name && first.kind == second.kind then
              true
            else if first.name == second.name then
              first.kind != second.kind && mountPath first.agent first.kind != mountPath second.agent second.kind
            else
              !(pathWithin (mountPath first.agent first.kind) (mountPath second.agent second.kind))
              && !(pathWithin (mountPath second.agent second.kind) (mountPath first.agent first.kind))
          ) volumes
        ) volumes;
        message = "tentaflake managed quota mounts must be distinct and may not overlap another agent's filesystems.";
      }
    ]
    ++ lib.concatLists (
      lib.mapAttrsToList (
        name: agent:
        [
          {
            assertion = lib.elem name containerNames;
            message = "tentaflake workspaceQuota key ${name} must exactly match an OCI agent container name.";
          }
          {
            assertion = lib.match "^[a-z0-9][a-z0-9-]{0,62}$" name != null;
            message = "tentaflake workspaceQuota names must be safe OCI identifiers.";
          }
          {
            assertion = safePath agent.workspace;
            message = "tentaflake workspaceQuota ${name} requires an explicit simple workspace below /var/lib.";
          }
          {
            assertion = agent.workspace != volumeRoot && !(lib.hasPrefix "${volumeRoot}/" agent.workspace);
            message = "tentaflake workspaceQuota ${name} may not mount over its backing-image directory.";
          }
        ]
        ++ lib.optionals (agent.state != null) [
          {
            assertion =
              safePath agent.state.path
              && !(pathWithin volumeRoot agent.state.path)
              && !(pathWithin agent.workspace agent.state.path);
            message = "tentaflake state quota ${name} requires a normalized private state source outside workspace and backing images.";
          }
          {
            assertion =
              builtins.hasAttr name config.tentaflake.agentInstances
              && agent.state.path == config.tentaflake.agentInstances.${name}.stateStorage
              && agent.ownerUid == config.tentaflake.agentInstances.${name}.uid
              && agent.ownerGid == config.tentaflake.agentInstances.${name}.gid;
            message = "tentaflake state quota ${name} must match its declared adapter state source and ownership.";
          }
          {
            assertion =
              !builtins.hasAttr name config.tentaflake.agentInstances
              || lib.all (
                directory:
                lib.match "[A-Za-z0-9._-]+(/[A-Za-z0-9._-]+)*" directory != null
                && !(lib.elem "." (lib.splitString "/" directory))
                && !(lib.elem ".." (lib.splitString "/" directory))
              ) config.tentaflake.agentInstances.${name}.stateDirectories;
            message = "tentaflake state quota ${name} requires normalized relative adapter initialization directories.";
          }
        ]
      ) enabledAgents
    );

    systemd = {
      tmpfiles.rules = [
        "d ${volumeRoot} 0700 root root -"
      ]
      ++ lib.optional (lib.any (
        volume: volume.kind == "state"
      ) volumes) "d ${volumeRoot}/state 0700 root root -";

      services = lib.foldl' (
        services: volume:
        services
        // {
          "tentaflake-${volume.kind}-quota-prepare-${volume.name}" =
            prepareService volume.name volume.agent
              volume.kind;
          "tentaflake-${volume.kind}-quota-${volume.name}" =
            ownerService volume.name volume.agent
              volume.kind;
        }
      ) { } volumes;

      mounts = map (
        {
          name,
          agent,
          kind,
        }:
        {
          description = "Fixed-size persistent ${kind} for ${name}";
          what = imagePath name kind;
          where = mountPath agent kind;
          type = "btrfs";
          options = "loop,nodev,nosuid,noatime";
          requires = [ (prepareUnit name kind) ];
          after = [
            "local-fs.target"
            (prepareUnit name kind)
          ];
          before = [
            (ownerUnit name kind)
            "umount.target"
          ];
          conflicts = [ "umount.target" ];
          wantedBy = [ "multi-user.target" ];
          # This managed image is intentionally mounted after the ordinary local
          # filesystems. Disable mount-unit defaults that would otherwise force it
          # back before local-fs.target and create an ordering cycle with prepare.
          unitConfig.DefaultDependencies = false;
        }
      ) volumes;
    };
  };
}
