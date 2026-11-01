{ pkgs, lib }:
{ file }:
let
  fail = message: throw "tentaflake systemFromData: ${message}";
  directory = builtins.dirOf (toString file);
  legacy = lib.filter (name: builtins.pathExists (directory + "/${name}")) [
    "agents.toml"
    "agents.json"
    "my-agents.nix"
  ];
  data =
    if builtins.baseNameOf (toString file) != "tentaflake.toml" then
      fail "the sole public desired-state file is tentaflake.toml; migrate legacy inventories explicitly."
    else if legacy != [ ] then
      fail "legacy declaration files remain: ${lib.concatStringsSep ", " legacy}; migrate and remove them before loading."
    else if !builtins.pathExists file then
      fail "tentaflake.toml is required; a missing desired-state file is not an empty host. Include the installation-owned file in the consumer or fork's Git source."
    else
      builtins.fromTOML (builtins.readFile file);
  validated = (import ./systemSchema.nix { inherit lib; }) data;
  descriptor = builtins.fromJSON (builtins.readFile ../schema/system-v1.json);
  # Service grants and workload bindings are validated data, projected intact
  # to existing host options. They never participate in instance construction.
  # In particular an enabled public host API must carry explicit peers, so
  # omission cannot select the module's legacy implicit-group authorization.
  projected = lib.concatLists (
    lib.mapAttrsToList (
      section: fields:
      lib.mapAttrsToList (
        field: value:
        let
          spec = fields.${field};
        in
        # Opt-in enable paths do not override independent consumer policy when
        # the public section remains disabled (including Research and backups).
        if spec ? enabledPath then
          if value then lib.setAttrByPath spec.enabledPath true else { }
        else if spec ? path then
          lib.setAttrByPath spec.path value
        else
          { }
      ) (validated.${section} or { })
    ) descriptor.sections
  );
in
builtins.deepSeq validated {
  imports = (import ./agentsFromData.nix { inherit pkgs lib; }) {
    data = { inherit (validated) schemaVersion agents; };
  };
  config = lib.mkMerge projected;
}
