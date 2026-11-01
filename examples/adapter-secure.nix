# Load instances once from TOML; attach separate operator-reviewed host policy.
# systemFile must name the consumer/fork's tracked tentaflake.toml, never the
# public input's empty template. A missing source is an error, not a fallback.
# The operator supplies VPN configuration/readiness and runtime credentials.
# Select a runnable adapter with model/Research hooks and matching broker settings.
{
  systemFromData,
  systemFile,
  agentName ? "coding",
  # Must match the selected TOML instance's native model configuration.
  model,
  inputMicrousdPerMillion,
  outputMicrousdPerMillion,
  providerCredentialFile,
  upstreamBaseUrl,
  vpnInterface,
  researchPolicy,
}:
let
  systemModule = systemFromData { file = systemFile; };
  hostPolicy =
    { config, lib, ... }:
    let
      agentId = "agent-${agentName}";
      instance = config.tentaflake.agentInstances.${agentId};
    in
    {
      assertions = [
        {
          assertion = instance.runnable;
          message = "The secure example requires a runnable TOML instance named ${agentName}, not a refusal scaffold.";
        }
      ];
      tentaflake = {
        security.profile = "balanced";
        broker.agents.${agentId} = {
          enable = true;
          networkName = "tf-${agentId}";
          subnet = "10.203.20.0/30";
          gateway = "10.203.20.1";
          llm = {
            enable = true;
            inherit providerCredentialFile upstreamBaseUrl;
            port = 7810;
            maxCompletionTokens = 4096;
            allowedModels = [
              {
                name = model;
                inherit inputMicrousdPerMillion outputMicrousdPerMillion;
              }
            ];
          };
          fetch.enable = false;
        };
        worker.agents.${agentId} = {
          enable = true;
          inherit (instance) workspace;
          containerUid = instance.uid;
          containerGid = instance.gid;
        };
        workspaceQuota.agents.${agentId} = {
          enable = true;
          inherit (instance) workspace;
          sizeMiB = 8192;
          ownerUid = instance.uid;
          ownerGid = instance.gid;
        };
        research.agents.${agentId}.uid = 62101;
      };
      services.secureResearch = researchPolicy // {
        enable = true;
        inherit vpnInterface;
        serviceUid = 4201;
        egressUid = 4202;
        summarizeOrder = lib.mkForce [ ];
      };
    };
in
{
  imports = [
    systemModule
    hostPolicy
  ];
}
