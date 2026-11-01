{ pkgs, lib }:

let
  preset = {
    schemaVersion = 1;
    identity = {
      id = "zeroclaw";
      version = "0.8.2";
      status = "compatibility";
    };
    artifact = {
      kind = "oci";
      reference = "ghcr.io/zeroclaw-labs/zeroclaw@sha256:eae321dac2d314bc282bdfb28b5378c9d527998f7e2fe0dee8315bfdcdf13a0c";
      reviewed = true;
      source = "ghcr.io/zeroclaw-labs/zeroclaw:v0.8.2";
    };
    command = [ "daemon" ];
    configuration = {
      format = "toml";
      readOnly = true;
    };
    ownership = {
      uid = 65534;
      gid = 65534;
    };
    layout = {
      state = "/var/lib/tentaflake/agents/<name>";
      workspace = "data";
      writable = [
        "state"
        "workspace"
      ];
      directories = [
        ".zeroclaw"
        ".zeroclaw/data"
        "data"
      ];
    };
    lifecycle = "service";
    model = {
      protocols = [ "openai-chat-completions" ];
      streaming = "unknown";
    };
    research = {
      supported = true;
    };
    execution = {
      interface = "worker-queue-v1";
      automatic = false;
    };
    capabilities = [
      "files"
      "shell"
      "terminal"
      "worker"
      "model"
      "research"
    ];
    evidence = {
      fixture = "source-regression";
      vendorAcceptance = "pending";
    };
  };
  hostIdentity = import ../lib/agentIdentity.nix;
  containerSecurity = import ../lib/containerSecurity.nix { inherit lib; };
  generateConfiguration =
    name: settings: (pkgs.formats.toml { }).generate "${name}-config.toml" settings;
  build =
    {
      name,
      agenixFile ? null,
      # Digest-pinned (this adapter). Overrides must be digest-pinned too — a
      # bare tag is rejected at eval time, see lib/pinnedImage.nix.
      image ? preset.artifact.reference,
      # Escape hatch for locally-built images that have no registry digest.
      allowMutableImage ? false,
      stateDir ? hostIdentity.stateDir name,
      seedDir ? null,
      gatewayPort ? 42617,
      hostPort ? null,
      servePort ? null,
      autoStart ? true,
      # Max number of processes in the container (--pids-limit). null = unlimited.
      pidsLimit ? 512,
      settings ? { },
      extraEnvironment ? { },
      extraVolumes ? [ ],
      extraContainerConfig ? { },
    }:

    { config, ... }:
    let
      containerName = hostIdentity.container name;
      backend = config.virtualisation.oci-containers.backend;
      serviceName = "${backend}-${containerName}.service";
      serviceAttr = lib.removeSuffix ".service" serviceName;
      securityProfile = config.tentaflake.security.profile;
      secure = containerSecurity.isSecure securityProfile;
      brokerCfg = lib.attrByPath [ containerName ] null config.tentaflake.broker.agents;
      brokerEnabled = brokerCfg != null && brokerCfg.enable;
      workerCfg = lib.attrByPath [ containerName ] null config.tentaflake.worker.agents;
      workerEnabled = workerCfg != null && workerCfg.enable;
      workerResultsDir = "/var/lib/tentaflake-worker-${containerName}/results";
      workerResultsMount = "/run/tentaflake-worker/results";
      quotaCfg = lib.attrByPath [ containerName ] null config.tentaflake.workspaceQuota.agents;
      quotaEnabled = quotaCfg != null && quotaCfg.enable;
      stateQuotaEnabled = quotaEnabled && quotaCfg.state != null;
      quotaUnit = "tentaflake-workspace-quota-${containerName}.service";
      brokerEnvironmentFile = "/run/tentaflake-broker/${containerName}/agent.env";
      brokerUnits =
        lib.optional brokerEnabled "tentaflake-broker-network-${containerName}.service"
        ++ lib.optional (
          brokerEnabled && brokerCfg.llm.enable
        ) "tentaflake-broker-llm-${containerName}.service"
        ++ lib.optional (
          brokerEnabled && brokerCfg.fetch.enable
        ) "tentaflake-broker-fetch-${containerName}.service";
      research = import ../lib/researchClient.nix {
        inherit
          config
          lib
          pkgs
          containerName
          settings
          ;
        adapter = "zeroclaw";
      };
      runtimeDependencies = brokerUnits ++ lib.optional quotaEnabled quotaUnit ++ research.units;
      secureUnitPolicy = import ../lib/serviceRecovery.nix;
      configFile = generateConfiguration containerName research.settings;
      nobodyUid = preset.ownership.uid;
      nobodyGid = preset.ownership.gid;
      owner = "${toString nobodyUid}:${toString nobodyGid}";

      baseContainer = {
        inherit autoStart;
        networks = lib.optional brokerEnabled brokerCfg.networkName;
        inherit image;
        cmd = preset.command;
        volumes = [
          "${stateDir}:/zeroclaw-data:rw"
          "${configFile}:/zeroclaw-data/.zeroclaw/config.toml:ro"
        ]
        ++ lib.optional workerEnabled "${workerResultsDir}:${workerResultsMount}:ro"
        ++ research.volumes
        ++ extraVolumes;
        ports = lib.optional (
          !secure && hostPort != null
        ) "127.0.0.1:${toString hostPort}:${toString gatewayPort}";
        environment = {
          HOME = "/zeroclaw-data";
          ZEROCLAW_DATA_DIR = "/zeroclaw-data/data";
          ZEROCLAW_gateway__port = toString gatewayPort;
        }
        // extraEnvironment;
        environmentFiles = lib.optional brokerEnabled brokerEnvironmentFile;
        extraOptions =
          lib.optional (!secure && agenixFile != null) "--env-file=${agenixFile}"
          ++ lib.optional (!secure) "--security-opt=no-new-privileges:true"
          ++ lib.optional (!secure && pidsLimit != null) "--pids-limit=${toString pidsLimit}";
      };

      securityResult = containerSecurity.apply {
        profile = securityProfile;
        inherit backend pidsLimit allowMutableImage;
        name = containerName;
        inherit owner;
        baseConfig = baseContainer;
        overrides = extraContainerConfig;
        allowedWritableSources = [ stateDir ];
        allowedWritableDestinations = [ "/zeroclaw-data" ];
        resources = config.tentaflake.security.resources;
        brokerNetwork = if brokerEnabled then brokerCfg.networkName else null;
        approvedEnvironmentFiles = lib.optional brokerEnabled brokerEnvironmentFile;
        approvedReadOnlySources = lib.optional workerEnabled workerResultsDir ++ research.readOnlySources;
        approvedReadOnlyDestinations =
          lib.optional workerEnabled workerResultsMount ++ research.readOnlyDestinations;
        researchPolicyEnabled = research.enabled;
        automaticStart = config.virtualisation.oci-containers.containers.${containerName}.autoStart;
        brokerPolicyEnabled = brokerEnabled;
        inherit workerEnabled;
        workspaceQuotaEnabled = quotaEnabled;
      };
    in
    {
      assertions =
        securityResult.assertions
        ++ research.assertions
        ++ lib.optionals secure [
          {
            assertion = !allowMutableImage;
            message = "tentaflake: secure ZeroClaw agent ${name} requires a digest-pinned image; allowMutableImage is dev-only.";
          }
          {
            assertion = agenixFile == null;
            message = "tentaflake: secure ZeroClaw agent ${name} may not receive a provider credential file; use the broker path once configured.";
          }
          {
            assertion = hostPort == null;
            message = "tentaflake: secure ZeroClaw agent ${name} may not publish hostPort; remove it or explicitly migrate the host to dev.";
          }
          {
            assertion = servePort == null;
            message = "tentaflake: secure ZeroClaw agent ${name} may not be published with Tailscale Serve.";
          }
          {
            assertion = !(containerSecurity.containsSensitiveValue extraEnvironment);
            message = "tentaflake: secure ZeroClaw agent ${name} has a secret-like extraEnvironment key, which would enter the Nix store.";
          }
          {
            assertion = !(containerSecurity.containsSensitiveValue settings);
            message = "tentaflake: secure ZeroClaw agent ${name} has a secret-like settings key, which would enter the Nix store.";
          }
          {
            assertion = !workerEnabled || workerCfg.workspace == "${stateDir}/data";
            message = "tentaflake: worker workspace for ${containerName} must exactly match ${stateDir}/data.";
          }
          {
            assertion =
              !workerEnabled || (workerCfg.containerUid == nobodyUid && workerCfg.containerGid == nobodyGid);
            message = "tentaflake: worker uid/gid for ${containerName} must match ${owner}.";
          }
          {
            assertion = !quotaEnabled || quotaCfg.workspace == "${stateDir}/data";
            message = "tentaflake: workspaceQuota for ${containerName} must exactly mount ${stateDir}/data.";
          }
          {
            assertion = !quotaEnabled || (quotaCfg.ownerUid == nobodyUid && quotaCfg.ownerGid == nobodyGid);
            message = "tentaflake: workspaceQuota owner for ${containerName} must match ${owner}.";
          }
        ];

      systemd.tmpfiles.rules = [
        "d ${stateDir} 0700 ${toString nobodyUid} ${toString nobodyGid} -"
      ]
      ++ lib.optionals (!stateQuotaEnabled) (
        map (
          directory: "d ${stateDir}/${directory} 0700 ${toString nobodyUid} ${toString nobodyGid} -"
        ) preset.layout.directories
      );

      systemd.services =
        lib.optionalAttrs (seedDir != null) {
          "${containerName}-seed" = {
            description = "Seed workspace for ZeroClaw agent ${name}";
            wantedBy = [ serviceName ];
            before = [ serviceName ];
            requires = [ "systemd-tmpfiles-setup.service" ] ++ lib.optional quotaEnabled quotaUnit;
            after = [ "systemd-tmpfiles-setup.service" ] ++ lib.optional quotaEnabled quotaUnit;
            serviceConfig = {
              Type = "oneshot";
              RemainAfterExit = true;
              User = toString nobodyUid;
              Group = toString nobodyGid;
              UMask = "0077";
            };
            script = ''
              ${pkgs.coreutils}/bin/cp -rn --no-preserve=mode,ownership \
                ${lib.escapeShellArg "${seedDir}/."} \
                ${lib.escapeShellArg "${stateDir}/.zeroclaw/data/"}
              ${pkgs.coreutils}/bin/chmod -R u+rwX ${lib.escapeShellArg stateDir}
            '';
          };
        }
        // lib.optionalAttrs (servePort != null && hostPort != null) {
          "${containerName}-tailscale-serve" = {
            description = "Tailscale Serve for ZeroClaw agent ${name}";
            wantedBy = [ "multi-user.target" ];
            after = [
              "tailscaled.service"
              serviceName
            ];
            wants = [
              "tailscaled.service"
              serviceName
            ];
            serviceConfig = {
              Type = "oneshot";
              RemainAfterExit = true;
              ExecStart = "${pkgs.tailscale}/bin/tailscale serve --bg --https=${toString servePort} 127.0.0.1:${toString hostPort}";
            };
          };
        }
        // lib.optionalAttrs (secure || runtimeDependencies != [ ]) {
          ${serviceAttr} =
            secureUnitPolicy
            // lib.optionalAttrs (runtimeDependencies != [ ]) {
              requires = runtimeDependencies;
              after = runtimeDependencies;
            };
        };

      virtualisation.oci-containers.containers.${containerName} = securityResult.container;
    };
in
{
  inherit (preset)
    schemaVersion
    identity
    artifact
    command
    ownership
    layout
    lifecycle
    model
    execution
    capabilities
    evidence
    ;
  configuration = preset.configuration // {
    generate = generateConfiguration;
  };
  research = preset.research // {
    configure = { original, server }: {
      settings = {
        browser.enabled = false;
        http_request.enabled = false;
        web_fetch.enabled = false;
        web_search.enabled = false;
        mcp = {
          enabled = true;
          servers = [
            (
              server
              // {
                name = "secure-research-tool";
                transport = "stdio";
              }
            )
          ];
        };
      };
      valid = lib.all (
        entry:
        builtins.isAttrs entry
        && entry ? command
        && !(entry ? url)
        && (entry.transport or "stdio") == "stdio"
      ) (original.mcp.servers or [ ]);
    };
  };
  inherit build preset;
  metadata =
    args: backend:
    let
      stateDir = args.stateDir or (hostIdentity.stateDir args.name);
      container = hostIdentity.container args.name;
    in
    {
      adapter = "zeroclaw";
      inherit (args) name;
      inherit container stateDir;
      stateStorage = stateDir;
      stateDirectories = preset.layout.directories;
      unit = "${backend}-${container}.service";
      workspace = "${stateDir}/data";
      uid = preset.ownership.uid;
      gid = preset.ownership.gid;
      runnable = true;
    };
}
