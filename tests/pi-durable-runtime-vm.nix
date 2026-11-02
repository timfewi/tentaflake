# Explicit, slow runtime fixture. Build/run the driver separately, never in fast.
# No provider request is issued: the configured broker upstream is loopback only.
{
  self,
  pkgs,
  runtimePackage,
  bundle,
}:
let
  inherit (pkgs) lib;
  mkAgent = import ../lib/mkAgent.nix { inherit lib pkgs; };
  names = [
    "pi-durable-a"
    "pi-durable-b"
  ];
  agents = lib.genAttrs names (name: "agent-${name}");
  # gVisor's virtual proc/status omits NoNewPrivs. Query the implemented prctl
  # getter instead of treating an absent presentation field as runtime evidence.
  noNewPrivsProbe = pkgs.pkgsStatic.runCommandCC "pi-durable-no-new-privs-probe" { } ''
    mkdir -p "$out/bin"
    "$CC" -O2 -Wall -Wextra -Werror -x c -o "$out/bin/pi-durable-no-new-privs-probe" - <<'C'
    #include <stdio.h>
    #include <sys/prctl.h>
    int main(void) {
      int value = prctl(PR_GET_NO_NEW_PRIVS, 0, 0, 0, 0);
      if (value != 1) {
        fprintf(stderr, "PR_GET_NO_NEW_PRIVS must be 1, got %d\n", value);
        return 1;
      }
      return 0;
    }
    C
  '';
  probe = pkgs.writeText "pi-durable-host-transport-probe.py" ''
    import importlib.util, json, os, socket, stat, struct, sys
    spec = importlib.util.spec_from_file_location("host_api", "${../host-api/tentaflake_host_api.py}")
    api = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(api)
    name = sys.argv[1]
    descriptor = {"transport": "runsc-pi-durable-v1", "socket": f"/run/tentaflake-pi-durable/{name}/control/runner.sock",
                  "token": f"/run/tentaflake-pi-durable/{name}/api-token"}
    assert os.geteuid() == 0
    assert 10100 in os.getgroups()
    status = open("/proc/self/status").read()
    assert "CapEff:\t0000000000000000" in status
    assert "CapBnd:\t0000000000000000" in status
    service = object.__new__(api.Service)
    if len(sys.argv) > 2:
        try:
            service.runner_exchange(descriptor, "GET", "/v1/capabilities")
        except api.APIError:
            sys.exit(0)
        raise AssertionError("unsafe transport was accepted")
    metadata = os.stat(descriptor["socket"])
    assert (metadata.st_uid, metadata.st_gid, stat.S_IMODE(metadata.st_mode)) == (10100, 10100, 0o620)
    with socket.socket(socket.AF_UNIX) as connection:
        connection.connect(descriptor["socket"])
        pid, uid, gid = struct.unpack("3i", connection.getsockopt(socket.SOL_SOCKET, socket.SO_PEERCRED, 12))
    assert uid == 0, (pid, uid, gid)
    gofer = open(f"/proc/{pid}/status").read()
    assert gofer.split("Uid:\t", 1)[1].splitlines()[0].split() == ["0"] * 4
    assert open(f"/proc/{pid}/uid_map").read().split() == ["0", "0", "4294967295"]
    result = service.runner_exchange(descriptor, "GET", "/v1/capabilities")
    assert result["runtime"] == "pi-durable"
    print(json.dumps({"listener_pid": pid, "listener_uid": uid, "inode_uid": metadata.st_uid}))
  '';
  sandboxProbe = pkgs.writeText "pi-durable-sandbox-probe.mjs" ''
    import assert from 'node:assert/strict';
    import fs from 'node:fs';
    import net from 'node:net';
    import { Research } from '${runtimePackage}/lib/tentaflake-pi-durable/mediation.mjs';
    assert.equal(process.getuid(), 10100);
    for (const path of ['/run/tentaflake-pi-durable/agent-pi-durable-b', '/run/tentaflake-pi-durable/agent-pi-durable-a',
      '/var/run/docker.sock', '/run/podman/podman.sock', '/run/tentaflake-api',
      '/run/tentaflake-research/agent-pi-durable-a', '/run/tentaflake-research/agent-pi-durable-b']) {
      assert.equal(fs.existsSync(path), false, path);
    }
    assert.throws(() => fs.writeFileSync('/etc/escape', 'denied'));
    const status = fs.readFileSync('/proc/self/status', 'utf8');
    assert.match(status, /CapEff:\s+0000000000000000/);
    // NoNewPrivs is separately checked through PR_GET_NO_NEW_PRIVS, not this
    // virtual proc/status, which does not expose the field in pinned gVisor.
    // All three addresses are assigned to loopback inside this NIC-less guest.
    // A guest-side positive probe proves the listeners exist before this denial.
    for (const host of ['198.18.0.1', '198.18.0.2', '198.18.0.3']) {
      await new Promise((resolve, reject) => {
        const socket = net.connect({host, port: 18443});
        socket.setTimeout(1000, () => { socket.destroy(); resolve(); });
        socket.on('error', resolve);
        socket.on('connect', () => { socket.destroy(); reject(new Error('direct egress: ' + host)); });
      });
    }
    const research = new Research(process.argv[2]);
    try {
      await research.initialize();
      // Exact babdca58 src/api.rs + service.rs: readiness is network-free and
      // reports the upstream-authenticated relay UID, not the capsule UID.
      const readiness = await research.request('tools/call', {name:'research_job', arguments:{operation:'readiness'}});
      assert.equal(readiness.isError ?? false, false);
      assert.equal(readiness.structuredContent.caller_uid, Number(process.argv[3]));
      const started = await research.request('tools/call', {name:'research_job', arguments:{operation:'start'}});
      assert.equal(started.isError ?? false, false);
      const job_id = started.structuredContent.job.id;
      const offline = await research.request('tools/call', {name:'research_fetch', arguments:{job_id, urls:['https://fixture.invalid/'], mode:'http'}});
      assert.equal(offline.isError, true);
      assert.equal(offline.structuredContent.error, 'egress_unavailable');
      const done = await research.request('tools/call', {name:'research_job', arguments:{operation:'finish', job_id}});
      assert.equal(done.isError ?? false, false);
    } finally { research.close(); }
  '';
in
pkgs.testers.runNixOSTest {
  name = "tentaflake-pi-durable-runtime";
  globalTimeout = 900;
  nodes.machine = { config, ... }: {
    imports = [
      self.nixosModules.default
    ]
    ++ map (
      name:
      mkAgent {
        adapter = "pi-durable";
        inherit name;
        autoStart = false;
      }
    ) names;
    virtualisation = {
      cores = 2;
      memorySize = 4096;
      diskSize = 8192;
      docker.enable = true;
      oci-containers.backend = "docker";
      # Pinned nixpkgs qemu-vm.nix:360,1278-1299 otherwise adds a user/slirp
      # NIC independently of testing/network.nix's VLAN NICs. Disable both.
      qemu.networkingOptions = lib.mkForce [ "-nic none" ];
      vlans = lib.mkForce [ ];
      interfaces = lib.mkForce { };
    };
    networking = {
      useDHCP = lib.mkForce false;
      nameservers = lib.mkForce [ "198.18.0.1" ];
      defaultGateway = lib.mkForce null;
      defaultGateway6 = lib.mkForce null;
      # The host module normally selects NetworkManager. This NIC-less fixture
      # has one explicit loopback owner instead; leave all firewall rules intact.
      networkmanager.enable = lib.mkForce false;
    };
    systemd = {
      services = {
        # Preserve the production configured-management assertions, but make
        # enrollment/daemon activity impossible in this NIC-less fixture.
        # This test does not establish remote management or SSH acceptance.
        tailscaled.unitConfig.ConditionPathExists = "/run/pi-durable-fixture/never-enroll";
        tailscaled-set.unitConfig.ConditionPathExists = "/run/pi-durable-fixture/never-enroll";
        pi-durable-fixture-loopback = {
          wantedBy = [ "multi-user.target" ];
          after = [ "network.target" ];
          before = [ "pi-durable-fixture-listener.service" ];
          serviceConfig = {
            Type = "oneshot";
            RemainAfterExit = true;
            CapabilityBoundingSet = [ "CAP_NET_ADMIN" ];
            NoNewPrivileges = true;
          };
          script = ''
            ${pkgs.iproute2}/bin/ip link set dev lo up
            ${pkgs.iproute2}/bin/ip address replace 198.18.0.1/32 dev lo
            ${pkgs.iproute2}/bin/ip address replace 198.18.0.2/32 dev lo
            ${pkgs.iproute2}/bin/ip address replace 198.18.0.3/32 dev lo
          '';
        };
        pi-durable-fixture-listener = {
          wantedBy = [ "multi-user.target" ];
          requires = [ "pi-durable-fixture-loopback.service" ];
          after = [ "pi-durable-fixture-loopback.service" ];
          serviceConfig = {
            ExecStart = "${pkgs.python3}/bin/python -m http.server 18443 --bind 0.0.0.0 --directory ${pkgs.emptyDirectory}";
            DynamicUser = true;
            NoNewPrivileges = true;
            CapabilityBoundingSet = "";
            RestrictAddressFamilies = [ "AF_INET" ];
          };
        };
        # A test-only probe unit mirrors the API authority boundary, not a root shell.
        pi-durable-transport-probe = {
          serviceConfig = {
            Type = "oneshot";
            User = "root";
            SupplementaryGroups = [ "10100" ];
            CapabilityBoundingSet = "";
            AmbientCapabilities = "";
            NoNewPrivileges = true;
            ProtectSystem = "strict";
            ProtectHome = true;
            PrivateTmp = true;
            RestrictAddressFamilies = [ "AF_UNIX" ];
            ExecStart = "${pkgs.python3}/bin/python ${probe} agent-pi-durable-a";
          };
        };
      };
      tmpfiles.rules = [
        "d /run/research-vpn 0755 root root -"
        "d /run/pi-durable-fixture 0700 root root -"
        "f /run/pi-durable-fixture/provider 0400 root root - synthetic-never-used"
      ];
    };
    tentaflake = {
      hostName = "pi-durable-fixture";
      adminUser = "operator";
      boot.enable = false;
      modernConsole.enable = false;
      nixSettings.enable = false;
      piDurableRuntime = {
        enable = true;
        transport = "runsc-pi-durable-v1";
        package = runtimePackage;
        inherit bundle;
        image = bundle.imageSelector;
      };
      security.resources = {
        memory = "512m";
        memorySwap = "512m";
        cpus = "1";
      };
      hostApi.enable = true;
      hostApi.peers = [
        {
          uid = 12345;
          agents = [ "agent-pi-durable-a" ];
          permissions = [
            "read"
            "run_read"
          ];
        }
      ];
      research.agents = {
        agent-pi-durable-a.uid = 62101;
        agent-pi-durable-b.uid = 62102;
      };
      broker.agents = lib.listToAttrs (
        lib.imap0 (index: name: {
          name = agents.${name};
          value = {
            enable = true;
            subnet = "10.203.${toString (80 + index)}.0/30";
            gateway = "10.203.${toString (80 + index)}.1";
            llm = {
              enable = true;
              streaming.enable = true;
              upstreamBaseUrl = "https://127.0.0.1:9443/v1/";
              providerCredentialFile = "/run/pi-durable-fixture/provider";
              # Present in the hash-pinned pi-durable-ai 1.4.0 openai-responses catalog;
              # only its name is used for runner initialization, never a request.
              allowedModels = [
                {
                  name = "gpt-4.1-mini";
                  inputMicrousdPerMillion = 1;
                  outputMicrousdPerMillion = 1;
                }
              ];
            };
          };
        }) names
      );
      worker.agents = lib.listToAttrs (
        map (name: {
          name = agents.${name};
          value = {
            enable = true;
            workspace = "/var/lib/tentaflake/agents/${name}/workspace";
            containerUid = 10100;
            containerGid = 10100;
          };
        }) names
      );
      workspaceQuota.agents = lib.listToAttrs (
        map (name: {
          name = agents.${name};
          value = {
            enable = true;
            workspace = "/var/lib/tentaflake/agents/${name}/workspace";
            sizeMiB = 128;
            state = {
              path = "/var/lib/tentaflake/agents/${name}/state";
              sizeMiB = 128;
            };
          };
        }) names
      );
    };
    services.secureResearch = {
      serviceUid = 4201;
      egressUid = 4202;
      vpnInterface = "fixture-vpn";
      resolvers = [ "198.18.0.1" ];
    };
    # Only the reviewed configuration is present; both real units above are
    # explicitly prevented from starting, with no credentials or Tailnet access.
    services.tailscale.enable = lib.mkForce true;
    environment = {
      systemPackages = [
        pkgs.python3
        pkgs.util-linux
        pkgs.curl
      ];
      etc = {
        "pi-durable-fixture-research-client".text =
          "${config.tentaflake.research.clientPackage}/bin/research-client";
        "pi-durable-fixture-probe".source = probe;
        "pi-durable-fixture-sandbox-probe".source = sandboxProbe;
      };
    };
  };
  testScript = ''
    import json, shlex
    start_all()
    machine.wait_for_unit("docker.service")
    machine.fail("systemctl is-active --quiet tailscaled.service")
    machine.fail("systemctl is-active --quiet tailscaled-set.service")
    # No QEMU NIC, host/slirp gateway, default route or external DNS path.
    machine.succeed("python3 -c 'from pathlib import Path; assert not any((p / \"device\").exists() for p in Path(\"/sys/class/net\").iterdir())'")
    assert json.loads(machine.succeed("ip -j -4 route show default")) == []
    assert json.loads(machine.succeed("ip -j -6 route show default")) == []
    quota_units = [
        f"tentaflake-{kind}-quota-agent-{name}.service"
        for name in ["pi-durable-a", "pi-durable-b"] for kind in ["state", "workspace"]
    ]
    try:
        machine.succeed("systemctl start " + " ".join(quota_units), timeout=60)
    except Exception:
        print(machine.execute("systemctl --no-pager --full status 'tentaflake-pi-durable-state-*' 'tentaflake-*-quota-*'")[1])
        print(machine.execute("journalctl -b --no-pager -n 100 -u 'tentaflake-pi-durable-state-*' -u 'tentaflake-*-quota-*'")[1])
        raise
    machine.wait_for_unit("pi-durable-fixture-loopback.service", timeout=15)
    addresses = json.loads(machine.succeed("ip -j -4 address show dev lo"))
    assigned = {entry["local"] for link in addresses for entry in link["addr_info"]}
    assert {"198.18.0.1", "198.18.0.2", "198.18.0.3"}.issubset(assigned), assigned
    machine.wait_for_unit("pi-durable-fixture-listener.service", timeout=15)
    for address in ["198.18.0.1", "198.18.0.2", "198.18.0.3"]:
        machine.wait_until_succeeds(f"curl --noproxy '*' --fail --silent --max-time 2 http://{address}:18443/ > /dev/null", timeout=15)
    machine.succeed("systemctl start docker-agent-pi-durable-a.service docker-agent-pi-durable-b.service", timeout=300)
    for index, name in enumerate(["agent-pi-durable-a", "agent-pi-durable-b"]):
        machine.wait_until_succeeds(f"test -S /run/tentaflake-pi-durable/{name}/control/runner.sock", timeout=120)
        data = json.loads(machine.succeed(f"docker inspect {name}"))[0]
        host = data["HostConfig"]
        assert host["Runtime"] == "runsc-pi-durable"
        assert host["ReadonlyRootfs"] and "ALL" in host["CapDrop"]
        security_options = set(host["SecurityOpt"])
        assert security_options & {"no-new-privileges", "no-new-privileges=true", "no-new-privileges:true"}, security_options
        assert not security_options & {"no-new-privileges=false", "no-new-privileges:false"}, security_options
        workspace = f"/var/lib/tentaflake/agents/{name[6:]}/workspace"
        machine.succeed(f"install -m 0500 -o 10100 -g 10100 ${noNewPrivsProbe}/bin/pi-durable-no-new-privs-probe {workspace}/nnp-probe")
        machine.succeed(f"docker exec {name} /workspace/nnp-probe")
        machine.succeed(f"rm {workspace}/nnp-probe")
        assert host["Memory"] == 512 * 1024 * 1024 and host["PidsLimit"] == 512
        assert host["NanoCpus"] == 1000000000
        assert data["Config"]["User"] == "10100:10100"
        machine.succeed(f"test $(stat -c %u:%g:%a /run/tentaflake-pi-durable/{name}) = 0:0:700")
        machine.succeed(f"test $(stat -c %u:%g:%a /run/tentaflake-pi-durable/{name}/control) = 10100:10100:710")
        machine.succeed(f"test $(stat -c %u:%g:%a /run/tentaflake-pi-durable/{name}/api-token) = 0:10100:440")
        for leaf in ["state", "workspace"]:
            machine.succeed(f"test $(stat -c %u:%g:%a /var/lib/tentaflake/agents/{name[6:]}/{leaf}) = 10100:10100:700")
        # UID10100 outside the private root cannot traverse even its own endpoint.
        machine.fail(f"setpriv --reuid=10100 --regid=10100 --clear-groups stat /run/tentaflake-pi-durable/{name}/control/runner.sock")
        client = machine.succeed("cat /etc/pi-durable-fixture-research-client").strip()
        # Feed code over stdin; do not introduce any additional sandbox bind mount.
        machine.succeed(f"docker exec -i {name} ${pkgs.nodejs_24}/bin/node --input-type=module - {shlex.quote(client)} {62101 + index} < /etc/pi-durable-fixture-sandbox-probe", timeout=90)
    machine.succeed("systemctl start pi-durable-transport-probe.service")
    machine.succeed("systemctl is-active tentaflake-api.service")
    # Exercise the actual API unit, including its resource-scoped incoming grants.
    def api_status(agent, uid):
        command = f"setpriv --reuid={uid} --regid={uid} --clear-groups curl --silent --output /dev/null --write-out '%{{http_code}}' --unix-socket /run/tentaflake-api/host.sock http://localhost/v1/agents/{agent}/runner/capabilities"
        return machine.succeed(command).strip()
    assert api_status("agent-pi-durable-a", 12345) == "200"
    assert api_status("agent-pi-durable-b", 12345) == "403"
    assert api_status("agent-pi-durable-a", 0) == "403"
    # Concrete provenance negatives run with the same empty-capability identity.
    def refusal():
        machine.succeed("systemd-run --wait --collect -p User=root -p SupplementaryGroups=10100 -p CapabilityBoundingSet= -p AmbientCapabilities= -p NoNewPrivileges=yes ${pkgs.python3}/bin/python /etc/pi-durable-fixture-probe agent-pi-durable-a refuse")
    root = "/run/tentaflake-pi-durable/agent-pi-durable-a"
    machine.succeed(f"chmod 0755 {root}")
    refusal()
    machine.succeed(f"chmod 0700 {root}; chmod 0770 {root}/control")
    refusal()
    machine.succeed(f"chmod 0710 {root}/control; chmod 0660 {root}/control/runner.sock")
    refusal()
    machine.succeed(f"chmod 0620 {root}/control/runner.sock; mv {root}/control {root}/saved; ln -s saved {root}/control")
    refusal()
    machine.succeed(f"rm {root}/control; mv {root}/saved {root}/control")
    # A correct-shape but foreign bearer must not authenticate to the real runner.
    machine.succeed(f"cp -p {root}/api-token {root}/saved-token; printf '%064d\\n' 0 > {root}/api-token")
    refusal()
    machine.succeed(f"cp -p {root}/saved-token {root}/api-token; rm {root}/saved-token")
    machine.succeed("systemctl start pi-durable-transport-probe.service")
    machine.succeed("systemctl stop docker-agent-pi-durable-a.service docker-agent-pi-durable-b.service")
    machine.succeed("test $(docker ps -q | wc -l) = 0")
  '';
}
