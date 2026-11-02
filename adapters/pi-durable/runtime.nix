{ pkgs, lib }:
{
  name,
  stateDir,
  autoStart ? false,
}:
{ config, ... }:
let
  identity = import ../../lib/agentIdentity.nix;
  security = import ../../lib/containerSecurity.nix { inherit lib; };
  transport = import ../../lib/piDurableTransport.nix { inherit lib pkgs; };
  container = identity.container name;
  backend = config.virtualisation.oci-containers.backend;
  unit = "${backend}-${container}";
  cfg = config.tentaflake.piDurableRuntime;
  uid = 10100;
  owner = "10100:10100";
  state = "${stateDir}/state";
  workspace = "${stateDir}/workspace";
  runtime = "/run/tentaflake-pi-durable/${container}";
  control = "${runtime}/control";
  broker = lib.attrByPath [ container ] null config.tentaflake.broker.agents;
  brokerEnabled =
    broker != null
    && broker.enable
    && broker.llm.enable
    && !broker.fetch.enable
    && broker.llm.streaming.enable
    && !broker.llm.strictCostEnvelope;
  worker = lib.attrByPath [ container ] null config.tentaflake.worker.agents;
  workerEnabled = worker != null && worker.enable;
  quota = lib.attrByPath [ container ] null config.tentaflake.workspaceQuota.agents;
  quotaEnabled = quota != null && quota.enable && quota.state != null;
  results = "/var/lib/tentaflake-worker-${container}/results";
  researchEnabled = builtins.hasAttr container config.tentaflake.research.agents;
  researchDirectory = "/run/tentaflake-research/${container}";
  researchPackage = config.tentaflake.research.clientPackage;
  researchPaths = lib.splitString "\n" (
    lib.removeSuffix "\n" (
      builtins.readFile "${pkgs.closureInfo { rootPaths = [ researchPackage ]; }}/store-paths"
    )
  );
  model =
    if brokerEnabled && lib.length broker.llm.allowedModels == 1 then
      (builtins.head broker.llm.allowedModels).name
    else
      "";
  configuration = (pkgs.formats.json { }).generate "${container}-pi-durable.json" {
    stateDir = "/state";
    workspace = "/workspace";
    socketPath = "/run/tentaflake-pi-durable/control/runner.sock";
    apiTokenFile = "/run/tentaflake-pi-durable/api-token";
    brokerTokenFile = "/run/tentaflake-pi-durable/broker-token";
    brokerBaseUrl =
      if brokerEnabled then "http://${broker.gateway}:${toString broker.llm.port}/v1" else "";
    inherit model;
    researchCommand = "${researchPackage}/bin/research-client";
  };
  dependencies = [
    "tentaflake-pi-durable-prepare-${container}.service"
    "tentaflake-broker-network-${container}.service"
    "tentaflake-broker-llm-${container}.service"
    "tentaflake-research-${container}.socket"
    "tentaflake-workspace-quota-${container}.service"
    "tentaflake-state-quota-${container}.service"
  ];
  result = security.apply {
    profile = config.tentaflake.security.profile;
    inherit backend owner workerEnabled;
    runtimeName = transport.name;
    name = container;
    pidsLimit = 512;
    resources = config.tentaflake.security.resources;
    reviewedImage = {
      inherit (cfg) bundle;
      inherit (cfg) image;
    };
    baseConfig = {
      inherit autoStart;
      inherit (cfg) image;
      imageFile = "${cfg.bundle}/image.tar.gz";
      cmd = [
        "/bin/tentaflake-pi-durable"
        "/etc/tentaflake/pi-durable.json"
      ];
      networks = lib.optional brokerEnabled broker.networkName;
      user = owner;
      environment.HOME = "/state";
      volumes = [
        "${state}:/state:rw"
        "${workspace}:/workspace:rw"
        "${control}:/run/tentaflake-pi-durable/control:rw"
        "${runtime}/api-token:/run/tentaflake-pi-durable/api-token:ro"
        "${runtime}/broker-token:/run/tentaflake-pi-durable/broker-token:ro"
        "${configuration}:/etc/tentaflake/pi-durable.json:ro"
        "${results}:/run/tentaflake-worker/results:ro"
        "${researchDirectory}:/run/tentaflake-research:ro"
      ]
      ++ map (path: "${path}:${path}:ro") researchPaths;
      extraOptions = [ "--workdir=/workspace" ];
    };
    allowedWritableSources = [
      state
      workspace
      control
    ];
    allowedWritableDestinations = [
      "/state"
      "/workspace"
      "/run/tentaflake-pi-durable/control"
    ];
    approvedReadOnlySources = [
      "${runtime}/api-token"
      "${runtime}/broker-token"
      results
      researchDirectory
    ];
    approvedReadOnlyDestinations = [
      "/run/tentaflake-pi-durable/api-token"
      "/run/tentaflake-pi-durable/broker-token"
      "/run/tentaflake-worker/results"
      "/run/tentaflake-research"
      "/etc/tentaflake/pi-durable.json"
    ];
    brokerNetwork = if brokerEnabled then broker.networkName else null;
    brokerPolicyEnabled = brokerEnabled;
    researchPolicyEnabled = researchEnabled;
    workspaceQuotaEnabled = quotaEnabled;
    automaticStart = autoStart;
  };
in
{
  assertions = result.assertions ++ [
    {
      assertion = transport.supported config;
      message = "tentaflake: Pi Durable requires its exact rootful, non-remapped runsc-pi-durable-v1 transport.";
    }
    {
      assertion =
        transport.containerSupported
          config.virtualisation.oci-containers.containers.${container};
      message = "tentaflake: Pi Durable does not support rootless or user-remapped containers.";
    }
    {
      assertion = config.tentaflake.security.profile == "balanced";
      message = "tentaflake: Pi Durable requires the shared balanced containment profile.";
    }
    {
      assertion = cfg.package.upstreamRevision == "35dfa757378bd7a8570670e99c2687bdd1039a06";
      message = "tentaflake: Pi Durable package must use the reviewed immutable SDK revision.";
    }
    {
      assertion = (cfg.bundle.runtimePackage or null) == cfg.package;
      message = "tentaflake: Pi Durable image bundle must be built from the selected reviewed runner package.";
    }
    {
      assertion = brokerEnabled && model != "";
      message = "tentaflake: Pi Durable requires one exact SSE-enabled Responses broker model, no legacy fetch, and no incompatible chat-only strict-cost policy; policy is never silently downgraded.";
    }
    {
      assertion = researchEnabled;
      message = "tentaflake: Pi Durable requires its exact isolated Research relay.";
    }
    {
      assertion =
        workerEnabled
        && worker.workspace == workspace
        && worker.containerUid == uid
        && worker.containerGid == uid;
      message = "tentaflake: Pi Durable requires its exact offline worker workspace and numeric owner.";
    }
    {
      # Pi Durable's offline_worker tool advertises these maxima; a narrower worker
      # would silently discard admitted calls and quarantine the instance.
      assertion = !workerEnabled || (worker.maxTimeoutSeconds >= 3600 && worker.maxRequestBytes >= 65536);
      message = "tentaflake: Pi Durable requires its offline worker to admit 3600-second timeouts and 65536-byte requests.";
    }
    {
      assertion = quotaEnabled && quota.workspace == workspace && quota.state.path == state;
      message = "tentaflake: Pi Durable requires its exact workspace and durable-state quotas.";
    }
  ];
  virtualisation.oci-containers.containers.${container} = result.container;
  systemd.services = {
    "tentaflake-pi-durable-state-${container}" = {
      description = "Create guarded Pi Durable mountpoints before quota preparation for ${container}";
      before = [
        "tentaflake-workspace-quota-prepare-${container}.service"
        "tentaflake-state-quota-prepare-${container}.service"
      ];
      serviceConfig = {
        Type = "oneshot";
        RemainAfterExit = true;
        UMask = "0077";
        NoNewPrivileges = true;
        PrivateDevices = true;
        PrivateTmp = true;
        ProtectSystem = "full";
        ProtectHome = true;
        RestrictAddressFamilies = [ "AF_UNIX" ];
        CapabilityBoundingSet = [
          "CAP_CHOWN"
          "CAP_DAC_OVERRIDE"
          "CAP_FOWNER"
        ];
      };
      script = ''
        probe=${lib.escapeShellArg stateDir}
        while [ "$probe" != / ]; do
          if [ -L "$probe" ]; then echo "Pi Durable refuses symlinked mountpoint ancestry" >&2; exit 1; fi
          if [ -e "$probe" ]; then
            if [ ! -d "$probe" ] || [ "$(${pkgs.coreutils}/bin/stat -c %u "$probe")" != 0 ]; then
              echo "Pi Durable requires root-owned mountpoint ancestry" >&2; exit 1
            fi
            mode="$(${pkgs.coreutils}/bin/stat -c %a "$probe")"
            if [ "$((0$mode & 0022))" -ne 0 ]; then
              echo "Pi Durable refuses writable mountpoint ancestry" >&2; exit 1
            fi
          fi
          probe="$(${pkgs.coreutils}/bin/dirname -- "$probe")"
        done
        if [ -e ${lib.escapeShellArg stateDir} ] && [ "$(${pkgs.coreutils}/bin/stat -c %a ${lib.escapeShellArg stateDir})" != 700 ]; then
          echo "Pi Durable refuses nonprivate existing instance parent" >&2; exit 1
        fi
        ${pkgs.coreutils}/bin/install -d -m 0700 ${lib.escapeShellArg stateDir}
        for directory in ${
          lib.escapeShellArgs [
            state
            workspace
          ]
        }; do
          if [ -L "$directory" ] || { [ -e "$directory" ] && { [ ! -d "$directory" ] || [ "$(${pkgs.coreutils}/bin/stat -c %u:%g:%a "$directory")" != 10100:10100:700 ]; }; }; then
            echo "Pi Durable refuses unexpected mountpoint ownership" >&2; exit 1
          fi
          ${pkgs.coreutils}/bin/install -d -m 0700 -o 10100 -g 10100 "$directory"
        done
      '';
    };
    "tentaflake-workspace-quota-prepare-${container}" = {
      requires = [ "tentaflake-pi-durable-state-${container}.service" ];
      after = [ "tentaflake-pi-durable-state-${container}.service" ];
    };
    "tentaflake-state-quota-prepare-${container}" = lib.mkIf (quota != null && quota.state != null) {
      requires = [ "tentaflake-pi-durable-state-${container}.service" ];
      after = [ "tentaflake-pi-durable-state-${container}.service" ];
    };
    "tentaflake-pi-durable-prepare-${container}" = {
      description = "Prepare private Pi Durable state and scoped virtual API credentials for ${container}";
      requires = [
        "tentaflake-broker-credentials-${container}.service"
        "tentaflake-workspace-quota-${container}.service"
        "tentaflake-state-quota-${container}.service"
      ];
      after = [
        "tentaflake-broker-credentials-${container}.service"
        "tentaflake-workspace-quota-${container}.service"
        "tentaflake-state-quota-${container}.service"
      ];
      before = [ "${unit}.service" ];
      serviceConfig = {
        Type = "oneshot";
        RemainAfterExit = true;
        RuntimeDirectory = "tentaflake-pi-durable/${container}";
        RuntimeDirectoryMode = "0700";
        RuntimeDirectoryPreserve = "yes";
        UMask = "0077";
        NoNewPrivileges = true;
        PrivateDevices = true;
        PrivateTmp = true;
        ProtectSystem = "strict";
        ProtectHome = true;
        ProtectKernelTunables = true;
        ProtectKernelModules = true;
        ProtectControlGroups = true;
        RestrictAddressFamilies = [ "AF_UNIX" ];
        ReadWritePaths = [
          stateDir
          runtime
        ];
        CapabilityBoundingSet = [
          "CAP_CHOWN"
          "CAP_DAC_OVERRIDE"
          "CAP_FOWNER"
        ];
      };
      script = ''
        # Check every existing ancestor before privileged ownership changes.
        for target in ${
          lib.escapeShellArgs [
            stateDir
            state
            workspace
            runtime
            control
            "${runtime}/api-token"
            "${runtime}/broker-token"
          ]
        }; do
          probe="$target"
          while [ "$probe" != / ]; do
            if [ -L "$probe" ]; then echo "Pi Durable refuses symlinked state/runtime paths" >&2; exit 1; fi
            probe="$(${pkgs.coreutils}/bin/dirname -- "$probe")"
          done
        done
        if [ "$(${pkgs.coreutils}/bin/stat -c %u:%g:%a ${lib.escapeShellArg runtime})" != 0:0:700 ]; then
          echo "Pi Durable requires a private root-owned runtime parent" >&2; exit 1
        fi
        if [ "$(${pkgs.coreutils}/bin/stat -c %u:%a ${lib.escapeShellArg stateDir})" != 0:700 ]; then
          echo "Pi Durable requires a private root-owned instance parent" >&2; exit 1
        fi
        for directory in ${
          lib.escapeShellArgs [
            state
            workspace
          ]
        }; do
          if [ -e "$directory" ] && [ "$(${pkgs.coreutils}/bin/stat -c %u:%g:%a "$directory")" != 10100:10100:700 ]; then
            echo "Pi Durable refuses unexpected state/workspace ownership" >&2; exit 1
          fi
          ${pkgs.coreutils}/bin/install -d -m 0700 -o 10100 -g 10100 "$directory"
        done
        if [ -e ${lib.escapeShellArg control} ] && [ "$(${pkgs.coreutils}/bin/stat -c %u:%g:%a ${lib.escapeShellArg control})" != 10100:10100:710 ]; then
          echo "Pi Durable refuses unexpected control directory ownership" >&2; exit 1
        fi
        ${pkgs.coreutils}/bin/install -d -m 0710 -o 10100 -g 10100 ${lib.escapeShellArg control}
        if [ ! -e ${lib.escapeShellArg "${runtime}/api-token"} ]; then
          (set -C; ${pkgs.openssl}/bin/openssl rand -hex 32 > ${lib.escapeShellArg "${runtime}/api-token"})
          if [ ! -f ${lib.escapeShellArg "${runtime}/api-token"} ] || [ "$(${pkgs.coreutils}/bin/stat -c %u:%h ${lib.escapeShellArg "${runtime}/api-token"})" != 0:1 ]; then
            echo "Pi Durable refuses unsafe new API credential" >&2; exit 1
          fi
          ${pkgs.coreutils}/bin/chown 0:10100 ${lib.escapeShellArg "${runtime}/api-token"}
          ${pkgs.coreutils}/bin/chmod 0440 ${lib.escapeShellArg "${runtime}/api-token"}
        fi
        if [ ! -f ${lib.escapeShellArg "${runtime}/api-token"} ] || [ "$(${pkgs.coreutils}/bin/stat -c %u:%g:%a:%h:%s ${lib.escapeShellArg "${runtime}/api-token"})" != 0:10100:440:1:65 ]; then
          echo "Pi Durable refuses unsafe existing API credential" >&2; exit 1
        fi
        if [ -e ${lib.escapeShellArg "${runtime}/broker-token"} ] && { [ ! -f ${lib.escapeShellArg "${runtime}/broker-token"} ] || [ "$(${pkgs.coreutils}/bin/stat -c %h ${lib.escapeShellArg "${runtime}/broker-token"})" != 1 ]; }; then
          echo "Pi Durable refuses unsafe broker credential destination" >&2; exit 1
        fi
        ${pkgs.coreutils}/bin/install -m 0400 -o 10100 -g 10100 \
          ${lib.escapeShellArg "/run/tentaflake-broker/${container}/agent-token"} \
          ${lib.escapeShellArg "${runtime}/broker-token"}
      '';
    };
    ${unit} = (import ../../lib/serviceRecovery.nix) // {
      requires = dependencies;
      after = dependencies;
      # The OCI module's explicit ExecStartPre loads the archive. Append to that
      # list, not service.preStart (which runs before the explicit load command).
      serviceConfig.ExecStartPre = lib.mkAfter [
        (import ../../lib/verifyLocalImage.nix {
          inherit lib pkgs backend;
          inherit (cfg) bundle image;
        })
        (pkgs.writeShellScript "pi-durable-private-socket-${name}" ''
          socket=${lib.escapeShellArg "${control}/runner.sock"}
          if [ -L "$socket" ] || { [ -e "$socket" ] && [ ! -S "$socket" ]; }; then
            echo "Pi Durable refuses unexpected API socket path" >&2; exit 1
          fi
          if [ -S "$socket" ]; then
            if [ "$(${pkgs.coreutils}/bin/stat -c %u "$socket")" != 10100 ]; then
              echo "Pi Durable refuses foreign API socket" >&2; exit 1
            fi
            ${pkgs.coreutils}/bin/rm -- "$socket"
          fi
        '')
      ];
    };
  };
}
