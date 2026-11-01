{
  config,
  lib,
  pkgs,
  ...
}:
let
  cfg = config.tentaflake.workloadMapping;
  instances = config.tentaflake.agentInstances;
  id = lib.types.strMatching "[A-Za-z0-9_-][A-Za-z0-9_.-]{0,63}";
  json = pkgs.formats.json { };
  policy = json.generate "tentaflake-workloads.json" {
    version = 1;
    docker = lib.getExe pkgs.docker;
    systemctl = "${pkgs.systemd}/bin/systemctl";
    bindings = lib.mapAttrsToList (name: binding: {
      agent_id = name;
      inherit (binding) tenant_id workload_id policy_revision;
      unit = instances.${name}.unit or "unknown";
    }) cfg.bindings;
  };
  common = {
    User = "root";
    Group = "root";
    StateDirectory = "tentaflake-workloads";
    StateDirectoryMode = "0700";
    RuntimeDirectory = "tentaflake-workloads";
    RuntimeDirectoryMode = "0755";
    RuntimeDirectoryPreserve = "yes";
    UMask = "0022";
    NoNewPrivileges = true;
    # Required only for /proc/PID/ns/user provenance of a non-root container PID.
    # No network-admin, mount, namespace-entry or BPF capability is granted.
    CapabilityBoundingSet = [ "CAP_SYS_PTRACE" ];
    AmbientCapabilities = [ "CAP_SYS_PTRACE" ];
    ProtectSystem = "strict";
    ProtectHome = true;
    PrivateTmp = true;
    PrivateDevices = true;
    PrivateNetwork = true;
    ProtectKernelTunables = true;
    ProtectKernelModules = true;
    ProtectKernelLogs = true;
    ProtectControlGroups = true;
    RestrictNamespaces = true;
    RestrictSUIDSGID = true;
    LockPersonality = true;
    RestrictRealtime = true;
    RestrictAddressFamilies = [ "AF_UNIX" ];
    IPAddressDeny = "any";
    SystemCallArchitectures = "native";
    ReadWritePaths = [
      "/var/lib/tentaflake-workloads"
      "/run/tentaflake-workloads"
    ];
    MemoryMax = "128M";
    TasksMax = 16;
  };
in
{
  options.tentaflake.workloadMapping = {
    enable = lib.mkEnableOption "fixed rootful Docker/kernel workload resource identity observer";
    package = lib.mkOption {
      type = lib.types.package;
      default = pkgs.callPackage ../pkgs/tentaflake-workloads { };
      description = "Immutable resolver helper; no configurable command or engine endpoint.";
    };
    bindings = lib.mkOption {
      default = { };
      description = "Host authorization policy keyed only by existing canonical managed agent IDs, not instance declarations.";
      type = lib.types.attrsOf (
        lib.types.submodule {
          options = {
            tenant_id = lib.mkOption { type = id; };
            workload_id = lib.mkOption { type = id; };
            policy_revision = lib.mkOption { type = id; };
          };
        }
      );
    };
  };
  config = lib.mkIf cfg.enable {
    assertions = [
      {
        assertion = config.virtualisation.oci-containers.backend == "docker";
        message = "workloadMapping supports only rootful Docker with exact systemd cgroups; other engines fail closed.";
      }
      {
        assertion = !(config.virtualisation.docker.daemon.settings ? userns-remap);
        message = "workloadMapping refuses daemon user-namespace remapping.";
      }
      {
        assertion = builtins.length (builtins.attrNames cfg.bindings) <= 64;
        message = "workloadMapping supports at most 64 explicit host bindings.";
      }
      {
        assertion = lib.all (
          name:
          builtins.hasAttr name instances
          && instances.${name}.runnable
          && instances.${name}.container == name
          && instances.${name}.unit == "docker-${name}.service"
          && builtins.hasAttr name config.virtualisation.oci-containers.containers
          && lib.hasPrefix "agent-" name
        ) (builtins.attrNames cfg.bindings);
        message = "workloadMapping bindings must reference unambiguous runnable canonical managed Docker agent declarations.";
      }
    ];
    environment.etc."tentaflake-workloads.json".source = policy;
    systemd.services.tentaflake-workloads = {
      description = "Read-only managed workload identity observer";
      wantedBy = [ "multi-user.target" ];
      requires = [ "docker.service" ];
      after = [ "docker.service" ];
      restartTriggers = [ policy ];
      serviceConfig = common // {
        ExecStart = "${cfg.package}/bin/tentaflake-workloads --run";
        Restart = "on-failure";
        RestartSec = "5s";
      };
    };
    # Explicit operator action only. Missing/corrupt prior authority never auto-resets.
    systemd.services.tentaflake-workloads-initialize = {
      description = "Explicit first-install workload generation authority bootstrap";
      conflicts = [ "tentaflake-workloads.service" ];
      before = [ "tentaflake-workloads.service" ];
      serviceConfig = common // {
        Type = "oneshot";
        ExecStart = "${cfg.package}/bin/tentaflake-workloads --initialize";
      };
    };
  };
}
