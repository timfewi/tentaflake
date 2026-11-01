# Pure validation shared by the public compiler and the cross-language corpus.
{ lib }:
data:
let
  schema = builtins.fromJSON (builtins.readFile ../schema/system-v1.json);
  catalog = builtins.fromJSON (builtins.readFile ../adapters/catalog.json);
  fail = message: throw "tentaflake desired system: ${message}";
  matches = pattern: value: builtins.isString value && builtins.match pattern value != null;
  statePath =
    spec: value:
    matches "/var/lib/[a-zA-Z0-9_-]+(/[a-zA-Z0-9_-]+)*" value
    && lib.all (root: value != root && !(lib.hasPrefix "${root}/" value)) spec.excludedRoots;
  canonicalBackupPath =
    value:
    let
      suffix =
        if lib.hasPrefix "/var/lib/tentaflake/agents/" value then
          lib.removeSuffix "/workspace" (lib.removePrefix "/var/lib/tentaflake/agents/" value)
        else if lib.hasPrefix "/var/lib/tentaflake-broker-llm-agent-" value then
          lib.removePrefix "/var/lib/tentaflake-broker-llm-agent-" value
        else
          "";
    in
    matches "[a-z0-9][a-z0-9-]*" suffix && builtins.stringLength suffix <= 57;
  typed =
    spec: value:
    if spec.type == "bool" then
      builtins.isBool value
    else if spec.type == "disabled" then
      value == false
    else if spec.type == "enum" then
      builtins.elem value spec.values
    else if spec.type == "name" then
      matches "[a-z0-9][a-z0-9-]*" value && builtins.stringLength value <= 57
    else if spec.type == "hostName" then
      matches "[a-z0-9]([a-z0-9-]*[a-z0-9])?" value && builtins.stringLength value <= 63
    else if spec.type == "uint" then
      builtins.isInt value && value >= 0
    else if spec.type == "positive" then
      builtins.isInt value && value > 0
    else if spec.type == "owner" then
      builtins.isInt value && value >= 1 && value <= 4294967294
    else if spec.type == "agentId" then
      matches "agent-[a-z0-9][a-z0-9-]*" value && builtins.stringLength value <= 63
    else if spec.type == "exactId" then
      matches "[A-Za-z0-9_-][A-Za-z0-9_.-]*" value && builtins.stringLength value <= 64
    else if spec.type == "runtimePath" then
      matches "/run/[A-Za-z0-9_-][A-Za-z0-9_.-]*(/[A-Za-z0-9_-][A-Za-z0-9_.-]*)*" value
      && builtins.stringLength value <= 256
    else if spec.type == "backupPath" then
      statePath spec value && builtins.stringLength value <= 256
    else if spec.type == "object" then
      builtins.isAttrs value
      && lib.all (key: builtins.hasAttr key value) (spec.required or [ ])
      && builtins.deepSeq (object "nested" spec.fields value) true
    else if spec.type == "list" then
      builtins.isList value
      && builtins.length value <= spec.max
      && lib.all (typed spec.items) value
      && (!(spec.unique or false) || builtins.length value == builtins.length (lib.unique value))
      && (
        !(spec ? uniqueBy)
        || (
          let
            keys = map (item: item.${spec.uniqueBy}) value;
          in
          builtins.length keys == builtins.length (lib.unique keys)
        )
      )
    else if spec.type == "map" then
      builtins.isAttrs value
      && builtins.length (builtins.attrNames value) <= spec.max
      && lib.all (typed spec.keys) (builtins.attrNames value)
      && lib.all (typed spec.items) (builtins.attrValues value)
    else if spec.type == "statePath" then
      statePath spec value
    else if spec.type == "runtimeDefinition" then
      builtins.deepSeq ((import ./runtimeContract.nix { inherit lib; }) value) true
    else if spec.type == "token" then
      matches "[a-zA-Z0-9_./+-]+" value
      && builtins.stringLength value <= 128
      && !(builtins.elem ".." (lib.splitString "/" value))
    else
      false;
  object =
    location: fields: value:
    if !builtins.isAttrs value then
      fail "${location} must be an object."
    else
      lib.mapAttrs (
        field: item:
        if !(builtins.hasAttr field fields) then
          fail "${location}.${field} is unknown or unsafe."
        else if !(typed fields.${field} item) then
          fail "${location}.${field} has an invalid or unsupported value."
        else
          item
      ) value;
  agent =
    entry:
    let
      validated = object "agent" schema.agent entry;
      # Force unknown-field/type refusals before indexing descriptor metadata.
      # attrNames alone does not evaluate the lazy validation values.
      checked = builtins.deepSeq validated validated;
      unsupported = lib.filter (
        field:
        schema.agent.${field} ? adapters && !(builtins.elem checked.adapter schema.agent.${field}.adapters)
      ) (builtins.attrNames checked);
    in
    if !(checked ? name) || !(checked ? adapter) then
      fail "agents require name and adapter."
    else if unsupported != [ ] then
      fail "unsupported adapter-specific agent fields."
    else if checked.adapter == "generic" && !(checked ? definition) then
      fail "generic agents require a runtime definition."
    else if
      builtins.elem checked.adapter (schema.stoppedAdapters or [ ]) && (checked.autoStart or false)
    then
      fail "stopped adapters cannot autoStart."
    else
      checked;
  checkedAgents = map agent data.agents;
  names = map (entry: entry.name) checkedAgents;
  sections = lib.mapAttrs (
    section: value:
    if
      builtins.elem section [
        "schemaVersion"
        "agents"
      ]
    then
      value
    else if section == "_securityNote" then
      if builtins.isString value then value else fail "_securityNote must be a string."
    else if !(builtins.hasAttr section schema.sections) then
      fail "unknown section ${section}."
    else
      object section schema.sections.${section} value
  ) data;
  result = sections // {
    agents = checkedAgents;
  };
  host = sections.hostApi or { };
  connectors = sections.connectors or { };
  mapping = sections.workloadMapping or { };
  storage = sections.storage or { };
  research = sections.research or { };
  researchAgents = research.agents or { };
  relayUids = map (client: client.uid) (builtins.attrValues researchAgents);
  researchValid =
    id:
    lib.any (
      agent:
      "agent-${agent.name}" == id
      && !(builtins.elem agent.adapter schema.stoppedAdapters)
      && (catalog.presets.${agent.adapter}.research.supported or false)
    ) checkedAgents;
  enabled = section: section.enable or false;
  inactive = section: enabled section || lib.all (key: key == "enable") (builtins.attrNames section);
  declared = id: builtins.elem id (map (name: "agent-${name}") names);
  permissionGates = {
    actions = "allowActions";
    logs = "allowLogs";
    tool_requests = "allowToolRequests";
    tool_decisions = "allowToolDecisions";
  };
  peerValid =
    peer:
    lib.all declared peer.agents
    && (peer.agents != [ ] || peer.permissions == [ "host_health" ] || peer.permissions == [ ])
    && lib.all (
      permission:
      !(builtins.hasAttr permission permissionGates) || host.${permissionGates.${permission}} or false
    ) peer.permissions
    && (
      !(builtins.elem "tool_decisions" peer.permissions) || builtins.elem "tool_requests" peer.permissions
    )
    && !(lib.any (p: builtins.elem p peer.permissions) [
      "run_read"
      "run_submit"
      "run_stop"
    ]);
  bindings = mapping.bindings or { };
  scopes = lib.mapAttrsToList (_: binding: [
    binding.tenant_id
    binding.workload_id
  ]) bindings;
  bindingValid =
    id:
    declared id
    && lib.any (
      agent: "agent-${agent.name}" == id && !(builtins.elem agent.adapter schema.stoppedAdapters)
    ) checkedAgents;
  servicesValid =
    inactive host
    && inactive connectors
    && inactive mapping
    && inactive storage
    && inactive research
    && (
      !(enabled research)
      || (
        researchAgents != { }
        && (sections.sandbox.profile or "balanced") != "dev"
        && lib.all researchValid (builtins.attrNames researchAgents)
        && builtins.length relayUids == builtins.length (lib.unique relayUids)
      )
    )
    && (
      !(enabled storage)
      || (
        storage ? paths
        && storage.paths != [ ]
        && storage ? repositoryFile
        && storage ? passwordFile
        && lib.all (
          path: canonicalBackupPath path || lib.any (agent: (agent.stateDir or null) == path) checkedAgents
        ) storage.paths
      )
    )
    && (!(enabled host) || host ? peers)
    && (!(host.allowToolDecisions or false) || (host.allowToolRequests or false))
    && lib.all peerValid (host.peers or [ ])
    && (!(enabled connectors) || connectors ? configFile)
    && (
      !(enabled mapping)
      || (
        (sections.container.backend or "docker") == "docker"
        && mapping ? bindings
        && lib.all bindingValid (builtins.attrNames bindings)
        && builtins.length scopes == builtins.length (lib.unique scopes)
      )
    );
in
if !builtins.isAttrs data then
  fail "root must be an object."
else if
  !builtins.isInt (data.schemaVersion or null) || data.schemaVersion != schema.schemaVersion
then
  fail "schemaVersion must be integer 1."
else if !(data ? agents) || !builtins.isList data.agents then
  fail "agents array is required."
else if builtins.length data.agents > 128 then
  fail "at most 128 agents are supported."
else if builtins.length names != builtins.length (lib.unique names) then
  fail "duplicate global agent identity."
else if !servicesValid then
  fail "invalid service grants, dependencies, runtime paths or workload scope."
else
  builtins.deepSeq result result
