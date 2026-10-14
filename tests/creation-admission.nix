# Real stopped creation and root reconciliation. The ignored Rust harness only
# prepares accepted/core-interrupted records; it adds no production CLI bypass.
{ self }:
{
  node = import ./fixtures/creation-node.nix { inherit self; };

  testScript = ''
    import json
    import shlex

    creation.start()
    creation.wait_for_unit("multi-user.target")
    creation.wait_for_unit("docker.service")
    creation.wait_for_unit("register-nix-paths.service")
    creation_plan = creation.succeed("readlink -f /etc/tentaflake/admission.json").strip()
    creation_manifest = json.loads(creation.succeed("cat " + shlex.quote(creation_plan)))
    creation_ledger = "/var/lib/tentaflake-admission/ledger.json"
    creation_owners = [volume["units"]["owner"] for volume in creation_manifest["volumes"]]
    creation_boot = creation.succeed("cat /proc/sys/kernel/random/boot_id").strip()

    def creation_snapshot():
        return json.loads(creation.succeed("cat " + shlex.quote(creation_ledger)))

    def creation_fingerprint(path=creation_ledger):
        return creation.succeed(shlex.join(["stat", "-c", "%d:%i:%f:%s:%y:%z", path])
                                + " && " + shlex.join(["sha256sum", path]))

    def creation_op(name):
        matches = [op for op in creation_snapshot()["api"]["operations"]
                   if op["mutation"]["operationId"] == name]
        assert len(matches) == 1, matches
        return matches[0]

    def creation_rows(environment):
        return [row for row in creation_snapshot()["storage"]
                if row["allocation"]["binding"] is not None
                and row["allocation"]["binding"]["instanceId"] == environment]

    def creation_seed(operation, nonce, mode):
        command = shlex.join(["timeout", "--kill-after=5s", "300s", "env", "TF_CREATION_PLAN=" + creation_plan,
            "TF_CREATION_TEMPLATE=fixture", "TF_CREATION_OPERATION=" + operation,
            "TF_CREATION_NONCE=" + nonce, "TF_CREATION_MODE=" + mode,
            "tentaflake-creation-fixture", "--ignored", "--exact",
            "admission_creation::vm_fixture::prepare", "--nocapture"])
        output = creation.succeed(command, timeout=320)
        lines = [line[len("TF_CREATION_RESULT="):] for line in output.splitlines()
                 if line.startswith("TF_CREATION_RESULT=")]
        assert len(lines) == 1, output
        result = json.loads(lines[0])
        assert result["new"] and result["operationId"] == operation, result
        assert result["environmentId"] == "env-" + nonce, result
        return result

    def creation_reconcile(action, operation, revision, success=True, plan=None):
        command = shlex.join(["tentaflake", "admission", "create-" + action,
                              plan or creation_plan, operation, str(revision)])
        if success:
            return creation.succeed(command, timeout=150)
        before = creation_fingerprint()
        result = creation.fail(command + " 2>&1", timeout=150)
        assert creation_fingerprint() == before, "refusal changed the ledger"
        return result

    def creation_stopped():
        assert creation.succeed("docker ps --all --quiet --no-trunc").strip() == ""
        assert json.loads(creation.succeed("losetup --list --json --output NAME,BACK-FILE"))["loopdevices"] == []
        mounts = json.loads(creation.succeed("findmnt --json --list --output TARGET"))["filesystems"]
        assert all(not row["target"].startswith("/var/lib/tentaflake-environments/") for row in mounts)
        jobs = creation.succeed("systemctl list-jobs --no-legend --no-pager --full")
        assert "tentaflake-environment-create@" not in jobs, jobs

    def creation_nix_evidence(*plans):
        creation.wait_for_unit("register-nix-paths.service")
        assert creation.succeed("systemctl show --value --property=Result register-nix-paths.service").strip() == "success"
        options = creation.succeed("findmnt --task 1 --first-only --direction backward -n -o OPTIONS --mountpoint /nix/store").strip().split(",")
        assert "ro" in options and "rw" not in options, options
        for plan in plans:
            root = "/nix/var/nix/gcroots/tentaflake/" + plan.rsplit("/", 1)[1]
            assert creation.succeed(shlex.join(["readlink", "--", root])).strip() == plan
            assert creation.succeed("test -L " + shlex.quote(root) + " && " + shlex.join(["stat", "-c", "%u:%h", root])).strip() == "0:1"

    with subtest("creation fixture bootstraps without starting its source or acquiring images"):
        creation.succeed("systemctl stop " + shlex.join(creation_owners))
        for unit in creation_owners:
            assert creation.succeed("systemctl show --value --property=ActiveState " + shlex.quote(unit)).strip() == "inactive"
        creation.succeed("timeout 150s systemctl stop docker.service docker.socket")
        creation.succeed(shlex.join(["tentaflake", "admission", "bootstrap-fence", creation_plan]))
        creation.succeed("systemctl start docker.service")
        creation.wait_for_unit("docker.service")
        creation.succeed(shlex.join(["tentaflake", "admission", "bootstrap", creation_plan]))
        creation_nix_evidence(creation_plan)
        assert creation_snapshot()["storage"] == []
        assert creation.succeed("docker image ls --quiet --no-trunc").strip() == ""
        creation_stopped()

    with subtest("actual creation supervisor prepares both volumes exactly once"):
        created = creation_seed("op-supervised", "01" * 16, "queue")
        assert creation_op("op-supervised")["state"] == "queued"
        rows = creation_rows(created["environmentId"])
        assert len(rows) == 2 and all(row["allocation"]["state"] == "unknown" for row in rows)
        unit = "tentaflake-environment-create@op-supervised.service"
        creation.succeed("timeout 320s systemctl start " + shlex.quote(unit), timeout=325)
        observed = creation_op("op-supervised")
        assert observed["state"] == "succeeded" and observed["recovery"] is None, observed
        assert observed["execution"]["bootId"] == creation_boot
        assert observed["execution"]["supervisor"]["unit"] == unit
        assert all(row["allocation"]["state"] == "present" and row["release"] is None
                   for row in creation_rows(created["environmentId"]))
        creation_stopped()
        # Check actual filesystem ownership independently of the owner's native
        # receipt. Read-only loops/mounts cannot repair a no-op ownership helper.
        ownership_mount = "/run/tentaflake-creation-owner-check"
        creation.succeed(shlex.join(["install", "-d", "-m", "0700", ownership_mount]))
        for volume in created["volumes"]:
            device = creation.succeed(shlex.join(["losetup", "--find", "--show", "--read-only", volume["imagePath"]])).strip()
            mounted = False
            try:
                creation.succeed(shlex.join(["mount", "--types", "btrfs", "--options",
                    "ro,nologreplay,nodev,nosuid,noatime", device, ownership_mount]))
                mounted = True
                assert creation.succeed(shlex.join(["stat", "-c", "%u:%g", ownership_mount])).strip() == "65534:65534"
            finally:
                if mounted:
                    creation.succeed(shlex.join(["umount", "--", ownership_mount]))
                creation.succeed(shlex.join(["losetup", "--detach", device]))
        creation.succeed(shlex.join(["rmdir", "--", ownership_mount]))
        creation_stopped()
        creation_nix_evidence(creation_plan)
        images = {volume["imagePath"]: creation_fingerprint(volume["imagePath"])
                  for volume in created["volumes"]}
        before = creation_fingerprint()
        creation.succeed("timeout 320s systemctl start " + shlex.quote(unit), timeout=325)
        assert creation_fingerprint() == before, "historical start changed the ledger"
        for path, identity in images.items():
            assert creation_fingerprint(path) == identity, "historical start reran a helper"
        creation_stopped()

    with subtest("real creation supervisor death retains prepared storage without replay"):
        interrupted = creation_seed("op-interrupted", "06" * 16, "queue")
        interruption = json.loads(creation.succeed(shlex.join(["timeout", "--kill-after=5s", "250s",
            "python3", "${./fixtures/creation-interrupt.py}", "op-interrupted",
            interrupted["environmentId"], creation_ledger]), timeout=260))
        interrupted_unit = "tentaflake-environment-create@op-interrupted.service"
        creation.wait_until_succeeds("test $(systemctl show --value --property=ActiveState "
            + shlex.quote(interrupted_unit) + ") = failed", timeout=30)
        assert creation.succeed("systemctl show --value --property=Result " + shlex.quote(interrupted_unit)).strip() == "signal"
        assert creation.succeed("systemctl show --value --property=ExecMainStatus " + shlex.quote(interrupted_unit)).strip() == "9"
        assert creation.succeed("systemctl show --value --property=MainPID " + shlex.quote(interrupted_unit)).strip() == "0"
        observed = creation_op("op-interrupted")
        assert observed["state"] == "unknown" and observed["execution"] == interruption["execution"]
        assert observed["execution"]["bootId"] == creation_boot
        assert all(row["allocation"]["state"] == "present" and row["release"] is None
                   for row in creation_rows(interrupted["environmentId"]))
        creation_stopped()
        interrupted_images = {volume["imagePath"]: creation_fingerprint(volume["imagePath"])
                              for volume in interrupted["volumes"]}
        before = creation_fingerprint()
        creation.succeed("timeout 320s systemctl start " + shlex.quote(interrupted_unit), timeout=325)
        assert creation_fingerprint() == before, "interrupted operation was redispatched"
        for path, identity in interrupted_images.items():
            assert creation_fingerprint(path) == identity, "interrupted operation reran a helper"
        creation_reconcile("recover", "op-interrupted", creation_snapshot()["revision"], success=False)
        creation_stopped()

    with subtest("queued absent pair abort requires exact CAS and preserves tombstones"):
        queued = creation_seed("op-queued-abort", "02" * 16, "queue")
        queued_revision = creation_snapshot()["revision"]
        creation_reconcile("abort", "op-queued-abort", queued_revision - 1, success=False)
        creation_reconcile("abort", "op-queued-abort", queued_revision)
        rows = creation_rows(queued["environmentId"])
        assert len(rows) == 2 and all(row["release"] is not None for row in rows)
        assert creation_op("op-queued-abort")["state"] == "aborted"
        before = creation_fingerprint()
        creation_reconcile("abort", "op-queued-abort", queued_revision)
        assert creation_fingerprint() == before

    with subtest("fixture-prepared interrupted operations cannot reconcile in the same boot"):
        # These use the real queue/claim/provision APIs and immutable helpers,
        # but intentionally synthetic supervisor identities, not crash evidence.
        empty = creation_seed("op-empty", "03" * 16, "unknown-empty")
        partial = creation_seed("op-partial", "04" * 16, "unknown-state")
        complete = creation_seed("op-complete", "05" * 16, "unknown-complete")
        for result in (empty, partial, complete):
            assert creation_op(result["operationId"])["state"] == "unknown"
            for action in ("abort", "recover"):
                creation_reconcile(action, result["operationId"], creation_snapshot()["revision"], success=False)
        creation_stopped()

    # Restart the same disk; never delete/reset its ledger or pretend a dead PID
    # alone fences old descendants. Source quota owners remain explicitly stopped.
    creation.shutdown()
    creation.start()
    creation.wait_for_unit("multi-user.target")
    creation.wait_for_unit("docker.service")
    assert creation.succeed("cat /proc/sys/kernel/random/boot_id").strip() != creation_boot
    assert creation.succeed("readlink -f /etc/tentaflake/admission.json").strip() == creation_plan
    creation_stopped()

    with subtest("staging and live creators or loop mappings refuse abort without ledger writes"):
        staging = empty["volumes"][0]["imagePath"] + ".new"
        creation.succeed(shlex.join(["install", "-m", "0600", "/dev/null", staging]))
        obstruction = creation_fingerprint(staging)
        creation_reconcile("abort", "op-empty", creation_snapshot()["revision"], success=False)
        assert creation_fingerprint(staging) == obstruction, "abort repaired staging"
        creation.succeed(shlex.join(["rm", "--", staging]))
        transient = "tentaflake-environment-create@unknown-fixture.service"
        unit_file = "/run/systemd/system/" + transient
        unit_text = "[Service]\nType=oneshot\nExecStart=/run/current-system/sw/bin/sleep 120\n"
        creation.succeed(shlex.join(["bash", "-euo", "pipefail", "-c",
            'test ! -e "$1"; printf "%s" "$2" > "$1"; systemctl daemon-reload',
            "--", unit_file, unit_text]))
        creation.succeed("systemctl start --no-block " + shlex.quote(transient))
        creation.wait_until_succeeds("test $(systemctl show --value --property=ActiveState " + shlex.quote(transient) + ") = activating")
        creation_reconcile("abort", "op-empty", creation_snapshot()["revision"], success=False)
        creation.succeed("timeout 30s systemctl stop " + shlex.quote(transient))
        assert creation.succeed("systemctl show --value --property=ActiveState " + shlex.quote(transient)).strip() == "inactive"
        # Mask the exact fixture launcher after draining it. Merely querying an
        # unknown template instance again could reload its inherited metadata.
        creation.succeed(shlex.join(["rm", "--", unit_file]))
        creation.succeed("systemctl mask --runtime " + shlex.quote(transient))
        assert creation.succeed("systemctl show --value --property=LoadState " + shlex.quote(transient)).strip() == "masked"
        image = next(volume["imagePath"] for volume in partial["volumes"] if volume["kind"] == "state")
        loop = creation.succeed(shlex.join(["losetup", "--find", "--show", "--read-only", image])).strip()
        try:
            creation_reconcile("abort", "op-empty", creation_snapshot()["revision"], success=False)
        finally:
            creation.succeed(shlex.join(["losetup", "--detach", loop]))
        creation_stopped()

    with subtest("recovery recognizes fully prepared storage without replaying ownership"):
        interrupted_revision = creation_snapshot()["revision"]
        creation_reconcile("recover", "op-interrupted", interrupted_revision)
        assert creation_op("op-interrupted")["state"] == "succeeded"
        assert creation_op("op-interrupted")["recovery"]["expectedLedgerRevision"] == interrupted_revision
        for path, identity in interrupted_images.items():
            assert creation_fingerprint(path) == identity, "recovery changed actual interrupted storage"
        before = creation_fingerprint()
        creation_reconcile("recover", "op-interrupted", interrupted_revision)
        assert creation_fingerprint() == before
        complete_images = {volume["imagePath"]: creation_fingerprint(volume["imagePath"])
                           for volume in complete["volumes"]}
        complete_revision = creation_snapshot()["revision"]
        creation_reconcile("recover", "op-complete", complete_revision)
        assert creation_op("op-complete")["state"] == "succeeded"
        assert creation_op("op-complete")["recovery"]["expectedLedgerRevision"] == complete_revision
        for path, identity in complete_images.items():
            assert creation_fingerprint(path) == identity, "recovery changed a prepared image"
        before = creation_fingerprint()
        creation_reconcile("recover", "op-complete", complete_revision)
        assert creation_fingerprint() == before
        creation_stopped()

    with subtest("partial abort retains its image while absent siblings alone release charges"):
        creation_reconcile("recover", "op-partial", creation_snapshot()["revision"], success=False)
        partial_image = next(volume["imagePath"] for volume in partial["volumes"] if volume["kind"] == "state")
        identity = creation_fingerprint(partial_image)
        partial_revision = creation_snapshot()["revision"]
        creation_reconcile("abort", "op-partial", partial_revision)
        rows = {row["allocation"]["kind"]: row for row in creation_rows(partial["environmentId"])}
        assert rows["state"]["release"] is None and rows["state"]["allocation"]["state"] == "present"
        assert rows["workspace"]["release"] is not None
        assert creation_fingerprint(partial_image) == identity
        empty_revision = creation_snapshot()["revision"]
        creation_reconcile("abort", "op-empty", empty_revision)
        assert all(row["release"] is not None for row in creation_rows(empty["environmentId"]))
        charged = [row for row in creation_snapshot()["storage"] if row["release"] is None]
        assert len(charged) == 7 and sum(row["allocation"]["bytes"] for row in charged) == 7 * 128 * 1024 * 1024
        creation_stopped()

    with subtest("publication removes templates without losing historical maintenance or retry receipts"):
        system = creation.succeed("readlink -f /run/current-system").strip()
        candidate = creation.succeed("readlink -f " + shlex.quote(system + "/specialisation/without-template")).strip()
        candidate_plan = creation.succeed("readlink -f " + shlex.quote(candidate + "/etc/tentaflake/admission.json")).strip()
        assert json.loads(creation.succeed("cat " + shlex.quote(candidate_plan)))["creationTemplates"] == {}
        revision = creation_snapshot()["revision"]
        publication = json.loads(creation.succeed(shlex.join(["tentaflake", "admission", "publication", "begin", candidate_plan, "remove-creation-template", str(revision)])))
        creation.succeed(shlex.join([candidate + "/bin/switch-to-configuration", "test"]), timeout=180)
        assert creation.succeed("readlink -f /etc/tentaflake/admission.json").strip() == candidate_plan
        # Finishing reads all recorded original paths/units despite an empty new
        # template catalog. Activation never starts a source quota owner here.
        creation.succeed(shlex.join(["tentaflake", "admission", "publication", "finish",
            "remove-creation-template", str(publication["begunRevision"])]))
        creation_nix_evidence(creation_plan, candidate_plan)
        before = creation_fingerprint()
        creation.succeed(shlex.join(["install", "-m", "0600", "/dev/null", staging]))
        try:
            for action, operation, expected in [("abort", "op-queued-abort", queued_revision),
                    ("abort", "op-partial", partial_revision), ("abort", "op-empty", empty_revision),
                    ("recover", "op-interrupted", interrupted_revision),
                    ("recover", "op-complete", complete_revision)]:
                creation_reconcile(action, operation, expected, plan=candidate_plan)
                assert creation_fingerprint() == before, "terminal retry changed published history"
        finally:
            creation.succeed(shlex.join(["rm", "--", staging]))
        creation_stopped()

    for store_unit in ("nix-store.mount", r"nix-.ro\x2dstore.mount"):
        assert creation.succeed("systemctl show --value --property=DefaultDependencies " + shlex.quote(store_unit)).strip() == "no"
        assert "umount.target" not in creation.succeed("systemctl show --value --property=Conflicts " + shlex.quote(store_unit)).split()
    creation.shutdown()
  '';
}
