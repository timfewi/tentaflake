# Explicit slow, isolated kernel/engine test; a consumer may extend it with its own collector/UI
# modules and assertions without replacing this resolver or its synthetic workloads.
{
  pkgs,
  extraModules ? [ ],
  extraTestScript ? "",
}:
let
  image = pkgs.dockerTools.buildImage {
    name = "workload-mapping-fixture";
    tag = "local";
    copyToRoot = pkgs.buildEnv {
      name = "workload-mapping-fixture-root";
      paths = [ pkgs.busybox ];
      pathsToLink = [ "/bin" ];
    };
    config.Cmd = [
      "/bin/sh"
      "-c"
      "while :; do /bin/sleep 1; done"
    ];
  };
  declaration = name: {
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
  };
in
pkgs.testers.runNixOSTest {
  name = "tentaflake-workload-mapping";
  globalTimeout = 600;
  nodes.machine = { ... }: {
    imports = [
      ../modules/agent-instances.nix
      ../modules/workload-mapping.nix
    ]
    ++ extraModules;
    virtualisation = {
      cores = 2;
      memorySize = 2048;
      vlans = pkgs.lib.mkForce [ ];
      interfaces = pkgs.lib.mkForce { };
      qemu.networkingOptions = pkgs.lib.mkForce [ "-nic none" ];
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
        containers = builtins.listToAttrs (
          map
            (name: {
              name = "agent-${name}";
              value = {
                image = "workload-mapping-fixture:local";
                imageFile = image;
                extraOptions = [ "--network=none" ];
              };
            })
            [
              "one"
              "two"
            ]
        );
      };
    };
    networking = {
      useDHCP = pkgs.lib.mkForce false;
      defaultGateway = pkgs.lib.mkForce null;
      defaultGateway6 = pkgs.lib.mkForce null;
    };
    tentaflake.agentDeclarations = map declaration [
      "one"
      "two"
    ];
    tentaflake.workloadMapping = {
      enable = true;
      bindings = {
        agent-one = {
          tenant_id = "tenant-one";
          workload_id = "workload-one";
          policy_revision = "fixture-1";
        };
        agent-two = {
          tenant_id = "tenant-two";
          workload_id = "workload-two";
          policy_revision = "fixture-1";
        };
      };
    };
    users.users.reader = {
      isNormalUser = true;
      uid = 2100;
    };
    environment.systemPackages = [ pkgs.python3 ];
  };
  testScript = ''
    import json
    start_all()
    machine.succeed("python3 -c 'from pathlib import Path; assert not any((p / \"device\").exists() for p in Path(\"/sys/class/net\").iterdir())'")
    assert json.loads(machine.succeed("ip -j -4 route show default")) == []
    assert json.loads(machine.succeed("ip -j -6 route show default")) == []
    machine.wait_for_unit("docker-agent-one.service")
    machine.wait_for_unit("docker-agent-two.service")
    # Missing authority fails closed; bootstrap is never an automatic dependency.
    machine.fail("test -e /var/lib/tentaflake-workloads/authority.json")
    machine.succeed("systemctl stop tentaflake-workloads.service")
    machine.succeed("systemctl start tentaflake-workloads-initialize.service")
    machine.succeed("systemctl reset-failed tentaflake-workloads.service; systemctl start tentaflake-workloads.service")
    def fact(name):
        return json.loads(machine.succeed("cat /run/tentaflake-workloads/agent-" + name + ".json"))
    def live(name):
        machine.wait_until_succeeds("python3 -c 'import json; assert json.load(open(\"/run/tentaflake-workloads/agent-" + name + ".json\"))[\"state\"] == \"live\"'", timeout=45)
        return fact(name)
    one = live("one")
    two = live("two")
    assert one["cgroup"] != two["cgroup"]
    assert one["container_id"] != two["container_id"]
    assert one["generation"] != two["generation"]
    for current in [one, two]:
        pid = str(current["pid"])
        assert machine.succeed("cat /proc/" + pid + "/cgroup").strip() == "0::" + current["cgroup"]
        assert int(machine.succeed("stat -c %i /sys/fs/cgroup" + current["cgroup"])) == current["cgroup_inode"]
        assert machine.succeed("stat -c '%u %a' /run/tentaflake-workloads/" + current["agent_id"] + ".json").strip() == "0 444"
    machine.succeed("su -s /bin/sh reader -c 'cat /run/tentaflake-workloads/agent-one.json >/dev/null'")
    machine.fail("su -s /bin/sh reader -c 'cat /var/lib/tentaflake-workloads/authority.json'")
    machine.fail("su -s /bin/sh reader -c 'docker --host=unix:///run/docker.sock ps'")
    machine.succeed("systemctl stop docker-agent-one.service")
    machine.wait_until_succeeds("python3 -c 'import json; assert json.load(open(\"/run/tentaflake-workloads/agent-one.json\"))[\"state\"] == \"unavailable\"'", timeout=45)
    stopped = fact("one")
    assert stopped["generation"] > one["generation"]
    assert all(stopped[k] is None for k in ["kernel_boot_id", "container_id", "pid", "start_ticks", "cgroup", "cgroup_inode"])
    machine.succeed("systemctl start docker-agent-one.service")
    restarted = live("one")
    assert restarted["generation"] > stopped["generation"]
    machine.succeed("systemctl restart tentaflake-workloads.service")
    assert live("one")["generation"] == restarted["generation"]
  ''
  + extraTestScript
  + ''
    # Corrupt authority is neither overwritten nor silently bootstrapped.
    machine.succeed("systemctl stop tentaflake-workloads.service")
    machine.succeed("printf invalid > /var/lib/tentaflake-workloads/authority.json")
    machine.succeed("systemctl start tentaflake-workloads.service")
    machine.wait_until_succeeds("systemctl show tentaflake-workloads.service -p SubState --value | grep -E 'auto-restart|failed'", timeout=30)
    machine.fail("systemctl start tentaflake-workloads-initialize.service")
    machine.succeed("test $(cat /var/lib/tentaflake-workloads/authority.json) = invalid")
    machine.succeed("systemctl stop tentaflake-workloads.service docker-agent-one.service docker-agent-two.service")
  '';
}
