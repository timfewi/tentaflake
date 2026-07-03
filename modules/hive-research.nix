{
  config,
  lib,
  ...
}:

# ────────────────────────────────────────────────────────────
# hive-research — unified web-research MCP server
#
# Serves normalized research_* MCP tools (research_search,
# research_extract, research_crawl, research_contacts) over MCP
# Streamable HTTP, aggregating Brave Search, Tavily, FireCrawl,
# Hunter.io and Spider Cloud with ordered failover. Hermes agents
# (via host networking) reach it on 127.0.0.1.
#
# The server is not bundled with tentaflake — bring it as a flake
# input and pass its package:
#
#   inputs.hive-research.url = "github:<you>/hive-research";
#
#   services.hive-research = {
#     enable  = true;
#     package = inputs.hive-research.packages.${pkgs.system}.default;
#     keyFiles = {
#       # env var -> file containing the key (e.g. agenix-decrypted).
#       # Provide any subset; providers without a key are disabled
#       # at startup and the server degrades gracefully.
#       BRAVE_API_KEY_FILE     = "/run/agenix/hive-brave-api-key";
#       TAVILY_API_KEY_FILE    = "/run/agenix/hive-tavily-api-key";
#       FIRECRAWL_API_KEY_FILE = "/run/agenix/hive-firecrawl-api-key";
#       HUNTER_API_KEY_FILE    = "/run/agenix/hive-hunter-api-key";
#       SPIDER_API_KEY_FILE    = "/run/agenix/hive-spider-api-key";
#     };
#   };
#
# Hermes config.yaml (per profile) to use it:
#   mcp_servers:
#     hive-research:
#       url: "http://127.0.0.1:7815/mcp"
# ────────────────────────────────────────────────────────────

let
  cfg = config.services.hive-research;

  # Key files are handed to the unit via systemd LoadCredential: works
  # with root-owned 0400/0600 secrets while the service itself runs as
  # an unprivileged DynamicUser. %d = the per-unit credentials dir.
  credName = envVar: lib.removeSuffix "_FILE" envVar;
in
{
  options.services.hive-research = {
    enable = lib.mkEnableOption "hive-research unified web-research MCP server";

    package = lib.mkOption {
      type = lib.types.package;
      description = "The hive-research package (provides bin/hive-research-mcp).";
    };

    port = lib.mkOption {
      type = lib.types.port;
      default = 7815;
      description = "TCP port for the MCP Streamable HTTP endpoint (path /mcp).";
    };

    host = lib.mkOption {
      type = lib.types.str;
      default = "127.0.0.1";
      description = ''
        Bind address. Loopback reaches Hermes containers using host
        networking (the default). Binding wider exposes a key-bearing
        endpoint — change deliberately and firewall accordingly.
      '';
    };

    keyFiles = lib.mkOption {
      type = lib.types.attrsOf lib.types.str;
      default = { };
      example = {
        BRAVE_API_KEY_FILE = "/run/agenix/hive-brave-api-key";
      };
      description = ''
        Provider key files: <ENV_VAR>_FILE name -> path to a file
        containing the API key. Recognized vars: BRAVE_API_KEY_FILE,
        FIRECRAWL_API_KEY_FILE, TAVILY_API_KEY_FILE, HUNTER_API_KEY_FILE,
        SPIDER_API_KEY_FILE. Any subset works; missing providers are
        disabled at startup.
      '';
    };
  };

  config = lib.mkIf cfg.enable {
    assertions = [
      {
        assertion = cfg.keyFiles != { };
        message = "services.hive-research: keyFiles is empty — the server would start with every provider disabled.";
      }
    ];

    systemd.services.hive-research = {
      description = "hive-research unified web-research MCP server";
      after = [ "network.target" ];
      wantedBy = [ "multi-user.target" ];

      serviceConfig = {
        ExecStart = lib.getExe' cfg.package "hive-research-mcp";
        DynamicUser = true;
        LoadCredential = lib.mapAttrsToList (envVar: path: "${credName envVar}:${path}") cfg.keyFiles;
        Restart = "on-failure";
        RestartSec = "3";
        PrivateTmp = true;
        NoNewPrivileges = true;
        ProtectSystem = "strict";
        ProtectHome = true;
        PrivateDevices = true;
        ProtectKernelModules = true;
        ProtectControlGroups = true;
        RestrictAddressFamilies = [
          "AF_UNIX"
          "AF_INET"
          "AF_INET6"
        ];
        RestrictNamespaces = true;
        LockPersonality = true;
        MemoryDenyWriteExecute = true;
      };

      environment = {
        HIVE_RESEARCH_HOST = cfg.host;
        HIVE_RESEARCH_PORT = toString cfg.port;
      }
      // lib.mapAttrs' (envVar: _: lib.nameValuePair envVar "%d/${credName envVar}") cfg.keyFiles;
    };
  };
}
