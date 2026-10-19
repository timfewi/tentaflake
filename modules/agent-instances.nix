{ config, lib, ... }:
let
  declarations = config.tentaflake.agentDeclarations;
  containers = map (entry: entry.container) declarations;
  field = type: lib.mkOption { inherit type; };
  column = lib.types.strMatching "[^\t\r\n]+";
  record = lib.types.submodule {
    options = {
      adapter = field column;
      name = field column;
      container = field column;
      unit = field column;
      stateDir = field column;
      stateStorage = field column;
      stateDirectories = lib.mkOption {
        type = lib.types.listOf column;
        default = [ ];
        description = "Adapter-owned relative state directories initialized inside a managed state filesystem.";
      };
      workspace = field column;
      uid = field lib.types.ints.unsigned;
      gid = field lib.types.ints.unsigned;
      runnable = field lib.types.bool;
    };
  };
in
{
  options.tentaflake = {
    agentDeclarations = lib.mkOption {
      type = lib.types.listOf record;
      default = [ ];
      internal = true;
      description = "Non-secret adapter declarations; duplicate container identities are rejected.";
    };
    agentInstances = lib.mkOption {
      type = lib.types.attrsOf record;
      readOnly = true;
      description = "Explicit adapter instance identities used by inventory and capability validation.";
    };
  };
  config.tentaflake.agentInstances =
    if builtins.length containers != builtins.length (lib.unique containers) then
      throw "tentaflake: duplicate adapter instance/container identity: ${lib.concatStringsSep ", " containers}."
    else
      builtins.listToAttrs (map (entry: lib.nameValuePair entry.container entry) declarations);
}
