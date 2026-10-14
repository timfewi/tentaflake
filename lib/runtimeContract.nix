{ lib }:
definition:
let
  fail = message: throw "tentaflake runtime definition: ${message}";
  fields =
    location: allowed: value:
    if !builtins.isAttrs value then
      fail "${location} must be an object."
    else if lib.subtractLists allowed (builtins.attrNames value) != [ ] then
      fail "${location} has unsupported fields."
    else
      value;
  raw = fields "root" [
    "schemaVersion"
    "image"
    "command"
    "ownership"
    "workspace"
    "resources"
    "lifecycle"
    "capabilities"
  ] definition;
  ownership = fields "ownership" [ "uid" "gid" ] (
    raw.ownership or (import ./runtimeCatalog.nix).presets.generic.ownership
  );
  resources = fields "resources" [
    "memory"
    "memorySwap"
    "cpus"
    "nofile"
    "pidsLimit"
    "tmpfsSize"
    "runTmpfsSize"
  ] (raw.resources or { });
  positive = n: builtins.isInt n && n > 0 && n <= 4294967294;
  inherit (import ./resourceLimits.nix { inherit lib; }) validResource maxMemoryBytes sizeBytes;
  command = raw.command or [ ];
  capabilities =
    raw.capabilities or [
      "files"
      "shell"
      "terminal"
    ];
  workspace = raw.workspace or "/workspace";
  lifecycle = raw.lifecycle or "stopped";
  imageSource = raw.image or null;
  localImage = fields "image" [ "kind" "descriptor" ] imageSource;
  descriptorValue = localImage.descriptor or null;
  descriptor =
    if builtins.isPath descriptorValue then
      "${descriptorValue}"
    else if lib.isDerivation descriptorValue || builtins.isString descriptorValue then
      toString descriptorValue
    else
      fail "local image descriptor must be a Nix derivation or contextual store path.";
  runtimeImage =
    if builtins.isString imageSource then
      {
        kind = "registry";
        reference = (import ./pinnedImage.nix { inherit lib; }) "generic" false imageSource;
      }
    else if
      (localImage.kind or null) != "local-artifact"
      || !builtins.hasContext descriptor
      || builtins.match "/nix/store/[0-9abcdfghijklmnpqrsvwxyz]{32}-[A-Za-z0-9+._=-]+" descriptor == null
      || lib.any (part: part == "." || part == "..") (lib.splitString "/" descriptor)
    then
      fail "local image requires an immutable contextual descriptor directly in the Nix store."
    else
      {
        kind = "local-artifact";
        inherit descriptor;
      };
in
if (raw.schemaVersion or null) != 1 then
  fail "schemaVersion must be 1."
else if !(raw ? image) || !(builtins.isString imageSource || builtins.isAttrs imageSource) then
  fail "image must be a digest-pinned OCI reference or a local artifact descriptor."
else if
  !builtins.isList command
  || command == [ ]
  || !lib.all (arg: builtins.isString arg && !(lib.hasInfix "\n" arg)) command
  || lib.head command == ""
then
  fail "command must be a non-empty argument vector."
else if !positive (ownership.uid or null) || !positive (ownership.gid or null) then
  fail "ownership requires positive numeric uid/gid."
else if
  !builtins.isString workspace || builtins.match "/workspace(/[A-Za-z0-9_-]+)*" workspace == null
then
  fail "workspace must be a normalized path at or below /workspace."
else if !lib.all (key: validResource key resources.${key}) (builtins.attrNames resources) then
  fail "resources must contain positive bounded OCI limits."
else if
  resources ? memory
  && resources ? memorySwap
  && sizeBytes maxMemoryBytes resources.memorySwap < sizeBytes maxMemoryBytes resources.memory
then
  fail "resources.memorySwap must be greater than or equal to resources.memory."
else if
  !lib.elem lifecycle [
    "stopped"
    "service"
  ]
then
  fail "lifecycle must be stopped or service."
else if
  !builtins.isList capabilities
  || !lib.all (
    cap:
    lib.elem cap [
      "files"
      "shell"
      "terminal"
      "model"
      "research"
      "worker"
    ]
  ) capabilities
  || lib.unique capabilities != capabilities
then
  fail "unsupported or duplicate capabilities."
else
  {
    schemaVersion = 1;
    # The local projection is a source locator, never an OCI reference. Only the
    # admission supervisor may resolve the descriptor and dispatch its image ID.
    image = if runtimeImage.kind == "registry" then runtimeImage.reference else runtimeImage.descriptor;
    inherit runtimeImage;
    inherit
      command
      ownership
      workspace
      resources
      lifecycle
      capabilities
      ;
  }
