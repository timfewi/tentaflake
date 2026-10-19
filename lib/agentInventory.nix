{ lib }:
{
  backend,
  containers,
  instances,
}:
lib.concatStringsSep "\n" (
  map (
    container:
    let
      def = containers.${container} or { volumes = [ ]; };
      runtime =
        if lib.hasPrefix "zeroclaw-" container then
          "zeroclaw"
        else if lib.hasPrefix "hermes-" container then
          "hermes"
        else
          "agent";
      fallback = {
        adapter = runtime;
        name = lib.removePrefix "${runtime}-" container;
        inherit container;
        unit = "${backend}-${container}.service";
        stateDir =
          if def.volumes == [ ] then
            "/var/lib/${container}"
          else
            lib.head (lib.splitString ":" (lib.head def.volumes));
      };
      entry = instances.${container} or fallback;
    in
    "${entry.adapter}\t${entry.name}\t${entry.container}\t${entry.unit}\t${entry.stateDir}"
  ) (lib.unique (builtins.attrNames (containers // instances)))
)
