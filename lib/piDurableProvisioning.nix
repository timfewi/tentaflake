# Derive policy, never identities: only reviewed public startup intent is selected.
# This pure projection is not runtime admission; adapter/service assertions remain.
{ lib }:
{ instances, policy }:
let
  requested = lib.filterAttrs (
    _: instance: instance.adapter == "pi-durable" && (instance.autoStart or false)
  ) instances;
  required = [
    "maxActiveAgents"
    "networkOctet"
    "provider"
    "model"
    "research"
    "resources"
    "broker"
    "worker"
    "quota"
  ];
  complete = lib.all (key: (policy.${key} or null) != null) required;
  count = builtins.length (builtins.attrNames requested);
  withinCapacity = complete && count <= policy.maxActiveAgents;
  remainder = value: divisor: value - (builtins.div value divisor) * divisor;
  # A fixed identity hash, not list position: adding/ordering stopped or active
  # declarations cannot change another agent's network or relay principal.
  hash =
    identity:
    (builtins.fromTOML "value = 0x${builtins.substring 0 8 (builtins.hashString "sha256" identity)}")
    .value;
  networkIndex = identity: remainder (hash "network:${identity}") 16384;
  subnetBase =
    identity:
    let
      index = networkIndex identity;
    in
    "10.${toString policy.networkOctet}.${toString (builtins.div index 64)}.${
      toString (4 * remainder index 64)
    }";
  relayUid =
    identity:
    policy.research.relayUidBase
    + remainder (hash "research:${identity}") (lib.max 1 policy.research.relayUidCount);
  cpu =
    millis:
    "${toString (builtins.div millis 1000)}.${
      builtins.substring 1 3 (toString (1000 + remainder millis 1000))
    }";
  mib = value: "${toString value}m";
  mapRequested = make: if withinCapacity then lib.mapAttrs make requested else { };
in
{
  inherit requested complete count;
  broker = mapRequested (
    identity: _:
    builtins.removeAttrs policy.broker [ "maxCompletionTokens" ]
    // {
      enable = true;
      # The canonical agent ID already fits the broker's 63-character limit.
      networkName = identity;
      subnet = "${subnetBase identity}/30";
      gateway =
        let
          index = networkIndex identity;
        in
        "10.${toString policy.networkOctet}.${toString (builtins.div index 64)}.${
          toString (4 * remainder index 64 + 1)
        }";
      llm = {
        enable = true;
        upstreamBaseUrl = policy.provider.upstreamBaseUrl;
        providerCredentialFile = policy.provider.credentialFile;
        allowedModels = [ policy.model ];
        maxCompletionTokens = policy.broker.maxCompletionTokens;
        streaming.enable = true;
        # This fixed Responses runtime cannot use the chat-only strict envelope.
        # The host must explicitly accept that incompatibility, not downgrade it.
        strictCostEnvelope = false;
      };
      fetch.enable = false;
    }
  );
  research = mapRequested (identity: _: { uid = relayUid identity; });
  worker = mapRequested (
    _: instance: {
      enable = true;
      inherit (instance) workspace;
      containerUid = 10100;
      containerGid = 10100;
      memory = mib policy.worker.memoryMiB;
      memorySwap = mib policy.worker.memoryMiB;
      cpus = cpu policy.worker.cpuMillis;
      inherit (policy.worker)
        pidsLimit
        maxInboxEntries
        maxQueueEntries
        maxRetainedResults
        maxSnapshotEntries
        maxRetainedEntries
        ;
      maxInboxBytes = policy.worker.maxInboxMiB * 1024 * 1024;
      maxQueueBytes = policy.worker.maxQueueMiB * 1024 * 1024;
      maxSnapshotBytes = policy.worker.maxSnapshotMiB * 1024 * 1024;
      maxRetainedResultBytes = policy.worker.maxRetainedResultMiB * 1024 * 1024;
      maxLogBytes = policy.worker.maxLogMiB * 1024 * 1024;
      workspaceTmpfsSize = mib policy.worker.workspaceTmpfsMiB;
      tmpTmpfsSize = mib policy.worker.tmpTmpfsMiB;
      maxRequestBytes = 65536;
      maxTimeoutSeconds = 3600;
    }
  );
  quota = mapRequested (
    _: instance: {
      enable = true;
      inherit (instance) workspace;
      ownerUid = 10100;
      ownerGid = 10100;
      sizeMiB = policy.quota.workspaceMiB;
      state = {
        path = instance.stateStorage;
        sizeMiB = policy.quota.stateMiB;
      };
    }
  );
  resources =
    if !complete then
      { }
    else
      {
        memory = mib policy.resources.memoryMiB;
        memorySwap = mib policy.resources.memoryMiB;
        cpus = cpu policy.resources.cpuMillis;
        tmpfsSize = mib policy.resources.tmpfsMiB;
        runTmpfsSize = mib policy.resources.runTmpfsMiB;
        inherit (policy.resources) nofile;
      };
}
