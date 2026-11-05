{
  config,
  lib,
  options,
  pkgs,
  ...
}:
let
  cfg = config.tentaflake.piDurableProvisioning;
  runtime = config.tentaflake.piDurableRuntime;
  plan = (import ../lib/piDurableProvisioning.nix { inherit lib; }) {
    instances = config.tentaflake.agentInstances;
    policy = cfg;
  };
  transport = import ../lib/piDurableTransport.nix { inherit lib pkgs; };
  hasResearch = options ? tentaflake.research.agents;
  researchService = lib.attrByPath [ "services" "secureResearch" ] { } config;
  researchTools = [
    "research_browser"
    "research_fetch"
    "research_job"
    "research_read"
    "research_search"
  ];
  unique = values: builtins.length values == builtins.length (lib.unique values);
  matches = expected: actual: lib.recursiveUpdate actual expected == actual;
  nullable =
    description: fields:
    lib.mkOption {
      type = lib.types.nullOr (lib.types.submodule { options = fields; });
      default = null;
      inherit description;
    };
  field = type: lib.mkOption { inherit type; };
  positiveFields = names: lib.genAttrs names (_: field lib.types.ints.positive);
  boundedMiB = field (lib.types.ints.between 1 1048576);
  cpuMillis = field (lib.types.ints.between 1 1024000);
  mapped = [
    {
      name = "broker";
      expected = plan.broker;
      actual = config.tentaflake.broker.agents;
    }
    {
      name = "worker";
      expected = plan.worker;
      actual = config.tentaflake.worker.agents;
    }
    {
      name = "workspace quota";
      expected = plan.quota;
      actual = config.tentaflake.workspaceQuota.agents;
    }
    {
      name = "Research";
      expected = plan.research;
      actual = lib.attrByPath [ "tentaflake" "research" "agents" ] { } config;
    }
  ];
in
{
  options.tentaflake.piDurableProvisioning = {
    enable = lib.mkEnableOption "host-reviewed, identity-derived policy for public Pi Durable startup requests";
    maxActiveAgents = lib.mkOption {
      type = lib.types.nullOr lib.types.ints.positive;
      default = null;
      description = "Finite configured runner capacity, not a declaration-count limit. Further autoStart requests are rejected at evaluation; no queue or automatic eviction is provided.";
    };
    networkOctet = lib.mkOption {
      type = lib.types.nullOr (lib.types.ints.between 0 255);
      default = null;
      description = "Owner-reserved 10.<octet>.0.0/16 pool for stable identity-hashed /30 broker networks. Review host/VPN routes first; changing the pool is an explicit stopped-runtime migration.";
    };
    allowNonStrictCostEnvelope = lib.mkOption {
      type = lib.types.bool;
      default = false;
      description = "Explicit acceptance that this Responses/SSE runtime cannot provide the broker's chat-only strict cost envelope. Ordinary finite request/token/cost budgets remain enforced; this never weakens an existing strict policy.";
    };
    provider =
      nullable
        "One owner-approved Responses provider and runtime credential channel; no browser-supplied provider or secret value."
        {
          upstreamBaseUrl = field lib.types.str;
          credentialFile = field lib.types.str;
        };
    model =
      nullable
        "One reviewed exact model from the pinned Pi OpenAI Responses catalog, with explicit positive micro-USD per-million-token prices."
        {
          name = field lib.types.str;
          inputMicrousdPerMillion = field lib.types.ints.positive;
          outputMicrousdPerMillion = field lib.types.ints.positive;
        };
    research =
      nullable
        "Approval of the complete existing pinned secure-research-tool connection and its fixed five-tool manifest. Public Pi agents[].tools may narrow registration, never admit an arbitrary MCP command or URL. Configure the service/providers/VPN separately."
        {
          tools = field (lib.types.listOf (lib.types.enum researchTools));
          relayUidBase = field (lib.types.ints.between 65536 4294967294);
          relayUidCount = field lib.types.ints.positive;
        };
    resources =
      nullable
        "Explicit shared controller containment limits; Pi Durable additionally has its fixed 512-PID ceiling."
        {
          memoryMiB = boundedMiB;
          inherit cpuMillis;
          tmpfsMiB = field (lib.types.ints.between 1 65536);
          runTmpfsMiB = field (lib.types.ints.between 1 65536);
          nofile = field lib.types.ints.positive;
        };
    broker =
      nullable
        "Finite per-identity LLM request/concurrency/budget policy. Aggregate host ceilings are capacity times these values."
        (positiveFields [
          "maxRequestBytes"
          "maxResponseBytes"
          "maxConcurrency"
          "maxRequestsPerMinute"
          "dailyRequestBudget"
          "dailyTokenBudget"
          "dailyCostMicrousd"
          "maxCompletionTokens"
        ]);
    worker =
      nullable
        "Finite offline-worker resources, queue, snapshot and retained-result storage. Existing local-reversible approval and no-egress enforcement is unchanged."
        (
          positiveFields [
            "pidsLimit"
            "maxInboxEntries"
            "maxQueueEntries"
            "maxRetainedResults"
            "maxSnapshotEntries"
            "maxRetainedEntries"
          ]
          // lib.genAttrs [
            "memoryMiB"
            "maxInboxMiB"
            "maxQueueMiB"
            "maxSnapshotMiB"
            "maxRetainedResultMiB"
            "maxLogMiB"
            "workspaceTmpfsMiB"
            "tmpTmpfsMiB"
          ] (_: boundedMiB)
          // {
            inherit cpuMillis;
          }
        );
    quota =
      nullable
        "Separate immutable private workspace and durable-state Btrfs image sizes; no existing-state conversion or resize."
        {
          workspaceMiB = field (lib.types.ints.between 128 1048576);
          stateMiB = field (lib.types.ints.between 128 1048576);
        };
  };

  config = lib.mkMerge [
    (lib.mkIf cfg.enable {
      assertions = [
        {
          assertion = plan.complete;
          message = "tentaflake Pi provisioning requires explicit provider, priced model, Research approval, resources, broker budgets, worker limits, quotas, network pool and finite maxActiveAgents host policy.";
        }
        {
          assertion = runtime.enable && transport.configurationSupported config;
          message = "tentaflake Pi provisioning requires separately admitted piDurableRuntime and its exact runsc-pi-durable-v1 transport; provisioning is not runtime authorization.";
        }
        {
          assertion =
            lib.isDerivation runtime.package && lib.isDerivation runtime.bundle && runtime.image != null;
          message = "tentaflake Pi provisioning requires the separately reviewed pinned runtime package, matching immutable image bundle and selector.";
        }
        {
          assertion = cfg.allowNonStrictCostEnvelope;
          message = "tentaflake Pi provisioning requires explicit owner acceptance of the Responses runtime's lack of chat-only strict-cost-envelope support; strict policy is never downgraded.";
        }
        {
          assertion = cfg.maxActiveAgents != null && plan.count <= cfg.maxActiveAgents;
          message = "tentaflake Pi provisioning capacity exhausted or unset: ${toString plan.count} startup requests; keep additional declarations autoStart = false or review host capacity. No worker queue or eviction is implied.";
        }
        {
          assertion = hasResearch && (plan.count == 0 || (researchService.enable or false));
          message = "tentaflake Pi provisioning requires the existing pinned isolated Research module and an owner-configured enabled service for requested starts; an all-stopped fleet needs no Research clients or running service.";
        }
        {
          assertion = cfg.research != null && lib.sort builtins.lessThan cfg.research.tools == researchTools;
          message = "tentaflake Pi provisioning requires explicit approval of the complete fixed research_job/search/fetch/browser/read connection manifest. Public Pi agents[].tools only narrows actual registration within that approved connection; it never admits generic MCP equipment.";
        }
        {
          assertion =
            cfg.research == null || cfg.research.relayUidBase + cfg.research.relayUidCount - 1 <= 4294967294;
          message = "tentaflake Pi provisioning Research relay UID pool exceeds the supported host UID range.";
        }
        {
          assertion = !plan.complete || unique (map (entry: entry.uid) (builtins.attrValues plan.research));
          message = "tentaflake Pi provisioning Research identity hashes collide in the reviewed UID pool; refuse aliasing and review an explicit pool migration.";
        }
        {
          assertion = !plan.complete || unique (map (entry: entry.subnet) (builtins.attrValues plan.broker));
          message = "tentaflake Pi provisioning broker identity hashes collide in the reviewed network pool; refuse shared networks and review an explicit pool migration.";
        }
      ]
      ++ lib.concatMap (
        mapping:
        lib.mapAttrsToList (identity: expected: {
          assertion = builtins.hasAttr identity mapping.actual && matches expected mapping.actual.${identity};
          message = "tentaflake Pi provisioning ${mapping.name} policy for ${identity} must exactly retain its identity-derived host grant; cross-agent paths, principals, models and budget overrides are refused.";
        }) mapping.expected
      ) mapped;
    })
    (lib.mkIf (cfg.enable && plan.complete) {
      tentaflake = {
        broker = {
          agents = plan.broker;
          maxEnabledAgents = cfg.maxActiveAgents;
          maxTotalConcurrency = cfg.maxActiveAgents * cfg.broker.maxConcurrency;
          maxTotalRequestsPerMinute = cfg.maxActiveAgents * cfg.broker.maxRequestsPerMinute;
          maxTotalDailyTokenBudget = cfg.maxActiveAgents * cfg.broker.dailyTokenBudget;
          maxTotalDailyCostMicrousd = cfg.maxActiveAgents * cfg.broker.dailyCostMicrousd;
        };
        worker.agents = plan.worker;
        workspaceQuota.agents = plan.quota;
        security.resources = plan.resources;
      };
    })
    (lib.mkIf (cfg.enable && plan.complete) (
      lib.optionalAttrs hasResearch {
        tentaflake.research.agents = plan.research;
      }
    ))
  ];
}
