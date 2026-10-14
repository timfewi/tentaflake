{
  config,
  lib,
  pkgs,
  ...
}:
let
  constants = import ../lib/constants.nix;
  cfg = config.tentaflake.workspaceQuota;
  enforced = config.tentaflake.security.admission != null;
  admissionCli = "${pkgs.callPackage ../pkgs/tentaflake-cli { }}/bin/tentaflake";
  utils = import (pkgs.path + "/nixos/lib/utils.nix") { inherit lib config pkgs; };
  enabledAgents = lib.filterAttrs (_: agent: agent.enable) cfg.agents;
  containerNames = lib.attrNames config.virtualisation.oci-containers.containers;
  volumeRoot = "/var/lib/tentaflake-workspace-volumes";
  imageRoot = kind: volumeRoot + lib.optionalString (kind == "state") "/state";
  mkQuotaHelpers = import ../lib/quotaHelpers.nix { inherit pkgs lib; };
  imagePath = name: kind: "${imageRoot kind}/${name}.img";
  imageBytes =
    agent: kind: (if kind == "state" then agent.state.sizeMiB else agent.sizeMiB) * 1024 * 1024;
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

  volumeHelpers =
    name: agent: kind:
    mkQuotaHelpers {
      inherit name kind;
      bytes = imageBytes agent kind;
      inherit (agent) ownerUid ownerGid;
      stateDirectories = lib.optionals (
        kind == "state"
      ) config.tentaflake.agentInstances.${name}.stateDirectories;
      imagePath = imagePath name kind;
      mountPath = mountPath agent kind;
    };

  admissionVolumes = builtins.listToAttrs (
    map (
      {
        name,
        agent,
        kind,
      }:
      let
        allocationId = "${kind}:${name}";
        binding = {
          inherit kind;
          imagePath = imagePath name kind;
          mountPath = mountPath agent kind;
          uid = agent.ownerUid;
          gid = agent.ownerGid;
        };
      in
      lib.nameValuePair allocationId {
        spec = {
          inherit allocationId kind;
          instanceId = name;
          ownerId = "host-root";
          inherit (binding) imagePath mountPath;
          bytes = imageBytes agent kind;
          inherit (agent) ownerUid ownerGid;
          bindingRevision = "sha256:${builtins.hashString "sha256" (builtins.toJSON binding)}";
        };
        units = {
          prepare = prepareUnit name kind;
          owner = ownerUnit name kind;
          mount = mountUnit agent kind;
        };
        helpers = lib.mapAttrs (_: toString) (volumeHelpers name agent kind);
      }
    ) volumes
  );

  prepareService =
    name: agent: kind:
    let
      target = mountPath agent kind;
      helpers = admissionVolumes."${kind}:${name}".helpers;
      stateParent = lib.optional (
        kind == "workspace" && agent.state != null && pathWithin agent.state.path agent.workspace
      ) (ownerUnit name "state");
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
      script = ''
        image=${lib.escapeShellArg (imagePath name kind)}
        if [ -e "$image" ] || [ -L "$image" ]; then
          exec ${helpers.verify}
        else
          exec ${helpers.create}
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
    in
    {
      description = "Apply ${kind} ownership after mounting ${name}";
      requires = [ (mountUnit agent kind) ] ++ stateDependency;
      after = [ (mountUnit agent kind) ] ++ stateDependency;
      wantedBy = [ "multi-user.target" ];
      script = "exec ${admissionVolumes."${kind}:${name}".helpers.owner}";
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

  guardedService =
    name: agent: kind:
    let
      stateDependency = lib.optional (kind == "workspace" && agent.state != null) (
        ownerUnit name "state"
      );
      command =
        action:
        lib.escapeShellArgs [
          admissionCli
          "admission"
          action
          (toString config.tentaflake.admissionManifest)
          "${kind}:${name}"
        ];
    in
    {
      description = "Guard fixed-size ${kind} lifecycle for ${name}";
      requires = [ "systemd-tmpfiles-setup.service" ] ++ stateDependency;
      after = [
        "local-fs.target"
        "systemd-tmpfiles-setup.service"
      ]
      ++ stateDependency;
      before = [ "umount.target" ];
      conflicts = [ "umount.target" ];
      wantedBy = [ "multi-user.target" ];
      serviceConfig = {
        Type = "oneshot";
        RemainAfterExit = true;
        ExecStart = command "volume-start";
        ExecStop = command "volume-stop";
        KillMode = "control-group";
        TimeoutStartSec = 300;
        TimeoutStopSec = 120;
        UMask = "0077";
        NoNewPrivileges = true;
        # Mount/unmount must affect the host. Filesystem namespace hardening
        # here would hide the mounted image from controllers and the ledger.
        PrivateMounts = false;
        PrivateTmp = false;
        ProtectSystem = false;
        ProtectHome = false;
      };
    };
in
{
  options.tentaflake.workspaceQuota.agents = lib.mkOption {
    type = lib.types.attrsOf agentType;
    default = { };
    description = "Per-controller fixed-size persistent workspace and optional private state filesystems.";
  };

  options.tentaflake.admissionVolumes = lib.mkOption {
    type = lib.types.attrsOf lib.types.raw;
    readOnly = true;
    internal = true;
    description = "Quota-owned immutable backing specifications, systemd units and mutation helpers for host admission.";
  };

  config = {
    tentaflake.admissionVolumes = admissionVolumes;
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
            if enforced then { enable = false; } else prepareService volume.name volume.agent volume.kind;
          "tentaflake-${volume.kind}-quota-${volume.name}" =
            (if enforced then guardedService else ownerService) volume.name volume.agent
              volume.kind;
        }
      ) { } volumes;

      units = lib.mkIf enforced (
        builtins.listToAttrs (
          map (volume: lib.nameValuePair (mountUnit volume.agent volume.kind) { enable = false; }) volumes
        )
      );

      mounts = lib.optionals (!enforced) (
        map (
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
        ) volumes
      );
    };
  };
}
