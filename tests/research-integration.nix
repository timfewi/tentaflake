# Real capsule transport; vendor agent images and live providers are not used.
{ self, pkgs }:
let
  inherit (pkgs) lib;
  builders = import ../lib { inherit pkgs lib; };
  genericNames = [
    "generic-research"
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
          image = builders.constants.hermesImage;
          command = [ "/bin/python3" ];
          lifecycle = "stopped";
          capabilities = lib.optional (name == "generic-research") "research";
        };
      }
    ) genericNames;
    virtualisation = {
      memorySize = 2048;
      docker.enable = true;
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
      };
    };
    services.secureResearch = {
      serviceUid = 4201;
      egressUid = 4202;
      vpnInterface = "fixture-vpn";
      resolvers = [ "9.9.9.9" ];
    };
    systemd.tmpfiles.rules = [ "d /run/research-vpn 0755 root root -" ];
    environment = {
      systemPackages = [ (pkgs.python3.withPackages (packages: [ packages.pyyaml ])) ];
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
  '';
}
