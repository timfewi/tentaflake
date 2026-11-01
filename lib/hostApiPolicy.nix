# Deliberately project non-secret declared policy, never raw agent settings,
# environment, host paths, credential references or upstream URLs.
{ config, lib }:
let
  containers = config.virtualisation.oci-containers.containers;
  instances = config.tentaflake.agentInstances;
  policy =
    name: _:
    let
      container = containers.${name} or null;
      broker = config.tentaflake.broker.agents.${name} or null;
      worker = config.tentaflake.worker.agents.${name} or null;
      quota = config.tentaflake.workspaceQuota.agents.${name} or null;
      llmEnabled = broker != null && broker.enable && broker.llm.enable;
      fetchEnabled = broker != null && broker.enable && broker.fetch.enable;
      workerEnabled = worker != null && worker.enable;
      quotaEnabled = quota != null && quota.enable;
    in
    {
      managed = builtins.hasAttr name instances;
      authority = "nix";
      auto_start = if container == null then null else container.autoStart;
      llm = {
        enabled = llmEnabled;
        allowed_models = lib.optionals llmEnabled (
          map (model: {
            inherit (model) name;
            input_microusd_per_million = model.inputMicrousdPerMillion;
            output_microusd_per_million = model.outputMicrousdPerMillion;
          }) broker.llm.allowedModels
        );
        max_completion_tokens = if llmEnabled then broker.llm.maxCompletionTokens else null;
        streaming = llmEnabled && broker.llm.streaming.enable;
        strict_cost_envelope = llmEnabled && broker.llm.strictCostEnvelope;
        budgets =
          if llmEnabled then
            {
              max_concurrency = broker.maxConcurrency;
              max_requests_per_minute = broker.maxRequestsPerMinute;
              daily_request_budget = broker.dailyRequestBudget;
              daily_token_budget = broker.dailyTokenBudget;
              daily_cost_microusd = broker.dailyCostMicrousd;
            }
          else
            null;
      };
      fetch = {
        enabled = fetchEnabled;
        allowed_hosts = if fetchEnabled then broker.fetch.allowedHosts else [ ];
      };
      research = {
        enabled = builtins.hasAttr name (lib.attrByPath [ "tentaflake" "research" "agents" ] { } config);
        readiness = "unverified";
      };
      worker = {
        enabled = workerEnabled;
        network = "none";
        limits =
          if workerEnabled then
            {
              max_timeout_seconds = worker.maxTimeoutSeconds;
              max_queue_entries = worker.maxQueueEntries;
              max_queue_bytes = worker.maxQueueBytes;
              max_snapshot_bytes = worker.maxSnapshotBytes;
              max_log_bytes = worker.maxLogBytes;
            }
          else
            null;
        approval_required = [
          "external-reversible"
          "irreversible"
          "financial"
          "production"
          "communicative"
          "privileged"
        ];
      };
      storage = {
        workspace_quota_mib = if quotaEnabled then quota.sizeMiB else null;
        private_state_quota_mib = if quotaEnabled && quota.state != null then quota.state.sizeMiB else null;
        resize = "migration-required";
      };
      management = {
        transport = config.tentaflake.management.transport;
        ssh_authorization = config.tentaflake.management.capabilities.sshAuthorization;
        remote_grants = "unverified";
      };
    };
in
lib.mapAttrs policy (containers // instances)
