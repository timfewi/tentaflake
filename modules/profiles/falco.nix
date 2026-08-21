# Opt-in runtime detection. Falco is intentionally separate from the core and
# observability profiles because it needs host-wide eBPF visibility and a
# narrow set of powerful capabilities.
{
  config,
  lib,
  ...
}:
let
  cfg = config.tentaflake.profiles.falco;
  configFile =
    if cfg.configFile != null then cfg.configFile else "${cfg.package}/etc/falco/falco.yaml";
in
{
  options.tentaflake.profiles.falco = {
    enable = lib.mkEnableOption "the Falco modern-eBPF runtime-detection profile";

    package = lib.mkOption {
      type = lib.types.package;
      description = ''
        Falco package containing bin/falco and the upstream rules/config tree.
        The pinned nixpkgs revision does not package Falco, so consumers must
        supply a reviewed, pinned package explicitly.
      '';
    };

    configFile = lib.mkOption {
      type = lib.types.nullOr lib.types.str;
      default = null;
      example = "/etc/falco/falco.yaml";
      description = "Falco configuration file; defaults to the package-provided config.";
    };

    extraArgs = lib.mkOption {
      type = lib.types.listOf lib.types.str;
      default = [ ];
      description = "Additional escaped arguments passed to Falco.";
    };
  };

  config = lib.mkIf cfg.enable {
    systemd.services.tentaflake-falco = {
      description = "Tentaflake Falco runtime detection (modern eBPF)";
      wantedBy = [ "multi-user.target" ];
      after = [ "local-fs.target" ];
      serviceConfig = {
        Type = "simple";
        ExecStart = "${lib.getExe' cfg.package "falco"} -c ${lib.escapeShellArg configFile} -o engine.kind=modern_ebpf ${lib.escapeShellArgs cfg.extraArgs}";
        Restart = "on-failure";
        RestartSec = "5s";
        User = "root";
        NoNewPrivileges = true;
        ProtectSystem = "strict";
        ProtectHome = "read-only";
        PrivateTmp = true;
        PrivateDevices = false;
        ProtectKernelTunables = true;
        ProtectKernelModules = true;
        ProtectControlGroups = true;
        RestrictNamespaces = true;
        LockPersonality = true;
        CapabilityBoundingSet = [
          "CAP_BPF"
          "CAP_PERFMON"
          "CAP_SYS_RESOURCE"
          "CAP_SYS_PTRACE"
        ];
        AmbientCapabilities = [
          "CAP_BPF"
          "CAP_PERFMON"
          "CAP_SYS_RESOURCE"
          "CAP_SYS_PTRACE"
        ];
        RestrictAddressFamilies = [
          "AF_UNIX"
          "AF_NETLINK"
        ];
        MemoryMax = "1G";
        TasksMax = 256;
      };
    };
  };
}
