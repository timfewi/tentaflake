{
  config,
  lib,
  pkgs,
  ...
}:
let
  cfg = config.tentaflake.api;
  cli = pkgs.callPackage ../pkgs/tentaflake-cli { };
  identity = lib.types.strMatching "[A-Za-z0-9][A-Za-z0-9_.-]{0,95}";
  credentialPath = lib.types.addCheck lib.types.str (
    path:
    builtins.stringLength path <= 4096
    && builtins.match "/[^[:space:]:%]+" path != null
    && !(lib.hasPrefix "/nix/store/" path)
    && path != "/nix/store"
    && lib.all (part: part != "" && part != "." && part != "..") (lib.drop 1 (lib.splitString "/" path))
  );
  listener = builtins.match "(127[.][0-9]{1,3}[.][0-9]{1,3}[.][0-9]{1,3}|[[]::1[]]):([0-9]{1,5})" cfg.listen;
  validListener =
    listener != null
    && (
      let
        address = lib.elemAt listener 0;
        port = lib.toInt (lib.elemAt listener 1);
      in
      port > 0
      && port <= 65535
      && (
        address == "[::1]"
        || lib.all (
          part:
          let
            value = lib.toInt part;
          in
          value <= 255 && toString value == part
        ) (lib.splitString "." address)
      )
    );
in
{
  options.tentaflake.api = {
    enable = lib.mkEnableOption "the private host API authentication service (resource operations remain unsupported)";
    host = lib.mkOption {
      type = identity;
      description = "Explicit public API host identifier, independent of deployment hostname.";
    };
    origin = lib.mkOption {
      type = lib.types.strMatching "https://[^/[:space:]@?#]+";
      description = "Exact HTTPS origin of the consumer-owned private relay, without a trailing slash.";
    };
    listen = lib.mkOption {
      type = lib.types.str;
      default = "127.0.0.1:18181";
      description = "Loopback socket address behind the separately configured private HTTPS relay.";
    };
    principal = {
      id = lib.mkOption {
        type = identity;
        description = "Explicit authenticated operator identifier.";
      };
      name = lib.mkOption {
        type = lib.types.strMatching "[^[:cntrl:]]{1,160}";
        description = "Operator display name; no deployment identity is supplied by the template.";
      };
    };
    operatorTokenFile = lib.mkOption {
      type = lib.types.nullOr credentialPath;
      default = null;
      description = "Runtime credential source path for one-use operator login. A string, never a Nix path or secret value.";
    };
    cliBearerFile = lib.mkOption {
      type = lib.types.nullOr credentialPath;
      default = null;
      description = "Runtime credential source path for the CLI bearer token. A string, never a Nix path or secret value.";
    };
    configuration = lib.mkOption {
      type = lib.types.nullOr lib.types.path;
      readOnly = true;
      internal = true;
      description = "Immutable non-secret API configuration bound into the admission plan.";
    };
  };

  config = {
    tentaflake.api.configuration =
      if !cfg.enable then
        null
      else
        pkgs.writeText "tentaflake-api.json" (
          builtins.toJSON {
            schemaVersion = 1;
            inherit (cfg)
              host
              origin
              listen
              principal
              ;
            operatorLogin = cfg.operatorTokenFile != null;
            cliBearer = cfg.cliBearerFile != null;
          }
        );
    assertions = lib.optionals cfg.enable [
      {
        assertion = config.tentaflake.security.admission != null;
        message = "tentaflake.api requires an explicitly bootstrapped and published host admission policy.";
      }
      {
        assertion = cfg.operatorTokenFile != null || cfg.cliBearerFile != null;
        message = "tentaflake.api requires at least one runtime authentication credential.";
      }
      {
        assertion = cfg.operatorTokenFile == null || cfg.operatorTokenFile != cfg.cliBearerFile;
        message = "tentaflake.api operator login and CLI bearer credentials must use distinct sources.";
      }
      {
        assertion = validListener;
        message = "tentaflake.api.listen must be a loopback address with a nonzero TCP port.";
      }
      {
        assertion = builtins.stringLength cfg.origin <= 2048;
        message = "tentaflake.api.origin exceeds the API origin length limit.";
      }
    ];
    users.groups.tentaflake-api = lib.mkIf cfg.enable { };
    users.users.tentaflake-api = lib.mkIf cfg.enable {
      isSystemUser = true;
      group = "tentaflake-api";
    };
    systemd = {
      sockets.tentaflake-api-owner = lib.mkIf cfg.enable {
        description = "Tentaflake private host API owner channel";
        bindsTo = [ "tentaflake-api.service" ];
        partOf = [ "tentaflake-api.service" ];
        listenStreams = [ "/run/tentaflake-api-owner.sock" ];
        socketConfig = {
          SocketUser = "root";
          SocketGroup = "tentaflake-api";
          SocketMode = "0660";
          RemoveOnStop = true;
        };
      };
      services = {
        tentaflake-api = lib.mkIf cfg.enable {
          description = "Tentaflake private host API HTTP transport";
          wantedBy = [ "multi-user.target" ];
          requires = [ "tentaflake-api-owner.socket" ];
          after = [
            "network.target"
            "tentaflake-api-owner.socket"
          ];
          serviceConfig = {
            Type = "simple";
            ExecStart = lib.escapeShellArgs [
              (lib.getExe cli)
              "api"
              "serve"
              (toString cfg.configuration)
            ];
            User = "tentaflake-api";
            Group = "tentaflake-api";
            UMask = "0077";
            ProtectSystem = "strict";
            ProtectHome = true;
            PrivateTmp = true;
            NoNewPrivileges = true;
            CapabilityBoundingSet = "";
            AmbientCapabilities = "";
            RestrictAddressFamilies = [
              "AF_INET"
              "AF_INET6"
              "AF_UNIX"
            ];
            InaccessiblePaths = [
              "-/var/lib/tentaflake-admission"
              "-/var/lib/tentaflake-admission.lock"
              "-/run/credentials/tentaflake-api-owner.service"
            ];
          };
        };
        tentaflake-api-owner = lib.mkIf cfg.enable {
          description = "Tentaflake private host API authority and sessions";
          bindsTo = [ "tentaflake-api.service" ];
          after = [ "tentaflake-api.service" ];
          partOf = [ "tentaflake-api.service" ];
          serviceConfig = {
            Type = "simple";
            ExecStart = lib.escapeShellArgs [
              (lib.getExe cli)
              "api"
              "owner"
              (toString cfg.configuration)
              (toString config.tentaflake.admissionManifest)
            ];
            User = "root";
            Group = "root";
            UMask = "0077";
            LoadCredential =
              lib.optional (cfg.operatorTokenFile != null) "operator-token:${cfg.operatorTokenFile}"
              ++ lib.optional (cfg.cliBearerFile != null) "cli-bearer:${cfg.cliBearerFile}";
            NoNewPrivileges = true;
            # /proc/1/ns/mnt is guarded by ptrace access checks even for UID 0.
            # Keep only the capability required to verify the host namespace.
            CapabilityBoundingSet = "CAP_SYS_PTRACE";
            AmbientCapabilities = "";
            RestrictAddressFamilies = [ "AF_UNIX" ];
            # The owner verifies PID 1's mount namespace itself. Do not introduce
            # implicit filesystem namespaces or create the ledger on service start.
          };
        };
      };
    };
  };
}
