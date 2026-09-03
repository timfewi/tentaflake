{
  config,
  lib,
  pkgs,
  ...
}:
let
  cfg = config.tentaflake.broker;
  brokerPackage = cfg.package;
  backend = config.virtualisation.oci-containers.backend;
  runtime = lib.getExe pkgs.${backend};
  json = pkgs.formats.json { };
  enabledAgents = lib.filterAttrs (_: agent: agent.enable) cfg.agents;
  containerNames = lib.attrNames config.virtualisation.oci-containers.containers;
  bridgeName = name: "tfb-${builtins.substring 0 8 (builtins.hashString "sha256" name)}";

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

  agentType = lib.types.submodule (
    { name, ... }:
    {
      options = {
        enable = lib.mkEnableOption "isolated brokers for OCI container ${name}";
        networkName = lib.mkOption {
          type = lib.types.str;
          default = "tf-${name}";
          description = "Dedicated internal OCI network name.";
        };
        subnet = lib.mkOption {
          type = lib.types.str;
          example = "10.203.10.0/30";
          description = "Unique IPv4 /30 subnet for this agent and its host gateway.";
        };
        gateway = lib.mkOption {
          type = lib.types.str;
          example = "10.203.10.1";
          description = "Host broker gateway address inside the dedicated subnet.";
        };
        maxRequestBytes = lib.mkOption {
          type = lib.types.ints.positive;
          default = 1024 * 1024;
        };
        maxResponseBytes = lib.mkOption {
          type = lib.types.ints.positive;
          default = 8 * 1024 * 1024;
        };
        maxConcurrency = lib.mkOption {
          type = lib.types.ints.positive;
          default = 4;
        };
        maxRequestsPerMinute = lib.mkOption {
          type = lib.types.ints.positive;
          default = 30;
        };
        dailyRequestBudget = lib.mkOption {
          type = lib.types.ints.positive;
          default = 1000;
        };
        dailyTokenBudget = lib.mkOption {
          type = lib.types.ints.positive;
          default = 1000000;
        };
        dailyCostMicrousd = lib.mkOption {
          type = lib.types.ints.positive;
          default = 10000000;
        };
        llm = {
          enable = lib.mkEnableOption "per-agent LLM credential broker";
          port = lib.mkOption {
            type = lib.types.port;
            default = 7810;
          };
          upstreamBaseUrl = lib.mkOption {
            type = lib.types.str;
            default = "";
            example = "https://api.openai.com/v1/";
          };
          providerCredentialFile = lib.mkOption {
            type = lib.types.str;
            default = "";
            example = "/run/agenix/openai-api-key";
            description = "Runtime-only host credential loaded by systemd; never mounted into the agent.";
          };
          allowedModels = lib.mkOption {
            type = lib.types.listOf modelType;
            default = [ ];
          };
          maxCompletionTokens = lib.mkOption {
            type = lib.types.ints.positive;
            default = 4096;
          };
        };
        fetch = {
          enable = lib.mkEnableOption "per-agent SSRF-safe fetch gateway";
          port = lib.mkOption {
            type = lib.types.port;
            default = 7811;
          };
          allowedHosts = lib.mkOption {
            type = lib.types.listOf lib.types.str;
            default = [ ];
            description = "Exact lowercase hostnames; wildcards and CIDRs are not accepted.";
          };
          allowedContentTypes = lib.mkOption {
            type = lib.types.listOf lib.types.str;
            default = [
              "application/json"
              "application/xml"
              "text/html"
              "text/plain"
              "text/xml"
            ];
          };
          maxRedirects = lib.mkOption {
            type = lib.types.ints.between 0 10;
            default = 3;
          };
        };
      };
    }
  );

  commonConfig = name: agent: mode: {
    agent = name;
    listen = "${agent.gateway}:${toString agent.${mode}.port}";
    token_file = "$CREDENTIALS_DIRECTORY/agent-token";
    audit_file = "/var/lib/tentaflake-broker-${mode}-${name}/audit.jsonl";
    budget_state_file = "/var/lib/tentaflake-broker-${mode}-${name}/budget.json";
    max_request_bytes = agent.maxRequestBytes;
    max_response_bytes = agent.maxResponseBytes;
    max_concurrency = agent.maxConcurrency;
    rate_window_seconds = 60;
    max_requests_per_window = agent.maxRequestsPerMinute;
    daily_request_budget = agent.dailyRequestBudget;
    daily_token_budget = agent.dailyTokenBudget;
    daily_cost_microusd = agent.dailyCostMicrousd;
  };

  llmConfig =
    name: agent:
    json.generate "tentaflake-broker-llm-${name}.json" (
      commonConfig name agent "llm"
      // {
        llm = {
          upstream_base_url = agent.llm.upstreamBaseUrl;
          provider_credential_file = "$CREDENTIALS_DIRECTORY/provider";
          max_completion_tokens = agent.llm.maxCompletionTokens;
          allowed_models = map (model: {
            inherit (model) name;
            input_microusd_per_million = model.inputMicrousdPerMillion;
            output_microusd_per_million = model.outputMicrousdPerMillion;
          }) agent.llm.allowedModels;
        };
      }
    );

  fetchConfig =
    name: agent:
    json.generate "tentaflake-broker-fetch-${name}.json" (
      commonConfig name agent "fetch"
      // {
        fetch = {
          allowed_hosts = agent.fetch.allowedHosts;
          allowed_content_types = agent.fetch.allowedContentTypes;
          quarantine_dir = "/var/lib/tentaflake-broker-fetch-${name}/quarantine";
          max_redirects = agent.fetch.maxRedirects;
        };
      }
    );

  credentialsService = name: agent: {
    "tentaflake-broker-credentials-${name}" = {
      description = "Generate scoped virtual broker credential for ${name}";
      before = [
        "tentaflake-broker-llm-${name}.service"
        "tentaflake-broker-fetch-${name}.service"
        "${backend}-${name}.service"
      ];
      serviceConfig = {
        Type = "oneshot";
        RemainAfterExit = true;
        UMask = "0077";
        NoNewPrivileges = true;
        PrivateDevices = true;
        PrivateTmp = true;
        ProtectClock = true;
        ProtectControlGroups = true;
        ProtectHome = true;
        ProtectHostname = true;
        ProtectKernelLogs = true;
        ProtectKernelModules = true;
        ProtectKernelTunables = true;
        ProtectSystem = "strict";
        RestrictAddressFamilies = [ "AF_UNIX" ];
        RestrictNamespaces = true;
        RestrictRealtime = true;
        RestrictSUIDSGID = true;
        LockPersonality = true;
        SystemCallArchitectures = "native";
        CapabilityBoundingSet = [ ];
      };
      script = ''
        runtime_dir=/run/tentaflake-broker/${lib.escapeShellArg name}
        token_file="$runtime_dir/agent-token"
        env_file="$runtime_dir/agent.env"
        ${pkgs.coreutils}/bin/install -d -m 0700 "$runtime_dir"
        if [ ! -s "$token_file" ]; then
          ${pkgs.openssl}/bin/openssl rand -hex 32 > "$token_file"
        fi
        token=$(${pkgs.coreutils}/bin/head -n 1 "$token_file")
        env_tmp="$env_file.tmp"
        {
          printf 'TENTAFLAKE_BROKER_TOKEN=%s\n' "$token"
          ${lib.optionalString agent.llm.enable ''
            printf 'OPENAI_API_KEY=%s\n' "$token"
            printf 'OPENAI_BASE_URL=http://%s:%s/v1\n' \
              ${lib.escapeShellArg agent.gateway} \
              ${lib.escapeShellArg (toString agent.llm.port)}
            printf 'TENTAFLAKE_LLM_BASE_URL=http://%s:%s/v1\n' \
              ${lib.escapeShellArg agent.gateway} \
              ${lib.escapeShellArg (toString agent.llm.port)}
          ''}
          ${lib.optionalString agent.fetch.enable ''
            printf 'TENTAFLAKE_FETCH_URL=http://%s:%s/v1/fetch\n' \
              ${lib.escapeShellArg agent.gateway} \
              ${lib.escapeShellArg (toString agent.fetch.port)}
          ''}
        } > "$env_tmp"
        ${pkgs.coreutils}/bin/chmod 0400 "$token_file" "$env_tmp"
        ${pkgs.coreutils}/bin/mv -f "$env_tmp" "$env_file"
      '';
    };
  };

  networkService = name: agent: {
    "tentaflake-broker-network-${name}" = {
      description = "Create isolated internal OCI network for ${name}";
      after = lib.optional (backend == "docker") "docker.service";
      requires = lib.optional (backend == "docker") "docker.service";
      before = [
        "tentaflake-broker-llm-${name}.service"
        "tentaflake-broker-fetch-${name}.service"
        "${backend}-${name}.service"
      ];
      path = [ pkgs.jq ];
      serviceConfig = {
        Type = "oneshot";
        RemainAfterExit = true;
      };
      script = ''
        bridge=${lib.escapeShellArg (bridgeName name)}
        if ${runtime} network inspect ${lib.escapeShellArg agent.networkName} \
          > /run/tentaflake-network-${lib.escapeShellArg name}.json 2>/dev/null; then
          ${pkgs.jq}/bin/jq -e \
            --arg subnet ${lib.escapeShellArg agent.subnet} \
            --arg gateway ${lib.escapeShellArg agent.gateway} \
            --arg interface "$bridge" \
            '${
              if backend == "docker" then
                ".[0] | .Internal == true and .Options[\"com.docker.network.bridge.name\"] == $interface and any(.IPAM.Config[]; .Subnet == $subnet and .Gateway == $gateway)"
              else
                ".[0] | .internal == true and .network_interface == $interface and any(.subnets[]; .subnet == $subnet and .gateway == $gateway)"
            }' \
            /run/tentaflake-network-${lib.escapeShellArg name}.json >/dev/null
        else
          ${runtime} network create --internal \
            ${
              if backend == "docker" then
                "--opt com.docker.network.bridge.name=\"$bridge\""
              else
                "--interface-name \"$bridge\""
            } \
            --subnet ${lib.escapeShellArg agent.subnet} \
            --gateway ${lib.escapeShellArg agent.gateway} \
            ${lib.escapeShellArg agent.networkName}
        fi
      '';
    };
  };

  hardenedService = name: mode: configFile: loadCredential: {
    description = "Tentaflake ${mode} policy broker for ${name}";
    requires = [
      "tentaflake-broker-credentials-${name}.service"
      "tentaflake-broker-network-${name}.service"
    ];
    after = [
      "tentaflake-broker-credentials-${name}.service"
      "tentaflake-broker-network-${name}.service"
      "network-online.target"
    ];
    wants = [ "network-online.target" ];
    serviceConfig = {
      Type = "simple";
      DynamicUser = true;
      StateDirectory = "tentaflake-broker-${mode}-${name}";
      StateDirectoryMode = "0700";
      LoadCredential = loadCredential;
      ExecStart = "${lib.getExe brokerPackage} --config ${configFile}";
      Restart = "on-failure";
      RestartSec = "5s";
      UMask = "0077";
      MemoryMax = cfg.serviceMemoryMaxBytes;
      TasksMax = cfg.serviceTasksMax;
      LimitNOFILE = cfg.serviceNoFileLimit;
      NoNewPrivileges = true;
      PrivateDevices = true;
      PrivateTmp = true;
      ProtectClock = true;
      ProtectControlGroups = true;
      ProtectHome = true;
      ProtectHostname = true;
      ProtectKernelLogs = true;
      ProtectKernelModules = true;
      ProtectKernelTunables = true;
      ProtectSystem = "strict";
      RestrictAddressFamilies = [
        "AF_UNIX"
        "AF_INET"
        "AF_INET6"
      ];
      RestrictRealtime = true;
      RestrictSUIDSGID = true;
      LockPersonality = true;
      CapabilityBoundingSet = [ ];
      SystemCallArchitectures = "native";
    };
    startLimitIntervalSec = 300;
    startLimitBurst = 5;
  };

  servicesFor =
    name: agent:
    credentialsService name agent
    // networkService name agent
    // lib.optionalAttrs agent.llm.enable {
      "tentaflake-broker-llm-${name}" = hardenedService name "llm" (llmConfig name agent) [
        "agent-token:/run/tentaflake-broker/${name}/agent-token"
        "provider:${agent.llm.providerCredentialFile}"
      ];
    }
    // lib.optionalAttrs agent.fetch.enable {
      "tentaflake-broker-fetch-${name}" = hardenedService name "fetch" (fetchConfig name agent) [
        "agent-token:/run/tentaflake-broker/${name}/agent-token"
      ];
    };

  allServices = lib.foldlAttrs (
    result: name: agent:
    result // servicesFor name agent
  ) { } enabledAgents;
  unique = values: lib.length values == lib.length (lib.unique values);
  sumEnabled =
    field: lib.foldl' (total: agent: total + agent.${field}) 0 (lib.attrValues enabledAgents);
  withinTotal = limit: value: limit == null || value <= limit;
  brokerPorts =
    agent:
    lib.optional agent.llm.enable agent.llm.port ++ lib.optional agent.fetch.enable agent.fetch.port;
  firewallInputRules = lib.concatStringsSep "\n" (
    lib.mapAttrsToList (name: agent: ''
      iifname "${bridgeName name}" ip saddr ${agent.subnet} tcp dport { ${
        lib.concatMapStringsSep ", " toString (brokerPorts agent)
      } } accept
      ip saddr ${agent.subnet} counter drop
    '') enabledAgents
  );
  firewallForwardRules = lib.concatStringsSep "\n" (
    lib.mapAttrsToList (_: agent: ''
      ip saddr ${agent.subnet} counter drop
    '') enabledAgents
  );
  ipv4Octets = value: map lib.toInt (lib.splitString "." value);
  validIpv4 =
    value:
    let
      parts = lib.splitString "." value;
    in
    lib.length parts == 4
    && lib.all (part: lib.match "^(0|[1-9][0-9]{0,2})$" part != null) parts
    && lib.all (part: lib.toInt part <= 255) parts;
  subnetMatchesGateway =
    subnet: gateway:
    let
      base = lib.removeSuffix "/30" subnet;
      baseParts = ipv4Octets base;
      gatewayParts = ipv4Octets gateway;
    in
    validIpv4 base
    && validIpv4 gateway
    && lib.take 3 baseParts == lib.take 3 gatewayParts
    && builtins.bitAnd (lib.last baseParts) 3 == 0
    && lib.last gatewayParts == lib.last baseParts + 1;
in
{
  options.tentaflake.broker = {
    package = lib.mkOption {
      type = lib.types.package;
      default = pkgs.callPackage ../pkgs/tentaflake-broker { };
      defaultText = lib.literalExpression "pkgs.callPackage ../pkgs/tentaflake-broker { }";
      description = "Broker package used by managed systemd services.";
    };
    maxEnabledAgents = lib.mkOption {
      type = lib.types.nullOr lib.types.ints.positive;
      default = null;
      description = "Optional host-wide ceiling for the number of enabled broker agent declarations.";
    };
    serviceMemoryMaxBytes = lib.mkOption {
      type = lib.types.ints.positive;
      default = 128 * 1024 * 1024;
      description = "Hard memory ceiling in bytes for each broker systemd service.";
    };
    serviceTasksMax = lib.mkOption {
      type = lib.types.ints.positive;
      default = 64;
      description = "Hard task ceiling for each broker systemd service.";
    };
    serviceNoFileLimit = lib.mkOption {
      type = lib.types.ints.positive;
      default = 4096;
      description = "Hard open-file-descriptor ceiling for each broker systemd service.";
    };
    maxTotalConcurrency = lib.mkOption {
      type = lib.types.nullOr lib.types.ints.positive;
      default = null;
      description = "Optional host-wide ceiling for the sum of enabled broker maxConcurrency values.";
    };
    maxTotalRequestsPerMinute = lib.mkOption {
      type = lib.types.nullOr lib.types.ints.positive;
      default = null;
      description = "Optional host-wide ceiling for the sum of enabled broker request-rate budgets.";
    };
    maxTotalDailyTokenBudget = lib.mkOption {
      type = lib.types.nullOr lib.types.ints.positive;
      default = null;
      description = "Optional host-wide ceiling for the sum of enabled broker daily token budgets.";
    };
    maxTotalDailyCostMicrousd = lib.mkOption {
      type = lib.types.nullOr lib.types.ints.positive;
      default = null;
      description = "Optional host-wide ceiling for the sum of enabled broker daily cost budgets in micro-USD.";
    };
    agents = lib.mkOption {
      type = lib.types.attrsOf agentType;
      default = { };
      description = "Per-container isolated broker and egress policy.";
    };
  };

  config = {
    assertions = [
      {
        assertion = enabledAgents == { } || config.tentaflake.security.profile != "dev";
        message = "tentaflake brokers require the balanced or strict security profile.";
      }
      {
        assertion = enabledAgents == { } || config.tentaflake.networking.enable;
        message = "tentaflake brokers require tentaflake.networking.enable so their host and forward firewall boundary is active.";
      }
      {
        assertion =
          cfg.maxEnabledAgents == null || lib.length (lib.attrNames enabledAgents) <= cfg.maxEnabledAgents;
        message = "tentaflake broker enabled-agent count exceeds maxEnabledAgents.";
      }
      {
        assertion = withinTotal cfg.maxTotalConcurrency (sumEnabled "maxConcurrency");
        message = "tentaflake broker enabled-agent concurrency budgets exceed maxTotalConcurrency.";
      }
      {
        assertion = withinTotal cfg.maxTotalRequestsPerMinute (sumEnabled "maxRequestsPerMinute");
        message = "tentaflake broker enabled-agent request-rate budgets exceed maxTotalRequestsPerMinute.";
      }
      {
        assertion = withinTotal cfg.maxTotalDailyTokenBudget (sumEnabled "dailyTokenBudget");
        message = "tentaflake broker enabled-agent daily token budgets exceed maxTotalDailyTokenBudget.";
      }
      {
        assertion = withinTotal cfg.maxTotalDailyCostMicrousd (sumEnabled "dailyCostMicrousd");
        message = "tentaflake broker enabled-agent daily cost budgets exceed maxTotalDailyCostMicrousd.";
      }
      {
        assertion = unique (lib.mapAttrsToList (_: agent: agent.networkName) enabledAgents);
        message = "tentaflake broker networkName values must be unique per agent.";
      }
      {
        assertion = unique (lib.mapAttrsToList (_: agent: agent.subnet) enabledAgents);
        message = "tentaflake broker subnets must be unique per agent.";
      }
      {
        assertion = unique (map bridgeName (lib.attrNames enabledAgents));
        message = "tentaflake broker agent names collide on their deterministic bridge interface; rename one agent.";
      }
    ]
    ++ lib.concatLists (
      lib.mapAttrsToList (name: agent: [
        {
          assertion = lib.elem name containerNames;
          message = "tentaflake broker key ${name} must exactly match an OCI agent container name.";
        }
        {
          assertion = lib.match "^[a-z0-9][a-z0-9-]{0,62}$" name != null;
          message = "tentaflake broker container keys must be safe and at most 63 characters.";
        }
        {
          assertion = agent.llm.enable || agent.fetch.enable;
          message = "tentaflake broker ${name} must enable llm, fetch, or both.";
        }
        {
          assertion = lib.match "^[a-z0-9][a-z0-9-]{0,62}$" agent.networkName != null;
          message = "tentaflake broker ${name} networkName is not a safe bounded OCI network name.";
        }
        {
          assertion =
            lib.hasPrefix "10." agent.subnet
            && lib.hasSuffix "/30" agent.subnet
            && subnetMatchesGateway agent.subnet agent.gateway;
          message = "tentaflake broker ${name} requires a valid private 10/8 IPv4 /30 subnet whose first usable address is the gateway.";
        }
        {
          assertion = validIpv4 agent.gateway && lib.hasPrefix "10." agent.gateway;
          message = "tentaflake broker ${name} requires an explicit private 10/8 gateway address.";
        }
        {
          assertion =
            !agent.llm.enable
            || (
              lib.hasPrefix "https://" agent.llm.upstreamBaseUrl
              && lib.hasSuffix "/" agent.llm.upstreamBaseUrl
              && !(lib.hasInfix "@" agent.llm.upstreamBaseUrl)
              && !(lib.hasInfix "?" agent.llm.upstreamBaseUrl)
              && !(lib.hasInfix "#" agent.llm.upstreamBaseUrl)
              && agent.llm.providerCredentialFile != ""
              && lib.match "^/run/[A-Za-z0-9._+/-]+$" agent.llm.providerCredentialFile != null
              && !(lib.elem ".." (lib.splitString "/" agent.llm.providerCredentialFile))
              && !(lib.elem "." (lib.splitString "/" agent.llm.providerCredentialFile))
              && !(lib.hasInfix "//" agent.llm.providerCredentialFile)
              && agent.llm.allowedModels != [ ]
              && unique (map (model: model.name) agent.llm.allowedModels)
              && lib.all (
                model:
                lib.match "^[A-Za-z0-9][A-Za-z0-9._:/+-]{0,199}$" model.name != null
                && model.inputMicrousdPerMillion > 0
                && model.outputMicrousdPerMillion > 0
              ) agent.llm.allowedModels
            );
          message = "tentaflake LLM broker ${name} requires HTTPS with trailing slash, a runtime credential below /run, and exact priced models.";
        }
        {
          assertion =
            !agent.fetch.enable || (agent.fetch.allowedHosts != [ ] && agent.fetch.allowedContentTypes != [ ]);
          message = "tentaflake fetch broker ${name} requires an exact non-empty hostname allowlist.";
        }
        {
          assertion =
            !agent.fetch.enable
            || lib.all (
              host:
              host == lib.toLower host
              && lib.all (label: lib.match "^[a-z0-9]([a-z0-9-]*[a-z0-9])?$" label != null) (
                lib.splitString "." host
              )
            ) agent.fetch.allowedHosts;
          message = "tentaflake fetch broker ${name} accepts only exact lowercase hostnames without wildcards.";
        }
        {
          assertion =
            !agent.fetch.enable
            || lib.all (
              contentType:
              contentType == lib.toLower contentType
              && lib.match "^[a-z0-9.+-]+/[a-z0-9.+-]+$" contentType != null
            ) agent.fetch.allowedContentTypes;
          message = "tentaflake fetch broker ${name} accepts only exact lowercase media types.";
        }
        {
          assertion = !agent.llm.enable || !agent.fetch.enable || agent.llm.port != agent.fetch.port;
          message = "tentaflake broker ${name} LLM and fetch listeners require distinct ports.";
        }
      ]) enabledAgents
    );

    systemd.services = allServices;

    networking.firewall = lib.mkIf (enabledAgents != { }) {
      filterForward = true;
      extraInputRules = firewallInputRules;
      extraForwardRules = firewallForwardRules;
    };
  };
}
