# Load instances once from JSON; attach separate operator-reviewed host policy.
# The operator supplies VPN configuration/readiness and runtime credentials.
# Select a runnable adapter with model/Research hooks and matching broker settings.
{
  agentsFromData,
  agentsFile,
  agentName ? "coding",
  # Must match the selected JSON instance's native model configuration.
  model,
  inputMicrousdPerMillion,
  outputMicrousdPerMillion,
  providerCredentialFile,
  upstreamBaseUrl,
  vpnInterface,
  researchPolicy,
}:
let
  agentModules = agentsFromData { file = agentsFile; };
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
          message = "The secure example requires a runnable JSON instance named ${agentName}, not a refusal scaffold.";
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
  imports = agentModules ++ [ hostPolicy ];
}
