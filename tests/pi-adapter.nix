{ pkgs }:
let
  inherit (pkgs) lib;
  builders = import ../lib { inherit pkgs lib; };
  adapter = builders.adapters.pi;
  contract = import ../lib/adapterContract.nix { inherit lib; };
  instance =
    args: backend:
    let
      config.virtualisation.oci-containers.backend = backend;
      selected = builders.mkAgent (
        {
          adapter = "pi";
          name = "pilot";
        }
        // args
      ) { inherit config; };
    in
    {
      record = builtins.head selected.tentaflake.agentDeclarations;
      module = (builtins.head selected.imports) { inherit config; };
    };
  docker = instance { } "docker";
  podman = instance { } "podman";
  service = docker.module.systemd.services.docker-pi-pilot;
  nixos =
    backend: extra:
    (import (pkgs.path + "/nixos/lib/eval-config.nix") {
      system = pkgs.stdenv.hostPlatform.system;
      modules = [
        ({ lib, ... }: {
          options.tentaflake.agentDeclarations = lib.mkOption {
            type = lib.types.listOf lib.types.attrs;
            default = [ ];
          };
          config = {
            system.stateVersion = "26.05";
            virtualisation.oci-containers.backend = backend;
            fileSystems."/" = {
              device = "none";
              fsType = "tmpfs";
            };
            boot.loader.grub.devices = [ "nodev" ];
          };
        })
        (builders.mkAgent {
          adapter = "pi";
          name = "pilot";
        })
        extra
      ];
    }).config;
  nixosDocker = nixos "docker" { };
  nixosPodman = nixos "podman" { };
  nixosCollision = nixos "docker" {
    virtualisation.oci-containers.containers.pi-pilot = {
      image = "example.invalid/unreviewed:latest";
      autoStart = true;
    };
  };
  conflicting = (adapter.build { name = "pilot"; }) {
    config = {
      virtualisation.oci-containers.backend = "docker";
      virtualisation.oci-containers.containers.pi-pilot = { };
      tentaflake.workspaceQuota.agents.pi-pilot.enable = true;
    };
  };
  rejects = value: !(builtins.tryEval (builtins.deepSeq value true)).success;
in
assert (contract "pi" adapter).identity.status == "scaffold";
assert lib.all (
  name:
  builders.runtimeCatalog.presets.${name}.identity.status != "scaffold"
  || !(builders.adapters.${name}.metadata { name = "fixture"; } "docker").runnable
) (builtins.attrNames builders.runtimeCatalog.presets);
assert adapter.identity.version == "1.4.0";
assert
  adapter.artifact == {
    kind = "unselected";
    reference = null;
    reviewed = false;
  };
assert adapter.command == [ ];
assert adapter.capabilities == [ ];
assert !adapter.research.supported;
assert !(adapter.research.configure { }).valid;
assert rejects (adapter.configuration.generate "pilot" { });
assert adapter.execution.interface == "unverified" && !adapter.execution.automatic;
assert adapter.evidence.vendorAcceptance == "pending";
assert docker.record.adapter == "pi" && !docker.record.runnable;
assert docker.record.container == "pi-pilot";
assert docker.record.unit == "docker-pi-pilot.service";
assert docker.record.uid == 10000 && docker.record.gid == 10000;
assert docker.record.stateDir == "/var/lib/pi-pilot";
assert docker.record.workspace == "/var/lib/pi-pilot/workspace";
assert podman.record.unit == "podman-pi-pilot.service";
assert
  builtins.attrNames docker.module == [
    "assertions"
    "systemd"
  ];
assert lib.all (item: item.assertion) docker.module.assertions;
assert builtins.attrNames docker.module.systemd == [ "services" ];
assert builtins.attrNames docker.module.systemd.services == [ "docker-pi-pilot" ];
assert service.wantedBy == [ ];
assert service.serviceConfig.Type == "oneshot" && service.serviceConfig.Restart == "no";
assert lib.hasInfix "acceptance" service.script && lib.hasInfix "exit 1" service.script;
assert builtins.attrNames podman.module.systemd.services == [ "podman-pi-pilot" ];
assert !(nixosDocker.virtualisation.oci-containers.containers ? pi-pilot);
assert nixosDocker.systemd.services.docker-pi-pilot.wantedBy == [ ];
assert nixosDocker.systemd.services.docker-pi-pilot.serviceConfig.Restart == "no";
assert lib.all (rule: !(lib.hasInfix "/var/lib/pi-pilot" rule)) nixosDocker.systemd.tmpfiles.rules;
assert !(nixosPodman.virtualisation.oci-containers.containers ? pi-pilot);
assert nixosPodman.systemd.services.podman-pi-pilot.wantedBy == [ ];
assert lib.any (
  item: !item.assertion && lib.hasInfix "cannot share its identity" item.message
) nixosCollision.assertions;
assert lib.count (item: !item.assertion) conflicting.assertions == 2;
assert lib.all (args: rejects (instance args "docker")) [
  { autoStart = true; }
  {
    settings = {
      debug = true;
    };
  }
  { stateDir = "/var/lib/pi-pilot/../outside"; }
  {
    image = "example.invalid/image@sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
  }
  { extraVolumes = [ "/host:/host:rw" ]; }
  { boardSocket = "/run/other/bridge.sock"; }
  { envFile = "/run/fixture.env"; }
];
true
