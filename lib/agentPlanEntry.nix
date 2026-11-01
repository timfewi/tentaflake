# CLI passes paths through --argstr, never by interpolating Nix expressions.
{
  file,
  flakeDir,
  backend,
  configurationName,
}:
let
  # Preserve the flake's normal source filtering. A path: snapshot would copy
  # ignored operator files into the store, potentially including credentials.
  installed = builtins.getFlake flakeDir;
  pkgs = installed.inputs.nixpkgs.legacyPackages.${builtins.currentSystem};
  # Read current desired state, not only the flake's installed/filtered snapshot.
  desired = (import ./systemSchema.nix { inherit (pkgs) lib; }) (
    builtins.fromTOML (builtins.readFile (flakeDir + "/tentaflake.toml"))
  );
  plan =
    (import ./agentPlan.nix {
      inherit pkgs backend;
      existingInstances = pkgs.lib.attrValues host.tentaflake.agentInstances;
      desiredAgents = desired.agents;
    })
      (builtins.fromJSON (builtins.readFile file));
  host = installed.nixosConfigurations.${configurationName}.config;
  declared = host.virtualisation.oci-containers.containers // host.tentaflake.agentInstances;
  conflicts = builtins.filter (instance: builtins.hasAttr instance.container declared) plan.instances;
  policyErrors =
    pkgs.lib.optional
      (
        pkgs.lib.any (instance: instance.adapter == "generic") plan.instances
        && host.tentaflake.security.profile != "balanced"
      )
      "Generic onboarding requires the balanced host profile; change host policy separately before import.";
in
{
  result = plan // {
    valid = plan.valid && conflicts == [ ] && policyErrors == [ ];
    errors =
      plan.errors
      ++ policyErrors
      ++ map (
        instance: "Container ${instance.container} already exists in the installed configuration."
      ) conflicts;
  };
}
