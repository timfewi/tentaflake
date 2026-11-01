# Source validation for an incoming stopped workload, never host activation.
# Use the same parser/builders as the installer and installed configuration.
{
  pkgs,
  backend ? "docker",
  existingInstances ? [ ],
  desiredAgents ? [ ],
}:
data:
let
  inherit (pkgs) lib;
  runtimeCatalog = import ./runtimeCatalog.nix;
  # Import adds stopped workloads only; host policy is never imported implicitly.
  schema = builtins.fromJSON (builtins.readFile ../schema/system-v1.json);
  allowed = builtins.attrNames schema.agent;
  validated = (import ./systemSchema.nix { inherit lib; }) data;
  importable =
    builtins.isAttrs data
    && data ? schemaVersion
    && data ? agents
    && lib.subtractLists [ "schemaVersion" "agents" "_securityNote" ] (builtins.attrNames data) == [ ]
    && builtins.isList data.agents
    && data.agents != [ ]
    && lib.all (
      entry: builtins.isAttrs entry && lib.subtractLists allowed (builtins.attrNames entry) == [ ]
    ) data.agents;
  evaluate =
    agents:
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
      ++ (import ./agentsFromData.nix { inherit pkgs lib; }) {
        data = {
          schemaVersion = 1;
          inherit agents;
        };
      };
    }).config;
  evaluated = evaluate validated.agents;
  # Desired additions may not be active in the installed host yet. Project
  # their metadata without applying incoming-only startup or host-policy checks.
  desiredInstances =
    if desiredAgents == [ ] then
      [ ]
    else
      lib.attrValues (evaluate desiredAgents).tentaflake.agentInstances;
  isolated = (import ./instanceIsolation.nix { inherit lib; }) (
    existingInstances ++ desiredInstances ++ lib.attrValues evaluated.tentaflake.agentInstances
  );
  containers = evaluated.virtualisation.oci-containers.containers;
  failures = map (item: item.message) (lib.filter (item: !item.assertion) evaluated.assertions);
  stopped = lib.all (container: !container.autoStart) (lib.attrValues containers);
  errors =
    failures
    ++ lib.optional (
      !stopped
    ) "Incoming workloads must be explicitly stopped; configure activation policy separately."
    ++ lib.optional (
      !isolated
    ) "Managed agent state and workspace paths must not overlap across instances.";
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
          vendorAcceptance = runtimeCatalog.presets.${instance.adapter}.evidence.vendorAcceptance;
        }) evaluated.tentaflake.agentInstances;
  }
