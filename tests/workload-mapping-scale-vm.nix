# Explicit slow scale test: the observer's supported maximum of 64 bindings on a
# real rootful Docker engine and systemd must keep every manifest live and fresh
# within the consumers' 10-second lease. NIC-less, synthetic workloads only.
{ pkgs }:
let
  inherit (pkgs) lib;
  count = 64;
  names = lib.genList (index: "n${toString index}") count;
  image = pkgs.dockerTools.buildImage {
    name = "workload-scale-fixture";
    tag = "local";
    copyToRoot = pkgs.buildEnv {
      name = "workload-scale-fixture-root";
      paths = [ pkgs.busybox ];
      pathsToLink = [ "/bin" ];
    };
    config.Cmd = [
      "/bin/sh"
      "-c"
      "while :; do /bin/sleep 1; done"
    ];
  };
  freshness = pkgs.writeText "workload-freshness.py" ''
    import json, sys, time
    from pathlib import Path

    expected, samples = int(sys.argv[1]), int(sys.argv[2])
    lease_ms = 10_000
    worst, stamps = 0, {}
    for _ in range(samples):
        manifests = sorted(Path("/run/tentaflake-workloads").glob("agent-*.json"))
        assert len(manifests) == expected, len(manifests)
        now = time.time() * 1000
        for path in manifests:
            fact = json.loads(path.read_text())
            assert fact["state"] == "live", (path.name, fact["state"])
            age = now - fact["verified_at_ms"]
            assert age < lease_ms, (path.name, age)
            worst = max(worst, age)
            stamps.setdefault(path.name, set()).add(fact["verified_at_ms"])
        time.sleep(1)
    # Over a longer window every binding must be re-stamped repeatedly.
    assert min(len(values) for values in stamps.values()) >= min(5, samples), stamps
    print(f"worst manifest age {worst:.0f} ms over {len(stamps)} bindings")
  '';
in
pkgs.testers.runNixOSTest {
  name = "tentaflake-workload-mapping-scale";
  globalTimeout = 1500;
  nodes.machine = {
    imports = [
      ../modules/agent-instances.nix
      ../modules/workload-mapping.nix
    ];
    virtualisation = {
      cores = 4;
      memorySize = 4096;
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
        containers = lib.genAttrs (map (name: "agent-${name}") names) (_: {
          image = "workload-scale-fixture:local";
          imageFile = image;
          extraOptions = [ "--network=none" ];
        });
      };
    };
    networking = {
      useDHCP = lib.mkForce false;
      defaultGateway = lib.mkForce null;
      defaultGateway6 = lib.mkForce null;
    };
    tentaflake.agentDeclarations = map (name: {
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
    }) names;
    tentaflake.workloadMapping = {
      enable = true;
      bindings = lib.genAttrs (map (name: "agent-${name}") names) (id: {
        tenant_id = "tenant-${id}";
        workload_id = "workload-${id}";
        policy_revision = "scale-1";
      });
    };
    environment.systemPackages = [ pkgs.python3 ];
  };
  testScript = ''
    start_all()
    machine.wait_until_succeeds(
        "test $(systemctl list-units --state=active --no-legend 'docker-agent-*.service' | wc -l) -eq ${toString count}",
        timeout=900,
    )
    machine.succeed("systemctl stop tentaflake-workloads.service")
    machine.succeed("systemctl start tentaflake-workloads-initialize.service")
    machine.succeed("systemctl reset-failed tentaflake-workloads.service; systemctl start tentaflake-workloads.service")
    machine.wait_until_succeeds(
        "python3 ${freshness} ${toString count} 1",
        timeout=120,
    )
    print(machine.succeed("python3 ${freshness} ${toString count} 30"))
  '';
}
