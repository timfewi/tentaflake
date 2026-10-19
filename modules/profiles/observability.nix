# Opt-in local observability stack. Every HTTP listener stays on loopback;
# operators may publish Grafana deliberately through an authenticated private
# path. This module is exported separately and is not part of the core module.
{
  config,
  lib,
  pkgs,
  ...
}:
let
  cfg = config.tentaflake.profiles.observability;
  brokerAgents = lib.attrByPath [ "tentaflake" "broker" "agents" ] { } config;
  enabledBrokerAgents = lib.filterAttrs (_: agent: agent.enable) brokerAgents;
  brokerSignals = lib.concatLists (
    lib.mapAttrsToList (
      name: agent:
      lib.optional agent.llm.enable {
        inherit name agent;
        mode = "llm";
      }
      ++ lib.optional agent.fetch.enable {
        inherit name agent;
        mode = "fetch";
      }
    ) enabledBrokerAgents
  );
  metricsDirectory = "/var/lib/tentaflake-observability/textfile";
  renderBrokerSignal =
    signal:
    let
      inherit (signal) name agent mode;
      stateDirectory = "/var/lib/tentaflake-broker-${mode}-${name}";
      auditFile = "${stateDirectory}/audit.jsonl";
      budgetFile = "${stateDirectory}/budget.json";
    in
    ''
      files=()
      for audit in \
        ${lib.escapeShellArg auditFile} \
        ${lib.escapeShellArg "${auditFile}.1"}; do
        if [ -r "$audit" ]; then
          files+=("$audit")
        fi
      done
      denials=0
      if [ "''${#files[@]}" -gt 0 ]; then
        denials=$(${pkgs.jq}/bin/jq -s \
          --argjson cutoff "$cutoff" \
          '[.[] | select((.ts // 0) >= $cutoff and .outcome == "denied")] | length' \
          "''${files[@]}")
      fi
      printf 'tentaflake_broker_policy_denials_5m{agent="%s",mode="%s"} %s\n' \
        ${lib.escapeShellArg name} ${lib.escapeShellArg mode} "$denials" >> "$tmp"
      if [ ${lib.escapeShellArg mode} = fetch ]; then
        printf 'tentaflake_broker_fetch_denials_5m{agent="%s"} %s\n' \
          ${lib.escapeShellArg name} "$denials" >> "$tmp"
      fi
      requests=0
      tokens=0
      cost=0
      if [ -r ${lib.escapeShellArg budgetFile} ]; then
        requests=$(${pkgs.jq}/bin/jq -r '.requests // 0' ${lib.escapeShellArg budgetFile})
        tokens=$(${pkgs.jq}/bin/jq -r '.tokens // 0' ${lib.escapeShellArg budgetFile})
        cost=$(${pkgs.jq}/bin/jq -r '.cost_microusd // 0' ${lib.escapeShellArg budgetFile})
      fi
      printf 'tentaflake_broker_budget_requests{agent="%s",mode="%s"} %s\n' \
        ${lib.escapeShellArg name} ${lib.escapeShellArg mode} "$requests" >> "$tmp"
      printf 'tentaflake_broker_budget_request_limit{agent="%s",mode="%s"} %s\n' \
        ${lib.escapeShellArg name} ${lib.escapeShellArg mode} \
        ${toString agent.dailyRequestBudget} >> "$tmp"
      printf 'tentaflake_broker_budget_tokens{agent="%s",mode="%s"} %s\n' \
        ${lib.escapeShellArg name} ${lib.escapeShellArg mode} "$tokens" >> "$tmp"
      printf 'tentaflake_broker_budget_token_limit{agent="%s",mode="%s"} %s\n' \
        ${lib.escapeShellArg name} ${lib.escapeShellArg mode} \
        ${toString agent.dailyTokenBudget} >> "$tmp"
      printf 'tentaflake_broker_budget_cost_microusd{agent="%s",mode="%s"} %s\n' \
        ${lib.escapeShellArg name} ${lib.escapeShellArg mode} "$cost" >> "$tmp"
      printf 'tentaflake_broker_budget_cost_limit_microusd{agent="%s",mode="%s"} %s\n' \
        ${lib.escapeShellArg name} ${lib.escapeShellArg mode} \
        ${toString agent.dailyCostMicrousd} >> "$tmp"
    '';
in
{
  options.tentaflake.profiles.observability = {
    enable = lib.mkEnableOption "the Prometheus, Grafana, Loki, and Alloy observability profile";

    prometheusPort = lib.mkOption {
      type = lib.types.port;
      default = 9090;
      description = "Loopback Prometheus port.";
    };

    grafanaPort = lib.mkOption {
      type = lib.types.port;
      default = 3000;
      description = "Loopback Grafana port.";
    };

    lokiPort = lib.mkOption {
      type = lib.types.port;
      default = 3100;
      description = "Loopback Loki port.";
    };

    alloyPort = lib.mkOption {
      type = lib.types.port;
      default = 12346;
      description = "Loopback Alloy diagnostics port.";
    };

    retention = lib.mkOption {
      type = lib.types.str;
      default = "14d";
      example = "30d";
      description = "Prometheus and Loki data retention window.";
    };

    grafanaSecretKeyFile = lib.mkOption {
      type = lib.types.nullOr lib.types.str;
      default = null;
      example = "/run/agenix/grafana-secret-key";
      description = ''
        Runtime file containing Grafana's persistent secret key. The value is
        loaded through a systemd credential and never enters the Nix store.
      '';
    };

    grafanaAdminPasswordFile = lib.mkOption {
      type = lib.types.nullOr lib.types.str;
      default = null;
      example = "/run/agenix/grafana-admin-password";
      description = ''
        Runtime file containing Grafana's initial admin password. The value is
        loaded through a systemd credential and never enters the Nix store.
      '';
    };
  };

  config = lib.mkIf cfg.enable {
    assertions = [
      {
        assertion = cfg.grafanaSecretKeyFile != null;
        message = "tentaflake observability requires grafanaSecretKeyFile; refusing to store a generated or literal key in the Nix store.";
      }
      {
        assertion = cfg.grafanaAdminPasswordFile != null;
        message = "tentaflake observability requires grafanaAdminPasswordFile; refusing Grafana's default admin password.";
      }
    ];

    services = {
      prometheus = {
        enable = true;
        listenAddress = "127.0.0.1";
        port = cfg.prometheusPort;
        retentionTime = cfg.retention;
        exporters.node = {
          enable = true;
          listenAddress = "127.0.0.1";
          port = 9100;
          enabledCollectors = [
            "systemd"
            "textfile"
          ];
          extraFlags = [ "--collector.textfile.directory=${metricsDirectory}" ];
        };
        rules = [
          ''
            groups:
              - name: tentaflake
                rules:
                  - alert: TentaflakeRootDiskCritical
                    expr: (node_filesystem_avail_bytes{mountpoint="/"} / node_filesystem_size_bytes{mountpoint="/"}) < 0.10
                    for: 5m
                    labels:
                      severity: critical
                    annotations:
                      summary: Tentaflake root filesystem has less than 10 percent free
                  - alert: TentaflakeAgentRestartFlapping
                    expr: changes(node_systemd_unit_state{name=~"(docker|podman|tentaflake).*",state="active"}[15m]) > 4
                    for: 5m
                    labels:
                      severity: warning
                    annotations:
                      summary: Tentaflake service repeatedly changed active state
                  - alert: TentaflakePolicyDenials
                    expr: tentaflake_broker_policy_denials_5m > 0
                    for: 5m
                    labels:
                      severity: warning
                    annotations:
                      summary: Broker policy denied one or more recent requests
                  - alert: TentaflakeUnusualFetchDenials
                    expr: tentaflake_broker_fetch_denials_5m > 10
                    for: 5m
                    labels:
                      severity: warning
                    annotations:
                      summary: Fetch broker shows an unusual denial burst
                  - alert: TentaflakeRequestBudgetNearLimit
                    expr: tentaflake_broker_budget_requests / tentaflake_broker_budget_request_limit > 0.90
                    for: 5m
                    labels:
                      severity: warning
                    annotations:
                      summary: Broker daily request budget is above 90 percent
                  - alert: TentaflakeTokenBudgetNearLimit
                    expr: tentaflake_broker_budget_tokens / tentaflake_broker_budget_token_limit > 0.90
                    for: 5m
                    labels:
                      severity: warning
                    annotations:
                      summary: Broker daily token budget is above 90 percent
                  - alert: TentaflakeCostBudgetNearLimit
                    expr: tentaflake_broker_budget_cost_microusd / tentaflake_broker_budget_cost_limit_microusd > 0.90
                    for: 5m
                    labels:
                      severity: warning
                    annotations:
                      summary: Broker daily cost budget is above 90 percent
          ''
        ];
        scrapeConfigs = [
          {
            job_name = "tentaflake-host";
            static_configs = [
              { targets = [ "127.0.0.1:9100" ]; }
            ];
          }
          {
            job_name = "prometheus";
            static_configs = [
              { targets = [ "127.0.0.1:${toString cfg.prometheusPort}" ]; }
            ];
          }
        ];
      };

      loki = {
        enable = true;
        configuration = {
          auth_enabled = false;
          server = {
            http_listen_address = "127.0.0.1";
            http_listen_port = cfg.lokiPort;
          };
          common = {
            path_prefix = "/var/lib/loki";
            replication_factor = 1;
            ring.kvstore.store = "inmemory";
          };
          schema_config.configs = [
            {
              from = "2024-01-01";
              store = "tsdb";
              object_store = "filesystem";
              schema = "v13";
              index = {
                prefix = "index_";
                period = "24h";
              };
            }
          ];
          storage_config.filesystem = {
            chunks_directory = "/var/lib/loki/chunks";
            rules_directory = "/var/lib/loki/rules";
          };
          compactor = {
            working_directory = "/var/lib/loki/compactor";
            retention_enabled = true;
            delete_request_store = "filesystem";
          };
          limits_config.retention_period = cfg.retention;
        };
      };

      alloy = {
        enable = true;
        configPath = "/etc/alloy";
        extraFlags = [
          "--server.http.listen-addr=127.0.0.1:${toString cfg.alloyPort}"
          "--disable-reporting"
        ];
      };

      grafana = {
        enable = true;
        openFirewall = false;
        settings = {
          server = {
            http_addr = "127.0.0.1";
            http_port = cfg.grafanaPort;
          };
          security = {
            admin_password = "$__file{/run/credentials/grafana.service/admin-password}";
            secret_key = "$__file{/run/credentials/grafana.service/secret-key}";
            disable_gravatar = true;
            cookie_samesite = "strict";
          };
          users = {
            allow_sign_up = false;
            allow_org_create = false;
          };
          analytics = {
            reporting_enabled = false;
            check_for_updates = false;
            check_for_plugin_updates = false;
            feedback_links_enabled = false;
          };
        };
        provision = {
          enable = true;
          datasources.settings = {
            apiVersion = 1;
            prune = true;
            datasources = [
              {
                name = "Prometheus";
                uid = "prometheus";
                type = "prometheus";
                access = "proxy";
                url = "http://127.0.0.1:${toString cfg.prometheusPort}";
                editable = false;
                isDefault = true;
              }
              {
                name = "Loki";
                uid = "loki";
                type = "loki";
                access = "proxy";
                url = "http://127.0.0.1:${toString cfg.lokiPort}";
                editable = false;
              }
            ];
          };
        };
      };
    };

    environment.etc."alloy/tentaflake.alloy".text = ''
      loki.write "local" {
        endpoint {
          url = "http://127.0.0.1:${toString cfg.lokiPort}/loki/api/v1/push"
        }
      }

      loki.source.journal "system" {
        forward_to = [loki.write.local.receiver]
        max_age    = "12h"
        labels     = { job = "systemd-journal" }
      }
    '';

    systemd = {
      services = {
        grafana.serviceConfig.LoadCredential = [
          "secret-key:${cfg.grafanaSecretKeyFile}"
          "admin-password:${cfg.grafanaAdminPasswordFile}"
        ];

        tentaflake-observability-metrics = {
          description = "Export Tentaflake broker policy and budget metrics";
          serviceConfig = {
            Type = "oneshot";
            User = "root";
            UMask = "0022";
            NoNewPrivileges = true;
            PrivateDevices = true;
            PrivateTmp = true;
            ProtectSystem = "strict";
            ProtectHome = true;
            ReadWritePaths = [ metricsDirectory ];
            RestrictAddressFamilies = [ "AF_UNIX" ];
            CapabilityBoundingSet = [ "CAP_DAC_READ_SEARCH" ];
            LockPersonality = true;
            MemoryDenyWriteExecute = true;
            RestrictRealtime = true;
            SystemCallArchitectures = "native";
          };
          script = ''
            set -euo pipefail
            tmp=${metricsDirectory}/tentaflake.prom.tmp
            final=${metricsDirectory}/tentaflake.prom
            cutoff=$((${pkgs.coreutils}/bin/date +%s - 300))
            : > "$tmp"
            ${lib.concatMapStringsSep "\n" renderBrokerSignal brokerSignals}
            ${pkgs.coreutils}/bin/chmod 0644 "$tmp"
            ${pkgs.coreutils}/bin/mv -f "$tmp" "$final"
          '';
        };
      };

      tmpfiles.rules = [ "d ${metricsDirectory} 0755 root root -" ];

      timers.tentaflake-observability-metrics = {
        wantedBy = [ "timers.target" ];
        timerConfig = {
          OnBootSec = "2min";
          OnUnitActiveSec = "1min";
          AccuracySec = "15s";
          Persistent = true;
          Unit = "tentaflake-observability-metrics.service";
        };
      };
    };
  };
}
