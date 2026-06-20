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
#   - State dir    /var/lib/hermes-<name>  (0700, owned by agent user)
#   - Docker container  hermes-<name>      (host networking, auto-start)
#   - HERMES_HOME pointing to its isolated state dir
# ────────────────────────────────────────────────────────────

{
  name,
  stateDir ? "/var/lib/hermes-${name}",

  # User / group
  user ? "hermes-${name}",
  group ? "hermes-${name}",
  uid ? null,
  gid ? null,

  # Container image. Default pulls the official hermes-agent image.
  # Override with any OCI-compatible image, e.g.:
  #   image = "ubuntu:24.04";
  # You can then install hermes-agent inside via a Dockerfile.
  image ? "ghcr.io/nousresearch/hermes-agent:latest",

  # Secrets — path to a .env file on the host, e.g.:
  #   envFile = "/run/secrets/hermes-coding.env";
  # The file should contain:
  #   OPENROUTER_API_KEY=sk-or-...
  #
  # Alternative: set agenixFile = "/run/agenix/hermes-key"
  # The container will read it at startup via extraEnvironment.
  envFile ? null,
  agenixFile ? null,

  # Extra volumes to mount into the container
  extraVolumes ? [ ],

  # Extra environment variables to set in the container
  extraEnvironment ? { },

  # Container command (default: gateway run --replace)
  cmd ? [
    "gateway"
    "run"
    "--replace"
  ],

  # Auto-start container with system
  autoStart ? true,

  # Create system user and group (disable if managing externally)
  createUser ? true,

  # Extra OCI container config options (merged into the container attrset)
  extraContainerConfig ? { },

  # Hermes YAML/JSON configuration — serialized to config.yaml at build time
  # and mounted read-only at $HERMES_HOME/config.yaml inside the container.
  # Set to `null` (default) to skip and use Hermes' built-in defaults.
  settings ? null,
}:

{ config, ... }:
let
  # Resolve effective UID for --user flag
  effectiveUid =
    if uid != null then
      uid
    else if createUser then
      config.users.users.${user}.uid
    else
      65534; # nobody fallback

  # Optional agenix inject: if agenixFile set, read secret into env
  agenixEnv = lib.optionalAttrs (agenixFile != null) {
    OPENROUTER_API_KEY = "__from_agenix__";
  };

  # Generate config.yaml derivation when settings are provided
  yamlFormat = pkgs.formats.yaml { };
  configYaml =
    if (settings != null && settings != { }) then
      yamlFormat.generate "hermes-${name}-config.yaml" settings
    else
      null;

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
      description = "Hermes agent: ${name}";
      shell = "${pkgs.bash}/bin/bash";
    };
  };

  users.groups = lib.mkIf createUser {
    ${group} = { };
  };

  # ── tmpfiles — create state directories with correct ownership ──
  systemd.tmpfiles.rules = [
    "d ${stateDir} 0700 ${user} ${group} -"
    "d ${stateDir}/workspace 0700 ${user} ${group} -"
    "d ${stateDir}/skills 0700 ${user} ${group} -"
    "d ${stateDir}/cron 0700 ${user} ${group} -"
  ];

  # ── OCI container ──
  virtualisation.oci-containers.containers."hermes-${name}" = lib.recursiveUpdate {
    inherit image autoStart cmd;

    volumes = [
      "${stateDir}:${stateDir}:rw"
    ]
    ++ lib.optional (configYaml != null) "${configYaml}:${stateDir}/config.yaml:ro"
    ++ extraVolumes;

    environment = {
      HERMES_HOME = stateDir;
      HERMES_AGENT_NAME = name;
    }
    // agenixEnv
    // extraEnvironment;

    environmentFiles = lib.optional (envFile != null) envFile;

    extraOptions = [
      "--network=host"
      "--user=${toString effectiveUid}"
    ];
  } extraContainerConfig;
}
