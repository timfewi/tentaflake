# Source validation for an incoming stopped workload, never host activation.
# Use the same parser/builders as the installer and installed configuration.
{
  pkgs,
  backend ? "docker",
}:
data:
let
  inherit (pkgs) lib;
  # Import adds ordinary stopped workloads. Operational host services, alternate
  # mounts/users and credential files stay in the administrator's configuration.
  allowed = [
    "adapter"
    "name"
    "autoStart"
    "definition"
    "image"
    "cmd"
    "settings"
    "containerUid"
    "containerGid"
    "pidsLimit"
  ];
  importable =
    builtins.isAttrs data
    &&
      builtins.attrNames data == [
        "agents"
        "schemaVersion"
      ]
    && builtins.isList data.agents
    && data.agents != [ ]
    && lib.all (
      entry: builtins.isAttrs entry && lib.subtractLists allowed (builtins.attrNames entry) == [ ]
    ) data.agents;
  builders = import ./default.nix { inherit pkgs lib; };
  modules = builders.agentsFromData {
    inherit data;
    inherit (builders) mkAgent mkHermesAgent mkZeroClawAgent;
  };
  evaluated =
    (import (pkgs.path + "/nixos/lib/eval-config.nix") {
      system = pkgs.stdenv.hostPlatform.system;
      specialArgs.researchFlake = null;
      modules = [
        ../modules/default.nix
        {
          system.stateVersion = "26.05";
          virtualisation.oci-containers.backend = backend;
          fileSystems."/" = {
            device = "/dev/disk/by-label/nixos";
            fsType = "btrfs";
          };
          boot.loader.grub.devices = [ "nodev" ];
          tentaflake = {
            adminUser = "operator";
            security.profile = "balanced";
            boot.enable = false;
            locale.enable = false;
            networking.enable = false;
            nixSettings.enable = false;
            packages.enable = false;
            shell.enable = false;
          };
        }
      ]
      ++ modules;
    }).config;
  containers = evaluated.virtualisation.oci-containers.containers;
  failures = map (item: item.message) (lib.filter (item: !item.assertion) evaluated.assertions);
  stopped = lib.all (container: !container.autoStart) (lib.attrValues containers);
  errors =
    failures
    ++ lib.optional (
      !stopped
    ) "Incoming workloads must be explicitly stopped; configure activation policy separately.";
in
if !importable then
  throw "tentaflake: onboarding requires schema-v1 agent additions without host authority overrides."
else
  {
    schemaVersion = 1;
    valid = errors == [ ];
    inherit errors;
    instances =
      if errors != [ ] then
        [ ]
      else
        lib.mapAttrsToList (name: instance: {
          inherit (instance)
            adapter
            name
            unit
            stateDir
            workspace
            uid
            gid
            runnable
            ;
          container = name;
          image = if builtins.hasAttr name containers then containers.${name}.image else null;
          requiresHostUpdate = true;
          vendorAcceptance = builders.runtimeCatalog.presets.${instance.adapter}.evidence.vendorAcceptance;
        }) evaluated.tentaflake.agentInstances;
  }
