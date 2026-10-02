{ lib }:
name: adapter:
let
  required = [
    "schemaVersion"
    "identity"
    "artifact"
    "command"
    "configuration"
    "ownership"
    "layout"
    "lifecycle"
    "model"
    "research"
    "execution"
    "build"
    "metadata"
  ];
  missing = lib.filter (field: !(builtins.hasAttr field adapter)) required;
  fields = {
    identity = [
      "id"
      "version"
      "status"
    ];
    artifact = [
      "kind"
      "reference"
      "reviewed"
    ];
    configuration = [
      "format"
      "readOnly"
      "generate"
    ];
    ownership = [
      "uid"
      "gid"
    ];
    layout = [
      "state"
      "workspace"
      "writable"
    ];
    model = [
      "protocols"
      "streaming"
    ];
    research = [
      "supported"
      "configure"
    ];
    execution = [
      "interface"
      "automatic"
    ];
  };
  incomplete = lib.concatLists (
    lib.mapAttrsToList (
      field: keys:
      if !builtins.isAttrs adapter.${field} then
        [ field ]
      else
        map (key: "${field}.${key}") (lib.filter (key: !(builtins.hasAttr key adapter.${field})) keys)
    ) fields
  );
  validArtifact =
    if adapter.artifact.kind == "oci" then
      adapter.artifact.reviewed
      && builtins.isString adapter.artifact.reference
      &&
        (import ./pinnedImage.nix { inherit lib; }) name false adapter.artifact.reference
        == adapter.artifact.reference
    else
      adapter.identity.status == "scaffold"
      && !adapter.artifact.reviewed
      && adapter.artifact.reference == null;
  valid =
    adapter.schemaVersion == 1
    && adapter.identity.id == name
    && builtins.isString adapter.identity.version
    && lib.elem adapter.identity.status [
      "compatibility"
      "scaffold"
      "operational"
    ]
    && lib.elem adapter.artifact.kind [
      "oci"
      "unselected"
    ]
    && builtins.isBool adapter.artifact.reviewed
    && (adapter.artifact.reference == null || builtins.isString adapter.artifact.reference)
    && validArtifact
    && builtins.isList adapter.command
    && lib.all builtins.isString adapter.command
    && lib.elem adapter.configuration.format [
      "yaml"
      "toml"
      "json"
    ]
    && builtins.isBool adapter.configuration.readOnly
    && adapter.configuration.readOnly
    && builtins.isFunction adapter.configuration.generate
    && builtins.isInt adapter.ownership.uid
    && adapter.ownership.uid > 0
    && adapter.ownership.uid <= 4294967294
    && builtins.isInt adapter.ownership.gid
    && adapter.ownership.gid > 0
    && adapter.ownership.gid <= 4294967294
    && builtins.isString adapter.layout.state
    && builtins.isString adapter.layout.workspace
    && builtins.isList adapter.layout.writable
    && lib.all builtins.isString adapter.layout.writable
    && lib.elem adapter.lifecycle [
      "service"
      "stopped-scaffold"
    ]
    && builtins.isList adapter.model.protocols
    && lib.all builtins.isString adapter.model.protocols
    && lib.elem adapter.model.streaming [
      "required"
      "optional"
      "unknown"
    ]
    && builtins.isBool adapter.research.supported
    && builtins.isFunction adapter.research.configure
    && builtins.isString adapter.execution.interface
    && builtins.isBool adapter.execution.automatic
    && builtins.isFunction adapter.build
    && builtins.isFunction adapter.metadata;
in
if missing != [ ] then
  throw "tentaflake: adapter ${name} contract is missing: ${lib.concatStringsSep ", " missing}."
else if incomplete != [ ] then
  throw "tentaflake: adapter ${name} contract has incomplete fields: ${lib.concatStringsSep ", " incomplete}."
else if !valid then
  throw "tentaflake: adapter ${name} has an invalid schema-v1 contract."
else
  adapter
