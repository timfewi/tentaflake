# ────────────────────────────────────────────────────────────
# agentsFromData — turn versioned adapter input and legacy agent arrays
# into the same list of NixOS modules my-agents.nix produces by hand.
#
# agents.json is declarative and NON-SECRET (git-tracked): instance arguments,
# model ids, providers, ports, and paths to runtime env files holding API keys.
# The keys themselves never appear here — only the `envFile` path that
# points at them.
#
# Usage (see configuration.nix):
#   agentsFromData { file = ./agents.json; inherit mkHermesAgent mkZeroClawAgent; }
# ────────────────────────────────────────────────────────────

{
  lib,
  pkgs ? null,
  ...
}:
{
  file ? null,
  data ? null,
  mkHermesAgent,
  mkZeroClawAgent,
  mkAgent ? null,
}:
let
  rawData =
    if (file == null) == (data == null) then
      fail "provide exactly one file or data object."
    else if data != null then
      data
    else
      builtins.fromJSON (builtins.readFile file);
  checkedData =
    if builtins.isAttrs rawData then
      checkFields "root" [ "schemaVersion" "agents" "hermes" "zeroclaw" "_securityNote" ] rawData
    else
      fail "root must be an object.";
  registry = import ../adapters { inherit pkgs lib; };
  inherit (import ./containerSecurity.nix { inherit lib; }) containsSensitiveValue;
  fail = message: throw "tentaflake agentsFromData: ${message}";
  checkFields =
    location: allowed: value:
    let
      unknown = lib.subtractLists allowed (builtins.attrNames value);
    in
    if unknown == [ ] then
      value
    else
      fail "${location} has unknown field(s): ${lib.concatStringsSep ", " unknown}.";
  checkEntry =
    kind: index: entry:
    let
      location = "${kind}[${toString index}]";
      generic = kind == "agents";
      allowed = [
        "name"
        "model"
        "provider"
        "base_url"
        "envFile"
      ]
      ++ lib.optionals (kind == "zeroclaw") [
        "hostPort"
        "servePort"
      ];
      checked =
        if generic then
          checkFields location (
            [ "adapter" ] ++ builtins.attrNames (builtins.functionArgs registry.${entry.adapter}.build)
          ) entry
        else
          checkFields location allowed entry;
      required =
        if generic then
          [
            "name"
            "adapter"
          ]
        else
          [
            "name"
            "model"
            "provider"
            "envFile"
          ]
          ++ lib.optionals (kind == "zeroclaw") [
            "hostPort"
            "servePort"
          ];
      missing = lib.filter (field: !(builtins.hasAttr field entry)) required;
      adapter = if generic then entry.adapter else kind;
    in
    if !builtins.isAttrs entry then
      fail "${location} must be an object."
    else if missing != [ ] then
      fail "${location} is missing: ${lib.concatStringsSep ", " missing}."
    else if !builtins.isString entry.name || builtins.match "[a-z0-9][a-z0-9-]*" entry.name == null then
      fail "${location}.name must contain lowercase ASCII letters, digits, and hyphens."
    else if !builtins.isString adapter || !(builtins.hasAttr adapter registry) then
      fail "${location} has unknown adapter; select ${lib.concatStringsSep ", " (builtins.attrNames registry)}."
    else if containsSensitiveValue entry then
      fail "${location} contains a secret-like field; use operator-provisioned runtime credential paths."
    else if entry ? autoStart && !builtins.isBool entry.autoStart then
      fail "${location}.autoStart must be a boolean."
    else if !generic && (!builtins.isString entry.model || !builtins.isString entry.provider) then
      fail "${location}.model and .provider must be strings."
    else if !generic && entry.envFile != null && !builtins.isString entry.envFile then
      fail "${location}.envFile must be a path string or null."
    else
      checked;
  entries =
    kind:
    let
      value = checkedData.${kind} or [ ];
    in
    if !builtins.isList value then
      fail "${kind} must be an array."
    else
      lib.imap0 (checkEntry kind) value;
  hermes = entries "hermes";
  zeroclaw = entries "zeroclaw";
  agents = entries "agents";
  identities =
    map (e: "hermes-${e.name}") hermes
    ++ map (e: "zeroclaw-${e.name}") zeroclaw
    ++ map (e: "${e.adapter}-${e.name}") agents;
  validate =
    if (checkedData.schemaVersion or 1) != 1 then
      fail "unsupported schemaVersion; expected 1."
    else if checkedData ? agents && !(checkedData ? schemaVersion) then
      fail "generic agents requires schemaVersion = 1."
    else if agents != [ ] && mkAgent == null then
      fail "generic agents requires the mkAgent helper."
    else if builtins.length identities != builtins.length (lib.unique identities) then
      fail "duplicate adapter instance/container identity: ${lib.concatStringsSep ", " identities}."
    else
      true;

  hermesModule =
    e:
    mkHermesAgent {
      inherit (e) name;
      inherit (e) envFile;
      settings.model = {
        default = e.model;
        inherit (e) provider;
      }
      // lib.optionalAttrs ((e.base_url or null) != null) { inherit (e) base_url; };
    };

  zeroclawModule =
    e:
    mkZeroClawAgent {
      inherit (e) name;
      agenixFile = e.envFile;
      inherit (e) hostPort;
      inherit (e) servePort;
      settings = {
        schema_version = 3;
        providers.models.${e.provider}.default = {
          inherit (e) model;
        }
        // lib.optionalAttrs ((e.base_url or null) != null) { uri = e.base_url; };
        runtime_profiles.default = {
          agentic = true;
          max_tool_iterations = 25;
        };
        agents.main = {
          model_provider = "${e.provider}.default";
          risk_profile = "default";
          runtime_profile = "default";
        };
        risk_profiles.default.level = "supervised";
      };
    };
in
assert validate;
map hermesModule hermes ++ map zeroclawModule zeroclaw ++ map mkAgent agents
