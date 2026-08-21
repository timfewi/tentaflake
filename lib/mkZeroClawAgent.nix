{ pkgs, lib }:

let
  constants = import ./constants.nix;
  pinnedImage = import ./pinnedImage.nix { inherit lib; };
  containerSecurity = import ./containerSecurity.nix { inherit lib; };
in

{
  name,
  agenixFile ? null,
  # Digest-pinned (lib/constants.nix). Overrides must be digest-pinned too — a
  # bare tag is rejected at eval time, see lib/pinnedImage.nix.
  image ? constants.zeroclawImage,
  # Escape hatch for locally-built images that have no registry digest.
  allowMutableImage ? false,
  stateDir ? "/var/lib/zeroclaw-${name}",
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
  containerName = "zeroclaw-${name}";
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
  runtimeDependencies = brokerUnits ++ lib.optional quotaEnabled quotaUnit;
  secureUnitPolicy = {
    startLimitIntervalSec = 300;
    startLimitBurst = 5;
    serviceConfig.RestartSec = "10s";
  };
  toml = pkgs.formats.toml { };
  configFile = toml.generate "${containerName}-config.toml" settings;
  owner = "65534:65534";

  baseContainer = {
    inherit autoStart;
    networks = lib.optional brokerEnabled brokerCfg.networkName;
    image = pinnedImage name allowMutableImage image;
    cmd = [ "daemon" ];
    volumes = [
      "${stateDir}:/zeroclaw-data:rw"
      "${configFile}:/zeroclaw-data/.zeroclaw/config.toml:ro"
    ]
    ++ lib.optional workerEnabled "${workerResultsDir}:${workerResultsMount}:ro"
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
    inherit backend pidsLimit;
    name = containerName;
    inherit owner;
    baseConfig = baseContainer;
    overrides = extraContainerConfig;
    allowedWritableSources = [ stateDir ];
    allowedWritableDestinations = [ "/zeroclaw-data" ];
    resources = config.tentaflake.security.resources;
    brokerNetwork = if brokerEnabled then brokerCfg.networkName else null;
    approvedEnvironmentFiles = lib.optional brokerEnabled brokerEnvironmentFile;
    approvedReadOnlySources = lib.optional workerEnabled workerResultsDir;
    approvedReadOnlyDestinations = lib.optional workerEnabled workerResultsMount;
    automaticStart = autoStart;
    brokerPolicyEnabled = brokerEnabled;
    inherit workerEnabled;
    workspaceQuotaEnabled = quotaEnabled;
  };
in
{
  assertions =
    securityResult.assertions
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
        assertion = !workerEnabled || (workerCfg.containerUid == 65534 && workerCfg.containerGid == 65534);
        message = "tentaflake: worker uid/gid for ${containerName} must match 65534:65534.";
      }
      {
        assertion = !quotaEnabled || quotaCfg.workspace == "${stateDir}/data";
        message = "tentaflake: workspaceQuota for ${containerName} must exactly mount ${stateDir}/data.";
      }
      {
        assertion = !quotaEnabled || (quotaCfg.ownerUid == 65534 && quotaCfg.ownerGid == 65534);
        message = "tentaflake: workspaceQuota owner for ${containerName} must match 65534:65534.";
      }
    ];

  systemd.tmpfiles.rules = [
    "d ${stateDir} 0700 65534 65534 -"
    "d ${stateDir}/.zeroclaw 0700 65534 65534 -"
    "d ${stateDir}/.zeroclaw/data 0700 65534 65534 -"
    "d ${stateDir}/data 0700 65534 65534 -"
  ];

  systemd.services =
    lib.optionalAttrs (seedDir != null) {
      "seed-${containerName}" = {
        description = "Seed workspace for ZeroClaw agent ${name}";
        wantedBy = [ serviceName ];
        before = [ serviceName ];
        after = [ "systemd-tmpfiles-setup.service" ];
        serviceConfig = {
          Type = "oneshot";
          RemainAfterExit = true;
          User = "65534";
          Group = "65534";
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
    // lib.optionalAttrs (secure && runtimeDependencies == [ ]) {
      ${serviceAttr} = secureUnitPolicy;
    }
    // lib.optionalAttrs (runtimeDependencies != [ ]) {
      ${serviceAttr} = secureUnitPolicy // {
        requires = runtimeDependencies;
        after = runtimeDependencies;
      };
    };

  virtualisation.oci-containers.containers.${containerName} = securityResult.container // {
    image = pinnedImage name allowMutableImage securityResult.container.image;
  };
}
