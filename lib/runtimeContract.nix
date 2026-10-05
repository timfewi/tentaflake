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
  size = n: builtins.isString n && builtins.match "[1-9][0-9]*[bkmgBKMG]?" n != null;
  validResource =
    key: value:
    if
      lib.elem key [
        "nofile"
        "pidsLimit"
      ]
    then
      positive value
    else if key == "cpus" then
      builtins.isString value
      && builtins.match "([1-9][0-9]*([.][0-9]+)?|0[.][0-9]*[1-9][0-9]*)" value != null
    else
      size value;
  command = raw.command or [ ];
  capabilities =
    raw.capabilities or [
      "files"
      "shell"
      "terminal"
    ];
  workspace = raw.workspace or "/workspace";
  lifecycle = raw.lifecycle or "stopped";
in
if (raw.schemaVersion or null) != 1 then
  fail "schemaVersion must be 1."
else if !(raw ? image) || !builtins.isString raw.image then
  fail "image must be a digest-pinned OCI reference."
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
      "worker"
    ]
  ) capabilities
  || lib.unique capabilities != capabilities
then
  fail "unsupported or duplicate capabilities; generic model/Research hooks are not accepted yet."
else
  {
    schemaVersion = 1;
    image = (import ./pinnedImage.nix { inherit lib; }) "generic" false raw.image;
    inherit
      command
      ownership
      workspace
      resources
      lifecycle
      capabilities
      ;
  }
