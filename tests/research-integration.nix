# Real capsule transport; vendor agent images and live providers are not used.
{ self, pkgs }:
let
  inherit (pkgs) lib;
  builders = import ../lib { inherit pkgs lib; };
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
    child = subprocess.Popen(
        [sys.argv[2], "--socket", "/run/tentaflake-research/socket"],
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
    ];
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
          lib.genAttrs [ "hermes-research" "zeroclaw-research" ] (
            name:
            (import ../lib/researchClient.nix {
              inherit config lib pkgs;
              containerName = name;
              settings = null;
            }).volumes
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
    for name, uid in [("hermes-research", 10000), ("zeroclaw-research", 65534)]:
        machine.wait_for_unit(f"tentaflake-research-{name}.socket")
        client = policies[name]["mcp_servers"]["secure-research-tool"]["command"] if name.startswith("hermes-") else policies[name]["mcp"]["servers"][0]["command"]
        argv = ["docker", "run", "--rm", "--runtime=runsc", "--network=none", "--read-only", "--user", f"{uid}:{uid}",
                "--cap-drop=ALL", "--security-opt=no-new-privileges:true", "--security-opt=apparmor=docker-default",
                "--pids-limit=128", "--memory=256m", "--tmpfs=/run:rw,nosuid,nodev,noexec,size=16m"]
        for mount in mounts[name] + ["${probe}:/research-probe.py:ro"]:
            argv += ["--volume", mount]
        argv += ["tentaflake-research-fixture:test", "/bin/python3", "/research-probe.py", str(uid), client]
        machine.succeed(shlex.join(argv))
  '';
}
