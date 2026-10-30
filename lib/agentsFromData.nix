# Internal versioned, non-secret parser shared by file loading and onboarding.
{
  lib,
  pkgs ? null,
  ...
}:
{
  file ? null,
  data ? null,
}:
let
  mkAgent = import ./mkAgent.nix { inherit pkgs lib; };
  fail = message: throw "tentaflake agentsFromData: ${message}";
  rawData =
    if (file == null) == (data == null) then
      fail "provide exactly one file or data object."
    else if data != null then
      data
    else if !lib.hasSuffix ".toml" (toString file) then
      fail "public inventories must be TOML files; manually migrate legacy JSON to agents.toml before loading."
    else if builtins.pathExists (builtins.dirOf (toString file) + "/agents.json") then
      fail "legacy agents.json conflicts with the TOML inventory; manually migrate it and remove agents.json before loading."
    else
      builtins.fromTOML (builtins.readFile file);
  checkFields =
    location: allowed: value:
    let
      unknown = lib.subtractLists allowed (builtins.attrNames value);
    in
    if unknown == [ ] then
      value
    else
      fail "${location} has unknown field(s): ${lib.concatStringsSep ", " unknown}.";
  finiteNumbers =
    value:
    if builtins.isFloat value then
      # Only finite floats subtract from themselves to zero (NaN != 0).
      value - value == 0
    else if builtins.isList value then
      lib.all finiteNumbers value
    else if builtins.isAttrs value then
      lib.all finiteNumbers (builtins.attrValues value)
    else
      true;
  checkedData =
    if !builtins.isAttrs rawData then
      fail "root must be an object."
    else if !finiteNumbers rawData then
      fail "inventory values must not contain non-finite numbers."
    else
      checkFields "root" [ "schemaVersion" "agents" "_securityNote" ] rawData;
  registry = import ../adapters { inherit pkgs lib; };
  inherit (import ./containerSecurity.nix { inherit lib; }) containsSensitiveValue;
  nullable = predicate: value: value == null || predicate value;
  strings = value: builtins.isList value && lib.all builtins.isString value;
  unsigned = value: builtins.isInt value && value >= 0;
  # Types describe builder arguments, never select a runtime or translate settings.
  types = {
    autoStart = builtins.isBool;
    allowMutableImage = builtins.isBool;
    createUser = builtins.isBool;
    stateDir = builtins.isString;
    image = builtins.isString;
    user = builtins.isString;
    group = builtins.isString;
    envFile = nullable builtins.isString;
    agenixFile = nullable builtins.isString;
    seedDir = nullable builtins.isString;
    networkMode = nullable builtins.isString;
    uid = nullable unsigned;
    gid = nullable unsigned;
    containerUid = unsigned;
    containerGid = unsigned;
    pidsLimit = nullable unsigned;
    gatewayPort = unsigned;
    hostPort = nullable unsigned;
    servePort = nullable unsigned;
    cmd = nullable strings;
    extraVolumes = strings;
    healDataDirs = strings;
    extraEnvironment =
      value: builtins.isAttrs value && lib.all builtins.isString (builtins.attrValues value);
    extraContainerConfig = builtins.isAttrs;
    settings = nullable builtins.isAttrs;
    definition = builtins.isAttrs;
    gitIdentity = nullable builtins.isAttrs;
    dashboard = nullable builtins.isAttrs;
    services = builtins.isAttrs;
    providerHealthcheck = nullable builtins.isAttrs;
  };
  checkEntry =
    index: entry:
    let
      location = "agents[${toString index}]";
      args = builtins.functionArgs registry.${entry.adapter}.build;
      missing = lib.filter (field: !(builtins.hasAttr field entry)) (
        [ "adapter" ] ++ builtins.attrNames (lib.filterAttrs (_: optional: !optional) args)
      );
      invalid = lib.filter (field: builtins.hasAttr field types && !(types.${field} entry.${field})) (
        builtins.attrNames entry
      );
      checked = checkFields location ([ "adapter" ] ++ builtins.attrNames args) entry;
    in
    if !builtins.isAttrs entry then
      fail "${location} must be an object."
    else if !(entry ? name) || !(entry ? adapter) then
      fail "${location} requires name and adapter."
    else if !builtins.isString entry.name || builtins.match "[a-z0-9][a-z0-9-]*" entry.name == null then
      fail "${location}.name must contain lowercase ASCII letters, digits, and hyphens."
    else if !builtins.isString entry.adapter || !(builtins.hasAttr entry.adapter registry) then
      fail "${location} has unknown adapter; select ${lib.concatStringsSep ", " (builtins.attrNames registry)}."
    else if missing != [ ] then
      fail "${location} is missing: ${lib.concatStringsSep ", " missing}."
    else if containsSensitiveValue entry then
      fail "${location} contains a secret-like field; use operator-provisioned runtime credential paths."
    else if invalid != [ ] then
      fail "${location} has invalid argument type(s): ${lib.concatStringsSep ", " invalid}."
    else
      checked;
  agents = lib.imap0 checkEntry checkedData.agents;
  identities = map (entry: (import ./agentIdentity.nix).container entry.name) agents;
  validate =
    if !builtins.isInt (checkedData.schemaVersion or null) || checkedData.schemaVersion != 1 then
      fail "schemaVersion is required and must be integer 1."
    else if !(checkedData ? agents) || !builtins.isList checkedData.agents then
      fail "agents must be an array."
    else if builtins.length identities != builtins.length (lib.unique identities) then
      fail "duplicate global instance/container identity: ${lib.concatStringsSep ", " identities}."
    else
      true;
in
assert validate;
map mkAgent agents
