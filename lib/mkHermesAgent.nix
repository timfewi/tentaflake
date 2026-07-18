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
#       envFile = "/run/secrets/hermes-coding.env";
#     })
#   ]
#
# Each agent gets:
#   - System user  hermes-<name>
#   - State dir    /var/lib/hermes-<name>  (0700, owned by the container uid)
#   - Docker container  hermes-<name>      (host networking, auto-start)
#   - HERMES_HOME pointing to its isolated state dir
#   - Hardened container defaults: --security-opt=no-new-privileges:true
#     (no setuid/setgid privilege escalation inside the container) and
#     --pids-limit=512 (fork-bomb ceiling; tune or disable via `pidsLimit`)
#
# Optional operational hardening (all default-off): UID alignment + ownership
# self-heal, a fail-loud provider preflight, in-container git identity,
# host-side secret-safe git auto-push, a tailnet-published dashboard, and
# durable agent-built services. See the argument block below and docs/.
# ────────────────────────────────────────────────────────────

let
  constants = import ./constants.nix;
in
{
  name,
  stateDir ? "/var/lib/hermes-${name}",

  # User / group
  user ? "hermes-${name}",
  group ? "hermes-${name}",
  uid ? null,
  gid ? null,

  # Container image. The default is pinned to an exact multi-platform manifest.
  # Update the digest deliberately when upgrading Hermes.
  # Override with any OCI-compatible image, e.g.:
  #   image = "ubuntu:24.04";
  # You can then install hermes-agent inside via a Dockerfile.
  image ? "docker.io/nousresearch/hermes-agent:latest@sha256:4a2f23bd3ffaa6ee7b3be8a302a38be43ab0321a2988cd3fb16b7dd472dde812",

  # Path to an env file (plaintext .env) on the host filesystem, e.g.:
  #   envFile = "/run/tentaflake/default.env";
  # The file is passed to Docker via --env-file and loaded at container start.
  # Useful for live ISO / non-agenix setups where secrets are on tmpfs.
  envFile ? null,

  # Path to an agenix-decrypted env file, e.g.:
  #   agenixFile = "/run/agenix/<name>-env";
  # The file is passed to Docker via --env-file and loaded at container start.
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

  # Network mode for the container. "host" shares host networking.
  # "bridge" isolates the container (requires port mappings in extraContainerConfig).
  networkMode ? "host",

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
  # Optional fields: reposRoot (default "<stateDir>/workspace"),
  # tokenEnvVar (default "GH_TOKEN"), interval (default "2min").
  gitAutoPush ? null,

  # Expose the agent's web dashboard. Launches `hermes dashboard` inside the
  # container and (optionally) publishes it on the tailnet via `tailscale serve`.
  # null = skip. Example:
  #   dashboard = { port = 9219; tailnetPort = 9119; };
  # `port` is the in-container bind port; `tailnetPort` (optional) is the external
  # HTTPS port `tailscale serve` listens on. Under host networking keep them
  # different to avoid a bind collision (the convention is internal = external+100).
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
  ctrBin = "${pkgs.${backend}}/bin/${backend}";

  ownUid = toString containerUid;
  ownGid = toString containerGid;

  # `docker exec` defaults to UID 0. Anything it writes into $HERMES_HOME then
  # lands root-owned and the agent (uid ${ownUid}) can no longer read/write it —
  # e.g. a root-owned cron/jobs.json or a 0555 profile dir breaks the scheduler.
  # Always exec as the agent uid so in-container writes stay agent-owned.
  ctrExec = "${ctrBin} exec -u ${ownUid}:${ownGid}";

  ctrService = "${backend}-hermes-${name}.service";

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
        ExecStart = "${pkgs.bash}/bin/bash -c 'cp -rn ${seedDir}/* ${stateDir}/ 2>/dev/null || true'";
        User = "root";
      };
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
          [ -d "$d" ] && chown -R ${ownUid}:${ownGid} "$d" 2>/dev/null || true
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
        ${ctrExec} hermes-${name} sh -c '
          git config --global user.name "${gitIdentity.name}"
          git config --global user.email "${gitIdentity.email}"
          git config --global init.defaultBranch main
          git config --global safe.directory "*"
          git config --global push.autoSetupRemote true
        ' || true
      '';
    };
  };

  autopushSvc = lib.optionalAttrs (gitAutoPush != null) (
    let
      reposRoot = gitAutoPush.reposRoot or "${stateDir}/workspace";
      tokenEnvVar = gitAutoPush.tokenEnvVar or "GH_TOKEN";
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
          EnvironmentFile = gitAutoPush.tokenEnvFile;
        };
        script = ''
          root='${reposRoot}'
          [ -d "$root" ] || exit 0
          helper='!f() { echo username=x-access-token; echo "password=''$${tokenEnvVar}"; }; f'
          for gd in $(find "$root" -maxdepth 4 -name .git -type d 2>/dev/null); do
            repo=$(dirname "$gd")
            url=$(git -C "$repo" remote get-url origin 2>/dev/null) || continue
            case "$url" in *github.com*) ;; *) continue ;; esac
            git -C "$repo" -c safe.directory='*' \
              -c url."https://github.com/".insteadOf="git@github.com:" \
              -c credential.helper="$helper" push origin HEAD 2>/dev/null || true
          done
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

in
{
  # ── System user ──
  users.users = lib.mkIf createUser {
    ${user} = {
      isSystemUser = true;
      inherit group;
      home = stateDir;
      createHome = false;
      uid = uid;
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
  systemd.tmpfiles.rules = [
    "d ${stateDir} 0700 ${ownUid} ${ownGid} -"
    "d ${stateDir}/workspace 0700 ${ownUid} ${ownGid} -"
    "d ${stateDir}/skills 0700 ${ownUid} ${ownGid} -"
    "d ${stateDir}/cron 0700 ${ownUid} ${ownGid} -"
  ];

  # ── systemd services ──
  systemd.services = lib.mkMerge [
    healSvc
    seedSvc
    gitIdentitySvc
    autopushSvc
    dashboardSvc
    agentServicesSvc
    providerHealthcheckSvc
    dashboardServe
    servicesServe
  ];

  # ── systemd timers (git auto-push) ──
  systemd.timers = lib.mkMerge [
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

  # ── OCI container ──
  # Build base config, then merge caller overrides, then append env-file and
  # hardening options (extraOptions from extraContainerConfig is preserved;
  # --env-file / --security-opt / --pids-limit are appended after the merge so
  # they're never lost to a caller's extraOptions override)
  virtualisation.oci-containers.containers."hermes-${name}" =
    let
      baseConfig = {
        inherit image autoStart;
        cmd = resolvedCmd;

        volumes = [
          "${stateDir}:${stateDir}:rw"
        ]
        ++ lib.optional (configYaml != null) "${configYaml}:${stateDir}/config.yaml:ro"
        ++ extraVolumes;

        environment = {
          HERMES_HOME = stateDir;
          HERMES_AGENT_NAME = name;
        }
        // extraEnvironment;

        extraOptions = [
          "--network=${networkMode}"
        ];
      };

      # Merge caller's extraContainerConfig on top
      merged = lib.recursiveUpdate baseConfig extraContainerConfig;
    in
    merged
    // {
      # Append --env-file and hardening flags AFTER the merge so they're never lost
      extraOptions =
        merged.extraOptions
        ++ lib.optional (envFile != null) "--env-file=${envFile}"
        ++ lib.optional (agenixFile != null) "--env-file=${agenixFile}"
        ++ [ "--security-opt=no-new-privileges:true" ]
        ++ lib.optional (pidsLimit != null) "--pids-limit=${toString pidsLimit}";
    };
}
