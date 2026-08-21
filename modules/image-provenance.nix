{
  config,
  lib,
  pkgs,
  ...
}:
let
  cfg = config.tentaflake.imageProvenance;
  containers = config.virtualisation.oci-containers.containers;
  agentContainers = lib.filterAttrs (
    _: container: (container.labels."io.tentaflake.agent" or "false") == "true"
  ) containers;
  secureAgents = lib.filterAttrs (
    _: container: (container.labels."io.tentaflake.security-profile" or "dev") != "dev"
  ) agentContainers;
  enabledPolicies = lib.filterAttrs (_: policy: policy.enable) cfg.agents;
  serviceName = name: "tentaflake-image-verify-${name}";
  ociServiceName = name: "${config.virtualisation.oci-containers.backend}-${name}";
  safeAbsolutePath =
    path:
    lib.hasPrefix "/" path
    && lib.match "^/[A-Za-z0-9._+/-]+$" path != null
    && !(lib.elem ".." (lib.splitString "/" path));
  verificationArgs =
    name: policy:
    let
      image = containers.${name}.image;
    in
    if policy.mode == "key" then
      [
        "--key"
        policy.publicKeyFile
        image
      ]
    else
      [
        "--certificate-identity"
        policy.certificateIdentity
        "--certificate-oidc-issuer"
        policy.certificateOidcIssuer
        image
      ];
  verificationService =
    name: policy:
    let
      unit = serviceName name;
      ociUnit = ociServiceName name;
    in
    {
      ${unit} = {
        description = "Verify the signed OCI image for ${name}";
        after = [ "network-online.target" ];
        wants = [ "network-online.target" ];
        before = [ "${ociUnit}.service" ];
        serviceConfig = {
          Type = "oneshot";
          RemainAfterExit = true;
          ExecStart = lib.escapeShellArgs (
            [
              "${pkgs.cosign}/bin/cosign"
              "verify"
            ]
            ++ verificationArgs name policy
          );
          DynamicUser = true;
          CacheDirectory = unit;
          Environment = "XDG_CACHE_HOME=/var/cache/${unit}";
          NoNewPrivileges = true;
          PrivateDevices = true;
          PrivateTmp = true;
          ProtectSystem = "strict";
          ProtectHome = true;
          ProtectKernelTunables = true;
          ProtectKernelModules = true;
          ProtectControlGroups = true;
          RestrictAddressFamilies = [
            "AF_UNIX"
            "AF_INET"
            "AF_INET6"
          ];
          CapabilityBoundingSet = "";
          LockPersonality = true;
          MemoryDenyWriteExecute = true;
          RestrictRealtime = true;
          SystemCallArchitectures = "native";
          SystemCallFilter = [ "@system-service" ];
        };
      };
      ${ociUnit} = {
        requires = [ "${unit}.service" ];
        after = [ "${unit}.service" ];
      };
    };
in
{
  options.tentaflake.imageProvenance = {
    requireForSecureAgents = lib.mkOption {
      type = lib.types.bool;
      default = false;
      description = ''
        Require every balanced/strict agent to have a Cosign verification
        policy. Enable only after recording a real upstream signing identity or
        public key; a digest alone is not evidence of publisher trust.
      '';
    };

    agents = lib.mkOption {
      default = { };
      description = "Per-container Cosign verification policies.";
      type = lib.types.attrsOf (
        lib.types.submodule (
          { ... }:
          {
            options = {
              enable = lib.mkEnableOption "Cosign verification before the container starts" // {
                default = true;
              };
              mode = lib.mkOption {
                type = lib.types.enum [
                  "key"
                  "keyless"
                ];
                default = "keyless";
                description = "Verify with a public key or an exact keyless certificate identity.";
              };
              publicKeyFile = lib.mkOption {
                type = lib.types.str;
                default = "";
                example = "/etc/tentaflake/image-signing.pub";
                description = "Absolute Cosign public-key path for key mode; public verification material is not secret.";
              };
              certificateIdentity = lib.mkOption {
                type = lib.types.str;
                default = "";
                example = "https://github.com/example/project/.github/workflows/release.yml@refs/tags/v1.2.3";
                description = "Exact Fulcio certificate identity for keyless mode.";
              };
              certificateOidcIssuer = lib.mkOption {
                type = lib.types.str;
                default = "https://token.actions.githubusercontent.com";
                description = "Exact OIDC issuer accepted for keyless mode.";
              };
            };
          }
        )
      );
    };
  };

  config = {
    assertions = [
      {
        assertion = lib.all (name: lib.hasAttr name agentContainers) (lib.attrNames enabledPolicies);
        message = "tentaflake image provenance policies must name an existing agent container exactly.";
      }
      {
        assertion =
          !cfg.requireForSecureAgents
          || lib.all (name: lib.hasAttr name enabledPolicies) (lib.attrNames secureAgents);
        message = "tentaflake imageProvenance.requireForSecureAgents requires an enabled Cosign policy for every balanced/strict agent.";
      }
    ]
    ++ lib.mapAttrsToList (name: policy: {
      assertion =
        if policy.mode == "key" then
          policy.publicKeyFile != "" && safeAbsolutePath policy.publicKeyFile
        else
          policy.certificateIdentity != "" && policy.certificateOidcIssuer != "";
      message = "tentaflake image provenance policy ${name} requires an exact public key path or keyless identity plus issuer.";
    }) enabledPolicies;

    systemd.services = lib.mkMerge (lib.mapAttrsToList verificationService enabledPolicies);
  };
}
