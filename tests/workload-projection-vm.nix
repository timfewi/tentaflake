# Synthetic-only integration test. Inject reviewed, pinned local snapshots; this
# expression deliberately discovers neither sibling repositories nor flake inputs.
# hostpulseModule is an instantiated NixOS module, not its package-taking factory.
# UI HTTP permission/scope acceptance remains separate: no UI is mocked here.
{
  pkgs,
  hostpulseModule,
  hostpulsePackage,
}:
let
  inherit (pkgs) lib;
  fixture = ./workload-projection-fixture.py;
  image = pkgs.dockerTools.buildImage {
    name = "workload-projection-synthetic";
    tag = "local";
    copyToRoot = pkgs.buildEnv {
      name = "workload-projection-synthetic-root";
      paths = [ pkgs.python3 ];
      pathsToLink = [ "/bin" ];
    };
    config.Cmd = [
      "${pkgs.python3}/bin/python3"
      "${fixture}"
      "workload"
    ];
  };
in
pkgs.testers.runNixOSTest {
  name = "tentaflake-workload-projection";
  globalTimeout = 600;
  nodes.machine = {
    imports = [
      ../modules/agent-instances.nix
      ../modules/workload-mapping.nix
      hostpulseModule
    ];
    virtualisation = {
      cores = 2;
      memorySize = 2048;
      vlans = lib.mkForce [ ];
      interfaces = lib.mkForce { };
      qemu.networkingOptions = lib.mkForce [ "-nic none" ];
      docker = {
        enable = true;
        daemon.settings = {
          exec-opts = [ "native.cgroupdriver=systemd" ];
          bridge = "none";
          iptables = false;
          ip6tables = false;
        };
      };
      oci-containers = {
        backend = "docker";
        containers = lib.genAttrs [ "agent-one" "agent-two" ] (name: {
          image = "workload-projection-synthetic:local";
          imageFile = image;
          environment.FIXTURE_MIB = if name == "agent-one" then "8" else "32";
          extraOptions = [
            "--network=none"
            "--cpus=0.5"
            "--pids-limit=32"
            (if name == "agent-one" then "--memory=96m" else "--memory=160m")
          ];
        });
      };
    };
    networking = {
      useDHCP = lib.mkForce false;
      defaultGateway = lib.mkForce null;
      defaultGateway6 = lib.mkForce null;
    };
    tentaflake.agentDeclarations =
      map
        (name: {
          adapter = "custom";
          inherit name;
          container = "agent-${name}";
          unit = "docker-agent-${name}.service";
          stateDir = "/var/lib/tentaflake/agents/${name}";
          stateStorage = "/var/lib/tentaflake/agents/${name}";
          workspace = "/var/lib/tentaflake/agents/${name}/workspace";
          uid = 2000;
          gid = 2000;
          runnable = true;
        })
        [
          "one"
          "two"
        ];
    tentaflake.workloadMapping = {
      enable = true;
      bindings = lib.genAttrs [ "agent-one" "agent-two" ] (
        agent:
        let
          name = lib.removePrefix "agent-" agent;
        in
        {
          tenant_id = "tenant-${name}";
          workload_id = "workload-${name}";
          policy_revision = "fixture-1";
        }
      );
    };
    users.users = {
      reader-one = {
        isNormalUser = true;
        uid = 2100;
      };
      reader-two = {
        isNormalUser = true;
        uid = 2101;
      };
      outsider = {
        isNormalUser = true;
        uid = 2102;
      };
    };
    services.hostpulse = {
      package = hostpulsePackage;
      system = {
        enable = true;
        configFile = "/run/workload-projection-policy/config.toml";
        projectionUiUid = 2100;
      };
    };
    systemd.services.workload-projection-policy = {
      before = [ "hostpulse-system.service" ];
      requiredBy = [ "hostpulse-system.service" ];
      serviceConfig = {
        Type = "oneshot";
        RemainAfterExit = true;
        TimeoutStartSec = 15;
        ExecStart = "${pkgs.python3}/bin/python3 ${fixture} provision";
      };
    };
    systemd.services.hostpulse-system.serviceConfig.TimeoutStartSec = 30;
    environment.systemPackages = [ pkgs.python3 ];
  };
  testScript = ''
    start_all()
    machine.wait_for_unit("multi-user.target", timeout=90)
    machine.succeed("timeout --signal=KILL 450 python3 ${fixture} test", timeout=460)
  '';
}
