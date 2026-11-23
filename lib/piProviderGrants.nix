{ lib }:
let
  revision = grant: builtins.hashString "sha256" (builtins.toJSON grant);
  protocols = {
    openai-responses = {
      provider = "openai";
      route = "/v1/responses";
    };
    anthropic-messages = {
      provider = "anthropic";
      route = "/v1/messages";
    };
    opencode-go-chat-completions = {
      provider = "opencode-go";
      route = "/v1/chat/completions";
    };
  };
  modelType = lib.types.submodule {
    options = {
      name = lib.mkOption {
        type = lib.types.str;
        description = "Exact upstream model identifier allowed for this agent.";
      };
      inputMicrousdPerMillion = lib.mkOption {
        type = lib.types.ints.unsigned;
        default = 0;
        description = "Input price in micro-USD per million tokens.";
      };
      outputMicrousdPerMillion = lib.mkOption {
        type = lib.types.ints.unsigned;
        default = 0;
        description = "Output price in micro-USD per million tokens.";
      };
    };
  };

  providerGrantType = lib.types.submodule {
    options = {
      version = lib.mkOption {
        type = lib.types.enum [ 1 ];
        default = 1;
      };
      provider = lib.mkOption {
        type = lib.types.enum [
          "openai"
          "anthropic"
          "opencode-go"
        ];
      };
      protocol = lib.mkOption { type = lib.types.enum (lib.attrNames protocols); };
      upstreamEndpoint = lib.mkOption {
        type = lib.types.str;
        description = "Exact HTTPS generation URL used only by the host broker.";
      };
      providerCredentialFile = lib.mkOption {
        type = lib.types.str;
        description = "Runtime-only provider credential below /run.";
      };
      model = lib.mkOption { type = modelType; };
      pricingRevision = lib.mkOption { type = lib.types.strMatching "^[a-zA-Z0-9_.:-]{1,128}$"; };
      currency = lib.mkOption {
        type = lib.types.enum [ "USD" ];
        default = "USD";
      };
      tools = lib.mkOption {
        type = lib.types.listOf (lib.types.strMatching "^[a-zA-Z0-9_.:-]{1,128}$");
      };
      maxOutputTokens = lib.mkOption { type = lib.types.ints.positive; };
    };
  };

  modelPolicy = model: {
    inherit (model) name;
    input_microusd_per_million = model.inputMicrousdPerMillion;
    output_microusd_per_million = model.outputMicrousdPerMillion;
  };
  credential = grant: "provider-${revision grant}";
in
{
  inherit
    revision
    protocols
    modelPolicy
    modelType
    providerGrantType
    ;
  validCredential =
    path:
    lib.match "^/run/[A-Za-z0-9._+/-]+$" path != null
    && !(lib.any (
      piece:
      lib.elem piece [
        "."
        ".."
        ""
      ]
    ) (lib.drop 1 (lib.splitString "/" path)));
  loadCredential = grant: "${credential grant}:${grant.providerCredentialFile}";
  broker = grant: {
    inherit (grant)
      version
      provider
      protocol
      currency
      tools
      ;
    revision = revision grant;
    model = modelPolicy grant.model;
    upstream_endpoint = grant.upstreamEndpoint;
    provider_credential_file = "$CREDENTIALS_DIRECTORY/${credential grant}";
    pricing_revision = grant.pricingRevision;
    max_output_tokens = grant.maxOutputTokens;
  };
  runner = base: grant: {
    inherit (grant)
      version
      provider
      protocol
      tools
      ;
    revision = revision grant;
    brokerBinding = true;
    model = grant.model.name;
    endpoint = base + protocols.${grant.protocol}.route;
    pricing = {
      inherit (grant) currency;
      revision = grant.pricingRevision;
      inherit (modelPolicy grant.model) input_microusd_per_million output_microusd_per_million;
    };
    budget.max_output_tokens = grant.maxOutputTokens;
  };
}
