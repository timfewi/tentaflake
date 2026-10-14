{
  config,
  lib,
  pkgs,
  ...
}:
let
  cfg = config.tentaflake.security;
  secure = cfg.profile != "dev";
  inherit (import ../lib/resourceLimits.nix { inherit lib; })
    maxMemoryBytes
    maxTmpfsBytes
    validSize
    sizeBytes
    validCpus
    validResource
    ;
in
{
  options.tentaflake.security = {
    profile = lib.mkOption {
      type = lib.types.enum [
        "dev"
        "balanced"
        "strict"
      ];
      default = "balanced";
      description = ''
        Agent security profile. balanced is the installed-host default. dev is
        a compatibility profile and is not suitable for untrusted 24/7 agents.
        strict currently fails evaluation rather than falling back to a weaker
        kernel boundary.
      '';
    };

    resources = {
      memory = lib.mkOption {
        type = lib.types.str;
        default = "2g";
        description = "Per-environment memory limit passed to the OCI runtime.";
      };
      memorySwap = lib.mkOption {
        type = lib.types.str;
        default = "2g";
        description = "Per-environment memory plus swap ceiling. Equal to memory disables additional swap.";
      };
      cpus = lib.mkOption {
        type = lib.types.str;
        default = "2.0";
        description = "Per-environment CPU quota in OCI --cpus syntax.";
      };
      nofile = lib.mkOption {
        type = lib.types.ints.positive;
        default = 4096;
        description = "Per-environment open-file ulimit.";
      };
      pidsLimit = lib.mkOption {
        type = lib.types.ints.positive;
        default = 512;
        description = "Maximum process count for a generic workload, including threads.";
      };
      tmpfsSize = lib.mkOption {
        type = lib.types.str;
        default = "256m";
        description = "Size of each general-purpose secure tmpfs mount.";
      };
      runTmpfsSize = lib.mkOption {
        type = lib.types.str;
        default = "64m";
        description = "Size of the secure /run tmpfs mount.";
      };
    };
  };

  config = lib.mkMerge [
    {
      assertions = [
        {
          assertion = cfg.profile != "strict";
          message = ''
            tentaflake.security.profile = "strict" is intentionally unavailable:
            the repository does not yet implement or test a separate-kernel
            MicroVM boundary. Use balanced, or keep strict as a deployment
            requirement until that boundary exists. No fallback was applied.
          '';
        }
      ]
      ++ lib.optionals secure [
        {
          assertion = config.tentaflake.hardening.enable;
          message = "tentaflake: balanced/strict requires tentaflake.hardening.enable = true.";
        }
        {
          assertion = config.security.apparmor.enable;
          message = "tentaflake: balanced/strict requires AppArmor on the host.";
        }
        {
          assertion = config.tentaflake.management.capabilities.privateConnectivity;
          message = "tentaflake: balanced/strict requires a configured private management transport.";
        }
        {
          assertion = config.tentaflake.management.capabilities.sshAuthorization != "disabled";
          message = "tentaflake: balanced/strict requires a configured private SSH authorization policy.";
        }
        {
          assertion = !config.services.openssh.enable;
          message = "tentaflake: balanced/strict forbids the public OpenSSH module; use the reviewed private management SSH policy.";
        }
        {
          assertion =
            validSize maxMemoryBytes cfg.resources.memory && validSize maxMemoryBytes cfg.resources.memorySwap;
          message = "tentaflake: secure memory and memorySwap limits must be positive byte sizes no greater than 1 TiB, with an optional K/M/G/T suffix.";
        }
        {
          assertion =
            !(validSize maxMemoryBytes cfg.resources.memory)
            || !(validSize maxMemoryBytes cfg.resources.memorySwap)
            ||
              sizeBytes maxMemoryBytes cfg.resources.memorySwap >= sizeBytes maxMemoryBytes cfg.resources.memory;
          message = "tentaflake: secure memorySwap must be greater than or equal to memory.";
        }
        {
          assertion = validCpus cfg.resources.cpus;
          message = "tentaflake: secure cpus must be a positive decimal no greater than 1024 with at most five fractional digits.";
        }
        {
          assertion =
            validResource "nofile" cfg.resources.nofile && validResource "pidsLimit" cfg.resources.pidsLimit;
          message = "tentaflake: secure file and process limits must be positive integers no greater than 4294967294.";
        }
        {
          assertion =
            validSize maxTmpfsBytes cfg.resources.tmpfsSize
            && validSize maxTmpfsBytes cfg.resources.runTmpfsSize;
          message = "tentaflake: secure tmpfs sizes must be positive byte sizes no greater than 64 GiB, with an optional K/M/G/T suffix.";
        }
      ];

      warnings = lib.optional (cfg.profile == "dev") ''
        tentaflake security profile "dev" preserves compatibility and is not a
        security boundary for untrusted 24/7 agents.
      '';

    }

    (lib.mkIf secure {
      environment.systemPackages = [ pkgs.gvisor ];

      virtualisation.docker.daemon.settings.runtimes.runsc = {
        path = lib.getExe' pkgs.gvisor "runsc";
      };

      virtualisation.podman.extraRuntimes = [ pkgs.gvisor ];
    })
  ];
}
