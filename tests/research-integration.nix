# Real capsule transport and owned Generic admission. The test-only observation
# producer/dummy interface exercise local readiness, not VPN/provider acceptance.
{ self, pkgs }:
let
  inherit (pkgs) lib;
  builders = import ../lib { inherit pkgs lib; };
  genericNames = [
    "generic-research"
    "generic-research-model"
    "generic-ungranted"
  ];
  image = pkgs.dockerTools.buildLayeredImage {
    name = "tentaflake-research-fixture";
    tag = "test";
    contents = pkgs.buildEnv {
      name = "research-fixture-root";
      paths = [ pkgs.python3 ];
      pathsToLink = [ "/bin" ];
    };
  };
  descriptor = builders.mkLocalImage image;
  fixtureObserver = pkgs.writeText "research-admission-observer.py" ''
    import json
    import os
    from pathlib import Path
    import tempfile
    import time

    directory = Path("/run/research-vpn")
    while True:
        try:
            mode = (directory / "fixture-mode").read_text().strip()
        except FileNotFoundError:
            mode = "ready"
        assert mode in {"ready", "draining", "offline"}
        # Operator input only: the real controller owns its distinct generation
        # and its three-second output lease. No Internet request is made here.
        observation = {"version": 1, "generation": "d57b890f-6996-41fb-bd08-431c83477815",
                       "mode": mode, "valid_until": int(time.time()) + 10}
        with tempfile.NamedTemporaryFile(mode="w", dir=directory, delete=False) as stream:
            os.fchmod(stream.fileno(), 0o644)
            json.dump(observation, stream)
            stream.flush()
            os.fsync(stream.fileno())
        os.replace(stream.name, directory / "observation.json")
        time.sleep(0.25)
  '';
  readinessProbe = pkgs.writeText "research-admitted-readiness.py" ''
    import http.client
    import json
    import os
    from pathlib import Path
    import signal
    import socket
    import subprocess
    import sys
    import time
    import urllib.parse

    combined = len(sys.argv) == 2 and sys.argv[1] == "combined"
    inside = len(sys.argv) == 1 or combined
    expected_uid = 62104 if combined else 62103
    if inside:
        assert os.getuid() == 10000 and os.getcwd() == "/workspace"
        command = os.environ["TENTAFLAKE_RESEARCH_COMMAND"]
        research_socket = os.environ["TENTAFLAKE_RESEARCH_SOCKET"]
        assert research_socket == "/run/tentaflake-research/socket"
        for path in ["/run/docker.sock", "/run/agent-research/socket", "/run/tentaflake-worker"]:
            assert not os.path.exists(path), path
        if combined:
            assert os.environ["OPENAI_API_KEY"] == os.environ["TENTAFLAKE_BROKER_TOKEN"]
            assert os.environ["OPENAI_BASE_URL"] == os.environ["TENTAFLAKE_LLM_BASE_URL"]
            endpoint = urllib.parse.urlsplit(os.environ["OPENAI_BASE_URL"])
            assert (endpoint.scheme, endpoint.hostname, endpoint.port, endpoint.path) == (
                "http", "10.203.75.1", 7810, "/v1")
            # The same admitted CID must authenticate to the real broker. No
            # model/provider request is sent, and neither key nor body is logged.
            for authenticated in [False, True]:
                connection = http.client.HTTPConnection(endpoint.hostname, endpoint.port, timeout=2)
                try:
                    headers = {"Authorization": "Bearer " + os.environ["OPENAI_API_KEY"]} if authenticated else {}
                    connection.request("GET", "/readyz", headers=headers)
                    response = connection.getresponse()
                    body = response.read(1025)
                    assert len(body) <= 1024
                    assert response.status == (200 if authenticated else 401)
                    if authenticated:
                        assert json.loads(body) == {"status": "ready"}
                finally:
                    connection.close()
        else:
            assert "OPENAI_API_KEY" not in os.environ
        Path("/workspace/research-proof").write_text("workspace")
        Path("/state/research-proof").write_text("state")
        try:
            Path("/root-write-probe").write_text("must fail")
        except OSError:
            pass
        else:
            raise AssertionError("container root was writable")
        try:
            connection = socket.create_connection(("9.9.9.9", 443), timeout=1)
        except OSError:
            pass
        else:
            connection.close()
            raise AssertionError("Research capsule gained direct IP access")
    else:
        # Host synchronization uses the real relay/client/readiness operation.
        # It supplies no evidence to the admission collector.
        command, research_socket, expected_uid = sys.argv[1:]
        expected_uid = int(expected_uid)
    messages = [
        {"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
            "protocolVersion": "2025-11-25", "capabilities": {},
            "clientInfo": {"name": "admitted-research-fixture", "version": "1"}}},
        {"jsonrpc": "2.0", "method": "notifications/initialized"},
        {"jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": {
            "name": "research_job", "arguments": {"operation": "readiness"}}},
    ]
    def expired(signum, frame):
        raise TimeoutError("fixture readiness deadline exceeded")

    signal.signal(signal.SIGALRM, expired)
    signal.alarm(10)
    child = subprocess.Popen([command, "--socket", research_socket],
                             stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True)
    try:
        for message in messages:
            child.stdin.write(json.dumps(message) + "\n")
            child.stdin.flush()
            if "id" not in message:
                continue
            while True:
                line = child.stdout.readline(65537)
                assert line and len(line) <= 65536, "missing or oversized client response"
                response = json.loads(line)
                if response.get("id") == message["id"]:
                    assert "error" not in response
                    break
        assert not response["result"].get("isError", False)
        report = response["result"]["structuredContent"]
        child.stdin.close()
        assert child.wait(timeout=2) == 0
    finally:
        signal.alarm(0)
        if child.poll() is None:
            child.kill()
            child.wait()
    assert report["schema_version"] == 1 and report["caller_uid"] == expected_uid
    assert report["untrusted"] is True and report["accepting"] is True
    assert report["egress"]["mode"] == "ready" and report["egress"]["lease_valid"] is True
    assert len(report["policy_sha256"]) == 64
    if not inside:
        lease = json.loads(Path("/run/agent-research-egress/control/state.json").read_text())
        assert report["egress"]["generation"] == lease["generation"]
        assert lease["mode"] == "ready" and lease["valid_until"] > int(time.time())
    print(json.dumps(report))
  '';
  probe = pkgs.writeText "research-capsule-probe.py" ''
    import json
    import os
    import socket
    import subprocess
    import sys

    assert os.getuid() == int(sys.argv[1])
    for path in ["/run/agent-research/socket", "/var/run/docker.sock", "/run/credentials"]:
        assert not os.path.exists(path), path
    try:
        connection = socket.create_connection(("9.9.9.9", 443), timeout=1)
    except OSError:
        pass
    else:
        connection.close()
        raise AssertionError("capsule had direct public IP access")
    if len(sys.argv) == 2:
        command = os.environ["TENTAFLAKE_RESEARCH_COMMAND"]
        research_socket = os.environ["TENTAFLAKE_RESEARCH_SOCKET"]
        assert research_socket == "/run/tentaflake-research/socket"
        assert os.access(command, os.X_OK), "projected Research command is unavailable"
        assert not os.path.exists("/run/tentaflake-broker")
        for peer in ["hermes-research", "zeroclaw-research", "generic-ungranted"]:
            assert not os.path.exists("/run/tentaflake-research/" + peer)
    elif sys.argv[2] == "ungranted":
        assert "TENTAFLAKE_RESEARCH_COMMAND" not in os.environ
        assert "TENTAFLAKE_RESEARCH_SOCKET" not in os.environ
        assert not os.path.exists("/run/tentaflake-research")
        assert not os.path.exists("/run/tentaflake-broker")
        print("ungranted generic capsule has no Research discovery, socket or direct IP")
        sys.exit(0)
    else:
        command = sys.argv[2]
        research_socket = "/run/tentaflake-research/socket"
    child = subprocess.Popen(
        [command, "--socket", research_socket],
        stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True,
    )
    def send(message):
        child.stdin.write(json.dumps(message) + "\n")
        child.stdin.flush()
    def receive(identifier):
        while True:
            line = child.stdout.readline()
            assert line, "client exited before response"
            message = json.loads(line)
            if message.get("id") == identifier:
                assert "error" not in message, message
                return message["result"]
    try:
        send({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
            "protocolVersion": "2025-11-25", "capabilities": {},
            "clientInfo": {"name": "gvisor-capsule-fixture", "version": "1"}}})
        receive(1)
        send({"jsonrpc": "2.0", "method": "notifications/initialized"})
        send({"jsonrpc": "2.0", "id": 2, "method": "tools/list", "params": {}})
        assert {tool["name"] for tool in receive(2)["tools"]} == {
            "research_job", "research_search", "research_fetch", "research_browser", "research_read"}
        send({"jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": {
            "name": "research_job", "arguments": {"operation": "start"}}})
        started = receive(3)
        assert not started.get("isError", False), started
        job_id = started["structuredContent"]["job"]["id"]
        args = {"job_id": job_id, "urls": ["https://example.com/"], "mode": "http"}
        send({"jsonrpc": "2.0", "id": 4, "method": "tools/call", "params": {
            "name": "research_fetch", "arguments": args}})
        offline = receive(4)
        result = offline.get("structuredContent", {})
        assert offline.get("isError", False) and result.get("error") == "egress_unavailable", offline
        send({"jsonrpc": "2.0", "id": 5, "method": "tools/call", "params": {
            "name": "research_job", "arguments": {"operation": "finish", "job_id": job_id}}})
        assert not receive(5).get("isError", False)
        child.stdin.close()
        assert child.wait(timeout=10) == 0
        print("research MCP works in gVisor; direct IP and offline fetch are denied")
    finally:
        if child.poll() is None:
            child.kill()
            child.wait()
  '';
in
pkgs.testers.runNixOSTest {
  name = "tentaflake-exclusive-research";
  nodes.machine = { config, ... }: {
    imports = [
      self.nixosModules.default
      (builders.mkHermesAgent {
        name = "research";
        autoStart = false;
        settings = {
          agent.disabled_toolsets = [ ];
        };
      })
      (builders.mkZeroClawAgent {
        name = "research";
        autoStart = false;
        settings = {
          browser.enabled = true;
          web_search.enabled = true;
          http_request.enabled = true;
        };
      })
    ]
    ++ map (
      name:
      builders.mkAgent {
        adapter = "generic";
        name = lib.removePrefix "generic-" name;
        definition = {
          schemaVersion = 1;
          image =
            if name != "generic-ungranted" then
              {
                kind = "local-artifact";
                inherit descriptor;
              }
            else
              builders.constants.hermesImage;
          command = [
            "/bin/python3"
            "-u"
            "-c"
            "import signal,sys,time; signal.signal(signal.SIGTERM, lambda *_: sys.exit(0)); print('research-admission-ready', flush=True); time.sleep(86400)"
          ];
          lifecycle = "stopped";
          capabilities =
            lib.optional (name == "generic-research-model") "model"
            ++ lib.optional (name != "generic-ungranted") "research";
          ownership = {
            uid = 10000;
            gid = 10000;
          };
          resources = {
            memory = "256m";
            memorySwap = "256m";
            cpus = "0.5";
            nofile = 128;
            pidsLimit = 128;
            tmpfsSize = "8m";
            runTmpfsSize = "8m";
          };
        };
      }
    ) genericNames;
    # Real Nix database/root registration for prebuilt closures only. Empty
    # build-users-group avoids changing ownership of the shared store; Nix's
    # internal remount is confined to each service's private mount namespace.
    nix = {
      enable = true;
      nrBuildUsers = 0;
      settings = {
        build-users-group = lib.mkForce "";
        max-jobs = lib.mkForce 0;
        builders = lib.mkForce "";
        substituters = lib.mkForce [ ];
        substitute = false;
      };
    };
    virtualisation = {
      writableStore = false;
      fileSystems."/nix/store".options = [ "ro" ];
      memorySize = 2048;
      cores = 2;
      diskSize = 2048;
      docker.enable = true;
      docker.daemon.settings = {
        live-restore = false;
        features.containerd-snapshotter = false;
      };
      oci-containers.backend = "docker";
    };
    tentaflake = {
      hostName = "research-fixture";
      adminUser = "operator";
      boot.enable = false;
      modernConsole.enable = false;
      nixSettings.enable = false;
      research.agents = {
        hermes-research.uid = 62101;
        zeroclaw-research.uid = 62102;
        generic-research.uid = 62103;
        generic-research-model.uid = 62104;
      };
      broker.agents.generic-research-model = {
        enable = true;
        networkName = "tf-generic-research-model";
        subnet = "10.203.75.0/30";
        gateway = "10.203.75.1";
        llm = {
          enable = true;
          # Readiness never contacts this reserved upstream. Provider acceptance
          # is outside this combined local transport/lifecycle fixture.
          upstreamBaseUrl = "https://combined-fixture.invalid/v1/";
          providerCredentialFile = "/run/tentaflake-research-model-fixture/provider";
          allowedModels = [
            {
              name = "fixture/model";
              inputMicrousdPerMillion = 1;
              outputMicrousdPerMillion = 1;
            }
          ];
        };
      };
      security.admission = {
        authorityEpoch = "research-vm";
        physicalSpaceHeadroomBytes = 67108864;
        capacity = {
          memoryBytes = 1744830464;
          memorySwapBytes = 1744830464;
          nanoCpus = 2000000000;
          pids = 1472;
          workspaceBytes = 268435456;
          stateBytes = 268435456;
        };
        hostReserve = {
          memoryBytes = 402653184;
          memorySwapBytes = 402653184;
          nanoCpus = 500000000;
          pids = 320;
          workspaceBytes = 0;
          stateBytes = 0;
        };
        researchReserve = {
          memoryBytes = 1073741824;
          memorySwapBytes = 1073741824;
          nanoCpus = 1000000000;
          pids = 1024;
          workspaceBytes = 0;
          stateBytes = 0;
        };
        maxActive = 1;
      };
      workspaceQuota.agents = lib.genAttrs [ "generic-research" "generic-research-model" ] (name: {
        enable = true;
        workspace = "/var/lib/${name}/workspace";
        sizeMiB = 128;
        ownerUid = 10000;
        ownerGid = 10000;
        state = {
          path = "/var/lib/${name}/state";
          sizeMiB = 128;
        };
      });
    };
    services.secureResearch = {
      serviceUid = 4201;
      egressUid = 4202;
      vpnInterface = "fixture-vpn";
      resolvers = [ "9.9.9.9" ];
    };
    systemd = {
      units =
        lib.genAttrs
          [
            "nix-store.mount"
            "nix-.ro\\x2dstore.mount"
          ]
          (_: {
            overrideStrategy = "asDropin";
            text = ''
              [Unit]
              DefaultDependencies=no
            '';
          });
      tmpfiles.rules = [ "d /run/research-vpn 0755 root root -" ];
      # The earlier transport projections run their own ephemeral fixture CIDs.
      # Only the declared admitted Generic units may mutate Docker after bootstrap.
      services =
        lib.genAttrs
          [
            "docker-hermes-research"
            "docker-zeroclaw-research"
            "docker-generic-ungranted"
          ]
          (_: {
            enable = false;
          })
        // {
          register-nix-paths = {
            preStart = ''
              test -d /nix/store/.links && test ! -L /nix/store/.links
            '';
            serviceConfig.PrivateMounts = true;
          };
          nix-daemon = {
            preStart = ''
              test -d /nix/store/.links && test ! -L /nix/store/.links
            '';
            serviceConfig.PrivateMounts = true;
          };
          tentaflake-research-fixture-observer.serviceConfig = {
            Type = "simple";
            ExecStart = "${pkgs.python3}/bin/python3 ${fixtureObserver}";
            User = "root";
            KillMode = "control-group";
            TimeoutStopSec = 5;
          };
        };
      slices.agent-research.sliceConfig = {
        MemoryMax = lib.mkForce "1073741824";
        CPUQuota = "100%";
      };
    };
    environment = {
      systemPackages = [
        (pkgs.python3.withPackages (packages: [ packages.pyyaml ]))
        pkgs.iproute2
        pkgs.util-linux
      ];
      etc = {
        "research-fixture-image".source = image;
        "research-fixture-mounts.json".text = builtins.toJSON (
          lib.genAttrs
            (
              [
                "hermes-research"
                "zeroclaw-research"
              ]
              ++ genericNames
            )
            (
              name:
              (import ../lib/researchClient.nix {
                inherit config lib pkgs;
                containerName = name;
                settings = null;
              }).volumes
            )
        );
        "research-fixture-generic.json".text = builtins.toJSON (
          lib.genAttrs genericNames (
            name:
            let
              container = config.virtualisation.oci-containers.containers.${name};
            in
            {
              inherit (container)
                environment
                environmentFiles
                networks
                extraOptions
                ;
              inherit (config.tentaflake.agentInstances.${name}) requestedCapabilities;
            }
          )
        );
        "research-fixture-configs.json".text = builtins.toJSON (
          lib.genAttrs [ "hermes-research" "zeroclaw-research" ] (
            name:
            let
              suffix = if lib.hasPrefix "hermes-" name then "/config.yaml:ro" else "/config.toml:ro";
              mounts = config.virtualisation.oci-containers.containers.${name}.volumes;
              mount = lib.findFirst (
                volume: lib.hasSuffix suffix volume
              ) (throw "missing generated agent settings") mounts;
            in
            lib.head (lib.splitString ":" mount)
          )
        );
      };
    };
  };
  testScript = ''
    import json
    import shlex

    start_all()
    machine.wait_for_unit("docker.service")
    machine.succeed("docker load < /etc/research-fixture-image")
    mounts = json.loads(machine.succeed("cat /etc/research-fixture-mounts.json"))
    generic = json.loads(machine.succeed("cat /etc/research-fixture-generic.json"))
    sources = json.loads(machine.succeed("cat /etc/research-fixture-configs.json"))
    policies = {}
    for name, path in sources.items():
        parser = "yaml.safe_load" if name.startswith("hermes-") else "tomllib.loads"
        script = f"import json, yaml, tomllib; print(json.dumps({parser}(open({path!r}).read())))"
        policies[name] = json.loads(machine.succeed("python3 -c " + shlex.quote(script)))
    assert {"web", "browser"}.issubset(policies["hermes-research"]["agent"]["disabled_toolsets"])
    assert set(policies["hermes-research"]["mcp_servers"]) == {"secure-research-tool"}
    for tool in ["browser", "web_search", "web_fetch", "http_request"]:
        assert policies["zeroclaw-research"][tool]["enabled"] is False
    assert policies["zeroclaw-research"]["mcp"]["servers"][0]["name"] == "secure-research-tool"
    # Project the generated discovery and exact grants into the existing fixture
    # image. This does not start a generated generic unit or accept a vendor image.
    for name, uid in [("hermes-research", 10000), ("zeroclaw-research", 65534),
                      ("generic-research", 10000), ("generic-ungranted", 10000)]:
        granted = name != "generic-ungranted"
        if granted:
            machine.wait_for_unit(f"tentaflake-research-{name}.socket")
        argv = ["docker", "run", "--rm", "--runtime=runsc", "--network=none", "--read-only", "--user", f"{uid}:{uid}",
                "--cap-drop=ALL", "--security-opt=no-new-privileges:true", "--security-opt=apparmor=docker-default",
                "--pids-limit=128", "--memory=256m", "--tmpfs=/run:rw,nosuid,nodev,noexec,size=16m"]
        if name in generic:
            declaration = generic[name]
            assert declaration["requestedCapabilities"] == (["research"] if granted else [])
            assert declaration["networks"] == [], "Research-only capsule gained a model network"
            assert declaration["environmentFiles"] == [], "Research-only capsule gained model credentials"
            assert "--network=none" in declaration["extraOptions"]
            expected_environment = {"HOME", "TENTAFLAKE_RESEARCH_COMMAND", "TENTAFLAKE_RESEARCH_SOCKET"} if granted else {"HOME"}
            assert set(declaration["environment"]) == expected_environment
            expected_socket = [f"/run/tentaflake-research/{name}:/run/tentaflake-research:ro"] if granted else []
            assert [mount for mount in mounts[name] if mount.startswith("/run/")] == expected_socket
            assert all(mount.endswith(":ro") for mount in mounts[name])
            if not granted:
                assert mounts[name] == [], "ungranted generic capsule gained a client closure"
                machine.fail("test -e /run/tentaflake-research/generic-ungranted/socket")
            for key, value in declaration["environment"].items():
                argv += ["--env", f"{key}={value}"]
            client_arguments = [] if granted else ["ungranted"]
        else:
            client = policies[name]["mcp_servers"]["secure-research-tool"]["command"] if name.startswith("hermes-") else policies[name]["mcp"]["servers"][0]["command"]
            client_arguments = [client]
        for mount in mounts[name] + ["${probe}:/research-probe.py:ro"]:
            argv += ["--volume", mount]
        argv += ["tentaflake-research-fixture:test", "/bin/python3", "/research-probe.py", str(uid)] + client_arguments
        machine.succeed(shlex.join(argv))

    # The preceding transport tests are now completely stopped. From here the
    # generated unit, installed plan, physical quotas and durable ledger own OCI.
    agent = "generic-research"
    plan_path = machine.succeed("readlink -f /etc/tentaflake/admission.json").strip()
    plan = json.loads(machine.succeed("cat " + shlex.quote(plan_path)))
    runtime = plan["runtimes"][agent]
    research = runtime["research"]
    unit = runtime["unit"]
    controller = research["controller"]["unit"]
    observer = "tentaflake-research-fixture-observer.service"
    control = research["controlPath"]
    relay_socket = research["relay"]["socketPath"]
    ledger = "/var/lib/tentaflake-admission/ledger.json"
    volumes = {volume["spec"]["kind"]: volume for volume in plan["volumes"] if volume["spec"]["instanceId"] == agent}
    combined_agent = "generic-research-model"
    assert set(plan["runtimes"]) == {agent, combined_agent} and runtime["model"] is None
    assert plan["instances"][agent]["requestedCapabilities"] == ["research"]
    assert research["relay"]["template"]["uid"] == 62103
    assert runtime["image"] == {"kind": "local-artifact", "descriptor": "${descriptor}"}

    def admission(action, identifier=None):
        args = ["tentaflake", "admission", action, plan_path]
        if identifier is not None:
            args.append(identifier)
        return shlex.join(args)

    def docker(*args):
        return shlex.join(["timeout", "20s", "docker", "--host=unix:///run/docker.sock", "--config=/var/empty", *args])

    def snapshot():
        return json.loads(machine.succeed("cat " + ledger))

    def current(identifier=agent):
        return max((receipt for receipt in snapshot()["receipts"] if receipt["request"]["instanceId"] == identifier),
                   key=lambda receipt: receipt["claimedRevision"])

    def stop_unit(name, allow_failed=False):
        machine.succeed("timeout 150s systemctl stop " + shlex.quote(name))
        state = machine.succeed("systemctl show --value --property=ActiveState " + shlex.quote(name)).strip()
        assert state == "inactive" or (allow_failed and state == "failed"), (name, state)
        jobs = machine.succeed("systemctl list-jobs --no-legend --no-pager --full")
        assert all(name not in line.split() for line in jobs.splitlines()), jobs

    def start_agent(succeeds=True, identifier=agent):
        selected_unit = plan["runtimes"][identifier]["unit"]
        previous = machine.succeed("systemctl show --value --property=InvocationID " + shlex.quote(selected_unit)).strip()
        command = "timeout 320s systemctl start " + shlex.quote(selected_unit)
        if succeeds:
            machine.succeed(command)
            machine.wait_for_unit(selected_unit)
        else:
            machine.fail(command)
            invocation = machine.succeed("systemctl show --value --property=InvocationID " + shlex.quote(selected_unit)).strip()
            assert invocation != previous and len(invocation) == 32 and all(c in "0123456789abcdef" for c in invocation), invocation
            for property_name, expected in [("ActiveState", "failed"), ("Result", "exit-code"), ("ExecMainStatus", "2")]:
                actual = machine.succeed("systemctl show --value --property=" + property_name + " " + shlex.quote(selected_unit)).strip()
                assert actual == expected, (property_name, actual)
            # Reset only this observed fresh admission refusal while the failed
            # state still pins the unit loaded. A rate-limit rejection is not
            # evidence that the collector ran or refused this changed source.
            machine.succeed("systemctl reset-failed " + shlex.quote(selected_unit))
            stop_unit(selected_unit)

    def nix_evidence(retained=False):
        machine.wait_for_unit("register-nix-paths.service")
        machine.wait_for_unit("nix-daemon.socket")
        assert machine.succeed("systemctl show --value --property=Result register-nix-paths.service").strip() == "success"
        options = machine.succeed("findmnt --task 1 --first-only --direction backward -n -o OPTIONS --mountpoint /nix/store").strip().split(",")
        assert "ro" in options and "rw" not in options, options
        machine.succeed("test -d /nix/store/.links && test ! -L /nix/store/.links")
        if retained:
            assert plan_path.startswith("/nix/store/") and "/" not in plan_path[len("/nix/store/"):], plan_path
            root = "/nix/var/nix/gcroots/tentaflake/" + plan_path.rsplit("/", 1)[1]
            assert machine.succeed("readlink -- " + shlex.quote(root)).strip() == plan_path
            assert machine.succeed("test -L " + shlex.quote(root) + " && stat -c %u:%h -- " + shlex.quote(root)).strip() == "0:1"

    def denied_without_dispatch(identifier=agent):
        before = snapshot()
        start_agent(succeeds=False, identifier=identifier)
        assert snapshot() == before, "capability refusal changed durable claims"
        machine.fail(docker("container", "inspect", identifier))

    def wait_ready(identifier=agent):
        selected = plan["runtimes"][identifier]["research"]
        ready_command = shlex.join([
            "timeout", "15s", "python3", "${readinessProbe}",
            selected["client"]["executable"], selected["relay"]["socketPath"],
            str(selected["relay"]["template"]["uid"]),
        ])
        machine.wait_until_succeeds(ready_command, timeout=40)
        return json.loads(machine.succeed("cat " + shlex.quote(control)))

    with subtest("Research-only Generic uses explicit image acquisition and fenced bootstrap"):
        nix_evidence()
        for selected in plan["runtimes"].values():
            stop_unit(selected["unit"])
        for volume in plan["volumes"]:
            image_path = volume["spec"]["imagePath"]
            machine.succeed("test ! -e " + shlex.quote(image_path) + " && test ! -e " + shlex.quote(image_path + ".new"))
            owner = volume["units"]["owner"]
            failed = machine.succeed("systemctl show --value --property=ActiveState " + shlex.quote(owner)).strip() == "failed"
            if failed:
                invocation = machine.succeed("systemctl show --value --property=InvocationID " + shlex.quote(owner)).strip()
                assert len(invocation) == 32 and all(c in "0123456789abcdef" for c in invocation), invocation
                assert machine.succeed("systemctl show --value --property=ExecMainStatus " + shlex.quote(owner)).strip() == "2"
                diagnostic = machine.succeed(shlex.join([
                    "journalctl", "--no-pager", "-o", "cat", "-n", "20", "_SYSTEMD_INVOCATION_ID=" + invocation,
                ]))
                assert "tentaflake: admission ledger unavailable; explicit bootstrap or recovery required" in diagnostic.splitlines(), diagnostic
            stop_unit(owner, allow_failed=failed)
        machine.succeed("test ! -e /var/lib/tentaflake-admission")
        for legacy in ["docker-hermes-research.service", "docker-zeroclaw-research.service", "docker-generic-ungranted.service"]:
            assert machine.succeed("systemctl show --value --property=LoadState " + legacy).strip() == "masked"
        daemon_info = json.loads(machine.succeed(docker("info", "--format", "{{json .}}")))
        assert daemon_info["Driver"] and all(row[0] != "driver-type" for row in daemon_info["DriverStatus"]), daemon_info["DriverStatus"]
        machine.succeed(admission("image-acquire", agent))
        machine.succeed("test ! -e /var/lib/tentaflake-admission")
        daemon_units = ["docker.service"]
        if machine.succeed("systemctl show --value --property=LoadState docker.socket").strip() != "not-found":
            daemon_units.append("docker.socket")
        machine.succeed("timeout 150s systemctl stop " + shlex.join(daemon_units))
        print(machine.succeed(${
          builtins.toJSON (
            "timeout 60s bash -euo pipefail -c "
            + pkgs.lib.escapeShellArg (builtins.readFile ./fixtures/admission-mount-inventory.sh)
          )
        }))
        machine.succeed(admission("bootstrap-fence"))
        machine.succeed("timeout 150s systemctl start docker.service")
        machine.wait_for_unit("docker.service")
        machine.succeed(admission("bootstrap"))
        nix_evidence(retained=True)
        for kind in ["state", "workspace"]:
            machine.succeed("timeout 80s systemctl start " + shlex.quote(volumes[kind]["units"]["owner"]))
        storage = snapshot()["storage"]
        assert len(storage) == 2 and all(row["allocation"]["state"] == "present" for row in storage)
        assert sum(row["allocation"]["bytes"] for row in storage) == 268435456
        assert snapshot()["receipts"] == []
        machine.fail(admission("runtime-start", agent))
        machine.succeed("ip link add fixture-vpn type dummy")
        machine.succeed("ip link set dev fixture-vpn up")
        requires = machine.succeed("systemctl show --value --property=Requires " + unit).split()
        assert {controller, research["service"]["unit"], research["egress"]["unit"]}.issubset(requires)
        # No observation producer exists yet; socket/process availability alone
        # cannot admit this first actual Generic invocation.
        denied_without_dispatch()
        machine.succeed("systemctl start " + observer)
        machine.succeed("systemctl start agent-research.service agent-research-egress.service")
        wait_ready()

    with subtest("missing and expired real controller leases cannot dispatch"):
        for missing in [True, False]:
            wait_ready()
            machine.succeed("systemctl kill --kill-whom=main --signal=STOP " + controller)
            saved = control + ".fixture-saved"
            try:
                if missing:
                    machine.succeed(shlex.join(["mv", "--", control, saved]))
                else:
                    expired = "import json,time; assert json.load(open(" + repr(control) + "))['valid_until'] <= int(time.time())"
                    machine.wait_until_succeeds("python3 -c " + shlex.quote(expired), timeout=10)
                denied_without_dispatch()
            finally:
                if missing:
                    machine.succeed(shlex.join(["mv", "--", saved, control]))
                machine.succeed("systemctl kill --kill-whom=main --signal=CONT " + controller)
            wait_ready()

    with subtest("real draining and offline controller states cannot dispatch"):
        for mode in ["draining", "offline"]:
            machine.succeed("printf '%s\\n' " + mode + " > /run/research-vpn/fixture-mode.next")
            machine.succeed("mv /run/research-vpn/fixture-mode.next /run/research-vpn/fixture-mode")
            condition = "import json; assert json.load(open(" + repr(control) + "))['mode'] == " + repr(mode)
            machine.wait_until_succeeds("python3 -c " + shlex.quote(condition), timeout=10)
            denied_without_dispatch()
            machine.succeed("rm /run/research-vpn/fixture-mode")
            wait_ready()

    with subtest("active relay unit without its exact socket path cannot dispatch"):
        machine.succeed(shlex.join(["mv", "--", relay_socket, relay_socket + ".fixture-saved"]))
        try:
            assert machine.succeed("systemctl show --value --property=ActiveState " + research["relay"]["socketUnit"]).strip() == "active"
            denied_without_dispatch()
        finally:
            machine.succeed(shlex.join(["mv", "--", relay_socket + ".fixture-saved", relay_socket]))
        wait_ready()

    with subtest("a relay process with another UID is not the granted Research transport"):
        template = research["relay"]["template"]["unit"]
        directory = "/run/systemd/system/" + template + ".d"
        dropin = directory + "/90-research-fixture.conf"
        socket_unit = research["relay"]["socketUnit"]
        stop_unit(socket_unit)
        machine.succeed(shlex.join(["mkdir", "-p", directory]))
        machine.succeed("printf '%s\\n' '[Service]' 'User=10000' > " + shlex.quote(dropin))
        machine.succeed("systemctl daemon-reload")
        machine.succeed("systemctl start " + socket_unit)
        try:
            denied_without_dispatch()
        finally:
            stop_unit(socket_unit)
            machine.succeed(shlex.join(["rm", "--", dropin]))
            machine.succeed("systemctl daemon-reload")
            machine.succeed("systemctl start " + socket_unit)
        wait_ready()

    with subtest("backend start dependencies must produce actual live processes"):
        for backend in [research["service"]["unit"], research["egress"]["unit"]]:
            stop_unit(backend)
            directory = "/run/systemd/system/" + backend + ".d"
            dropin = directory + "/90-research-fixture.conf"
            machine.succeed(shlex.join(["mkdir", "-p", directory]))
            machine.succeed("printf '%s\\n' '[Unit]' 'ConditionPathExists=/run/research-fixture-never-created' > " + shlex.quote(dropin))
            machine.succeed("systemctl daemon-reload")
            try:
                denied_without_dispatch()
                assert machine.succeed("systemctl show --value --property=MainPID " + backend).strip() == "0"
            finally:
                machine.succeed(shlex.join(["rm", "--", dropin]))
                machine.succeed("systemctl daemon-reload")
                machine.succeed("systemctl start agent-research.service agent-research-egress.service")
            wait_ready()

    def assert_active(identifier=agent):
        selected_runtime = plan["runtimes"][identifier]
        selected_research = selected_runtime["research"]
        model = selected_runtime["model"]
        receipt = current(identifier)
        assert receipt["reservation"]["state"] == "active"
        assert receipt["reservation"]["resources"] == plan["instances"][identifier]["resources"]
        dispatch = receipt["runtime"]
        cid = dispatch["containerId"]
        assert len(cid) == 64 and all(c in "0123456789abcdef" for c in cid)
        observed = json.loads(machine.succeed(docker("container", "inspect", cid)))[0]
        assert observed["Id"] == cid and observed["State"]["Running"]
        assert observed["Image"] == dispatch["intent"]["imageId"]
        assert dispatch["intent"]["image"] == selected_runtime["image"]
        assert dispatch["intent"]["specRevision"] == plan["instances"][identifier]["revision"]
        assert observed["Config"]["Labels"]["io.tentaflake.operation"] == receipt["request"]["operationId"]
        assert observed["Config"]["User"] == "10000:10000"
        host = observed["HostConfig"]
        assert host["Runtime"] == "runsc"
        if model:
            network = json.loads(machine.succeed(docker("network", "inspect", model["network"]["name"])))[0]
            assert host["NetworkMode"] == network["Id"]
            assert network["Internal"] is True and set(network["Containers"]) == {cid}
            assert network["Options"]["com.docker.network.bridge.name"] == model["network"]["bridge"]
            assert network["IPAM"]["Config"] == [{"Subnet": model["network"]["subnet"], "Gateway": model["network"]["gateway"]}]
            assert set(observed["NetworkSettings"]["Networks"]) == {model["network"]["name"]}
            assert observed["NetworkSettings"]["Networks"][model["network"]["name"]]["NetworkID"] == network["Id"]
        else:
            assert host["NetworkMode"] == "none"
        assert host["ReadonlyRootfs"] and not host["Privileged"] and not host["AutoRemove"]
        assert set(host["CapDrop"]) == {"ALL"} and not host["CapAdd"]
        security = {option.replace(":", "=", 1) for option in host["SecurityOpt"]}
        assert security in (
            {"no-new-privileges=true", "apparmor=docker-default"},
            {"no-new-privileges", "apparmor=docker-default"},
        )
        assert host["Memory"] == 268435456 and host["MemorySwap"] == 268435456
        assert host["NanoCpus"] == 500000000 and host["PidsLimit"] == 128
        assert {item["Name"]: (item["Soft"], item["Hard"]) for item in host["Ulimits"]} == {
            "nofile": (128, 128), "nproc": (128, 128),
        }
        assert set(host["Tmpfs"]) == {"/tmp", "/run", "/var/tmp"}
        for options in host["Tmpfs"].values():
            assert set(options.split(",")) == {"rw", "nosuid", "nodev", "noexec", "size=8m"}
        expected_mounts = {}
        for volume in selected_runtime["container"]["volumes"]:
            source, target, mode = volume.split(":")
            expected_mounts[target] = (source, mode != "ro")
        actual_mounts = {item["Destination"]: (item["Source"], item["RW"]) for item in observed["Mounts"] if item["Type"] == "bind"}
        assert actual_mounts == expected_mounts
        assert actual_mounts["/run/tentaflake-research"] == ("/run/tentaflake-research/" + identifier, False)
        assert not any(item["Type"] == "volume" for item in observed["Mounts"])
        probe_args = ["combined"] if model else []
        report = json.loads(machine.succeed(docker("exec", cid, "/bin/python3", "-c", machine.succeed("cat ${readinessProbe}"), *probe_args)))
        lease = json.loads(machine.succeed("cat " + shlex.quote(control)))
        assert report["egress"]["generation"] == lease["generation"]
        assert report["caller_uid"] == selected_research["relay"]["template"]["uid"]
        machine.wait_until_succeeds("journalctl --no-pager -o cat -n 60 -u " + selected_runtime["unit"] + " | grep -Fx research-admission-ready", timeout=30)
        return receipt, lease["generation"]

    with subtest("owned Generic reaches Active only with authenticated same-generation Research"):
        wait_ready()
        start_agent()
        first, generation = assert_active()
        assert snapshot()["storage"] == storage

    with subtest("controller loss stops the exact CID and old generation cannot restart"):
        stop_unit(controller)
        machine.wait_until_succeeds("test $(systemctl show --value --property=ActiveState " + unit + ") = inactive", timeout=40)
        retained = current()
        assert retained["reservation"]["state"] == "retained"
        assert retained["runtime"] == first["runtime"]
        assert retained["reservation"]["resources"]["memoryBytes"] == 0
        machine.fail(docker("container", "inspect", first["runtime"]["containerId"]))
        stop_unit(observer)
        machine.succeed("rm /run/research-vpn/observation.json")
        machine.succeed("systemctl start " + controller)
        denied_without_dispatch()
        machine.succeed("systemctl start " + observer)
        renewed = wait_ready()
        assert renewed["generation"] != generation
        start_agent()
        second, current_generation = assert_active()
        assert current_generation == renewed["generation"]
        assert second["request"]["operationId"] != first["request"]["operationId"]
        assert second["runtime"]["containerId"] != first["runtime"]["containerId"]
        stop_unit(unit)
        assert current()["reservation"]["state"] == "retained"
        assert snapshot()["storage"] == storage
        machine.fail(docker("container", "inspect", second["runtime"]["containerId"]))

    with subtest("one Generic requires both live Model and Research before a combined claim"):
        combined_runtime = plan["runtimes"][combined_agent]
        combined_unit = combined_runtime["unit"]
        combined_research = combined_runtime["research"]
        model = combined_runtime["model"]
        assert plan["instances"][combined_agent]["requestedCapabilities"] == ["model", "research"]
        assert model is not None and combined_research is not None
        assert combined_research["relay"]["template"]["uid"] == 62104
        assert combined_runtime["image"] == runtime["image"]
        assert {grant["capability"] for grant in plan["policy"]["grants"]
                if grant["instanceId"] == combined_agent and grant["ownerId"] == "host-root"} == {"model", "research"}
        assert plan["policy"]["maxActive"] == 1
        assert all(receipt["reservation"]["state"] == "retained" for receipt in snapshot()["receipts"])
        requires = machine.succeed("systemctl show --value --property=Requires " + combined_unit).split()
        assert {model["unit"], controller, combined_research["service"]["unit"], combined_research["egress"]["unit"]}.issubset(requires)
        combined_volumes = {volume["spec"]["kind"]: volume for volume in plan["volumes"]
                            if volume["spec"]["instanceId"] == combined_agent}
        for kind in ["state", "workspace"]:
            machine.succeed("timeout 80s systemctl start " + shlex.quote(combined_volumes[kind]["units"]["owner"]))
        combined_storage = snapshot()["storage"]
        assert len(combined_storage) == 4 and all(row["allocation"]["state"] == "present" for row in combined_storage)
        assert sum(row["allocation"]["bytes"] for row in combined_storage) == 536870912
        # Runtime-only fixture credential, never passed in argv/output or sent
        # upstream. The real broker/config/collector are used without patches.
        machine.succeed("python3 - <<'PY'\nimport os\nfrom pathlib import Path\np = Path('/run/tentaflake-research-model-fixture')\np.mkdir(mode=0o700)\nfd = os.open(p / 'provider', os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o400)\nwith os.fdopen(fd, 'w') as stream:\n    stream.write(os.urandom(32).hex() + '\\n')\nPY")
        broker = model["unit"]
        health_command = "python3 -c " + shlex.quote(
            "import json,urllib.request; response=urllib.request.urlopen('http://10.203.75.1:7810/healthz', timeout=2); "
            "assert response.status == 200 and json.load(response) == {'status':'ready'}")
        machine.succeed("systemctl start " + broker)
        machine.wait_until_succeeds(health_command, timeout=30)
        wait_ready(combined_agent)

        # Research remains genuinely ready while the required broker has no
        # process. A skipped dependency must not become positive Model evidence.
        stop_unit(broker)
        directory = "/run/systemd/system/" + broker + ".d"
        dropin = directory + "/90-research-fixture.conf"
        machine.succeed(shlex.join(["mkdir", "-p", directory]))
        machine.succeed("printf '%s\\n' '[Unit]' 'ConditionPathExists=/run/research-fixture-never-created' > " + shlex.quote(dropin))
        machine.succeed("systemctl daemon-reload")
        try:
            wait_ready(combined_agent)
            denied_without_dispatch(combined_agent)
            assert machine.succeed("systemctl show --value --property=MainPID " + broker).strip() == "0"
        finally:
            machine.succeed(shlex.join(["rm", "--", dropin]))
            machine.succeed("systemctl daemon-reload")
            machine.succeed("systemctl start " + broker)
        machine.wait_until_succeeds(health_command, timeout=30)

        # Conversely a healthy broker cannot authorize a draining Research
        # controller. Both collectors must accept this same startup invocation.
        machine.succeed("printf '%s\\n' draining > /run/research-vpn/fixture-mode.next")
        machine.succeed("mv /run/research-vpn/fixture-mode.next /run/research-vpn/fixture-mode")
        condition = "import json; assert json.load(open(" + repr(control) + "))['mode'] == 'draining'"
        try:
            machine.wait_until_succeeds("python3 -c " + shlex.quote(condition), timeout=10)
            machine.succeed(health_command)
            denied_without_dispatch(combined_agent)
        finally:
            machine.succeed("rm /run/research-vpn/fixture-mode")
        wait_ready(combined_agent)

    with subtest("the same admitted CID authenticates Model and Research without provider requests"):
        audit_command = "sha256sum -- /var/lib/tentaflake-broker-llm-generic-research-model/audit.jsonl"
        audit_before = machine.succeed(audit_command)
        budget_absent = "test ! -e /var/lib/tentaflake-broker-llm-generic-research-model/budget.json"
        machine.succeed(budget_absent)
        start_agent(identifier=combined_agent)
        combined_receipt, combined_generation = assert_active(combined_agent)
        assert snapshot()["storage"] == combined_storage
        assert current()["reservation"]["state"] == "retained"
        assert sum(receipt["reservation"]["state"] == "active" for receipt in snapshot()["receipts"]) == 1
        # The probe uses only /readyz and Research's read-only readiness RPC.
        assert machine.succeed(audit_command) == audit_before
        machine.succeed(budget_absent)
        stop_unit(broker)
        machine.wait_until_succeeds("test $(systemctl show --value --property=ActiveState " + combined_unit + ") = inactive", timeout=40)
        retained = current(combined_agent)
        assert retained["reservation"]["state"] == "retained"
        assert retained["runtime"] == combined_receipt["runtime"]
        assert retained["reservation"]["resources"]["memoryBytes"] == 0
        assert snapshot()["storage"] == combined_storage
        machine.fail(docker("container", "inspect", combined_receipt["runtime"]["containerId"]))
        assert wait_ready(combined_agent)["generation"] == combined_generation
    nix_evidence(retained=True)
    for mount_unit in ["nix-store.mount", r"nix-.ro\x2dstore.mount"]:
        assert machine.succeed("systemctl show --value --property=DefaultDependencies " + shlex.quote(mount_unit)).strip() == "no"
        assert "umount.target" not in machine.succeed("systemctl show --value --property=Conflicts " + shlex.quote(mount_unit)).split()
    machine.shutdown()
  '';
}
