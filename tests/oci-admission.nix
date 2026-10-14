# Actual offline OCI admission through the installed CLI, ledger and systemd.
# The locally built archive has content identity, not registry/publisher proof.
# Storage publication and physical-headroom cases stay in storage-admission.nix.
{ self }:
let
  # Imports must not depend on the node's config-derived pkgs argument.
  pkgs = self.inputs.nixpkgs.legacyPackages.x86_64-linux;
  builders = self.lib.x86_64-linux;
  archive = pkgs.dockerTools.buildLayeredImage {
    name = "tentaflake-oci-admission-fixture";
    tag = "fixture";
    contents = pkgs.buildEnv {
      name = "tentaflake-oci-admission-root";
      paths = [ pkgs.busybox ];
      pathsToLink = [ "/bin" ];
    };
    config = {
      Env = [ "PATH=/bin" ];
      User = "65534:65534";
    };
  };
  descriptor = builders.mkLocalImage archive;
  names = [
    "oci-a"
    "oci-b"
  ];
  definition = {
    schemaVersion = 1;
    image = {
      kind = "local-artifact";
      inherit descriptor;
    };
    command = [
      "/bin/sh"
      "-c"
      "trap 'exit 0' TERM INT; printf 'oci-admission-ready\\n'; while :; do sleep 3600 & wait $!; done"
    ];
    lifecycle = "stopped";
    capabilities = [
      "files"
      "shell"
      "terminal"
    ];
    ownership = {
      uid = 65534;
      gid = 65534;
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
  node = { lib, ... }: {
    imports = [
      self.nixosModules.default
    ]
    ++ map (
      name:
      builders.mkAgent {
        adapter = "generic";
        inherit name definition;
      }
    ) names;
    virtualisation = {
      oci-containers.backend = "docker";
      docker.enable = true;
      docker.daemon.settings."live-restore" = false;
      memorySize = 1536;
      diskSize = 2048;
    };
    environment.systemPackages = [ pkgs.util-linux ];
    tentaflake = {
      hostName = "oci-host";
      adminUser = "admin";
      boot.enable = false;
      modernConsole.enable = false;
      nixSettings.enable = false;
      shell.enable = true;
      security.admission = {
        authorityEpoch = "oci-vm";
        physicalSpaceHeadroomBytes = 67108864;
        # maxActive permits two; the aggregate compute budget minus the explicit
        # host reserve admits exactly one of these otherwise identical agents.
        capacity = {
          memoryBytes = 536870912;
          memorySwapBytes = 536870912;
          nanoCpus = 1000000000;
          pids = 256;
          workspaceBytes = 268435456;
          stateBytes = 268435456;
        };
        hostReserve = {
          memoryBytes = 268435456;
          memorySwapBytes = 268435456;
          nanoCpus = 500000000;
          pids = 128;
          workspaceBytes = 0;
          stateBytes = 0;
        };
        researchReserve = lib.genAttrs [
          "memoryBytes"
          "memorySwapBytes"
          "nanoCpus"
          "pids"
          "workspaceBytes"
          "stateBytes"
        ] (_: 0);
        maxActive = 2;
      };
      workspaceQuota.agents = lib.genAttrs (map (name: "generic-${name}") names) (name: {
        enable = true;
        workspace = "/var/lib/${name}/workspace";
        sizeMiB = 128;
        ownerUid = 65534;
        ownerGid = 65534;
        state = {
          path = "/var/lib/${name}/state";
          sizeMiB = 128;
        };
      });
    };
    system.stateVersion = "26.05";
  };
in
{
  inherit node;
  # A separate clean ledger makes both terminal Unknown cases independent.
  # The caller starts this identical node only after `oci` has shut down.
  crashNode = node;

  testScript = ''
    import json
    import shlex

    # The caller starts this node after its other integration nodes shut down.
    oci.start()
    oci.wait_for_unit("multi-user.target")
    oci.wait_for_unit("docker.service")
    oci_plan = oci.succeed("readlink -f /etc/tentaflake/admission.json").strip()
    oci_manifest = json.loads(oci.succeed("cat " + shlex.quote(oci_plan)))
    oci_ids = ["generic-oci-a", "generic-oci-b"]
    oci_ledger = "/var/lib/tentaflake-admission/ledger.json"
    oci_units = {name: oci_manifest["runtimes"][name]["unit"] for name in oci_ids}
    oci_volumes = {item["spec"]["allocationId"]: item for item in oci_manifest["volumes"]}

    def oci_command(action, name=None):
        args = ["tentaflake", "admission", action, oci_plan]
        if name is not None:
            args.append(name)
        return shlex.join(args)

    def oci_docker(*args):
        return shlex.join(["timeout", "10s", "docker", "--host=unix:///run/docker.sock", "--config=/var/empty", *args])

    def oci_snapshot():
        return json.loads(oci.succeed("cat " + oci_ledger))

    def oci_current(name):
        receipts = [item for item in oci_snapshot()["receipts"] if item["request"]["instanceId"] == name]
        return max(receipts, key=lambda item: item["claimedRevision"])

    def oci_stop_unit(unit, allow_failed=False):
        oci.succeed("timeout 150s systemctl stop " + shlex.quote(unit))
        state = oci.succeed("systemctl show --value --property=ActiveState " + shlex.quote(unit)).strip()
        assert state == "inactive" or (allow_failed and state == "failed"), (unit, state)
        jobs = oci.succeed("systemctl list-jobs --no-legend --no-pager --full")
        assert all(unit not in line.split() for line in jobs.splitlines()), jobs

    def oci_start(name, succeeds=True):
        command = "timeout 320s systemctl start " + shlex.quote(oci_units[name])
        if succeeds:
            oci.succeed(command)
            oci.wait_for_unit(oci_units[name])
        else:
            oci.fail(command)

    def oci_inspect(identity):
        return json.loads(oci.succeed(oci_docker("container", "inspect", identity)))[0]

    def oci_bootstrap(node, plan):
        units = ["docker.service"]
        if node.succeed("systemctl show --value --property=LoadState docker.socket").strip() != "not-found":
            units.append("docker.socket")
        node.succeed("timeout 150s systemctl stop " + shlex.join(units))
        node.succeed(shlex.join(["tentaflake", "admission", "bootstrap-fence", plan]))
        node.succeed("timeout 150s systemctl start docker.service")
        node.wait_for_unit("docker.service")
        node.succeed(shlex.join(["tentaflake", "admission", "bootstrap", plan]))

    def oci_assert_active(name):
        receipt = oci_current(name)
        assert receipt["reservation"]["state"] == "active", receipt
        assert receipt["reservation"]["resources"] == oci_manifest["instances"][name]["resources"], receipt
        dispatch = receipt["runtime"]
        cid = dispatch["containerId"]
        assert len(cid) == 64 and all(c in "0123456789abcdef" for c in cid), cid
        intent = dispatch["intent"]
        assert intent["imageId"] == oci_image_id
        assert intent["image"] == oci_manifest["runtimes"][name]["image"]
        assert intent["specRevision"] == oci_manifest["instances"][name]["revision"]
        assert intent["bootId"] == oci.succeed("cat /proc/sys/kernel/random/boot_id").strip()
        assert intent["daemon"]["id"] == oci.succeed(oci_docker("info", "--format", "{{.ID}}")).strip()
        for identity, unit in [(intent["daemon"], "docker.service"), (intent["supervisor"], oci_units[name])]:
            assert identity["invocationId"] == oci.succeed("systemctl show --value --property=InvocationID " + shlex.quote(unit)).strip()
            assert identity["pid"] == int(oci.succeed("systemctl show --value --property=MainPID " + shlex.quote(unit)).strip())
            stat = oci.succeed("cat /proc/" + str(identity["pid"]) + "/stat")
            assert identity["startTimeTicks"] == int(stat.rsplit(")", 1)[1].split()[19])
        observed = oci_inspect(cid)
        assert observed["Id"] == cid and observed["Name"] == "/" + name
        assert observed["State"]["Running"] and observed["Image"] == oci_image_id
        labels = observed["Config"]["Labels"]
        assert labels["io.tentaflake.operation"] == receipt["request"]["operationId"]
        assert labels["io.tentaflake.instance"] == name
        assert labels["io.tentaflake.spec-revision"] == intent["specRevision"]
        host = observed["HostConfig"]
        assert host["Runtime"] == "runsc" and host["NetworkMode"] == "none", host
        assert set(observed["NetworkSettings"]["Networks"]) <= {"none"}
        assert host["ReadonlyRootfs"] and not host["Privileged"] and not host["AutoRemove"]
        assert observed["Config"]["User"] == "65534:65534"
        assert set(host["CapDrop"]) == {"ALL"} and not host["CapAdd"]
        security = {option.replace(":", "=", 1) for option in host["SecurityOpt"]}
        assert security in (
            {"no-new-privileges=true", "apparmor=docker-default"},
            {"no-new-privileges", "apparmor=docker-default"},
        ), security
        for key, expected in [("Memory", 268435456), ("MemorySwap", 268435456), ("NanoCpus", 500000000), ("PidsLimit", 128)]:
            assert host[key] == expected, (key, host[key])
        assert {item["Name"]: (item["Soft"], item["Hard"]) for item in host["Ulimits"]} == {
            "nofile": (128, 128), "nproc": (128, 128)
        }
        assert set(host["Tmpfs"]) == {"/tmp", "/run", "/var/tmp"}
        for options in host["Tmpfs"].values():
            assert set(options.split(",")) == {"rw", "nosuid", "nodev", "noexec", "size=8m"}, options
        mounts = {item["Destination"]: (item["Source"], item["RW"]) for item in observed["Mounts"] if item["Type"] == "bind"}
        assert all(item["Type"] != "volume" for item in observed["Mounts"]), observed["Mounts"]
        assert mounts == {
            "/state": ("/var/lib/" + name + "/state", True),
            "/workspace": ("/var/lib/" + name + "/workspace", True),
        }, mounts
        environment = {item.split("=", 1)[0] for item in observed["Config"]["Env"]}
        assert not environment.intersection({"OPENAI_API_KEY", "OPENAI_BASE_URL", "TENTAFLAKE_BROKER_TOKEN", "TENTAFLAKE_LLM_BASE_URL", "TENTAFLAKE_RESEARCH_COMMAND", "TENTAFLAKE_RESEARCH_SOCKET"})
        oci.succeed(oci_docker("exec", cid, "/bin/sh", "-c",
            "test $(id -u) = 65534 && test $(pwd) = /workspace && "
            "test ! -e /run/docker.sock && test ! -e /run/tentaflake-research && test ! -e /run/tentaflake-worker && "
            "printf state-ok > /state/probe && printf workspace-ok > /workspace/probe"))
        oci.succeed("test $(cat /var/lib/" + name + "/state/probe) = state-ok")
        oci.succeed("test $(cat /var/lib/" + name + "/workspace/probe) = workspace-ok")
        oci.wait_until_succeeds("timeout 5s journalctl --no-pager -o cat -n 60 -u " + shlex.quote(oci_units[name]) + " | grep -Fx oci-admission-ready", timeout=30)
        return receipt

    with subtest("explicit local image acquisition creates no runtime claim"):
        for unit in list(oci_units.values()) + [item["units"]["owner"] for item in oci_volumes.values()]:
            oci_stop_unit(unit)
        oci.succeed("test ! -e /var/lib/tentaflake-admission")
        for name in oci_ids:
            oci.fail(oci_command("runtime-start", name))
            oci.fail(oci_docker("container", "inspect", name))
        source = oci_manifest["runtimes"][oci_ids[0]]["image"]
        assert source["kind"] == "local-artifact"
        oci_descriptor = json.loads(oci.succeed("cat " + shlex.quote(source["descriptor"])))
        assert oci_descriptor["schemaVersion"] == 1 and oci_descriptor["format"] == "docker-archive"
        assert oci_descriptor["os"] == "linux" and oci_descriptor["architecture"] == "amd64"
        oci_image_id = oci_descriptor["imageId"]
        oci.fail(oci_docker("image", "inspect", oci_image_id))
        oci.succeed(oci_command("image-acquire", oci_ids[0]))
        image = json.loads(oci.succeed(oci_docker("image", "inspect", oci_image_id)))[0]
        assert image["Id"] == oci_image_id and image["RepoDigests"] in (None, [])
        assert image["Os"] == "linux" and image["Architecture"] == "amd64"
        oci.succeed("test ! -e /var/lib/tentaflake-admission")

    with subtest("bootstrap and real quotas prepare both offline environments"):
        oci.fail(oci_command("bootstrap"))
        oci.succeed("test ! -e /var/lib/tentaflake-admission")
        oci_bootstrap(oci, oci_plan)
        assert oci_snapshot()["receipts"] == [] and oci_snapshot()["storage"] == []
        for name in oci_ids:
            for kind in ("state", "workspace"):
                volume = oci_volumes[kind + ":" + name]
                oci.succeed("systemctl start " + shlex.quote(volume["units"]["owner"]))
                oci.wait_for_unit(volume["units"]["owner"])
                mount = volume["spec"]["mountPath"]
                assert oci.succeed("findmnt -n -o FSTYPE --mountpoint " + shlex.quote(mount)).strip() == "btrfs"
                options = oci.succeed("findmnt -n -o OPTIONS --mountpoint " + shlex.quote(mount)).strip().split(",")
                assert {"nodev", "nosuid", "noatime"}.issubset(options), options
        oci_storage = oci_snapshot()["storage"]
        assert len(oci_storage) == 4 and all(item["allocation"]["state"] == "present" for item in oci_storage)
        assert sum(item["allocation"]["bytes"] for item in oci_storage) == 536870912

    with subtest("one live agent consumes the aggregate compute budget"):
        oci_start(oci_ids[0])
        first = oci_assert_active(oci_ids[0])
        before_denial = oci_snapshot()
        oci_start(oci_ids[1], succeeds=False)
        assert oci_snapshot() == before_denial
        oci.fail(oci_docker("container", "inspect", oci_ids[1]))
        assert oci_inspect(first["runtime"]["containerId"])["State"]["Running"]
        assert "resource-exhausted" in oci.succeed("journalctl --no-pager -o cat -n 40 -u " + shlex.quote(oci_units[oci_ids[1]]))
        oci_stop_unit(oci_units[oci_ids[1]], allow_failed=True)

    with subtest("exact container stop releases compute and retains every volume"):
        oci_stop_unit(oci_units[oci_ids[0]])
        oci.fail(oci_docker("container", "inspect", first["runtime"]["containerId"]))
        retained = oci_current(oci_ids[0])
        assert retained["reservation"]["state"] == "retained"
        assert retained["runtime"] == first["runtime"]
        assert retained["reservation"]["resources"] == {
            "memoryBytes": 0, "memorySwapBytes": 0, "nanoCpus": 0, "pids": 0,
            "workspaceBytes": 134217728, "stateBytes": 134217728,
        }
        assert oci_snapshot()["storage"] == oci_storage
        oci_start(oci_ids[1])
        second = oci_assert_active(oci_ids[1])
        assert second["runtime"]["containerId"] != first["runtime"]["containerId"]
        oci_stop_unit(oci_units[oci_ids[1]])
        oci.fail(oci_docker("container", "inspect", second["runtime"]["containerId"]))
        assert oci_current(oci_ids[1])["reservation"]["state"] == "retained"
        assert oci_snapshot()["storage"] == oci_storage

    with subtest("failed create preserves foreign same-name container and Unknown charge"):
        foreign = oci.succeed(oci_docker("container", "create", "--name", oci_ids[0], "--network=none", "--read-only", "--user=65534:65534", oci_image_id, "/bin/sleep", "3600")).strip()
        original_foreign = oci_inspect(foreign)
        assert not original_foreign["State"]["Running"]
        assert "io.tentaflake.operation" not in (original_foreign["Config"]["Labels"] or {})
        oci_start(oci_ids[0], succeeds=False)
        unknown = oci_current(oci_ids[0])
        assert unknown["reservation"]["state"] == "unknown"
        assert unknown["reservation"]["resources"] == oci_manifest["instances"][oci_ids[0]]["resources"]
        assert unknown["runtime"]["containerId"] is None
        assert unknown["runtime"]["intent"]["imageId"] == oci_image_id
        durable_unknown = oci_snapshot()
        for name in oci_ids:
            oci_stop_unit(oci_units[name], allow_failed=True)
            oci_start(name, succeeds=False)
            assert oci_snapshot() == durable_unknown
            assert oci_inspect(foreign) == original_foreign
        assert oci_inspect(oci_ids[0])["Id"] == foreign
        oci.fail(oci_docker("container", "inspect", oci_ids[1]))
        assert oci_snapshot()["storage"] == oci_storage

    oci.shutdown()
  '';

  crashTestScript = ''
    # No VM overlap and no alternate image, fake daemon or production test hook.
    oci_crash.start()
    oci_crash.wait_for_unit("multi-user.target")
    oci_crash.wait_for_unit("docker.service")
    crash_plan = oci_crash.succeed("readlink -f /etc/tentaflake/admission.json").strip()
    crash_manifest = json.loads(oci_crash.succeed("cat " + shlex.quote(crash_plan)))
    crash_ids = ["generic-oci-a", "generic-oci-b"]
    crash_units = {name: crash_manifest["runtimes"][name]["unit"] for name in crash_ids}
    crash_ledger = "/var/lib/tentaflake-admission/ledger.json"

    def crash_command(action, name=None):
        args = ["tentaflake", "admission", action, crash_plan]
        if name is not None:
            args.append(name)
        return shlex.join(args)

    def crash_snapshot():
        return json.loads(oci_crash.succeed("cat " + crash_ledger))

    with subtest("an active supervisor crash across daemon generations retains its exact claim"):
        quota_units = [volume["units"]["owner"] for volume in crash_manifest["volumes"]]
        oci_crash.succeed("timeout 150s systemctl stop " + shlex.join(list(crash_units.values()) + quota_units))
        oci_crash.succeed("test ! -e /var/lib/tentaflake-admission")
        oci_crash.succeed(crash_command("image-acquire", crash_ids[0]))
        oci_bootstrap(oci_crash, crash_plan)
        oci_crash.succeed("timeout 150s systemctl start " + shlex.join(quota_units))
        oci_crash.succeed("timeout 320s systemctl start " + shlex.quote(crash_units[crash_ids[0]]))
        before_crash = crash_snapshot()
        assert len(before_crash["receipts"]) == 1
        active = before_crash["receipts"][0]
        assert active["reservation"]["state"] == "active", active
        dispatch = active["runtime"]
        cid = dispatch["containerId"]
        supervisor = dispatch["intent"]["supervisor"]
        daemon = dispatch["intent"]["daemon"]
        assert len(cid) == 64
        assert oci_crash.succeed(oci_docker("container", "inspect", "--format", "{{.State.Running}}", cid)).strip() == "true"
        main_pid = int(oci_crash.succeed("systemctl show --value --property=MainPID " + shlex.quote(crash_units[crash_ids[0]])))
        assert main_pid == supervisor["pid"] and main_pid > 0
        assert oci_crash.succeed("systemctl show --value --property=InvocationID docker.service").strip() == daemon["invocationId"]
        # Freeze the owned main before changing the daemon. Its Docker wait
        # client may exit, but cannot race the main into ordinary stop cleanup.
        oci_crash.succeed("kill -STOP " + str(main_pid))
        killed = False
        try:
            oci_crash.wait_until_succeeds("test $(awk '{print $3}' /proc/" + str(main_pid) + "/stat) = T", timeout=10)
            daemon_units = ["docker.service"]
            if oci_crash.succeed("systemctl show --value --property=LoadState docker.socket").strip() != "not-found":
                daemon_units.append("docker.socket")
            oci_crash.succeed("timeout 150s systemctl stop " + shlex.join(daemon_units))
            assert oci_crash.succeed("systemctl show --value --property=MainPID docker.service").strip() == "0"
            oci_crash.succeed("timeout 150s systemctl start docker.service")
            oci_crash.wait_for_unit("docker.service")
            assert oci_crash.succeed("systemctl show --value --property=InvocationID docker.service").strip() != daemon["invocationId"]
            new_daemon_pid = int(oci_crash.succeed("systemctl show --value --property=MainPID docker.service"))
            new_daemon_stat = oci_crash.succeed("cat /proc/" + str(new_daemon_pid) + "/stat")
            assert int(new_daemon_stat.rsplit(")", 1)[1].split()[19]) > daemon["startTimeTicks"]
            assert crash_snapshot() == before_crash
            oci_crash.succeed("kill -KILL " + str(main_pid))
            killed = True
        finally:
            # Failure diagnostics must not leave a suspended unit blocking shutdown.
            if not killed:
                oci_crash.execute("kill -CONT " + str(main_pid))
        oci_crash.wait_until_succeeds("test $(systemctl show --value --property=ActiveState " + shlex.quote(crash_units[crash_ids[0]]) + ") = failed", timeout=150)
        after_crash = crash_snapshot()
        assert len(after_crash["receipts"]) == 1
        unknown = after_crash["receipts"][0]
        assert unknown["reservation"]["state"] == "unknown", unknown
        assert unknown["reservation"]["resources"] == crash_manifest["instances"][crash_ids[0]]["resources"]
        assert unknown["request"] == active["request"] and unknown["runtime"] == dispatch
        assert after_crash["storage"] == before_crash["storage"]
        retained_container = json.loads(oci_crash.succeed(oci_docker("container", "inspect", cid)))[0]
        assert retained_container["Id"] == cid and not retained_container["State"]["Running"]
        assert retained_container["Config"]["Labels"]["io.tentaflake.operation"] == active["request"]["operationId"]
        for name in crash_ids:
            oci_crash.fail("timeout 320s systemctl start " + shlex.quote(crash_units[name]))
            assert crash_snapshot() == after_crash
            assert json.loads(oci_crash.succeed(oci_docker("container", "inspect", cid)))[0] == retained_container
        assert oci_crash.succeed(oci_docker("container", "ls", "--all", "--quiet", "--no-trunc")).split() == [cid]
        oci_crash.fail(oci_docker("container", "inspect", crash_ids[1]))

    oci_crash.shutdown()
  '';
}
