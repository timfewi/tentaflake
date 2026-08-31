{ pkgs, lib }:

# ────────────────────────────────────────────────────────────
# mkHermesAgent — create one isolated Hermes agent as a NixOS module
#
# Usage in my-agents.nix:
#
#   { mkHermesAgent }:
#   [
#     (mkHermesAgent {
#       name    = "coding";
#       autoStart = false;
#     })
#   ]
#
# Each agent gets:
#   - System user  hermes-<name>
#   - State dir    /var/lib/hermes-<name>  (0700, owned by the container uid)
#   - OCI container hermes-<name> (balanced: network=none + gVisor)
#   - HERMES_HOME pointing to its isolated state dir
#   - Shared security-profile policy applied after caller overrides
#
# Optional operational hardening (all default-off): UID alignment + ownership
# self-heal, a fail-loud provider preflight, in-container git identity,
# host-side secret-safe git auto-push, a tailnet-published dashboard, and
# durable agent-built services. See the argument block below and docs/.
# ────────────────────────────────────────────────────────────

let
  constants = import ./constants.nix;
  pinnedImage = import ./pinnedImage.nix { inherit lib; };
  containerSecurity = import ./containerSecurity.nix { inherit lib; };
in
{
  name,
  stateDir ? "/var/lib/hermes-${name}",

  # User / group
  user ? "hermes-${name}",
  group ? "hermes-${name}",
  uid ? null,
  gid ? null,

  # Container image. Default is the official hermes-agent image, digest-pinned
  # in lib/constants.nix. Overrides MUST also be digest-pinned — a bare tag is
  # rejected at eval time, because the registry can repoint a tag at different
  # bytes between rebuilds. Example:
  #   image = "ubuntu@sha256:<64 hex digest>";
  image ? constants.hermesImage,

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
  containerUid ? constants.containerUid,
  containerGid ? constants.containerGid,

  # Extra host directories (e.g. mounted data slices) to chown to the container
  # uid on every boot, so rebuilds *heal* ownership instead of breaking writes.
  healDataDirs ? [ ],

  # Configure git identity inside the container. Re-applied on every boot, so it
  # survives container recreate (which wipes the ephemeral in-container $HOME).
  # null = skip. Example:
  #   gitIdentity = { name = "myagent"; email = "myagent@example.com"; };
  gitIdentity ? null,

  # Push the agent's git repos to their remotes from the HOST, using a token the
  # agent never sees (Hermes strips secrets from the agent terminal by design,
  # so the agent commits locally and the host pushes). Targets GitHub https
  # remotes. null = skip. Example:
  #   gitAutoPush = { tokenEnvFile = "/run/agenix/hermes-<name>-env"; };
  # Required: tokenEnvFile and allowedRemotes. Optional: allowedBranches
  # (default [ "main" ]), reposRoot, tokenEnvVar, and interval.
  gitAutoPush ? null,

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
  containerName = "hermes-${name}";
  brokerCfg = lib.attrByPath [ containerName ] null config.tentaflake.broker.agents;
  brokerEnabled = brokerCfg != null && brokerCfg.enable;
  workerCfg = lib.attrByPath [ containerName ] null config.tentaflake.worker.agents;
  workerEnabled = workerCfg != null && workerCfg.enable;
  workerResultsDir = "/var/lib/tentaflake-worker-${containerName}/results";
  workerResultsMount = "/run/tentaflake-worker/results";
  quotaCfg = lib.attrByPath [ containerName ] null config.tentaflake.workspaceQuota.agents;
  quotaEnabled = quotaCfg != null && quotaCfg.enable;
  quotaUnit = "tentaflake-workspace-quota-${containerName}.service";
  workerStateUnit = "tentaflake-worker-state-${containerName}.service";
  brokerEnvironmentFile = "/run/tentaflake-broker/${containerName}/agent.env";
  brokerUnits =
    lib.optional brokerEnabled "tentaflake-broker-network-${containerName}.service"
    ++ lib.optional (
      brokerEnabled && brokerCfg.llm.enable
    ) "tentaflake-broker-llm-${containerName}.service"
    ++ lib.optional (
      brokerEnabled && brokerCfg.fetch.enable
    ) "tentaflake-broker-fetch-${containerName}.service";
  runtimeDependencies =
    brokerUnits
    ++ lib.optional quotaEnabled quotaUnit
    ++ lib.optional workerEnabled workerStateUnit;
  secureUnitPolicy = {
    startLimitIntervalSec = 300;
    startLimitBurst = 5;
    serviceConfig.RestartSec = "10s";
  };
  securityResources = config.tentaflake.security.resources;
  ctrBin = "${pkgs.${backend}}/bin/${backend}";
  tentaflakeCli = pkgs.callPackage ../pkgs/tentaflake-cli { };

  ownUid = toString containerUid;
  ownGid = toString containerGid;

  # `docker exec` defaults to UID 0. Anything it writes into $HERMES_HOME then
  # lands root-owned and the agent (uid ${ownUid}) can no longer read/write it —
  # e.g. a root-owned cron/jobs.json or a 0555 profile dir breaks the scheduler.
  # Always exec as the agent uid so in-container writes stay agent-owned.
  ctrExec = "${ctrBin} exec -u ${ownUid}:${ownGid}";

  ctrService = "${backend}-hermes-${name}.service";
  ctrServiceAttr = lib.removeSuffix ".service" ctrService;

  # Generate config.yaml derivation when settings are provided
  yamlFormat = pkgs.formats.yaml { };
  configYaml =
    if (settings != null && settings != { }) then
      yamlFormat.generate "hermes-${name}-config.yaml" settings
    else
      null;

  # Resolve the container command. When `dashboard` is set and the caller didn't
  # override `cmd`, the dashboard is launched as a separate post-start unit (see
  # below) rather than mangling the entrypoint, so the default command stands.
  resolvedCmd =
    if cmd != null then
      cmd
    else
      [
        "gateway"
        "run"
        "--replace"
      ];

  # Directories whose ownership we keep aligned to the container uid each boot.
  healDirs = [ stateDir ] ++ healDataDirs;

  # ── Optional service fragments (merged into systemd.services below) ──────────

  seedSvc = lib.optionalAttrs (seedDir != null) {
    "seed-hermes-${name}" = {
      description = "Seed base files for Hermes agent ${name}";
      requires = [ "local-fs.target" ];
      after = [
        "tmpfiles-setup.service"
        "hermes-${name}-heal-uid.service"
      ];
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
    "hermes-${name}-heal-uid" = {
      description = "Align Hermes ${name} state + data dirs to the container uid (${ownUid})";
      after = [ "tmpfiles-setup.service" ];
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
    "hermes-${name}-git-identity" = {
      description = "Configure git identity inside Hermes ${name}";
      after = [ ctrService ];
      requires = [ ctrService ];
      wantedBy = [ ctrService ];
      serviceConfig = {
        Type = "oneshot";
        RemainAfterExit = true;
      };
      script = ''
        ${ctrExec} hermes-${name} git config --global user.name ${lib.escapeShellArg gitIdentity.name}
        ${ctrExec} hermes-${name} git config --global user.email ${lib.escapeShellArg gitIdentity.email}
        ${ctrExec} hermes-${name} git config --global init.defaultBranch main
        ${ctrExec} hermes-${name} git config --global push.autoSetupRemote true
      '';
    };
  };

  autopushSvc = lib.optionalAttrs (gitAutoPush != null) (
    let
      reposRoot = gitAutoPush.reposRoot or "${stateDir}/workspace";
      tokenEnvVar = gitAutoPush.tokenEnvVar or "GH_TOKEN";
      allowedRemotes = gitAutoPush.allowedRemotes or [ ];
      allowedBranches = gitAutoPush.allowedBranches or [ "main" ];
    in
    {
      "hermes-${name}-autopush" = {
        description = "Push Hermes ${name} git repos to their GitHub remotes";
        after = [ ctrService ];
        path = [
          pkgs.git
          pkgs.findutils
          pkgs.coreutils
        ];
        serviceConfig = {
          Type = "oneshot";
          EnvironmentFile = gitAutoPush.tokenEnvFile or "/dev/null";
        };
        script = ''
          root=${lib.escapeShellArg reposRoot}
          [ -d "$root" ] || exit 0
          helper='!f() { echo username=x-access-token; echo "password=''$${tokenEnvVar}"; }; f'
          rc=0
          while IFS= read -r -d "" gd; do
            repo=$(dirname "$gd")
            url=$(git -C "$repo" -c safe.directory="$repo" remote get-url origin 2>/dev/null) || {
              printf 'git-autopush DENY agent=%q repo=%q reason=no-origin\n' ${lib.escapeShellArg name} "$repo" >&2
              rc=1
              continue
            }
            if ! ${lib.getExe tentaflakeCli} remote-check "$url" ${lib.escapeShellArgs allowedRemotes}; then
              printf 'git-autopush DENY agent=%q repo=%q remote=%q reason=remote-policy\n' ${lib.escapeShellArg name} "$repo" "$url" >&2
              rc=1
              continue
            fi
            branch=$(git -C "$repo" -c safe.directory="$repo" symbolic-ref --quiet --short HEAD) || {
              printf 'git-autopush DENY agent=%q repo=%q reason=detached-head\n' ${lib.escapeShellArg name} "$repo" >&2
              rc=1
              continue
            }
            branch_ok=false
            for allowed_branch in ${lib.escapeShellArgs allowedBranches}; do
              if [ "$branch" = "$allowed_branch" ]; then
                branch_ok=true
                break
              fi
            done
            if [ "$branch_ok" != true ]; then
              printf 'git-autopush DENY agent=%q repo=%q branch=%q reason=branch-policy\n' ${lib.escapeShellArg name} "$repo" "$branch" >&2
              rc=1
              continue
            fi
            printf 'git-autopush ALLOW agent=%q repo=%q remote=%q branch=%q\n' ${lib.escapeShellArg name} "$repo" "$url" "$branch"
            if ! git -C "$repo" -c safe.directory="$repo" \
              -c credential.helper="$helper" push origin "HEAD:refs/heads/$branch"; then
              printf 'git-autopush ERROR agent=%q repo=%q branch=%q reason=push-failed\n' ${lib.escapeShellArg name} "$repo" "$branch" >&2
              rc=1
            fi
          done < <(find "$root" -maxdepth 4 -name .git \( -type d -o -type f \) -print0 2>/dev/null)
          exit "$rc"
        '';
      };
    }
  );

  # Launch `hermes dashboard` inside the container, auto-restarting (a foreground
  # docker exec under Type=simple dies when the container restarts → systemd
  # restarts it once the container is back).
  dashboardSvc = lib.optionalAttrs (dashboard != null) {
    "hermes-${name}-dashboard" = {
      description = "Hermes ${name} web dashboard (port ${toString dashboard.port})";
      after = [ ctrService ];
      requires = [ ctrService ];
      wantedBy = [ "multi-user.target" ];
      serviceConfig = {
        Type = "simple";
        Restart = "always";
        RestartSec = 10;
        ExecStart = "${ctrExec} hermes-${name} sh -c 'exec hermes dashboard --host 0.0.0.0 --port ${toString dashboard.port} --no-open --skip-build'";
      };
    };
  };

  # Durable agent-built services (e.g. a knowledge-base web app the agent built).
  agentServicesSvc = lib.mapAttrs' (
    svcName: def:
    lib.nameValuePair "hermes-${name}-svc-${svcName}" {
      description = "Hermes ${name} service: ${svcName}";
      after = [ ctrService ];
      requires = [ ctrService ];
      wantedBy = [ "multi-user.target" ];
      serviceConfig = {
        Type = "simple";
        Restart = "always";
        RestartSec = 10;
        ExecStart = "${ctrExec} hermes-${name} sh -c ${lib.escapeShellArg def.startCommand}";
      };
    }
  ) services;

  providerHealthcheckSvc = lib.optionalAttrs (providerHealthcheck != null) {
    "hermes-${name}-provider-healthcheck" = {
      description = "Provider preflight for Hermes ${name} (fail-loud auth/endpoint check)";
      after = [ ctrService ];
      requires = [ ctrService ];
      wantedBy = [ "multi-user.target" ];
      serviceConfig = {
        Type = "oneshot";
        RemainAfterExit = true;
      };
      script = ''
        ${ctrExec} hermes-${name} sh -c '
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
      serveUnit "hermes-${name}-dashboard-serve" dashboard.tailnetPort dashboard.port
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
        serveUnit "hermes-${name}-svc-${svcName}-serve" def.tailnetPort (def.port or def.tailnetPort)
      else
        { }
    )
  ) { } (lib.attrNames services);

  baseContainer = {
    inherit autoStart;
    networks = lib.optional brokerEnabled brokerCfg.networkName;
    image = pinnedImage name allowMutableImage image;
    cmd = resolvedCmd;
    volumes = [
      "${stateDir}:${stateDir}:rw"
    ]
    ++ lib.optional workerEnabled "${workerResultsDir}:${workerResultsMount}:ro"
    ++ lib.optional (configYaml != null) "${configYaml}:${stateDir}/config.yaml:ro"
    ++ extraVolumes;
    environment = {
      HERMES_HOME = stateDir;
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
    inherit backend pidsLimit;
    name = "hermes-${name}";
    owner = "${ownUid}:${ownGid}";
    baseConfig = baseContainer;
    overrides = extraContainerConfig;
    allowedWritableSources = [ stateDir ];
    allowedWritableDestinations = [ stateDir ];
    resources = securityResources;
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
    ++ lib.optionals (gitAutoPush != null) [
      {
        assertion = gitAutoPush ? tokenEnvFile;
        message = "tentaflake: Hermes agent ${name} gitAutoPush requires tokenEnvFile pointing to a runtime-only host credential file.";
      }
      {
        assertion = (gitAutoPush.allowedRemotes or [ ]) != [ ];
        message = "tentaflake: Hermes agent ${name} gitAutoPush requires a non-empty allowedRemotes list of full canonical HTTPS repository URLs.";
      }
      {
        assertion = (gitAutoPush.allowedBranches or [ "main" ]) != [ ];
        message = "tentaflake: Hermes agent ${name} gitAutoPush requires at least one allowed branch.";
      }
      {
        assertion = lib.match "^[A-Z_][A-Z0-9_]*$" (gitAutoPush.tokenEnvVar or "GH_TOKEN") != null;
        message = "tentaflake: Hermes agent ${name} gitAutoPush tokenEnvVar is not a safe environment variable name.";
      }
      {
        assertion = lib.match "^/run/[A-Za-z0-9._+/-]+$" (gitAutoPush.tokenEnvFile or "") != null;
        message = "tentaflake: Hermes agent ${name} gitAutoPush tokenEnvFile must be an absolute, shell-safe runtime path below /run.";
      }
      {
        assertion =
          let
            workspaceRoot = "${stateDir}/workspace";
            reposRoot = gitAutoPush.reposRoot or workspaceRoot;
          in
          reposRoot == workspaceRoot || lib.hasPrefix "${workspaceRoot}/" reposRoot;
        message = "tentaflake: Hermes agent ${name} gitAutoPush reposRoot must stay inside its own workspace.";
      }
      {
        assertion = lib.all (
          remote:
          lib.match "^https://github[.]com/[A-Za-z0-9._-]+/[A-Za-z0-9._-]+([.]git)?$" remote != null
          && !(lib.hasSuffix ".git.git" remote)
        ) (gitAutoPush.allowedRemotes or [ ]);
        message = "tentaflake: Hermes agent ${name} gitAutoPush allowedRemotes must contain canonical github.com HTTPS repository URLs.";
      }
    ]
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
      "d ${stateDir}/workspace 0700 ${ownUid} ${ownGid} -"
      "d ${stateDir}/skills 0700 ${ownUid} ${ownGid} -"
      "d ${stateDir}/cron 0700 ${ownUid} ${ownGid} -"
    ];

    # ── systemd services ──
    services = lib.mkMerge [
      healSvc
      seedSvc
      gitIdentitySvc
      autopushSvc
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
          bindsTo = lib.optional workerEnabled workerStateUnit;
        };
      })
    ];

    # ── systemd timers (git auto-push) ──
    timers = lib.mkMerge [
      (lib.optionalAttrs (gitAutoPush != null) {
        "hermes-${name}-autopush" = {
          wantedBy = [ "timers.target" ];
          timerConfig = {
            OnBootSec = "3min";
            OnUnitActiveSec = gitAutoPush.interval or "2min";
          };
        };
      })
    ];
  };

  virtualisation.oci-containers.containers."hermes-${name}" = securityResult.container // {
    image = pinnedImage name allowMutableImage securityResult.container.image;
  };
}
