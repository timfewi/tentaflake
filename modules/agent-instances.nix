{
  config,
  lib,
  pkgs,
  ...
}:
let
  declarations = config.tentaflake.agentDeclarations;
  containers = map (entry: entry.container) declarations;
  runnable = lib.filter (entry: entry.runnable) declarations;
  parentGuard = ''
    for directory in /var /var/lib /var/lib/tentaflake /var/lib/tentaflake/agents; do
      if [ -L "$directory" ] || { [ -e "$directory" ] && [ ! -d "$directory" ]; }; then
        echo "tentaflake: refusing unsafe agent parent: $directory" >&2
        exit 1
      fi
      if [ ! -e "$directory" ]; then
        ${pkgs.coreutils}/bin/mkdir -m 0755 "$directory"
      fi
      owner=$(${pkgs.coreutils}/bin/stat -c %u "$directory")
      mode=$(${pkgs.coreutils}/bin/stat -c %a "$directory")
      if [ "$owner" != 0 ] || [ "$((0$mode & 022))" != 0 ]; then
        echo "tentaflake: agent parents must be root-owned and not group/world writable: $directory" >&2
        exit 1
      fi
    done
  '';
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
  config = {
    tentaflake.agentInstances =
      if builtins.length containers != builtins.length (lib.unique containers) then
        throw "tentaflake: duplicate global instance/container identity: ${lib.concatStringsSep ", " containers}."
      else
        builtins.listToAttrs (map (entry: lib.nameValuePair entry.container entry) declarations);
    # Check/create only shared parents before any tmpfiles state initialization.
    # Scaffolds reserve identities without touching runtime state.
    systemd.services = lib.mkIf (runnable != [ ]) (
      {
        systemd-tmpfiles-setup.preStart = lib.mkBefore parentGuard;
      }
      // builtins.listToAttrs (
        map (
          entry:
          lib.nameValuePair (lib.removeSuffix ".service" entry.unit) {
            requires = [ "systemd-tmpfiles-setup.service" ];
            after = [ "systemd-tmpfiles-setup.service" ];
            preStart = lib.mkBefore parentGuard;
          }
        ) runnable
      )
    );
  };
}
