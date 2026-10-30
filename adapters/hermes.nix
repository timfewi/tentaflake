{ pkgs, lib }:

# ────────────────────────────────────────────────────────────
# Internal Hermes implementation, selected by adapter = "hermes" in agents.json.
#
# Each agent gets:
#   - System user  agent-<name>
#   - State dir    /var/lib/tentaflake/agents/<name>  (0700, owned by the container uid)
#   - OCI container agent-<name> (balanced: network=none + gVisor)
#   - HERMES_HOME pointing to its isolated state dir
#   - Shared security-profile policy applied after caller overrides
#
# Optional operational hardening (all default-off): UID alignment + ownership
# self-heal, a fail-loud provider preflight, in-container git identity,
# a tailnet-published dashboard, and durable agent-built services.
# Host-side git auto-push is disabled; see docs/07-operations.md.
# ────────────────────────────────────────────────────────────

let
  preset = {
    schemaVersion = 1;
    identity = {
      id = "hermes";
      version = "image-snapshot-2026-07-18";
      status = "compatibility";
    };
    artifact = {
      kind = "oci";
      reference = "docker.io/nousresearch/hermes-agent@sha256:4a2f23bd3ffaa6ee7b3be8a302a38be43ab0321a2988cd3fb16b7dd472dde812";
      reviewed = true;
      source = "docker.io/nousresearch/hermes-agent:latest";
    };
    command = [
      "gateway"
      "run"
      "--replace"
    ];
    configuration = {
      format = "yaml";
      readOnly = true;
    };
    ownership = {
      uid = 10000;
      gid = 10000;
    };
    layout = {
      state = "/var/lib/tentaflake/agents/<name>";
      workspace = "workspace";
      writable = [
        "state"
        "workspace"
      ];
      directories = [
        "workspace"
        "skills"
        "cron"
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
    name: settings: (pkgs.formats.yaml { }).generate "${name}-config.yaml" settings;
  build =
    {
      name,
      stateDir ? hostIdentity.stateDir name,

      # User / group
      user ? "agent-${name}",
      group ? "agent-${name}",
      uid ? null,
      gid ? null,

      # Container image. Default is the official hermes-agent image, digest-pinned
      # in this adapter. Overrides MUST also be digest-pinned — a bare tag is
      # rejected at eval time, because the registry can repoint a tag at different
      # bytes between rebuilds. Example:
      #   image = "ubuntu@sha256:<64 hex digest>";
      image ? preset.artifact.reference,

      # Dev-only escape hatch for locally-built images without a registry digest.
      allowMutableImage ? false,

      # Path to an env file (plaintext .env) on the host filesystem, e.g.:
      #   envFile = "/run/tentaflake/default.env";
      # The file is passed to Docker via --env-file and loaded at container start.
      # Dev-only compatibility path. Secure profiles reject real agent credentials.
      envFile ? null,

      # Path to an agenix-decrypted env file, e.g.:
      #   agenixFile = "/run/agenix/<name>-env";
      # Dev-only compatibility path. Secure profiles reject real agent credentials.
      agenixFile ? null,

      # Optional: path to a directory of base reference files (SOUL.md, AGENTS.md, BRAND.md, skills/)
      # On first boot, contents are copied into stateDir (no-clobber — Hermes' runtime modifications
      # are preserved across rebuilds). Set to null (default) to skip.
      seedDir ? null,

      # Extra volumes to mount into the container
      extraVolumes ? [ ],

      # Extra environment variables to set in the container
      extraEnvironment ? { },

      # Container command. Default: `gateway run --replace` (the image's entrypoint
      # is `hermes`, so these are appended to it). Leave null to use the default,
      # which becomes a dashboard+gateway bootstrap automatically when `dashboard`
      # is set.
      cmd ? null,

      # Compatibility override for the dev profile. Secure profiles select
      # network=none until the brokered network from Phase B is configured.
      networkMode ? null,

      # Max number of processes in the container (--pids-limit). Caps fork bombs
      # without starving real work — agents compile code, so 200 would be too
      # tight; 512 is a sane ceiling. Set to null to skip the flag (unlimited).
      pidsLimit ? 512,

      # Auto-start container with system
      autoStart ? true,

      # Create system user and group (disable if managing externally)
      createUser ? true,

      # Extra OCI container config options (merged into the container attrset)
      extraContainerConfig ? { },

      # Hermes YAML/JSON configuration — serialized to config.yaml at build time
      # and mounted read-only at $HERMES_HOME/config.yaml inside the container.
      # Set to `null` (default) to skip and use Hermes' built-in defaults.
      #
      # NOTE: config.yaml is mounted READ-ONLY by design (config is declarative —
      # edit it here + rebuild, not via the dashboard). The dashboard's "save
      # config" button will return a 500 against this read-only file; that is
      # expected. See docs/07-operations.md.
      settings ? null,

      # ── Operational hardening (all optional, default-off) ───────────────────────

      # UID/GID the container process runs as (the image's internal `hermes` user).
      # State dirs + mounted data are owned by this so the container can write them.
      containerUid ? preset.ownership.uid,
      containerGid ? preset.ownership.gid,

      # Extra host directories (e.g. mounted data slices) to chown to the container
      # uid on every boot, so rebuilds *heal* ownership instead of breaking writes.
      healDataDirs ? [ ],

      # Configure git identity inside the container. Re-applied on every boot, so it
      # survives container recreate (which wipes the ephemeral in-container $HOME).
      # null = skip. Example:
      #   gitIdentity = { name = "myagent"; email = "myagent@example.com"; };
      gitIdentity ? null,

      # Expose the agent's web dashboard. Launches `hermes dashboard` inside the
      # container and (optionally) publishes it on the tailnet via `tailscale serve`.
      # null = skip. Example:
      #   dashboard = { port = 9219; tailnetPort = 9119; };
      # `port` is the in-container bind port; `tailnetPort` (optional) is the external
      # HTTPS port `tailscale serve` listens on. Dev-only: secure profiles reject
      # dashboard and service publication.
      dashboard ? null,

      # Expose additional agent-built web services durably (survive restart/recreate)
      # and optionally publish them on the tailnet. Attrset of name -> definition.
      # Each runs inside the container with auto-restart. Example:
      #   services.knowledge-base = {
      #     startCommand = "cd $HERMES_HOME/workspace/kb && exec ./.venv/bin/python app.py";
      #     tailnetPort = 9122;            # optional: publish on the tailnet
      #     port = 9191;                   # optional: in-container port (for serve target)
      #   };
      services ? { },

      # Fail-loud provider preflight: POST a 1-token completion to the model endpoint
      # at boot and log a clear PASS/FAIL with the HTTP status — so a misconfigured
      # base_url / API key surfaces as an obvious auth error in the journal instead of
      # as downstream "agent crashed / protocol violation" noise. null = skip. Example:
      #   providerHealthcheck = {
      #     url = "https://api.example.com/v1";   # the model base_url
      #     model = "my-model";
      #     apiKeyEnv = "MY_API_KEY";             # env var name (value stays in the container)
      #   };
      providerHealthcheck ? null,
    }:

    { config, ... }:
    let
      # Container runtime binary (docker or podman, per oci-containers backend)
      backend = config.virtualisation.oci-containers.backend;
      securityProfile = config.tentaflake.security.profile;
      secure = containerSecurity.isSecure securityProfile;
      containerName = hostIdentity.container name;
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
        adapter = "hermes";
      };
      runtimeDependencies = brokerUnits ++ lib.optional quotaEnabled quotaUnit ++ research.units;
      secureUnitPolicy = import ../lib/serviceRecovery.nix;
      securityResources = config.tentaflake.security.resources;
      ctrBin = "${pkgs.${backend}}/bin/${backend}";

      ownUid = toString containerUid;
      ownGid = toString containerGid;

      # `docker exec` defaults to UID 0. Anything it writes into $HERMES_HOME then
      # lands root-owned and the agent (uid ${ownUid}) can no longer read/write it —
      # e.g. a root-owned cron/jobs.json or a 0555 profile dir breaks the scheduler.
      # Always exec as the agent uid so in-container writes stay agent-owned.
      ctrExec = "${ctrBin} exec -u ${ownUid}:${ownGid}";

      ctrService = "${backend}-agent-${name}.service";
      ctrServiceAttr = lib.removeSuffix ".service" ctrService;

      # Generate config.yaml derivation when settings are provided
      configYaml =
        if (research.settings != null && research.settings != { }) then
          generateConfiguration containerName research.settings
        else
          null;

      # Resolve the container command. When `dashboard` is set and the caller didn't
      # override `cmd`, the dashboard is launched as a separate post-start unit (see
      # below) rather than mangling the entrypoint, so the default command stands.
      resolvedCmd = if cmd != null then cmd else preset.command;

      # Directories whose ownership we keep aligned to the container uid each boot.
      healDirs = [ stateDir ] ++ healDataDirs;

      # ── Optional service fragments (merged into systemd.services below) ──────────

      seedSvc = lib.optionalAttrs (seedDir != null) {
        "agent-${name}-seed" = {
          description = "Seed base files for Hermes agent ${name}";
          requires = [
            "local-fs.target"
            "systemd-tmpfiles-setup.service"
            "agent-${name}-heal-uid.service"
          ]
          ++ lib.optional quotaEnabled quotaUnit;
          after = [
            "systemd-tmpfiles-setup.service"
            "agent-${name}-heal-uid.service"
          ]
          ++ lib.optional quotaEnabled quotaUnit;
          before = [ ctrService ];
          wantedBy = [ ctrService ];
          serviceConfig = {
            Type = "oneshot";
            RemainAfterExit = true;
            User = ownUid;
            Group = ownGid;
            UMask = "0077";
          };
          script = ''
            ${pkgs.coreutils}/bin/cp -rn --no-preserve=mode,ownership \
              ${lib.escapeShellArg "${seedDir}/."} ${lib.escapeShellArg "${stateDir}/"}
            ${pkgs.coreutils}/bin/chmod -R u+rwX ${lib.escapeShellArg stateDir}
          '';
        };
      };

      # UID self-heal: own state + data dirs to the container uid before the
      # container starts, so a rebuild that re-creates dirs (or a uid drift) heals
      # rather than breaking $HERMES_HOME writes.
      healSvc = {
        "agent-${name}-heal-uid" = {
          description = "Align Hermes ${name} state + data dirs to the container uid (${ownUid})";
          requires = [ "systemd-tmpfiles-setup.service" ] ++ lib.optional quotaEnabled quotaUnit;
          after = [ "systemd-tmpfiles-setup.service" ] ++ lib.optional quotaEnabled quotaUnit;
          before = [ ctrService ];
          wantedBy = [ ctrService ];
          path = [ pkgs.coreutils ];
          serviceConfig = {
            Type = "oneshot";
            RemainAfterExit = true;
          };
          script = ''
            for d in ${lib.escapeShellArgs healDirs}; do
              [ ! -e "$d" ] || chown -hR ${ownUid}:${ownGid} "$d"
            done
          '';
        };
      };

      gitIdentitySvc = lib.optionalAttrs (gitIdentity != null) {
        "agent-${name}-git-identity" = {
          description = "Configure git identity inside Hermes ${name}";
          after = [ ctrService ];
          requires = [ ctrService ];
          wantedBy = [ ctrService ];
          serviceConfig = {
            Type = "oneshot";
            RemainAfterExit = true;
          };
          script = ''
            ${ctrExec} agent-${name} git config --global user.name ${lib.escapeShellArg gitIdentity.name}
            ${ctrExec} agent-${name} git config --global user.email ${lib.escapeShellArg gitIdentity.email}
            ${ctrExec} agent-${name} git config --global init.defaultBranch main
            ${ctrExec} agent-${name} git config --global push.autoSetupRemote true
          '';
        };
      };

      # Launch `hermes dashboard` inside the container, auto-restarting (a foreground
      # docker exec under Type=simple dies when the container restarts → systemd
      # restarts it once the container is back).
      dashboardSvc = lib.optionalAttrs (dashboard != null) {
        "agent-${name}-dashboard" = {
          description = "Hermes ${name} web dashboard (port ${toString dashboard.port})";
          after = [ ctrService ];
          requires = [ ctrService ];
          wantedBy = [ "multi-user.target" ];
          serviceConfig = {
            Type = "simple";
            Restart = "always";
            RestartSec = 10;
            ExecStart = "${ctrExec} agent-${name} sh -c 'exec hermes dashboard --host 0.0.0.0 --port ${toString dashboard.port} --no-open --skip-build'";
          };
        };
      };

      # Durable agent-built services (e.g. a knowledge-base web app the agent built).
      agentServicesSvc = lib.mapAttrs' (
        svcName: def:
        lib.nameValuePair "agent-${name}-svc-${svcName}" {
          description = "Hermes ${name} service: ${svcName}";
          after = [ ctrService ];
          requires = [ ctrService ];
          wantedBy = [ "multi-user.target" ];
          serviceConfig = {
            Type = "simple";
            Restart = "always";
            RestartSec = 10;
            ExecStart = "${ctrExec} agent-${name} sh -c ${lib.escapeShellArg def.startCommand}";
          };
        }
      ) services;

      providerHealthcheckSvc = lib.optionalAttrs (providerHealthcheck != null) {
        "agent-${name}-provider-healthcheck" = {
          description = "Provider preflight for Hermes ${name} (fail-loud auth/endpoint check)";
          after = [ ctrService ];
          requires = [ ctrService ];
          wantedBy = [ "multi-user.target" ];
          serviceConfig = {
            Type = "oneshot";
            RemainAfterExit = true;
          };
          script = ''
            ${ctrExec} agent-${name} sh -c '
              code=$(curl -s -o /dev/null -w "%{http_code}" \
                -X POST "${providerHealthcheck.url}/chat/completions" \
                -H "Authorization: Bearer ''$${providerHealthcheck.apiKeyEnv}" \
                -H "Content-Type: application/json" \
                -d "{\"model\":\"${providerHealthcheck.model}\",\"messages\":[{\"role\":\"user\",\"content\":\"ping\"}],\"max_tokens\":1}")
              if [ "$code" = "200" ]; then
                echo "[provider-healthcheck] ${name}: OK (${providerHealthcheck.url})"
              else
                echo "[provider-healthcheck] ${name}: FAIL HTTP $code against ${providerHealthcheck.url} — check model.base_url and ${providerHealthcheck.apiKeyEnv}" >&2
              fi
            ' || echo "[provider-healthcheck] ${name}: could not run preflight (container not ready?)" >&2
          '';
        };
      };

      # ── tailscale serve fragments (one unit per published port) ──────────────────
      serveUnit = unitName: extPort: intPort: {
        "${unitName}" = {
          description = "tailscale serve for ${unitName} (:${toString extPort})";
          wantedBy = [ "multi-user.target" ];
          after = [
            "tailscaled.service"
            ctrService
          ];
          wants = [ "tailscaled.service" ];
          serviceConfig = {
            Type = "oneshot";
            RemainAfterExit = true;
          };
          script = ''
            ${pkgs.tailscale}/bin/tailscale serve --bg --https=${toString extPort} 127.0.0.1:${toString intPort} || true
          '';
        };
      };

      dashboardServe =
        if (dashboard != null && (dashboard.tailnetPort or null) != null) then
          serveUnit "agent-${name}-dashboard-serve" dashboard.tailnetPort dashboard.port
        else
          { };

      servicesServe = lib.foldl' (
        acc: svcName:
        acc
        // (
          let
            def = services.${svcName};
          in
          if (def.tailnetPort or null) != null then
            serveUnit "agent-${name}-svc-${svcName}-serve" def.tailnetPort (def.port or def.tailnetPort)
          else
            { }
        )
      ) { } (lib.attrNames services);

      # The configurable native home is independent of the host storage layout.
      nativeStateDir = "/state";
      baseContainer = {
        inherit autoStart;
        networks = lib.optional brokerEnabled brokerCfg.networkName;
        inherit image;
        cmd = resolvedCmd;
        volumes = [
          "${stateDir}:${nativeStateDir}:rw"
        ]
        ++ lib.optional workerEnabled "${workerResultsDir}:${workerResultsMount}:ro"
        ++ lib.optional (configYaml != null) "${configYaml}:${nativeStateDir}/config.yaml:ro"
        ++ research.volumes
        ++ extraVolumes;
        environment = {
          HERMES_HOME = nativeStateDir;
          HERMES_AGENT_NAME = name;
        }
        // extraEnvironment;
        environmentFiles = lib.optional brokerEnabled brokerEnvironmentFile;
        extraOptions =
          lib.optional (!secure) "--network=${if networkMode == null then "host" else networkMode}"
          ++ lib.optional (!secure && envFile != null) "--env-file=${envFile}"
          ++ lib.optional (!secure && agenixFile != null) "--env-file=${agenixFile}"
          ++ lib.optional (!secure) "--security-opt=no-new-privileges:true"
          ++ lib.optional (!secure && pidsLimit != null) "--pids-limit=${toString pidsLimit}";
      };

      securityResult = containerSecurity.apply {
        profile = securityProfile;
        inherit backend pidsLimit allowMutableImage;
        name = "agent-${name}";
        owner = "${ownUid}:${ownGid}";
        baseConfig = baseContainer;
        overrides = extraContainerConfig;
        allowedWritableSources = [ stateDir ];
        allowedWritableDestinations = [ nativeStateDir ];
        resources = securityResources;
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
            message = "tentaflake: secure Hermes agent ${name} requires a digest-pinned image; allowMutableImage is dev-only.";
          }
          {
            assertion = networkMode == null || networkMode == "none";
            message = "tentaflake: secure Hermes agent ${name} may not override networkMode; brokered networking is not configured yet.";
          }
          {
            assertion = envFile == null && agenixFile == null;
            message = "tentaflake: secure Hermes agent ${name} may not receive an env-file with real credentials; use the broker path once configured.";
          }
          {
            assertion = !(containerSecurity.containsSensitiveValue extraEnvironment);
            message = "tentaflake: secure Hermes agent ${name} has a secret-like extraEnvironment key, which would enter the Nix store.";
          }
          {
            assertion =
              !(containerSecurity.containsSensitiveValue (if settings == null then { } else settings));
            message = "tentaflake: secure Hermes agent ${name} has a secret-like settings key, which would enter the Nix store.";
          }
          {
            assertion = dashboard == null && services == { };
            message = "tentaflake: secure Hermes agent ${name} may not publish dashboards or agent-built services before an authenticated broker boundary exists.";
          }
          {
            assertion = providerHealthcheck == null;
            message = "tentaflake: secure Hermes agent ${name} may not run a provider check with a real provider credential inside the container.";
          }
          {
            assertion = healDataDirs == [ ];
            message = "tentaflake: secure Hermes agent ${name} may not recursively re-own caller-supplied host paths through healDataDirs.";
          }
          {
            assertion = !workerEnabled || workerCfg.workspace == "${stateDir}/workspace";
            message = "tentaflake: worker workspace for ${containerName} must exactly match ${stateDir}/workspace.";
          }
          {
            assertion =
              !workerEnabled
              || (workerCfg.containerUid == containerUid && workerCfg.containerGid == containerGid);
            message = "tentaflake: worker uid/gid for ${containerName} must match the controller container.";
          }
          {
            assertion = !quotaEnabled || quotaCfg.workspace == "${stateDir}/workspace";
            message = "tentaflake: workspaceQuota for ${containerName} must exactly mount ${stateDir}/workspace.";
          }
          {
            assertion =
              !quotaEnabled || (quotaCfg.ownerUid == containerUid && quotaCfg.ownerGid == containerGid);
            message = "tentaflake: workspaceQuota owner for ${containerName} must match the controller container.";
          }
        ];

      # ── System user ──
      users.users = lib.mkIf createUser {
        ${user} = {
          isSystemUser = true;
          inherit group;
          home = stateDir;
          createHome = false;
          inherit uid;
          description = "Hermes agent ${name}";
          shell = "${pkgs.bash}/bin/bash";
        };
      };

      users.groups = lib.mkIf createUser {
        # Always define the group (the system user's primary group must exist);
        # only pin an explicit gid when one was provided.
        ${group} = lib.optionalAttrs (gid != null) { inherit gid; };
      };

      # ── tmpfiles — create state directories owned by the container uid ──
      # (numeric uid/gid: the container runs as the image's `hermes` user, not the
      # host system user, so dirs must be owned by that uid for writes to succeed)
      systemd = {
        tmpfiles.rules = [
          "d ${stateDir} 0700 ${ownUid} ${ownGid} -"
        ]
        ++ lib.optionals (!stateQuotaEnabled) (
          map (directory: "d ${stateDir}/${directory} 0700 ${ownUid} ${ownGid} -") preset.layout.directories
        );

        # ── systemd services ──
        services = lib.mkMerge [
          healSvc
          seedSvc
          gitIdentitySvc
          dashboardSvc
          agentServicesSvc
          providerHealthcheckSvc
          dashboardServe
          servicesServe
          (lib.optionalAttrs secure {
            ${ctrServiceAttr} = secureUnitPolicy;
          })
          (lib.optionalAttrs (runtimeDependencies != [ ]) {
            ${ctrServiceAttr} = {
              requires = runtimeDependencies;
              after = runtimeDependencies;
            };
          })
        ];
      };

      virtualisation.oci-containers.containers."agent-${name}" = securityResult.container;
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
        agent.disabled_toolsets = lib.unique (
          (original.agent.disabled_toolsets or [ ])
          ++ [
            "web"
            "browser"
          ]
        );
        mcp_servers.secure-research-tool = server // {
          keepalive_interval = 20;
        };
      };
      valid = lib.all (entry: builtins.isAttrs entry && entry ? command && !(entry ? url)) (
        lib.attrValues (original.mcp_servers or { })
      );
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
      adapter = "hermes";
      inherit (args) name;
      inherit container stateDir;
      stateStorage = stateDir;
      stateDirectories = preset.layout.directories;
      unit = "${backend}-${container}.service";
      workspace = "${stateDir}/workspace";
      uid = args.containerUid or preset.ownership.uid;
      gid = args.containerGid or preset.ownership.gid;
      runnable = true;
    };
}
