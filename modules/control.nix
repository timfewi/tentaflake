{
  config,
  lib,
  pkgs,
  ...
}:
let
  cfg = config.tentaflake.control;
  inherit (config.tentaflake) adminUser;
  inherit (config.tentaflake.security) resources;
  inherit (import ../lib/resourceLimits.nix { inherit lib; }) maxMemoryBytes sizeBytes;

  control = pkgs.callPackage ../pkgs/tentaflake-control { };
  ui = pkgs.callPackage ../pkgs/tentaflake-ui { };
  broker = pkgs.callPackage ../pkgs/tentaflake-broker { };

  state = "/var/lib/tentaflake";
  controlState = "${state}/control";
  site = "/run/tentaflake/site";
  # Written by the control service, read by the broker.
  routes = "/run/tentaflake/broker";
  controlPort = 8080;
  brokerPort = 7810;
  # The bridges of the two environment networks (control/src/environments.ts).
  bridges = [
    "tf-model-only"
    "tf-open"
  ];

  runtimeSource = lib.fileset.toSource {
    root = ../images/runtime;
    fileset = ../images/runtime;
  };
  # Tagged with the store hash of its inputs, so it is rebuilt when they change and only then.
  image = "tentaflake-runtime:${builtins.substring 0 32 (baseNameOf runtimeSource)}";

  controlConfig = (pkgs.formats.json { }).generate "tentaflake-control.json" {
    hostname = "127.0.0.1";
    port = controlPort;
    siteFile = "${site}/site.json";
    domainFile = "${controlState}/domain";
    databasePath = "${controlState}/control.sqlite";
    keyFile = "${controlState}/control.key";
    setupTokenFile = "${controlState}/setup-token";
    dockerSocket = "/run/docker.sock";
    inherit brokerPort image;
    brokerDirectory = routes;
    brokerUser = "tentaflake-broker";
    uiDirectory = "${ui}";
    limits = {
      memoryMiB = sizeBytes maxMemoryBytes resources.memory / (1024 * 1024);
      cpus = builtins.fromJSON resources.cpus;
      pids = resources.pidsLimit;
    };
  };

  # Runs at every boot and whenever the domain, the setup token or a password
  # changes: creates the control service's secrets once, derives the HTTPS
  # site and shows how to reach it on the console; the journal gets the URL.
  setup = pkgs.writeShellApplication {
    name = "tentaflake-setup";
    runtimeInputs = with pkgs; [
      coreutils
      diffutils
      gawk
      iproute2
      jq
      shadow
      systemd
      util-linux
    ];
    text = ''
      control=${controlState}
      site=${site}
      admin=${lib.escapeShellArg adminUser}
      install -d -m 0755 ${state} "$site" /run/issue.d
      install -d -m 0700 -o tentaflake -g tentaflake "$control"

      # A private file of the control service, created once.
      create() {
        local file=$1
        shift
        [[ -e $file ]] && return 0
        (umask 077 && "$@" > "$file.tmp")
        chown tentaflake:tentaflake "$file.tmp"
        mv "$file.tmp" "$file"
      }
      token() { head -c 24 /dev/urandom | basenc --base64url; }
      # The host key encrypts the secrets in the database. The setup token is
      # single-use: the control service empties it once the admin exists.
      create "$control/control.key" head -c 32 /dev/urandom
      create "$control/setup-token" token
      token=$(cat "$control/setup-token")

      # Without SSH, the provider's console is the way in until the private
      # management transport is enrolled: the admin gets a first password once,
      # expired so that the first login replaces it, and shown until then.
      shadow() { awk -F: -v user="$admin" -v field="$1" '$1 == user { print $field }' /etc/shadow; }
      password_file=${state}/console-password
      if [[ ! -e $password_file ]]; then
        if [[ $(shadow 2) =~ ^[!*]*$ ]]; then
          password=$(head -c 15 /dev/urandom | basenc --base32 | tr '[:upper:]' '[:lower:]')
          (umask 077 && echo "$password" > "$password_file.tmp")
          echo "$admin:$password" | chpasswd
          chage --lastday 0 "$admin"
          mv "$password_file.tmp" "$password_file"
        else
          (umask 077 && : > "$password_file")
        fi
      fi
      password=$(cat "$password_file")
      if [[ -n $password && $(shadow 3) != 0 ]]; then
        : > "$password_file"
        password=
      fi

      # The site: the domain chosen in setup, else a name derived from the
      # address of the route out (the public IPv4 address on most VPS).
      address=
      for _ in $(seq 30); do
        address=$(ip -4 -json route get 192.0.2.1 | jq -r '.[0].prefsrc // empty') || true
        [[ -n $address ]] && break
        sleep 2
      done
      lines=()
      if [[ -z $address ]]; then
        lines+=("Tentaflake: no IPv4 address with a route out yet; retrying.")
      else
        derived=''${address//./-}.sslip.io
        domain=$(tr -d '[:space:]' 2> /dev/null < "$control/domain") || true
        pattern='^([a-z0-9]([a-z0-9-]{0,61}[a-z0-9])?\.)+[a-z]([a-z0-9-]{0,61}[a-z0-9])?$'
        if [[ ''${#domain} -gt 253 || ! $domain =~ $pattern ]]; then domain=$derived; fi

        jq -n --arg address "$address" --arg derived "$derived" \
          '{ address: $address, defaultDomain: $derived }' > "$site/site.json.tmp"
        mv "$site/site.json.tmp" "$site/site.json"
        printf '%s {\n\treverse_proxy 127.0.0.1:${toString controlPort}\n}\n' "$domain" > "$site/site.caddy.tmp"
        if cmp -s "$site/site.caddy.tmp" "$site/site.caddy" && systemctl is-active --quiet caddy.service; then
          rm "$site/site.caddy.tmp"
        else
          mv "$site/site.caddy.tmp" "$site/site.caddy"
          systemctl reload-or-restart --no-block caddy.service
        fi

        lines+=("Tentaflake: https://$domain")
        case $address in
          10.* | 192.168.* | 172.1[6-9].* | 172.2[0-9].* | 172.3[01].*)
            lines+=("  $address is a private address: the name above only works where it is reachable.")
            ;;
        esac
      fi
      # The journal outlives the setup token and the console password, so
      # only the console's notice, a file for root alone, shows them.
      printf '%s\n' "''${lines[@]}"
      if [[ -n $token ]]; then
        echo "  Setup token: on the console, or sudo cat $control/setup-token"
        lines+=("  Setup token: $token")
      fi
      [[ -n $password ]] && lines+=("  Console login: $admin, password $password (replaced at the first login)")
      (umask 077 && printf '%s\n' "''${lines[@]}" "" > /run/issue.d/tentaflake.issue.tmp)
      mv /run/issue.d/tentaflake.issue.tmp /run/issue.d/tentaflake.issue
      agetty --reload || true
      [[ -n $address ]]
    '';
  };

  hardening = {
    CapabilityBoundingSet = "";
    LockPersonality = true;
    NoNewPrivileges = true;
    PrivateDevices = true;
    PrivateTmp = true;
    ProtectClock = true;
    ProtectControlGroups = true;
    ProtectHome = true;
    ProtectHostname = true;
    ProtectKernelLogs = true;
    ProtectKernelModules = true;
    ProtectKernelTunables = true;
    ProtectProc = "invisible";
    ProtectSystem = "strict";
    RemoveIPC = true;
    RestrictNamespaces = true;
    RestrictRealtime = true;
    RestrictSUIDSGID = true;
    SystemCallArchitectures = "native";
    SystemCallErrorNumber = "EPERM";
    SystemCallFilter = [ "@system-service" ];
    Restart = "always";
    RestartSec = "5s";
  };
in
{
  options.tentaflake.control.enable = lib.mkEnableOption ''
    the control service, the broker, the runtime image build and Caddy with
    automatic HTTPS in front of the web UI
  '';

  config = lib.mkIf cfg.enable {
    assertions = [
      {
        assertion = config.tentaflake.containerBackend == "docker" && config.virtualisation.docker.enable;
        message = "tentaflake.control needs Docker: the control service uses the Docker Engine API.";
      }
    ];

    users = {
      users.tentaflake = {
        isSystemUser = true;
        group = "tentaflake";
      };
      users.tentaflake-broker = {
        isSystemUser = true;
        group = "tentaflake-broker";
      };
      groups.tentaflake = { };
      groups.tentaflake-broker = { };
    };

    systemd = {
      paths.tentaflake-setup = {
        wantedBy = [ "multi-user.target" ];
        pathConfig.PathChanged = [
          "${controlState}/domain"
          "${controlState}/setup-token"
          "/etc/shadow"
        ];
      };

      services = {
        # The directory of Caddy's admin socket.
        caddy.serviceConfig.RuntimeDirectory = "caddy";

        tentaflake-setup = {
          description = "Tentaflake secrets, HTTPS site and console notice";
          wantedBy = [ "multi-user.target" ];
          wants = [ "network-online.target" ];
          after = [ "network-online.target" ];
          serviceConfig = {
            Type = "oneshot";
            ExecStart = lib.getExe setup;
            Restart = "on-failure";
            RestartSec = "30s";
          };
        };

        # Built on the host from images/runtime: every pin lives in its Dockerfile.
        tentaflake-runtime-image = {
          description = "Tentaflake runtime image";
          wantedBy = [ "multi-user.target" ];
          wants = [ "network-online.target" ];
          requires = [ "docker.service" ];
          after = [
            "docker.service"
            "network-online.target"
          ];
          path = [ config.virtualisation.docker.package ];
          script = ''
            docker image inspect ${image} > /dev/null 2>&1 || docker build --tag ${image} ${runtimeSource}
          '';
          serviceConfig = {
            Type = "oneshot";
            RemainAfterExit = true;
            Restart = "on-failure";
            RestartSec = "1min";
            TimeoutStartSec = "30min";
          };
        };

        # Privileged through the Docker socket; listens on loopback behind Caddy.
        tentaflake-control = {
          description = "Tentaflake control service";
          wantedBy = [ "multi-user.target" ];
          wants = [ "tentaflake-setup.service" ];
          requires = [ "docker.service" ];
          after = [
            "docker.service"
            "tentaflake-setup.service"
          ];
          serviceConfig = hardening // {
            ExecStart = "${lib.getExe control} ${controlConfig}";
            User = "tentaflake";
            Group = "tentaflake";
            SupplementaryGroups = [ "docker" ];
            StateDirectory = "tentaflake/control";
            StateDirectoryMode = "0700";
            # The broker's routes and credentials. The service hands each file
            # to the broker's user (CAP_CHOWN, which reaches only the paths it
            # can write); the broker opens them by name but cannot list or
            # change the directory.
            RuntimeDirectory = "tentaflake/broker";
            RuntimeDirectoryMode = "0711";
            CapabilityBoundingSet = "CAP_CHOWN";
            AmbientCapabilities = "CAP_CHOWN";
            UMask = "0077";
            RestrictAddressFamilies = [
              "AF_UNIX"
              "AF_INET"
              "AF_INET6"
            ];
            # Bun's JavaScript engine compiles to executable memory.
            MemoryDenyWriteExecute = false;
          };
        };

        # Its own user, without the Docker group: it reads the routes and
        # credentials the control service hands to it, keeps its audit log and
        # budgets, and forwards model calls.
        tentaflake-broker = {
          description = "Tentaflake model broker";
          wantedBy = [ "multi-user.target" ];
          after = [ "tentaflake-control.service" ];
          # It listens on the gateway of the model-only network, which the control
          # service creates; until that exists it fails and starts again.
          startLimitIntervalSec = 0;
          path = [
            pkgs.iproute2
            pkgs.jq
          ];
          script = ''
            address=$(ip -4 -json address show dev ${lib.head bridges} | jq -er '.[0].addr_info[0].local')
            jq -n --arg listen "$address:${toString brokerPort}" '{
              listen: $listen,
              routes_file: "${routes}/routes.json",
              audit_file: "${state}/broker/audit.jsonl",
              budget_state_file: "${state}/broker/budget.json"
            }' > "$RUNTIME_DIRECTORY/broker.json"
            exec ${lib.getExe broker} --config "$RUNTIME_DIRECTORY/broker.json"
          '';
          serviceConfig = hardening // {
            User = "tentaflake-broker";
            Group = "tentaflake-broker";
            StateDirectory = "tentaflake/broker";
            StateDirectoryMode = "0700";
            RuntimeDirectory = "tentaflake/broker-config";
            RuntimeDirectoryMode = "0700";
            UMask = "0077";
            RestrictAddressFamilies = [
              "AF_UNIX"
              "AF_INET"
              "AF_INET6"
              "AF_NETLINK"
            ];
            MemoryDenyWriteExecute = true;
          };
        };
      };
    };
    # Automatic HTTPS for the site the setup service writes; plain HTTP only
    # redirects. HTTP/3 stays off, so UDP 443 stays closed. The admin API is
    # a socket only Caddy can use: no other local user reconfigures the proxy.
    services.caddy = {
      enable = true;
      globalConfig = ''
        admin unix//run/caddy/admin.sock|0600
        servers {
          protocols h1 h2
        }
      '';
      extraConfig = ''
        import ${site}/*.caddy
      '';
    };

    networking.firewall = {
      allowedTCPPorts = [
        80
        443
      ];
      # Environments reach the broker on the model-only gateway, from either network.
      interfaces = lib.genAttrs bridges (_: {
        allowedTCPPorts = [ brokerPort ];
      });
    };
  };
}
