{ pkgs }:
let
  inherit (pkgs) lib;
  builders = import ../lib { inherit pkgs lib; };
  evaluate =
    extra:
    (import (pkgs.path + "/nixos/lib/eval-config.nix") {
      system = pkgs.stdenv.hostPlatform.system;
      specialArgs.researchFlake = null;
      modules = [
        ../modules/default.nix
        {
          system.stateVersion = "26.05";
          fileSystems."/" = {
            device = "/dev/disk/by-label/nixos";
            fsType = "btrfs";
          };
          boot.loader.grub.devices = [ "nodev" ];
          users.users.operator.uid = 1000;
          tentaflake = {
            adminUser = "operator";
            boot.enable = false;
            locale.enable = false;
            networking.enable = false;
            nixSettings.enable = false;
            packages.enable = false;
            shell.enable = false;
          };
        }
        extra
      ];
    }).config;
  failures = config: lib.filter (entry: !entry.assertion) config.assertions;
  denies = fragment: config: lib.any (entry: lib.hasInfix fragment entry.message) (failures config);
  valid = evaluate { };
  alias = evaluate { tentaflake.tailscale.enable = true; };
  disabledAlias = evaluate { tentaflake.tailscale.enable = false; };
  disabled = evaluate { tentaflake.management.enable = false; };
  absentService = evaluate { services.tailscale.enable = lib.mkForce false; };
  absentSsh = evaluate { services.tailscale.extraSetFlags = lib.mkForce [ ]; };
  disabledSsh = evaluate { tentaflake.management.ssh.policy = "disabled"; };
  conflictingSsh = value: evaluate { services.tailscale.extraUpFlags = [ value ]; };
  publicSsh = evaluate { tentaflake.ssh.enable = true; };
  development = evaluate {
    tentaflake.security.profile = "dev";
    tentaflake.management.ssh.policy = "disabled";
  };
  unknownTransport = evaluate { tentaflake.management.transport = "unreviewed"; };
  forgedCapability = evaluate {
    tentaflake.management.capabilities.privateConnectivity = lib.mkForce true;
  };
  manifest = valid.environment.etc."tentaflake/security.tsv".text;
  capsules = evaluate {
    imports = [
      (builders.mkHermesAgent {
        name = "management-hermes";
        autoStart = false;
      })
      (builders.mkZeroClawAgent {
        name = "management-zero";
        autoStart = false;
      })
    ];
  };
  brokerCapsule = evaluate {
    imports = [
      (builders.mkHermesAgent {
        name = "management-broker";
        autoStart = false;
      })
    ];
    tentaflake.networking.enable = lib.mkForce true;
    tentaflake.broker.agents.hermes-management-broker = {
      enable = true;
      subnet = "10.203.20.0/30";
      gateway = "10.203.20.1";
      llm = {
        enable = true;
        upstreamBaseUrl = "https://api.example.com/v1/";
        providerCredentialFile = "/run/agenix/example-provider";
        allowedModels = [
          {
            name = "example/model";
            inputMicrousdPerMillion = 1000000;
            outputMicrousdPerMillion = 2000000;
          }
        ];
      };
    };
  };
  capsuleManifest = capsules.environment.etc."tentaflake/security.tsv".text;
  brokerManifest = brokerCapsule.environment.etc."tentaflake/security.tsv".text;
in
assert lib.assertMsg (failures valid == [ ]) (
  builtins.toJSON (map (entry: entry.message) (failures valid))
);
assert valid.tentaflake.management.enable;
assert valid.tentaflake.management.transport == "tailscale";
assert valid.tentaflake.management.ssh.policy == "tailnet-policy";
assert valid.tentaflake.management.capabilities.privateConnectivity;
assert valid.tentaflake.management.capabilities.sshAuthorization == "tailnet-policy";
assert alias.tentaflake.management.capabilities == valid.tentaflake.management.capabilities;
assert alias.services.tailscale.enable == valid.services.tailscale.enable;
assert alias.services.tailscale.extraSetFlags == valid.services.tailscale.extraSetFlags;
assert alias.services.tailscale.extraUpFlags == valid.services.tailscale.extraUpFlags;
assert lib.elem "--ssh" valid.services.tailscale.extraSetFlags;
assert lib.elem "--advertise-tags=tag:agent-host" valid.services.tailscale.extraUpFlags;
assert
  !(lib.any (flag: lib.hasPrefix "--advertise-tags" flag) valid.services.tailscale.extraSetFlags);
assert lib.hasPrefix "manifest\t2\nhost\tbalanced\tfalse\tfalse\t" manifest;
assert lib.hasInfix "\nmanagement\ttailscale\ttrue\ttrue\ttailnet-policy\n" manifest;
assert lib.hasInfix "host\tbalanced\tfalse\tfalse\ttrue\ttrue\tfalse\t36\n" capsuleManifest;
assert lib.hasInfix "host\tbalanced\tfalse\ttrue\ttrue\ttrue\tfalse\t36\n" brokerManifest;
assert lib.all (name: lib.hasInfix "agent\t${name}\tbalanced" capsuleManifest) [
  "hermes-management-hermes"
  "zeroclaw-management-zero"
];
assert denies "private management transport" disabledAlias;
assert denies "private management transport" disabled;
assert denies "private management transport" absentService;
assert denies "private SSH authorization policy" absentSsh;
assert denies "private SSH authorization policy" disabledSsh;
assert lib.all (value: denies "private SSH authorization policy" (conflictingSsh value)) [
  "--ssh=false"
  "--ssh=0"
  "--"
];
assert denies "public OpenSSH" publicSsh;
assert failures development == [ ];
assert development.tentaflake.management.capabilities.sshAuthorization == "disabled";
assert lib.elem "--ssh=false" development.services.tailscale.extraSetFlags;
assert
  !(builtins.tryEval (builtins.deepSeq unknownTransport.tentaflake.management.transport true))
  .success;
assert
  !(builtins.tryEval forgedCapability.tentaflake.management.capabilities.privateConnectivity).success;
true
