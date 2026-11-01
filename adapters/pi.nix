# Default admission is refused. The real thin runner uses submission-wide
# quarantine around the pinned SDK, never its unsafe default close/resume path.
{ pkgs, lib, ... }:
let
  admitted =
    config: args:
    let
      policy = {
        enable = false;
        package = null;
        bundle = null;
        image = null;
      }
      // (lib.attrByPath [ "tentaflake" "piRuntime" ] { } config);
      container = "agent-${args.name}";
      root = args.stateDir or "/var/lib/tentaflake/agents/${args.name}";
      broker = lib.attrByPath [ "tentaflake" "broker" "agents" container ] null config;
      worker = lib.attrByPath [ "tentaflake" "worker" "agents" container ] null config;
      quota = lib.attrByPath [ "tentaflake" "workspaceQuota" "agents" container ] null config;
    in
    policy.enable
    && lib.isDerivation policy.package
    && lib.isDerivation policy.bundle
    && (import ../lib/piTransport.nix { inherit lib pkgs; }).configurationSupported config
    && (policy.package.upstreamRevision or null) == "35dfa757378bd7a8570670e99c2687bdd1039a06"
    && (policy.bundle.runtimePackage or null) == policy.package
    && (policy.bundle.imageVerificationVersion or null) == 1
    && policy.image != null
    && (policy.bundle.imageSelector or null) == policy.image
    && lib.attrByPath [ "tentaflake" "security" "profile" ] null config == "balanced"
    && broker != null
    && broker.enable
    && broker.llm.enable
    && !broker.fetch.enable
    && broker.llm.streaming.enable
    && !broker.llm.strictCostEnvelope
    && lib.length broker.llm.allowedModels == 1
    && worker != null
    && worker.enable
    && worker.workspace == "${root}/workspace"
    && worker.containerUid == 10100
    && worker.containerGid == 10100
    && quota != null
    && quota.enable
    && quota.workspace == "${root}/workspace"
    && quota.state != null
    && quota.state.path == "${root}/state"
    && lib.hasAttrByPath [ "tentaflake" "research" "agents" container ] config;
  scaffold = import ../lib/stoppedScaffold.nix { inherit lib; } {
    preset = {
      schemaVersion = 1;
      identity = {
        id = "pi";
        version = "1.4.0";
        status = "scaffold";
      };
      artifact = {
        kind = "unselected";
        reference = null;
        reviewed = false;
      };
      command = [ ];
      configuration = {
        format = "none";
        readOnly = true;
      };
      ownership = {
        uid = 10100;
        gid = 10100;
      };
      layout = {
        state = "/var/lib/tentaflake/agents/<name>";
        workspace = "workspace";
        writable = [ ];
        directories = [ ];
      };
      lifecycle = "stopped-scaffold";
      model = {
        protocols = [ ];
        streaming = "unknown";
      };
      research = {
        supported = true;
      };
      execution = {
        interface = "unverified";
        automatic = false;
      };
      capabilities = [ ];
      evidence = {
        fixture = "refusal-regression";
        vendorAcceptance = "pending";
      };
    };
    id = "pi";
    displayName = "Pi";
    reason = "Pi is blocked without an explicitly admitted pinned runner image bundle and exact broker/Research/worker/quota policy. Upstream close/resume can redispatch an uncertain model request; submission-wide quarantine and lifecycle acceptance evidence are required. See docs/agent-adapters.md.";
    settingsReason = "Pi settings are unavailable until runtime acceptance.";
  };
in
scaffold
// {
  inherit admitted;
  metadata =
    args: backend:
    (scaffold.metadata args backend)
    // {
      stateStorage = "${args.stateDir or "/var/lib/tentaflake/agents/${args.name}"}/state";
    };
  research = scaffold.preset.research // {
    configure =
      {
        server ? null,
        ...
      }:
      {
        settings = { };
        valid =
          server != null
          && server ? args
          && server ? command
          &&
            server.args == [
              "--socket"
              "/run/tentaflake-research/socket"
            ]
          && lib.hasPrefix "/nix/store/" server.command
          && lib.hasSuffix "/bin/research-client" server.command;
      };
  };
  build =
    {
      name,
      autoStart ? false,
      stateDir ? "/var/lib/tentaflake/agents/${name}",
      settings ? { },
    }@args:
    { config, ... }:
    if settings != { } then
      throw "tentaflake: Pi has only fixed generated runtime configuration."
    else if
      !builtins.isString stateDir
      || builtins.match "/var/lib/[a-zA-Z0-9_-]+(/[a-zA-Z0-9_-]+)*" stateDir == null
    then
      throw "tentaflake: Pi requires a normalized state directory below /var/lib."
    else
      let
        enabled = lib.attrByPath [ "tentaflake" "piRuntime" "enable" ] false config;
      in
      {
        # Module structure must not depend on config: it is collected before
        # config exists. Only option definitions may be conditionally enabled.
        config = lib.mkMerge [
          {
            assertions = [
              {
                assertion = !enabled || admitted config args;
                message = "tentaflake: Pi runtime admission requires package, bundle, image and exact broker/Research/worker/quota policy.";
              }
              {
                assertion = enabled || !autoStart;
                message = "tentaflake: unadmitted Pi must keep autoStart = false.";
              }
            ];
          }
          (lib.mkIf enabled (
            (import ./pi/runtime.nix { inherit pkgs lib; } {
              inherit name stateDir autoStart;
            })
              { inherit config; }
          ))
          (lib.mkIf (!enabled) ((scaffold.build (args // { autoStart = false; })) { inherit config; }))
        ];
      };
}
