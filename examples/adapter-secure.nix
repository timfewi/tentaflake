# Import with the exported mkAgent helper and operator-reviewed non-secret policy.
# The operator supplies VPN configuration/readiness and runtime credentials.
# This example uses the compatible Hermes runtime. OpenClaw cannot run yet.
{
  mkAgent,
  model,
  inputMicrousdPerMillion,
  outputMicrousdPerMillion,
  providerCredentialFile,
  upstreamBaseUrl,
  vpnInterface,
  researchPolicy,
}:
{ lib, ... }:
{
  imports = [
    (mkAgent {
      adapter = "hermes";
      name = "assistant";
      autoStart = true;
      # Only the host-generated virtual broker key enters the container.
      settings.model = {
        default = model;
        provider = "custom";
        base_url = "http://10.203.20.1:7810/v1";
      };
    })
  ];
  tentaflake = {
    security.profile = "balanced";
    broker.agents.hermes-assistant = {
      enable = true;
      networkName = "tf-hermes-assistant";
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
    worker.agents.hermes-assistant = {
      enable = true;
      workspace = "/var/lib/hermes-assistant/workspace";
      containerUid = 10000;
      containerGid = 10000;
    };
    workspaceQuota.agents.hermes-assistant = {
      enable = true;
      workspace = "/var/lib/hermes-assistant/workspace";
      sizeMiB = 8192;
      ownerUid = 10000;
      ownerGid = 10000;
    };
    research.agents.hermes-assistant.uid = 62101;
  };
  services.secureResearch = researchPolicy // {
    enable = true;
    inherit vpnInterface;
    serviceUid = 4201;
    egressUid = 4202;
    summarizeOrder = lib.mkForce [ ];
  };
}
