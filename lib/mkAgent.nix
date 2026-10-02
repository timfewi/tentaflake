{ pkgs, lib }:
let
  registry = import ../adapters { inherit pkgs lib; };
  validate = import ./adapterContract.nix { inherit lib; };
in
args@{ adapter, name, ... }:
let
  selected =
    if builtins.isString adapter && builtins.hasAttr adapter registry then
      validate adapter registry.${adapter}
    else
      throw "tentaflake: unknown adapter; select one of ${lib.concatStringsSep ", " (builtins.attrNames registry)}.";
  options = builtins.removeAttrs args [ "adapter" ];
  unknown = lib.subtractLists (builtins.attrNames (builtins.functionArgs selected.build)) (
    builtins.attrNames options
  );
in
if !builtins.isString name || builtins.match "[a-z0-9][a-z0-9-]*" name == null then
  throw "tentaflake: adapter instance name must contain lowercase ASCII letters, digits, and hyphens."
else if unknown != [ ] then
  throw "tentaflake: adapter ${adapter} does not support option(s): ${lib.concatStringsSep ", " unknown}."
else if (options.settings or null) != null && !builtins.isAttrs options.settings then
  throw "tentaflake: adapter ${adapter} settings must be an attribute set or null."
else
  { config, ... }:
  {
    imports = [ (selected.build options) ];
    tentaflake.agentDeclarations = [
      (selected.metadata options config.virtualisation.oci-containers.backend)
    ];
  }
