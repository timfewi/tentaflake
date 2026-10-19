# Serializable support facts shared by Nix, the operator CLI and documentation.
let
  catalog = builtins.fromJSON (builtins.readFile ../adapters/catalog.json);
in
if catalog.schemaVersion != 1 || !builtins.isAttrs catalog.presets then
  throw "tentaflake: unsupported runtime catalog schema."
else
  catalog
