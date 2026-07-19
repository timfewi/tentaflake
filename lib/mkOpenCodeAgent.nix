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

let
  constants = import ./constants.nix;
  pinnedImage = import ./pinnedImage.nix { inherit lib; };
in

{
  name,
  # Container image. Default is the official OpenCode image, digest-pinned in
  # lib/constants.nix. Overrides MUST also be digest-pinned — a bare tag is
  # rejected at eval time, because the registry can repoint a tag at different
  # bytes between rebuilds.
  image ? constants.opencodeImage,
  # Escape hatch for locally-built images, which have no registry digest to pin
  # to. Setting this to true gives up reproducibility for this agent.
  allowMutableImage ? false,
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
  # Publishing puts the agent's session API in reach of the WHOLE tailnet, so
  # this requires OPENCODE_SERVER_PASSWORD (opencode's HTTP basic auth) via
  # envFile/agenixFile/extraEnvironment unless `allowUnauthenticatedServe` is
  # set. Enforced twice: at eval, then again on the file contents at start.
  # Must differ from hostPort — `tailscale serve ... off` clears a whole port.
  servePort ? null,
  # Opt out of the servePort credential check above. Only sane when something
  # else in front (an authenticating proxy, tailnet ACLs) gates access.
  allowUnauthenticatedServe ? false,
  # Plaintext env file (proxy/provider key, OPENCODE_SERVER_PASSWORD, …).
  envFile ? null,
  # agenix-decrypted env file (installed systems). Exactly one of envFile /
  # agenixFile is typical; both are passed if set.
  agenixFile ? null,
  # Optional provider auth.json mounted read-only into the data dir.
  authFile ? null,
  # OpenCode config (opencode.json) as a Nix attrset, serialized and mounted ro.
  # The generated file lands in the WORLD-READABLE Nix store, so never inline a
  # provider key here — use OpenCode's `{env:VAR}` substitution and pass the
  # value through envFile/agenixFile instead.
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

  # Files whose contents become container env vars — the only places
  # OPENCODE_SERVER_PASSWORD may come from. Deliberately NOT extraEnvironment:
  # that attrset is rendered into the container unit's start script, which lands
  # in the world-readable Nix store (content-addressed, so deleting the source
  # never removes it). A password there leaks to every user on the host.
  credentialFiles =
    lib.optional (envFile != null) envFile ++ lib.optional (agenixFile != null) agenixFile;
  serveAuthenticated = allowUnauthenticatedServe;

  # The eval-time assertion can only see that *some* credential file is set, not
  # what is in it — and the documented proxy-key pattern puts an unrelated key in
  # that same file. So re-check at start, where the file actually exists, and
  # fail closed before the tailnet mapping is created.
  # ponytail: substring match, not a parser — `OPENCODE_SERVER_PASSWORD=""`
  # passes. Parse the env file properly if that turns up in practice.
  serveAuthCheck = pkgs.writeShellScript "${containerName}-serve-auth-check" ''
    if ! ${pkgs.gnugrep}/bin/grep -qsE '^[[:space:]]*OPENCODE_SERVER_PASSWORD=.+' ${lib.escapeShellArgs credentialFiles}; then
      # Declining to start is not enough: `tailscale serve --bg` state lives in
      # tailscaled and is restored on boot, so a mapping published earlier (when
      # the password was still there) would survive this failure and keep
      # fronting a now-unauthenticated session API. Tear it down, then fail.
      # ExecStop cannot cover this — systemd runs ExecStopPost, not ExecStop,
      # when ExecStartPre fails.
      ${pkgs.tailscale}/bin/tailscale serve --https=${toString servePort} off || true
      echo 'tentaflake: OpenCode agent "${name}" would publish servePort ${toString servePort} on the tailnet,' >&2
      echo 'but no non-empty OPENCODE_SERVER_PASSWORD was found in: ${lib.concatStringsSep ", " credentialFiles}' >&2
      echo 'Refusing to expose an unauthenticated session API (any stale publication was torn down).' >&2
      echo 'Add OPENCODE_SERVER_PASSWORD to that file, or set allowUnauthenticatedServe = true' >&2
      echo 'to acknowledge access is gated elsewhere.' >&2
      exit 1
    fi
  '';
in
{
  assertions = [
    {
      # `opencode serve` binds 0.0.0.0 in the container; tailscale serve then
      # publishes it tailnet-wide, where any peer could open a session and make
      # the agent run tool calls. This is only the cheap first gate — it can see
      # that a credential source exists, not that it carries a password. The
      # serve unit's ExecStartPre re-checks the file contents at runtime.
      assertion = servePort == null || serveAuthenticated || credentialFiles != [ ];
      message = ''
        tentaflake: OpenCode agent "${name}" sets servePort = ${toString servePort} but has no
        envFile / agenixFile, so it would be published on the tailnet with no
        authentication — any tailnet peer could create sessions and make the
        agent run tool calls.

        Set envFile or agenixFile to a file defining OPENCODE_SERVER_PASSWORD
        (opencode's HTTP basic auth; OPENCODE_SERVER_USERNAME defaults to
        "opencode"), or set `allowUnauthenticatedServe = true;` to acknowledge
        that access is gated some other way.

        Do NOT put the password in `extraEnvironment` — that is rendered into
        the container unit's start script in the world-readable Nix store.
      '';
    }
    {
      # ExecStop runs `tailscale serve --https=<port> off`, which drops the whole
      # handler set for that port — so a collision would tear down another
      # agent's publication, not just this one's.
      assertion = servePort == null || servePort != hostPort;
      message = ''
        tentaflake: OpenCode agent "${name}" uses the same port ${toString servePort} for both
        hostPort and servePort. Give servePort a distinct tailnet port.
      '';
    }
  ];

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
        # Without this, stopping the container leaves the tailnet mapping live,
        # because ExecStop only fires when this unit itself is stopped.
        partOf = [ serviceName ];
        serviceConfig = {
          Type = "oneshot";
          RemainAfterExit = true;
          ExecStartPre = lib.optional (!serveAuthenticated && credentialFiles != [ ]) serveAuthCheck;
          ExecStart = "${pkgs.tailscale}/bin/tailscale serve --bg --https=${toString servePort} 127.0.0.1:${toString hostPort}";
          # Serve config lives in tailscaled, not here: without this the agent
          # stays published on the tailnet after the unit stops — including when
          # `servePort` is removed from the Nix config and the unit disappears.
          ExecStop = "${pkgs.tailscale}/bin/tailscale serve --https=${toString servePort} off";
        };
      };
    };

  virtualisation.oci-containers.containers.${containerName} = {
    inherit autoStart;
    image = pinnedImage name allowMutableImage image;
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
