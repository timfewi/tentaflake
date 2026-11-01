{
  config,
  lib,
  pkgs,
  ...
}:
let
  inherit (lib)
    mkEnableOption
    mkIf
    mkOption
    types
    ;
  cfg = config.tentaflake.control;
  hashType = types.strMatching "[0-9a-f]{64}";
  settings = pkgs.writeText "tentaflake-control.json" (
    builtins.toJSON {
      source = cfg.desiredSource;
      inherit (cfg) normalizer peers hashes;
      schema = "${../schema/system-v1.json}";
      state = "/var/lib/tentaflake-control";
      socket = "/run/tentaflake-control/control.sock";
      server_uid = cfg.uid;
      minimum_profile = cfg.minimumProfile;
      builder =
        if cfg.build.enable then
          {
            template = cfg.build.template;
            template_hash = cfg.build.templateHash;
            nix = "${config.nix.package}/bin/nix";
            target = cfg.build.target;
            deadline_seconds = cfg.build.deadlineSeconds;
          }
        else
          null;
    }
  );
in
{
  options.tentaflake.control = {
    enable = mkEnableOption "approval-bound unprivileged desired-system control (no activation)";
    package = mkOption {
      type = types.package;
      default = pkgs.callPackage ../pkgs/tentaflake-control { };
      description = "Package providing the unprivileged tentaflake-control server and client.";
    };
    uid = mkOption {
      type = types.ints.between 1 4294967294;
      description = "Deployment-reserved non-root UID, also verified by Unix clients.";
    };
    desiredSource = mkOption {
      type = types.str;
      default = "/var/lib/tentaflake/desired/tentaflake.toml";
      description = "Read-only desired source. All path components must be real, not symlinks.";
    };
    normalizer = mkOption {
      type = types.str;
      default = "${pkgs.callPackage ../pkgs/tentaflake-cli { }}/bin/tentaflake";
      description = "Fixed immutable /nix/store executable for tentaflake config normalize.";
    };
    minimumProfile = mkOption {
      type = types.enum [
        "balanced"
        "strict"
      ];
      default = "balanced";
      description = "Minimum allowed candidate sandbox; a stricter current source also cannot be downgraded.";
    };
    peers = mkOption {
      type = types.listOf (
        types.submodule {
          options = {
            uid = mkOption { type = types.ints.unsigned; };
            roles = mkOption {
              type = types.listOf (
                types.enum [
                  "read"
                  "propose"
                  "approve"
                  "build"
                ]
              );
              description = "Exact granted operations; roles do not imply one another.";
            };
          };
        }
      );
      default = [ ];
      description = "Exact Unix peer UIDs; there is no group or root bypass.";
    };
    hashes = {
      policy = mkOption {
        type = hashType;
        description = "Reviewed approval-policy revision SHA-256.";
      };
      compiler = mkOption {
        type = hashType;
        description = "SHA-256 of fixed normalizer executable bytes.";
      };
      inputs = mkOption {
        type = hashType;
        description = "SHA-256 of reviewed template flake.lock, or declared disabled-build input revision.";
      };
    };
    build = {
      enable = mkEnableOption "explicitly granted fixed-target Nix daemon builds";
      template = mkOption {
        type = types.str;
        default = "";
        description = "Reviewed immutable store source; no symlinks, dotfiles or runtime/private directories.";
      };
      templateHash = mkOption {
        type = types.str;
        default = "";
        description = "Control template inventory SHA-256 (see system-control guide).";
      };
      target = mkOption {
        type = types.str;
        default = "";
        description = "Fixed nixosConfigurations attribute name.";
      };
      daemonReady = mkOption {
        type = types.bool;
        default = false;
        description = "Operator confirms daemon access, pinned inputs and isolated build budgets; does not configure trusted-users.";
      };
      deadlineSeconds = mkOption {
        type = types.ints.between 1 3600;
        default = 900;
      };
    };
  };
  config = mkIf cfg.enable {
    assertions = [
      {
        assertion = cfg.peers != [ ];
        message = "tentaflake.control requires exact peer UID/role grants.";
      }
      {
        assertion = lib.hasPrefix "/nix/store/" cfg.normalizer;
        message = "tentaflake.control normalizer must be an immutable store executable.";
      }
      {
        assertion =
          !cfg.build.enable
          || (
            cfg.build.daemonReady
            && config.nix.enable
            && config.nix.settings.sandbox
            && lib.hasPrefix "/nix/store/" cfg.build.template
            && builtins.match "[0-9a-f]{64}" cfg.build.templateHash != null
            && builtins.match "[a-zA-Z0-9_-]{1,64}" cfg.build.target != null
          );
        message = "Control builds require reviewed template/hash/target, sandboxed Nix daemon and explicit daemonReady grant.";
      }
    ];
    users.groups.tentaflake-control = { };
    users.users.tentaflake-control = {
      isSystemUser = true;
      inherit (cfg) uid;
      group = "tentaflake-control";
      home = "/var/lib/tentaflake-control";
      description = "Approval-bound system draft/build controller";
    };
    environment.etc."tentaflake-control.json".source = settings;
    environment.systemPackages = [ cfg.package ];
    systemd.services.tentaflake-control = {
      description = "Tentaflake approval-bound system control (never activation)";
      wantedBy = [ "multi-user.target" ];
      after = lib.optionals cfg.build.enable [ "nix-daemon.socket" ];
      requires = lib.optionals cfg.build.enable [ "nix-daemon.socket" ];
      serviceConfig = {
        ExecStart = "${cfg.package}/bin/tentaflake-control serve ${settings}";
        User = "tentaflake-control";
        Group = "tentaflake-control";
        StateDirectory = "tentaflake-control";
        StateDirectoryMode = "0700";
        RuntimeDirectory = "tentaflake-control";
        RuntimeDirectoryMode = "0711";
        UMask = "0077";
        NoNewPrivileges = true;
        CapabilityBoundingSet = "";
        AmbientCapabilities = "";
        PrivateTmp = true;
        PrivateDevices = true;
        PrivateNetwork = true;
        ProtectSystem = "strict";
        ProtectHome = true;
        ProtectKernelTunables = true;
        ProtectKernelModules = true;
        ProtectKernelLogs = true;
        ProtectControlGroups = true;
        ProtectClock = true;
        RestrictSUIDSGID = true;
        RestrictRealtime = true;
        LockPersonality = true;
        RestrictAddressFamilies = [ "AF_UNIX" ];
        # Resolve the current source entry; file bind mounts would hide atomic
        # replacements. ProtectSystem already keeps desired source read-only.
        ReadWritePaths = [
          "/var/lib/tentaflake-control"
          "/run/tentaflake-control"
        ];
        InaccessiblePaths = lib.optionals (!cfg.build.enable) [ "/nix/var/nix/daemon-socket" ];
        TasksMax = 32;
        MemoryMax = "512M";
        CPUQuota = "100%";
        LimitNOFILE = 128;
        LimitCORE = 0;
        TimeoutStopSec = 10;
        KillMode = "control-group";
        Restart = "on-failure";
        RestartSec = 5;
      };
    };
  };
}
