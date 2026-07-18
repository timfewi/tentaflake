# mkOpenCodeAgent — run OpenCode (https://opencode.ai) as an isolated tentaflake agent.
#
# OpenCode ships a headless HTTP server (`opencode serve`) exposing a documented
# OpenAPI 3.1 interface. That makes it a clean target for external orchestrators
# (n8n, CI, cron): create a session, post a task, read the result:
#
#   POST /session                       -> { id }
#   POST /session/{id}/message          body: { parts: [ { type: "text", text } ] }
#
# Each agent gets the same isolation contract as the Hermes/ZeroClaw runtimes:
# its own container, its own uid, its own 0700 state dir, secrets via --env-file.
#
# Credentials — two supported patterns (see docs/08-opencode.md):
#   1. Local LLM proxy (recommended, host-portable): point `settings.provider`
#      base_url at your proxy and pass its key in the env file. Works on the
#      deployed host, keeps agents isolated, one upstream key to manage.
#   2. Reuse an existing auth.json: set `authFile` to a provider auth.json; it is
#      mounted read-only at the container's data dir. Convenient for local dev,
#      but the file must exist on the target host.
{ pkgs, lib }:

{
  name,
  # Container image. The official OpenCode image ships the `opencode` binary.
  image ? "ghcr.io/anomalyco/opencode:latest",
  # Host state dir — holds the agent's XDG config/data (auth, sessions, logs).
  stateDir ? "/var/lib/opencode-${name}",
  # Directory OpenCode operates in (the "project"). Mounted rw at /workspace.
  workspaceDir ? "${stateDir}/workspace",
  # Files copied into the workspace on first boot (never overwrites existing).
  seedDir ? null,
  # Port `opencode serve` listens on inside the container.
  gatewayPort ? 4096,
  # Host loopback port forwarded to the gateway (n8n / local callers hit this).
  hostPort,
  # Optional tailnet HTTPS port published via `tailscale serve`. null = no serve.
  servePort ? null,
  # Plaintext env file (proxy/provider key, OPENCODE_SERVER_PASSWORD, …).
  envFile ? null,
  # agenix-decrypted env file (installed systems). Exactly one of envFile /
  # agenixFile is typical; both are passed if set.
  agenixFile ? null,
  # Optional provider auth.json mounted read-only into the data dir.
  authFile ? null,
  # OpenCode config (opencode.json) as a Nix attrset, serialized and mounted ro.
  settings ? { },
  # uid/gid the container runs as; state is owned by it.
  containerUid ? 65534,
  containerGid ? 65534,
  autoStart ? true,
  # Max processes in the container (--pids-limit). null = unlimited.
  pidsLimit ? 512,
  extraEnvironment ? { },
  extraVolumes ? [ ],
}:

{ config, ... }:
let
  containerName = "opencode-${name}";
  serviceName = "${config.virtualisation.oci-containers.backend}-${containerName}.service";
  json = pkgs.formats.json { };
  configFile = json.generate "${containerName}-opencode.json" settings;

  owner = "${toString containerUid}:${toString containerGid}";

  # XDG layout under the mounted state dir so config/auth/data persist on the host.
  dataHome = "/data/.local/share";
  configHome = "/data/.config";
in
{
  systemd.tmpfiles.rules = [
    "d ${stateDir} 0700 ${toString containerUid} ${toString containerGid} -"
    "d ${stateDir}/.config 0700 ${toString containerUid} ${toString containerGid} -"
    "d ${stateDir}/.config/opencode 0700 ${toString containerUid} ${toString containerGid} -"
    "d ${stateDir}/.local 0700 ${toString containerUid} ${toString containerGid} -"
    "d ${stateDir}/.local/share 0700 ${toString containerUid} ${toString containerGid} -"
    "d ${stateDir}/.local/share/opencode 0700 ${toString containerUid} ${toString containerGid} -"
    "d ${workspaceDir} 0700 ${toString containerUid} ${toString containerGid} -"
  ];

  systemd.services =
    lib.optionalAttrs (seedDir != null) {
      "seed-${containerName}" = {
        description = "Seed workspace for OpenCode agent ${name}";
        wantedBy = [ serviceName ];
        before = [ serviceName ];
        after = [ "systemd-tmpfiles-setup.service" ];
        serviceConfig = {
          Type = "oneshot";
          RemainAfterExit = true;
          # --no-clobber (-n) so agent edits are never overwritten on reboot.
          ExecStart = "${pkgs.bash}/bin/bash -c 'cp -rn --no-preserve=mode,ownership ${seedDir}/. ${workspaceDir}/ 2>/dev/null || true; chown -R ${owner} ${workspaceDir}'";
        };
      };
    }
    // lib.optionalAttrs (servePort != null) {
      "${containerName}-tailscale-serve" = {
        description = "Tailscale Serve for OpenCode agent ${name}";
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
    };

  virtualisation.oci-containers.containers.${containerName} = {
    inherit image autoStart;
    cmd = [
      "serve"
      "--hostname"
      "0.0.0.0"
      "--port"
      (toString gatewayPort)
    ];
    volumes = [
      "${stateDir}:/data:rw"
      "${workspaceDir}:/workspace:rw"
      "${configFile}:${configHome}/opencode/opencode.json:ro"
    ]
    ++ lib.optional (authFile != null) "${authFile}:${dataHome}/opencode/auth.json:ro"
    ++ extraVolumes;
    ports = [ "127.0.0.1:${toString hostPort}:${toString gatewayPort}" ];
    environment = {
      HOME = "/data";
      XDG_CONFIG_HOME = configHome;
      XDG_DATA_HOME = dataHome;
      # OpenCode operates on this dir; set it as the working project.
      OPENCODE_PROJECT = "/workspace";
    }
    // extraEnvironment;
    workdir = "/workspace";
    user = owner;
    extraOptions = [
      "--security-opt=no-new-privileges:true"
    ]
    ++ lib.optional (envFile != null) "--env-file=${envFile}"
    ++ lib.optional (agenixFile != null) "--env-file=${agenixFile}"
    ++ lib.optional (pidsLimit != null) "--pids-limit=${toString pidsLimit}";
  };
}
