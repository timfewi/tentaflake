{ self, pkgs }:
let
  inherit (pkgs) lib;
  builders = self.lib.${pkgs.stdenv.hostPlatform.system};
  host = {
    system.stateVersion = "26.05";
    fileSystems."/" = {
      device = "/dev/disk/by-label/nixos";
      fsType = "ext4";
    };
    boot.loader.grub.devices = [ "nodev" ];
    virtualisation.oci-containers.backend = "docker";
    tentaflake = {
      hostName = "research-policy-fixture";
      adminUser = "operator";
      security.profile = "balanced";
      boot.enable = false;
      locale.enable = false;
      networking.enable = false;
      nixSettings.enable = false;
      packages.enable = false;
      shell.enable = false;
      research.agents.hermes-policy.uid = 62101;
    };
    services.secureResearch = {
      serviceUid = 4201;
      egressUid = 4202;
      vpnInterface = "fixture-vpn";
      resolvers = [ "9.9.9.9" ];
    };
  };
  evaluate =
    args: extra:
    (import (pkgs.path + "/nixos/lib/eval-config.nix") {
      system = pkgs.stdenv.hostPlatform.system;
      modules = [
        self.nixosModules.default
        host
        (builders.mkHermesAgent (
          {
            name = "policy";
            autoStart = false;
          }
          // args
        ))
        extra
      ];
    }).config;
  failures =
    config: map (entry: entry.message) (lib.filter (entry: !entry.assertion) config.assertions);
  denies = fragment: config: lib.any (message: lib.hasInfix fragment message) (failures config);
  valid = evaluate { settings.agent.disabled_toolsets = [ "terminal" ]; } { };
  remote = evaluate { settings.mcp_servers.remote.url = "https://connector.example.org/mcp"; } { };
  missingMounts = evaluate { extraContainerConfig.volumes = [ ]; } { };
  legacy = evaluate { } {
    tentaflake.networking.enable = lib.mkForce true;
    tentaflake.broker.agents.hermes-policy = {
      enable = true;
      networkName = "tf-policy";
      subnet = "10.203.44.0/30";
      gateway = "10.203.44.1";
      fetch = {
        enable = true;
        allowedHosts = [ "docs.example.org" ];
      };
    };
  };
  summary = evaluate { } {
    services.secureResearch.summarizeOrder = [ "openai" ];
  };
  duplicateUid = evaluate { } {
    services.secureResearch.containerClients.another.uid = 62101;
  };
  client = import ../lib/researchClient.nix {
    inherit lib pkgs;
    config = valid;
    containerName = "hermes-policy";
    settings.agent.disabled_toolsets = [ "terminal" ];
    hermes = true;
  };
  record = lib.findFirst (line: lib.hasPrefix "agent\thermes-policy\t" line) "" (
    lib.splitString "\n" valid.environment.etc."tentaflake/security.tsv".text
  );
in
assert lib.elemAt (lib.splitString "\t" record) 10 == "true";
assert failures valid == [ ];
assert lib.all (name: lib.elem name client.settings.agent.disabled_toolsets) [
  "terminal"
  "web"
  "browser"
];
assert denies "only local stdio MCP" remote;
assert denies "retain its exact read-only" missingMounts;
assert denies "disable its legacy fetch broker" legacy;
assert denies "leave summarization disabled" summary;
assert denies "distinct upstream client UIDs" duplicateUid;
pkgs.runCommand "tentaflake-research-policy" { } "touch $out"
