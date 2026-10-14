# Storage-only acceptance through the real host CLI, D-Bus and quota services.
# Generic OCI controllers remain stopped: this does not prove aggregate OCI
# startup admission, image acquisition or vendor capability acceptance.
{ self }:
{
  node =
    {
      config,
      lib,
      pkgs,
      ...
    }:
    let
      builders = self.lib.x86_64-linux;
      names = [
        "storage"
        "overbudget"
      ];
      definition = {
        schemaVersion = 1;
        image = builders.constants.hermesImage;
        command = [ "unused-storage-fixture" ];
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
      reserve = lib.genAttrs [
        "memoryBytes"
        "memorySwapBytes"
        "nanoCpus"
        "pids"
        "workspaceBytes"
        "stateBytes"
      ] (_: 0);
    in
    {
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
        # These tests execute prebuilt closures; admission does not support the
        # VM harness's writable-store overlay with unobserved backing options.
        writableStore = false;
        oci-containers.backend = "docker";
        docker.enable = true;
        docker.daemon.settings."live-restore" = false;
        memorySize = 768;
        diskSize = 2048;
      };
      environment.systemPackages = [
        pkgs.btrfs-progs
        pkgs.util-linux
      ];
      environment.etc."tentaflake/stale-admission.json".source =
        pkgs.writeText "tentaflake-stale-admission.json" config.tentaflake.admissionManifest.text;
      # A genuine installed generation, changing capacity without resizing or
      # rebinding any image. The base generation keeps its overbudget fixture.
      specialisation.publication.configuration.tentaflake.security.admission.capacity = {
        workspaceBytes = lib.mkForce 268435456;
        stateBytes = lib.mkForce 268435456;
      };

      tentaflake = {
        hostName = "storage-host";
        adminUser = "admin";
        boot.enable = false;
        modernConsole.enable = false;
        nixSettings.enable = false;
        shell.enable = true;
        security.admission = {
          authorityEpoch = "storage-vm";
          physicalSpaceHeadroomBytes = 67108864;
          capacity = {
            memoryBytes = 2147483648;
            memorySwapBytes = 2147483648;
            nanoCpus = 2000000000;
            pids = 512;
            workspaceBytes = 134217728;
            stateBytes = 134217728;
          };
          hostReserve = reserve;
          researchReserve = reserve;
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

  testScript = ''
    # All original integration nodes have shut down before this phase starts.
    storage.start()
    storage.wait_for_unit("multi-user.target")
    storage.wait_for_unit("docker.service")

    storage_plan = storage.succeed("readlink -f /etc/tentaflake/admission.json").strip()
    stale_storage_plan = storage.succeed("readlink -f /etc/tentaflake/stale-admission.json").strip()
    storage_manifest = json.loads(storage.succeed(f"cat {shlex.quote(storage_plan)}"))
    storage_volumes = {item["spec"]["allocationId"]: item for item in storage_manifest["volumes"]}
    storage_units = [item["units"]["owner"] for item in storage_volumes.values()]
    storage_state = storage_volumes["state:generic-storage"]
    storage_workspace = storage_volumes["workspace:generic-storage"]
    storage_ledger = "/var/lib/tentaflake-admission/ledger.json"

    def storage_command(action, allocation=None, plan=storage_plan):
        arguments = ["tentaflake", "admission", action, plan]
        if allocation is not None:
            arguments.append(allocation)
        return shlex.join(arguments)

    def stop_storage_units():
        failed_before = {
            unit for unit in storage_units
            if storage.succeed("systemctl show --value --property=ActiveState " + shlex.quote(unit)).strip() == "failed"
        }
        storage.succeed("timeout 150s systemctl stop " + shlex.join(storage_units))
        for unit in storage_units:
            state = storage.succeed("systemctl show --value --property=ActiveState " + shlex.quote(unit)).strip()
            assert state == "inactive" or (unit in failed_before and state == "failed"), (unit, state)
        jobs = storage.succeed("systemctl list-jobs --no-legend --no-pager --full")
        assert all(not set(storage_units).intersection(line.split()) for line in jobs.splitlines()), jobs

    def storage_snapshot():
        return json.loads(storage.succeed(f"cat {storage_ledger}"))

    def start_storage_workspace():
        storage.succeed("systemctl start " + shlex.quote(storage_workspace["units"]["owner"]))

    def image_identity(volume):
        return storage.succeed("stat -c '%d:%i:%s' " + shlex.quote(volume["spec"]["imagePath"])).strip()

    def assert_storage_absent(volume):
        path = volume["spec"]["imagePath"]
        storage.succeed("test ! -e " + shlex.quote(path))
        storage.succeed("test ! -e " + shlex.quote(path + ".new"))

    with subtest("storage boot and manual entrypoints cannot bypass explicit bootstrap"):
        storage.succeed("test ! -e /var/lib/tentaflake-admission")
        for volume in storage_volumes.values():
            storage.fail("systemctl start " + shlex.quote(volume["units"]["owner"]))
            assert_storage_absent(volume)
            for field in ("prepare", "mount"):
                unit = volume["units"][field]
                assert storage.succeed("systemctl show --value --property=LoadState " + shlex.quote(unit)).strip() == "masked"
                storage.fail("systemctl start " + shlex.quote(unit))
        stop_storage_units()
        storage.fail(storage_command("volume-start", "state:generic-storage"))
        storage.fail(storage_command("bootstrap", plan=stale_storage_plan))
        storage.succeed("test ! -e /var/lib/tentaflake-admission")
        storage.fail(storage_command("bootstrap"))
        docker_units = ["docker.service"]
        if storage.succeed("systemctl show --value --property=LoadState docker.socket").strip() != "not-found":
            docker_units.append("docker.socket")
        storage.succeed("timeout 150s systemctl stop " + shlex.join(docker_units))
        print(storage.succeed(${
          builtins.toJSON (
            "timeout 60s bash -euo pipefail -c "
            + self.inputs.nixpkgs.lib.escapeShellArg (builtins.readFile ./fixtures/admission-mount-inventory.sh)
          )
        }))
        storage.succeed(storage_command("bootstrap-fence"))
        storage.succeed("timeout 150s systemctl start docker.service")
        storage.wait_for_unit("docker.service")
        storage.succeed(storage_command("bootstrap"))
        initial = storage_snapshot()
        assert initial["storage"] == [] and initial["receipts"] == [], initial
        assert initial["published"]["manifestPath"] == storage_plan, initial
        storage.fail(storage_command("bootstrap"))
        assert storage_snapshot() == initial

    with subtest("physical headroom refuses creation before any durable claim"):
        backing_root = "/var/lib/tentaflake-workspace-volumes"
        filler = storage.succeed("mktemp /var/lib/storage-headroom.XXXXXX").strip()
        try:
            # Keep the disposable file outside the protected inventory, but
            # consume the exact same ext4 capacity pool with real allocation.
            devices = storage.succeed("stat -c %d -- " + shlex.join([backing_root, filler])).split()
            assert len(devices) == 2 and devices[0] == devices[1], devices
            pool = storage.succeed("findmnt -n -r -o ID,FSTYPE --target " + shlex.quote(backing_root)).split()
            assert len(pool) == 2 and pool[1] == "ext4", pool
            assert storage.succeed("findmnt -n -r -o ID,FSTYPE --target " + shlex.quote(filler)).split() == pool
            available_blocks, fragment_bytes = map(int, storage.succeed("stat -f -c '%a %S' " + backing_root).split())
            available = available_blocks * fragment_bytes
            target_free = 128 * 1024 * 1024
            required = storage_state["spec"]["bytes"] + storage_manifest["physicalSpaceHeadroomBytes"]
            assert available > required > target_free
            storage.succeed("fallocate --length " + str(available - target_free) + " -- " + shlex.quote(filler))
            available_blocks, fragment_bytes = map(int, storage.succeed("stat -f -c '%a %S' " + backing_root).split())
            remaining = available_blocks * fragment_bytes
            assert 96 * 1024 * 1024 <= remaining <= 160 * 1024 * 1024, remaining
            assert remaining < required, (remaining, required)
            unit = storage_state["units"]["owner"]
            storage.fail("systemctl start " + shlex.quote(unit))
            assert "storage admission failed; no image creation authorized" in storage.succeed(
                "journalctl --no-pager -o cat -n 30 -u " + shlex.quote(unit)
            )
            assert_storage_absent(storage_state)
            assert_storage_absent(storage_workspace)
            assert storage_snapshot() == initial
        finally:
            storage.succeed("rm -- " + shlex.quote(filler))
            stop_storage_units()

    with subtest("guarded storage creates exact host mounts and retains charges on stop"):
        storage.fail(storage_command("volume-start", "workspace:generic-storage"))
        start_storage_workspace()
        for volume in (storage_state, storage_workspace):
            spec = volume["spec"]
            storage.wait_for_unit(volume["units"]["owner"])
            assert storage.succeed("stat -c %s " + shlex.quote(spec["imagePath"])).strip() == str(spec["bytes"])
            assert storage.succeed("findmnt -n -o FSTYPE --mountpoint " + shlex.quote(spec["mountPath"])).strip() == "btrfs"
            options = storage.succeed("findmnt -n -o OPTIONS --mountpoint " + shlex.quote(spec["mountPath"])).strip().split(",")
            assert {"nodev", "nosuid", "noatime"}.issubset(options), options
            assert storage.succeed("stat -c '%u:%g:%a' " + shlex.quote(spec["mountPath"])).strip() == "65534:65534:700"
            source = storage.succeed("findmnt -n -o SOURCE --mountpoint " + shlex.quote(spec["mountPath"])).strip().split("[", 1)[0]
            assert storage.succeed("losetup --noheadings --list --raw --output BACK-FILE " + shlex.quote(source)).strip() == spec["imagePath"]
            storage.succeed("printf persistent > " + shlex.quote(spec["mountPath"] + "/marker"))
        charged = storage_snapshot()
        allocations = [receipt["allocation"] for receipt in charged["storage"]]
        assert {allocation["id"] for allocation in allocations} == {"state:generic-storage", "workspace:generic-storage"}
        assert all(allocation["state"] == "present" and allocation["bytes"] == 134217728 for allocation in allocations)
        assert sum(allocation["bytes"] for allocation in allocations) == 268435456
        assert charged["receipts"] == [], "storage acceptance must not invent OCI runtime receipts"
        identities = {volume["spec"]["allocationId"]: image_identity(volume) for volume in (storage_state, storage_workspace)}
        stop_storage_units()
        for volume in (storage_state, storage_workspace):
            storage.fail("findmnt --mountpoint " + shlex.quote(volume["spec"]["mountPath"]))
        assert storage_snapshot() == charged
        start_storage_workspace()
        for volume in (storage_state, storage_workspace):
            assert image_identity(volume) == identities[volume["spec"]["allocationId"]]
            assert storage.succeed("cat " + shlex.quote(volume["spec"]["mountPath"] + "/marker")).strip() == "persistent"
        assert storage_snapshot() == charged
        stop_storage_units()

    with subtest("aggregate storage refuses a second environment without creating images"):
        for kind in ("state", "workspace"):
            volume = storage_volumes[f"{kind}:generic-overbudget"]
            storage.fail("systemctl start " + shlex.quote(volume["units"]["owner"]))
            assert_storage_absent(volume)
        assert storage_snapshot() == charged
        stop_storage_units()

    with subtest("retained missing images and partial staging never authorize recreation"):
        image = storage_state["spec"]["imagePath"]
        saved = "/var/lib/storage-state-image.saved"
        storage.succeed("mv " + shlex.join([image, saved]))
        try:
            storage.fail("systemctl start " + shlex.quote(storage_state["units"]["owner"]))
            assert_storage_absent(storage_state)
            stop_storage_units()
            storage.fail(storage_command("storage-recover", "state:generic-storage"))
            assert storage_snapshot() == charged
        finally:
            storage.succeed("mv " + shlex.join([saved, image]))
        staging = image + ".new"
        storage.succeed("printf incomplete > " + shlex.quote(staging))
        try:
            storage.fail("systemctl start " + shlex.quote(storage_state["units"]["owner"]))
            assert storage.succeed("cat " + shlex.quote(staging)).strip() == "incomplete"
            assert image_identity(storage_state) == identities["state:generic-storage"]
            assert storage_snapshot() == charged
        finally:
            storage.succeed("rm -- " + shlex.quote(staging))
        stop_storage_units()
        storage.succeed(storage_command("storage-recover", "state:generic-storage"))
        charged = storage_snapshot()
        assert len(charged["storage"]) == 2
        start_storage_workspace()
        assert storage.succeed("cat /var/lib/generic-storage/state/marker").strip() == "persistent"
        stop_storage_units()

    with subtest("stale installed authority and unsafe or missing ledgers fail closed"):
        storage.succeed("ln -sfn " + shlex.join([stale_storage_plan, "/etc/tentaflake/admission.json"]))
        try:
            storage.fail("systemctl start " + shlex.quote(storage_state["units"]["owner"]))
        finally:
            storage.succeed("ln -sfn " + shlex.join([storage_plan, "/etc/tentaflake/admission.json"]))
        stop_storage_units()
        storage.succeed("chmod 0777 /var/lib/tentaflake-admission")
        try:
            storage.fail("systemctl start " + shlex.quote(storage_state["units"]["owner"]))
        finally:
            storage.succeed("chmod 0700 /var/lib/tentaflake-admission")
        stop_storage_units()
        storage.succeed("mv /var/lib/tentaflake-admission /var/lib/storage-ledger.saved")
        try:
            storage.fail("systemctl start " + shlex.quote(storage_state["units"]["owner"]))
            storage.succeed("test ! -e /var/lib/tentaflake-admission")
        finally:
            storage.succeed("mv /var/lib/storage-ledger.saved /var/lib/tentaflake-admission")
        stop_storage_units()
        assert storage_snapshot() == charged
        for volume in (storage_state, storage_workspace):
            assert image_identity(volume) == identities[volume["spec"]["allocationId"]]
        start_storage_workspace()
        stop_storage_units()
        assert storage.succeed("docker ps --all --quiet").strip() == ""

    base_system = storage.succeed("readlink -f /run/current-system").strip()
    candidate_system = storage.succeed("readlink -f " + shlex.quote(base_system + "/specialisation/publication")).strip()
    candidate_plan = storage.succeed("readlink -f " + shlex.quote(candidate_system + "/etc/tentaflake/admission.json")).strip()
    candidate_manifest = json.loads(storage.succeed("cat " + shlex.quote(candidate_plan)))
    assert candidate_manifest["policy"]["generation"] == storage_manifest["policy"]["generation"]
    assert candidate_manifest["policy"]["revision"] != storage_manifest["policy"]["revision"]
    assert candidate_manifest["instances"] == storage_manifest["instances"]
    assert [volume["spec"] for volume in candidate_manifest["volumes"]] == [volume["spec"] for volume in storage_manifest["volumes"]]

    def publication(action, *arguments, success=True):
        command = shlex.join(["tentaflake", "admission", "publication", action, *map(str, arguments)])
        if not success:
            return storage.fail(command)
        return json.loads(storage.succeed(command))

    def install_storage_generation(system, plan):
        # NixOS may try wanted quota units during activation. Pending must refuse
        # those starts; exit 4 is accepted only for these expected failed units.
        code, output = storage.execute(shlex.quote(system + "/bin/switch-to-configuration") + " test")
        assert code in (0, 4), (code, output)
        failed = storage.succeed("systemctl list-units --state=failed --no-legend --plain").splitlines()
        failed_names = {line.split()[0] for line in failed if line.strip()}
        assert failed_names.issubset(set(storage_units)), (failed_names, output)
        assert code == 0 or failed_names, output
        assert storage.succeed("readlink -f /run/current-system").strip() == system
        assert storage.succeed("readlink -f /etc/tentaflake/admission.json").strip() == plan
        storage.succeed("systemctl daemon-reload")
        stop_storage_units()
        for unit in storage_units:
            assert plan in storage.succeed("systemctl show --value --property=ExecStart " + shlex.quote(unit))

    with subtest("publication requires quiescence and retains Pending until exact installation"):
        stable = publication("status")
        assert stable["published"]["manifestPath"] == storage_plan and stable["pending"] is None
        start_storage_workspace()
        publication("begin", candidate_plan, "publish-capacity", stable["ledgerRevision"], success=False)
        assert publication("status") == stable
        stop_storage_units()
        publication("begin", candidate_plan, "publish-capacity", stable["ledgerRevision"] - 1, success=False)
        assert publication("status") == stable
        begun = publication("begin", candidate_plan, "publish-capacity", stable["ledgerRevision"])
        assert begun["outcome"] == "pending"
        pending = publication("status")
        assert pending["pending"] == begun and pending["published"] == stable["published"]
        assert pending["ledgerRevision"] == stable["ledgerRevision"] + 1
        assert publication("begin", candidate_plan, "publish-capacity", stable["ledgerRevision"]) == begun
        publication("finish", "publish-capacity", stable["ledgerRevision"], success=False)
        publication("finish", "publish-capacity", pending["ledgerRevision"], success=False)
        assert publication("status") == pending
        storage.fail("systemctl start " + shlex.quote(storage_workspace["units"]["owner"]))
        stop_storage_units()
        assert publication("status") == pending
        assert storage_snapshot()["storage"] == charged["storage"]
        install_storage_generation(candidate_system, candidate_plan)
        assert publication("status") == pending
        finished = publication("finish", "publish-capacity", pending["ledgerRevision"])
        assert finished["outcome"] == "published"
        published = publication("status")
        assert published["pending"] is None and published["published"]["manifestPath"] == candidate_plan
        assert published["ledgerRevision"] == pending["ledgerRevision"] + 1
        assert publication("finish", "publish-capacity", pending["ledgerRevision"]) == finished
        assert publication("begin", candidate_plan, "publish-capacity", stable["ledgerRevision"]) == finished
        assert publication("status") == published
        for volume in (storage_state, storage_workspace):
            storage.fail("findmnt --mountpoint " + shlex.quote(volume["spec"]["mountPath"]))
        start_storage_workspace()
        for volume in (storage_state, storage_workspace):
            assert image_identity(volume) == identities[volume["spec"]["allocationId"]]
            assert storage.succeed("cat " + shlex.quote(volume["spec"]["mountPath"] + "/marker")).strip() == "persistent"
        stop_storage_units()
        assert storage_snapshot()["storage"] == charged["storage"]

    with subtest("publication cancellation requires the old generation restored"):
        begun = publication("begin", storage_plan, "rollback-capacity", published["ledgerRevision"])
        pending = publication("status")
        assert pending["pending"] == begun
        install_storage_generation(base_system, storage_plan)
        publication("cancel", "rollback-capacity", pending["ledgerRevision"], success=False)
        assert publication("status") == pending
        install_storage_generation(candidate_system, candidate_plan)
        publication("cancel", "rollback-capacity", pending["ledgerRevision"] - 1, success=False)
        cancelled = publication("cancel", "rollback-capacity", pending["ledgerRevision"])
        assert cancelled["outcome"] == "cancelled"
        restored = publication("status")
        assert restored["pending"] is None and restored["published"] == published["published"]
        assert publication("cancel", "rollback-capacity", pending["ledgerRevision"]) == cancelled
        publication("finish", "rollback-capacity", pending["ledgerRevision"], success=False)
        assert publication("status") == restored
        for volume in (storage_state, storage_workspace):
            storage.fail("findmnt --mountpoint " + shlex.quote(volume["spec"]["mountPath"]))
        start_storage_workspace()
        for volume in (storage_state, storage_workspace):
            assert image_identity(volume) == identities[volume["spec"]["allocationId"]]
        stop_storage_units()

    with subtest("a real failed creation retains Unknown and prevents publication begin"):
        # This slot now fits the increased capacity. Its immutable create helper
        # must refuse existing target data after the guarded durable claim.
        unused_state = storage_volumes["state:generic-overbudget"]
        obstacle = unused_state["spec"]["mountPath"] + "/publication-obstacle"
        storage.succeed("printf preserved > " + shlex.quote(obstacle))
        try:
            storage.fail("systemctl start " + shlex.quote(unused_state["units"]["owner"]))
            stop_storage_units()
            allocations = [receipt["allocation"] for receipt in storage_snapshot()["storage"]]
            unknown = [allocation for allocation in allocations if allocation["id"] == unused_state["spec"]["allocationId"]]
            assert len(unknown) == 1 and unknown[0]["state"] == "unknown", allocations
            assert unknown[0]["bytes"] == unused_state["spec"]["bytes"]
            assert_storage_absent(unused_state)
            assert storage.succeed("cat " + shlex.quote(obstacle)).strip() == "preserved"
            uncertain = publication("status")
            uncertain_ledger = storage_snapshot()
            error = publication("begin", storage_plan, "unknown-refused", uncertain["ledgerRevision"], success=False)
            assert "publication requires retained runtime claims and Present storage; recover uncertain state before beginning" in error
            assert publication("status") == uncertain and uncertain["pending"] is None
            assert storage_snapshot() == uncertain_ledger
        finally:
            storage.succeed("rm -- " + shlex.quote(obstacle))
    storage.shutdown()
  '';
}
