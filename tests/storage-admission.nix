# Storage, Worker maintenance and HTTP authentication through real host services.
# Generic OCI controllers remain stopped: this does not prove aggregate OCI
# startup admission, Worker capability readiness or job execution acceptance.
{ self }:
let
  apiProbe = self.inputs.nixpkgs.legacyPackages.x86_64-linux.writeText "tentaflake-api-http-fixture.py" ''
    import hashlib
    import http.client
    import json
    import os
    import pathlib
    import pwd
    import re
    import secrets
    import signal
    import socket
    import stat
    import struct
    import subprocess
    import sys
    import time

    OPERATOR = pathlib.Path("/run/tentaflake-api-fixture-operator")
    BEARER = pathlib.Path("/run/tentaflake-api-fixture-bearer")
    STATE = pathlib.Path("/run/tentaflake-api-fixture-session.json")
    LEDGER = pathlib.Path("/var/lib/tentaflake-admission/ledger.json")
    ORIGIN = "https://api.example.invalid"
    HOST = "api.example.invalid"

    def save(path, value):
        with path.open("x") as target:
            target.write(value)

    def request(method, path, expected, payload=None, extra=None):
        headers = {"Host": HOST, "Origin": ORIGIN}
        headers.update(extra or {})
        data = None
        if payload is not None:
            data = json.dumps(payload).encode()
            headers["Content-Type"] = "application/json"
        connection = http.client.HTTPConnection("127.0.0.1", 18181, timeout=25)
        try:
            connection.request(method, path, body=data, headers=headers)
            response = connection.getresponse()
            body = response.read(32769)
            assert len(body) <= 32768, "HTTP response bound"
            assert response.status == expected, (method, path, response.status, expected)
            assert response.getheader("Cache-Control") == "no-store", "cache policy"
            assert response.getheader("X-Content-Type-Options") == "nosniff", "content sniffing"
            result = json.loads(body) if body else None
            if expected >= 400:
                assert set(result) == {"error", "requestId"}, "error shape"
                assert OPERATOR.read_text().encode() not in body, "operator credential in response"
                assert BEARER.read_text().encode() not in body, "bearer credential in response"
            return result, response.getheader("Set-Cookie")
        finally:
            connection.close()

    def ledger():
        with LEDGER.open("rb") as source:
            raw = source.read(4 * 1024 * 1024 + 1)
        assert len(raw) <= 4 * 1024 * 1024, "ledger bound"
        assert OPERATOR.read_text().encode() not in raw, "operator credential persisted"
        assert BEARER.read_text().encode() not in raw, "bearer credential persisted"
        return json.loads(raw)

    def wait_ready():
        deadline = time.monotonic() + 10
        while True:
            try:
                return request("GET", "/api/meta", 200)[0]
            except (ConnectionError, OSError):
                if time.monotonic() >= deadline:
                    raise AssertionError("API did not listen before deadline") from None
                time.sleep(0.1)

    def no_logged_credentials():
        output = subprocess.run(["journalctl", "--no-pager", "-o", "cat", "-n", "200",
                                 "-u", "tentaflake-api.service", "-u", "tentaflake-api-owner.service"],
                                check=True, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, timeout=5).stdout
        assert OPERATOR.read_text().encode() not in output, "operator credential in journal"
        assert BEARER.read_text().encode() not in output, "bearer credential in journal"

    def main_pid(unit):
        value = subprocess.run(["systemctl", "show", "--value", "--property=MainPID", unit],
                               check=True, capture_output=True, text=True, timeout=5).stdout.strip()
        assert value.isdecimal() and int(value) > 0, "service has no main process"
        return int(value)

    def boundary():
        wait_ready()
        account = pwd.getpwnam("tentaflake-api")
        assert account.pw_uid != 0 and account.pw_gid != 0, "frontend account is privileged"
        frontend = main_pid("tentaflake-api.service")
        owner = main_pid("tentaflake-api-owner.service")
        for pid, uid, capabilities in [(frontend, account.pw_uid, 0), (owner, 0, 1 << 19)]:
            fields = dict(line.split(":", 1) for line in pathlib.Path(f"/proc/{pid}/status").read_text().splitlines())
            assert set(fields["Uid"].split()) == {str(uid)}, "service uid differs"
            assert int(fields["CapEff"].strip(), 16) == capabilities, "unexpected effective capabilities"
        host_namespace = os.stat("/proc/1/ns/mnt")
        owner_namespace = os.stat(f"/proc/{owner}/ns/mnt")
        assert (owner_namespace.st_dev, owner_namespace.st_ino) == (host_namespace.st_dev, host_namespace.st_ino), "owner left host namespace"
        environment = pathlib.Path(f"/proc/{frontend}/environ").read_bytes().split(b"\0")
        assert not any(value.startswith(b"CREDENTIALS_DIRECTORY=") for value in environment), "frontend received credentials"
        channel = os.stat("/run/tentaflake-api-owner.sock")
        assert stat.S_ISSOCK(channel.st_mode) and stat.S_IMODE(channel.st_mode) == 0o660, "owner socket protection"
        assert (channel.st_uid, channel.st_gid) == (0, account.pw_gid), "owner socket ownership"
        denied_paths = [str(LEDGER), "/var/lib/tentaflake-admission.lock", str(OPERATOR), str(BEARER),
                        "/run/credentials/tentaflake-api-owner.service/operator-token",
                        "/run/credentials/tentaflake-api-owner.service/cli-bearer"]
        code = """import os, sys
    for path in sys.argv[1:]:
        try:
            descriptor = os.open(path, os.O_RDONLY | os.O_NONBLOCK)
        except PermissionError:
            continue
        os.close(descriptor)
        raise AssertionError('frontend accessed a protected file')
    """
        subprocess.run(["nsenter", f"--mount=/proc/{frontend}/ns/mnt", "setpriv",
                        "--reuid", str(account.pw_uid), "--regid", str(account.pw_gid), "--clear-groups",
                        "python3", "-c", code, *denied_paths], check=True, timeout=10)
        # Root can reach this socket but is not its authorized frontend peer.
        # A valid discovery frame must be refused before any response bytes.
        assert os.geteuid() == 0, "wrong-peer probe must run as root"
        frame = json.dumps({"schemaVersion": 1, "method": "GET", "target": "/api/meta",
                            "headers": [["host", list(HOST.encode("ascii"))],
                                        ["origin", list(ORIGIN.encode("ascii"))]], "body": []}).encode()
        assert 0 < len(frame) <= 1024, "wrong-peer frame bound"
        with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as channel:
            channel.settimeout(5)
            channel.connect("/run/tentaflake-api-owner.sock")
            try:
                channel.sendall(struct.pack("!I", len(frame)) + frame)
                assert channel.recv(1) == b"", "owner answered an unauthorized peer"
            except (ConnectionResetError, BrokenPipeError):
                pass
        request("GET", "/api/meta", 200)

    def live_cookie():
        saved = json.loads(STATE.read_text())
        request("GET", "/api/v1/session", 200, extra={"Cookie": saved["cookie"]})
        assert ledger() == saved["ledger"], "session read mutated ledger"

    os.umask(0o077)
    phase = sys.argv[1]
    if phase == "seed":
        save(OPERATOR, secrets.token_hex(32))
        save(BEARER, secrets.token_hex(32))
        assert OPERATOR.read_text() != BEARER.read_text(), "independent fixture credentials"
    elif phase == "rotate":
        OPERATOR.write_text(secrets.token_hex(32))
    elif phase == "boundary":
        boundary()
    elif phase == "active":
        live_cookie()
    elif phase == "live-login":
        wait_ready()
        before = ledger()
        _, cookie = request("POST", "/api/v1/session", 200,
                            {"method": "operator-token", "secret": OPERATOR.read_text()})
        assert cookie is not None, "active session cookie missing"
        cookie = cookie.split("; ", 1)[0]
        assert re.fullmatch(r"__Host-tentaflake-session=[0-9a-f]{64}", cookie), "active cookie shape"
        after = ledger()
        fingerprint = hashlib.sha256(b"tentaflake/operator-login/v1\0" + OPERATOR.read_bytes()).hexdigest()
        assert after == {**before, "revision": before["revision"] + 1,
                         "operatorLogins": before["operatorLogins"] + [
                             {"fingerprint": fingerprint, "consumedRevision": before["revision"] + 1}]}, "fresh login changed more than consumption"
        STATE.write_text(json.dumps({"ledger": after, "cookie": cookie}))
        live_cookie()
    elif phase == "unavailable":
        wait_ready()
        before = ledger()
        owner = main_pid("tentaflake-api-owner.service")
        descriptor = os.pidfd_open(owner)
        try:
            assert main_pid("tentaflake-api-owner.service") == owner, "owner changed before pause"
            signal.pidfd_send_signal(descriptor, signal.SIGSTOP)
            deadline = time.monotonic() + 5
            while pathlib.Path(f"/proc/{owner}/stat").read_text().rsplit(")", 1)[1].split()[0] != "T":
                assert time.monotonic() < deadline, "owner did not stop"
                time.sleep(0.02)
            request("GET", "/api/meta", 503)
            request("GET", "/api/v1/session", 503,
                    extra={"Authorization": "Bearer " + BEARER.read_text()})
            assert ledger() == before, "unavailable owner mutated ledger"
        finally:
            try:
                signal.pidfd_send_signal(descriptor, signal.SIGCONT)
            finally:
                os.close(descriptor)
        wait_ready()
        assert ledger() == before, "late read mutated ledger"
        no_logged_credentials()
    elif phase == "login":
        before = ledger()
        meta = wait_ready()
        assert meta == {"service": "tentaflake", "apiVersions": [1], "host": "api-fixture",
                        "auth": {"methods": ["operator-token"]}, "backend": "implemented"}, "minimal discovery"
        request("GET", "/api/meta", 403, extra={"Host": "other.example.invalid"})
        request("GET", "/api/v1/session", 401)
        request("GET", "/api/v1/capabilities", 401)
        login = {"method": "operator-token", "secret": OPERATOR.read_text()}
        request("POST", "/api/v1/session", 403, login, {"Origin": "https://other.example.invalid"})
        request("POST", "/api/v1/session", 403, login, {"Host": "other.example.invalid"})
        request("POST", "/api/v1/session", 401, {"method": "operator-token", "secret": secrets.token_hex(32)})
        assert ledger() == before, "refused login changed durable state"
        session, cookie_header = request("POST", "/api/v1/session", 200, login)
        assert cookie_header is not None, "session cookie missing"
        parts = cookie_header.split("; ")
        assert re.fullmatch(r"__Host-tentaflake-session=[0-9a-f]{64}", parts[0]), "cookie shape"
        assert set(parts[1:]) == {"Secure", "HttpOnly", "SameSite=Strict", "Path=/"}, "cookie attributes"
        assert set(session) == {"user", "csrfToken", "expiresAt", "grants", "revisions", "mutationEpoch"}, "session shape"
        assert session["user"] == {"id": "fixture-operator", "name": "Fixture operator"}, "configured identity"
        assert session["grants"] == [], "authentication must not invent grants"
        assert re.fullmatch(r"[0-9a-f]{64}", session["csrfToken"]), "CSRF shape"
        assert re.fullmatch(r"[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}:[0-9]{2}Z", session["expiresAt"]), "session expiry"
        cookie = {"Cookie": parts[0]}
        observed, _ = request("GET", "/api/v1/session", 200, extra=cookie)
        assert observed["user"] == session["user"] and observed["csrfToken"] == session["csrfToken"], "session identity"
        after = ledger()
        assert after["revision"] == before["revision"] + 1, "exact login consumption revision"
        assert after["storage"] == before["storage"] and after["receipts"] == before["receipts"], "login resource mutation"
        assert after["published"] == before["published"], "login authority mutation"
        fingerprint = hashlib.sha256(b"tentaflake/operator-login/v1\0" + OPERATOR.read_bytes()).hexdigest()
        assert after["operatorLogins"] == [{"fingerprint": fingerprint, "consumedRevision": after["revision"]}], "single durable fingerprint"
        request("POST", "/api/v1/session", 401, login)
        request("DELETE", "/api/v1/session", 403, extra=cookie)
        request("DELETE", "/api/v1/session", 403, extra={**cookie, "X-Tentaflake-CSRF": "incorrect"})
        request("GET", "/api/v1/session", 200, extra=cookie)
        bearer = {"Authorization": "Bearer " + BEARER.read_text()}
        authenticated, _ = request("GET", "/api/v1/session", 200, extra=bearer)
        assert authenticated["user"] == session["user"] and authenticated["grants"] == [], "independent CLI identity"
        request("GET", "/api/v1/session", 400, extra={**cookie, **bearer})
        request("GET", "/api/v1/session", 401, extra={"Authorization": "Bearer " + OPERATOR.read_text()})
        capabilities, _ = request("GET", "/api/v1/capabilities", 200, extra=cookie)
        assert capabilities["revisions"] == session["revisions"], "capability authority differs from session"
        assert capabilities["features"] == {}, "declaration must not enable resource actions"
        assert all(not option["availability"]["available"] and option["evidence"] == []
                   for option in capabilities["isolation"]), "declaration must not invent runtime acceptance"
        plan = json.loads(pathlib.Path("/etc/tentaflake/admission.json").read_text())
        policy = plan["policy"]
        assert capabilities["limits"]["maxActiveEnvironments"] == min(1024, policy["maxActive"]), "published active bound"
        for field, resource, unit, minimum, maximum in [("cpu", "nanoCpus", 1000000000, 1, 1024),
                ("memoryMiB", "memoryBytes", 1048576, 128, 1048576),
                ("storageMiB", "workspaceBytes", 1048576, 128, 1048576)]:
            budget = policy["capacity"][resource] - policy["hostReserve"][resource] - policy["researchReserve"][resource]
            upper = min(maximum, min(budget, policy["perInstance"][resource]) // unit)
            assert capabilities["limits"][field]["max"] == (upper if upper >= minimum else 0), "published resource bound"
        same, _ = request("GET", "/api/v1/capabilities", 200, extra=bearer)
        assert same == capabilities, "cookie and bearer capability authority differs"
        for path in ["/api/v1/environments"]:
            result, _ = request("GET", path, 501, extra=cookie)
            assert result["error"]["code"] == "unsupported", "unimplemented operation must refuse"
        result, cleared = request("DELETE", "/api/v1/session", 204,
                                  extra={**cookie, "X-Tentaflake-CSRF": session["csrfToken"]})
        assert result is None and cleared is not None and "Max-Age=0" in cleared, "logout cookie expiry"
        request("GET", "/api/v1/session", 401, extra=cookie)
        assert ledger() == after, "read/refusal/logout changed durable state"
        save(STATE, json.dumps({"ledger": after, "cookie": parts[0]}))
        no_logged_credentials()
    elif phase == "restart":
        saved = json.loads(STATE.read_text())
        wait_ready()
        request("POST", "/api/v1/session", 401, {"method": "operator-token", "secret": OPERATOR.read_text()})
        request("GET", "/api/v1/session", 401, extra={"Cookie": saved["cookie"]})
        request("GET", "/api/v1/session", 200, extra={"Authorization": "Bearer " + BEARER.read_text()})
        assert ledger() == saved["ledger"], "restart resurrected consumed credential"
        no_logged_credentials()
    else:
        raise AssertionError("unknown fixture phase")
    print("API fixture phase passed: " + phase)
  '';
in
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
          inherit name;
          definition =
            definition
            // lib.optionalAttrs (name == "storage") {
              capabilities = definition.capabilities ++ [ "worker" ];
            };
        }
      ) names;

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
        # These tests execute prebuilt closures; admission does not support the
        # VM harness's writable-store overlay with unobserved backing options.
        writableStore = false;
        fileSystems."/nix/store".options = [ "ro" ];
        oci-containers.backend = "docker";
        docker.enable = true;
        docker.daemon.settings."live-restore" = false;
        memorySize = 768;
        diskSize = 2048;
      };
      # Keep shutdown executables available until the final shutdown ramfs;
      # persistent volumes retain their normal unmount behavior.
      systemd.units =
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
      environment.systemPackages = [
        pkgs.btrfs-progs
        pkgs.python3
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
      # The fixture owns each activation. Publication still accepts failed
      # activation only for the existing guarded quota services.
      systemd = {
        services = {
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
          tentaflake-worker-generic-storage.wantedBy = lib.mkForce [ ];
          tentaflake-worker-image.wantedBy = lib.mkForce [ ];
          tentaflake-api.wantedBy = lib.mkForce [ ];
        };
        paths.tentaflake-worker-generic-storage.wantedBy = lib.mkForce [ ];
      };

      tentaflake = {
        hostName = "storage-host";
        adminUser = "admin";
        boot.enable = false;
        modernConsole.enable = false;
        nixSettings.enable = false;
        shell.enable = true;
        api = {
          enable = true;
          host = "api-fixture";
          origin = "https://api.example.invalid";
          listen = "127.0.0.1:18181";
          principal = {
            id = "fixture-operator";
            name = "Fixture operator";
          };
          operatorTokenFile = "/run/tentaflake-api-fixture-operator";
          cliBearerFile = "/run/tentaflake-api-fixture-bearer";
        };
        worker.agents.generic-storage = {
          enable = true;
          workspace = "/var/lib/generic-storage/workspace";
          containerUid = 65534;
          containerGid = 65534;
        };
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
    storage_refusals = {}
    storage_worker = storage_manifest["workers"]["generic-storage"]
    storage_worker_image = storage_manifest["workerImage"]
    storage_worker_units = [storage_worker["watcher"], storage_worker["unit"], storage_worker_image["unit"]]
    storage_worker_pending = storage_worker["stateDir"] + "/pending/maintenance_pending.json"

    def storage_nix_evidence(*plans):
        storage.wait_for_unit("register-nix-paths.service")
        storage.wait_for_unit("nix-daemon.socket")
        assert storage.succeed("systemctl show --value --property=Result register-nix-paths.service").strip() == "success"
        options = storage.succeed("findmnt --task 1 --first-only --direction backward -n -o OPTIONS --mountpoint /nix/store").strip().split(",")
        assert "ro" in options and "rw" not in options, options
        storage.succeed("test -d /nix/store/.links && test ! -L /nix/store/.links")
        for plan in plans:
            assert plan.startswith("/nix/store/") and "/" not in plan[len("/nix/store/"):], plan
            root = "/nix/var/nix/gcroots/tentaflake/" + plan.rsplit("/", 1)[1]
            assert storage.succeed("readlink -- " + shlex.quote(root)).strip() == plan
            assert storage.succeed("test -L " + shlex.quote(root) + " && stat -c %u:%h -- " + shlex.quote(root)).strip() == "0:1"

    storage_nix_evidence()

    def worker_command(action, *arguments):
        return shlex.join([storage_worker["executable"], "--config", storage_worker["config"], action, *arguments])

    def worker_queue_snapshot():
        # Metadata and bytes, excluding access times changed by observation.
        script = 'find "$1" -xdev -printf "%P %y %m %U %G %s %T@\\n" | sort; find "$1" -xdev -type f -exec sha256sum -- {} + | sort'
        return storage.succeed(shlex.join(["bash", "-euo", "pipefail", "-c", script, "--", storage_worker["stateDir"]]))

    def worker_refuses_mutations():
        before = worker_queue_snapshot()
        containers = storage.succeed("docker ps --all --quiet")
        for action, arguments in [("drain", []), ("approve", ["maintenance_pending"]), ("deny", ["maintenance_pending"])]:
            error = storage.fail(worker_command(action, *arguments) + " 2>&1")
            assert "Worker" in error and any(word in error for word in ("authority", "Pending", "unknown")), error
            assert worker_queue_snapshot() == before
        storage.fail(shlex.join(["tentaflake", "admission", "worker-load", storage_worker_image["config"]]))
        assert worker_queue_snapshot() == before
        assert storage.succeed("docker ps --all --quiet") == containers

    def stop_storage_workers():
        storage.succeed("timeout 150s systemctl stop " + shlex.join(storage_worker_units))
        for unit in storage_worker_units:
            assert storage.succeed("systemctl show --value --property=ActiveState " + shlex.quote(unit)).strip() == "inactive"
        jobs = storage.succeed("systemctl list-jobs --no-legend --no-pager --full")
        assert all(not set(storage_worker_units).intersection(line.split()) for line in jobs.splitlines()), jobs

    def storage_command(action, allocation=None, plan=storage_plan):
        arguments = ["tentaflake", "admission", action, plan]
        if allocation is not None:
            arguments.append(allocation)
        return shlex.join(arguments)

    def storage_owner_state(unit):
        output = storage.succeed("systemctl show --property=ActiveState,Result,ExecMainStatus,InvocationID " + shlex.quote(unit))
        return dict(line.split("=", 1) for line in output.splitlines())

    def record_storage_refusal(unit, previous):
        state = storage_owner_state(unit)
        invocation = state["InvocationID"]
        assert len(invocation) == 32 and all(char in "0123456789abcdef" for char in invocation), (unit, state)
        assert invocation != previous, (unit, state)
        assert state["ActiveState"] == "failed" and state["Result"] == "exit-code" and state["ExecMainStatus"] == "2", (unit, state)
        storage_refusals[unit] = invocation

    def refuse_storage_start(volume):
        # A workspace start first starts its state owner; that prerequisite is
        # the exact refusal expected by these storage and Pending scenarios.
        instance = volume["spec"]["allocationId"].split(":", 1)[1]
        refused_unit = storage_volumes["state:" + instance]["units"]["owner"]
        previous = storage_owner_state(refused_unit)["InvocationID"]
        storage.fail("systemctl start " + shlex.quote(volume["units"]["owner"]))
        record_storage_refusal(refused_unit, previous)

    def stop_storage_units():
        failed_before = set()
        for unit in storage_units:
            state = storage_owner_state(unit)
            if state["ActiveState"] == "failed":
                assert storage_refusals.get(unit) == state["InvocationID"], (unit, state)
                assert state["Result"] == "exit-code" and state["ExecMainStatus"] == "2", (unit, state)
                failed_before.add(unit)
        storage.succeed("timeout 150s systemctl stop " + shlex.join(storage_units))
        for unit in storage_units:
            state = storage.succeed("systemctl show --value --property=ActiveState " + shlex.quote(unit)).strip()
            assert state == "inactive" or (unit in failed_before and state == "failed"), (unit, state)
        jobs = storage.succeed("systemctl list-jobs --no-legend --no-pager --full")
        assert all(not set(storage_units).intersection(line.split()) for line in jobs.splitlines()), jobs
        # Clear only proven CLI refusals after their stop/no-job evidence. A
        # successful stop can unload its unit, so never reset all owners blindly.
        for unit in failed_before:
            state = storage_owner_state(unit)
            if state["ActiveState"] == "failed":
                assert state["InvocationID"] == storage_refusals[unit], (unit, state)
                assert state["Result"] == "exit-code" and state["ExecMainStatus"] == "2", (unit, state)
                storage.succeed("systemctl reset-failed " + shlex.quote(unit))
                assert storage_owner_state(unit)["ActiveState"] == "inactive", unit
        storage_refusals.clear()

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
        stop_storage_workers()
        request = json.dumps({"version": 1, "id": "maintenance_pending", "action_class": "communicative", "argv": ["true"], "timeout_seconds": 1})
        storage.succeed("umask 077; printf %s " + shlex.quote(request) + " > " + shlex.quote(storage_worker_pending))
        worker_refuses_mutations()
        for volume in storage_volumes.values():
            refuse_storage_start(volume)
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
        storage_nix_evidence(storage_plan)
        initial = storage_snapshot()
        assert initial["storage"] == [] and initial["receipts"] == [], initial
        assert initial["published"]["manifestPath"] == storage_plan, initial
        storage.fail(storage_command("bootstrap"))
        assert storage_snapshot() == initial
        assert json.loads(storage.succeed(worker_command("maintenance"))) == {
            "schemaVersion": 1, "agent": "generic-storage", "runningEntries": 0, "jobsEntries": 0,
        }

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
            refuse_storage_start(storage_state)
            assert "storage admission failed; no image creation authorized" in storage.succeed(
                "journalctl --no-pager -o cat -n 30 -u " + shlex.quote(unit)
            )
            assert_storage_absent(storage_state)
            assert_storage_absent(storage_workspace)
            assert storage_snapshot() == initial
        finally:
            storage.succeed("rm -- " + shlex.quote(filler))
            stop_storage_units()

    with subtest("real HTTP authentication consumes credentials durably without resource grants"):
        # Credentials exist only in the guest runtime. This exercises the local
        # HTTP listener behind its declared origin, not an HTTPS relay or a UI.
        api_probe = ${builtins.toJSON (toString apiProbe)}
        api_configuration = json.loads(storage.succeed("cat " + shlex.quote(storage_manifest["apiConfig"])))
        assert api_configuration["origin"] == "https://api.example.invalid"
        assert api_configuration["operatorLogin"] and api_configuration["cliBearer"]
        before_auth = storage_snapshot()
        storage.succeed(shlex.join(["python3", api_probe, "seed"]))
        try:
            storage.succeed("systemctl start tentaflake-api.service")
            storage.wait_for_unit("tentaflake-api.service")
            storage.succeed(shlex.join(["python3", api_probe, "login"]))
            storage.succeed(shlex.join(["python3", api_probe, "boundary"]))
            authenticated = storage_snapshot()
            assert authenticated["revision"] == before_auth["revision"] + 1
            assert authenticated["storage"] == before_auth["storage"]
            assert authenticated["receipts"] == before_auth["receipts"]
            # The earlier cookie was explicitly logged out. Rotate only the
            # guest credential and reload its owner to establish a fresh active
            # session before each independent process-lifetime revocation.
            for action in ("restart", "kill"):
                storage.succeed(shlex.join(["python3", api_probe, "rotate"]))
                storage.succeed("systemctl restart tentaflake-api.service")
                storage.wait_for_unit("tentaflake-api.service")
                storage.succeed(shlex.join(["python3", api_probe, "live-login"]))
                active = storage_snapshot()
                assert active["revision"] == authenticated["revision"] + 1
                assert active["storage"] == authenticated["storage"]
                assert active["receipts"] == authenticated["receipts"]
                owner_invocation = storage.succeed("systemctl show --value --property=InvocationID tentaflake-api-owner.service").strip()
                assert len(owner_invocation) == 32
                storage.succeed(shlex.join(["python3", api_probe, "active"]))
                if action == "kill":
                    storage.succeed("systemctl kill --kill-whom=main --signal=KILL tentaflake-api.service")
                    storage.wait_until_succeeds('test "$(systemctl show --value --property=ActiveState tentaflake-api.service)" = failed', timeout=20)
                    storage.wait_until_succeeds('test "$(systemctl show --value --property=ActiveState tentaflake-api-owner.service)" = inactive', timeout=20)
                    storage.wait_until_succeeds('test "$(systemctl show --value --property=ActiveState tentaflake-api-owner.socket)" = inactive', timeout=20)
                    storage.succeed("test ! -e /run/tentaflake-api-owner.sock")
                    assert storage_snapshot() == active
                    storage.succeed("systemctl start tentaflake-api.service")
                else:
                    storage.succeed("systemctl restart tentaflake-api.service")
                storage.wait_for_unit("tentaflake-api.service")
                storage.succeed(shlex.join(["python3", api_probe, "restart"]))
                assert storage.succeed("systemctl show --value --property=InvocationID tentaflake-api-owner.service").strip() != owner_invocation
                assert storage_snapshot() == active
                authenticated = active
            storage.succeed(shlex.join(["python3", api_probe, "boundary"]))
            storage.succeed(shlex.join(["python3", api_probe, "unavailable"]))
            assert storage_snapshot() == authenticated
        finally:
            storage.succeed("timeout 30s systemctl stop tentaflake-api.service")
            for unit in ("tentaflake-api.service", "tentaflake-api-owner.service", "tentaflake-api-owner.socket"):
                assert storage.succeed("systemctl show --value --property=ActiveState " + shlex.quote(unit)).strip() == "inactive"
            storage.succeed("test ! -e /run/tentaflake-api-owner.sock")

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
            refuse_storage_start(volume)
            assert_storage_absent(volume)
        assert storage_snapshot() == charged
        stop_storage_units()

    with subtest("retained missing images and partial staging never authorize recreation"):
        image = storage_state["spec"]["imagePath"]
        saved = "/var/lib/storage-state-image.saved"
        storage.succeed("mv " + shlex.join([image, saved]))
        try:
            refuse_storage_start(storage_state)
            assert_storage_absent(storage_state)
            stop_storage_units()
            storage.fail(storage_command("storage-recover", "state:generic-storage"))
            assert storage_snapshot() == charged
        finally:
            storage.succeed("mv " + shlex.join([saved, image]))
        staging = image + ".new"
        storage.succeed("printf incomplete > " + shlex.quote(staging))
        try:
            refuse_storage_start(storage_state)
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
            refuse_storage_start(storage_state)
            worker_refuses_mutations()
        finally:
            storage.succeed("ln -sfn " + shlex.join([storage_plan, "/etc/tentaflake/admission.json"]))
        stop_storage_units()
        storage.succeed("chmod 0777 /var/lib/tentaflake-admission")
        try:
            refuse_storage_start(storage_state)
            worker_refuses_mutations()
        finally:
            storage.succeed("chmod 0700 /var/lib/tentaflake-admission")
        stop_storage_units()
        storage.succeed("mv /var/lib/tentaflake-admission /var/lib/storage-ledger.saved")
        try:
            refuse_storage_start(storage_state)
            worker_refuses_mutations()
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
    assert candidate_manifest["apiConfig"] == storage_manifest["apiConfig"]
    assert candidate_manifest["instances"] == storage_manifest["instances"]
    assert [volume["spec"] for volume in candidate_manifest["volumes"]] == [volume["spec"] for volume in storage_manifest["volumes"]]

    def publication(action, *arguments, success=True):
        command = shlex.join(["tentaflake", "admission", "publication", action, *map(str, arguments)])
        if not success:
            # The CLI reports refusals on stderr; fail still checks the exit status.
            return storage.fail(command + " 2>&1")
        return json.loads(storage.succeed(command))

    def install_storage_generation(system, plan):
        # NixOS may try wanted quota units during activation. Pending must refuse
        # those starts; exit 4 is accepted only for these expected failed units.
        previous = {unit: storage_owner_state(unit)["InvocationID"] for unit in storage_units}
        code, output = storage.execute(shlex.quote(system + "/bin/switch-to-configuration") + " test")
        assert code in (0, 4), (code, output)
        failed = storage.succeed("systemctl list-units --state=failed --no-legend --plain").splitlines()
        failed_names = {line.split()[0] for line in failed if line.strip()}
        assert failed_names.issubset(set(storage_units)), (failed_names, output)
        assert code == 0 or failed_names, output
        for unit in failed_names:
            record_storage_refusal(unit, previous[unit])
        assert storage.succeed("readlink -f /run/current-system").strip() == system
        assert storage.succeed("readlink -f /etc/tentaflake/admission.json").strip() == plan
        storage.succeed("systemctl daemon-reload")
        stop_storage_units()
        for unit in storage_units:
            assert plan in storage.succeed("systemctl show --value --property=ExecStart " + shlex.quote(unit))

    with subtest("Worker watchers and the real guarded image loader must stop before publication"):
        before_load = publication("status")
        before_storage = storage_snapshot()
        queue_before = worker_queue_snapshot()
        # This invokes the actual guarded archive load after authority exists.
        # RemainAfterExit keeps the loader active even with no Docker containers.
        storage.succeed("systemctl start " + shlex.quote(storage_worker_image["unit"]))
        storage.wait_for_unit(storage_worker_image["unit"])
        stable = publication("status")
        assert stable["ledgerRevision"] == before_load["ledgerRevision"] + 2
        assert stable["workerImageLoad"]["outcome"] == "completed"
        hidden = storage.succeed("tentaflake admission publication status --hide")
        assert "/nix/store/" not in hidden
        assert json.loads(hidden)["workerImageLoad"]["configuration"] == "redacted"
        assert stable["published"] == before_load["published"] and stable["pending"] is None
        after_storage = storage_snapshot()
        assert after_storage["storage"] == before_storage["storage"]
        assert after_storage["receipts"] == before_storage["receipts"]
        assert storage.succeed("docker ps --all --quiet").strip() == ""
        error = publication("begin", candidate_plan, "worker-loader-active", stable["ledgerRevision"], success=False)
        assert "Worker service, watcher and image loader must be stopped" in error, error
        assert publication("status") == stable and worker_queue_snapshot() == queue_before
        stop_storage_workers()
        start_storage_workspace()
        storage.succeed("systemctl start " + shlex.quote(storage_worker["watcher"]))
        storage.wait_for_unit(storage_worker["watcher"])
        error = publication("begin", candidate_plan, "worker-watcher-active", stable["ledgerRevision"], success=False)
        assert "Worker service, watcher and image loader must be stopped" in error, error
        assert publication("status") == stable and worker_queue_snapshot() == queue_before
        stop_storage_workers()
        stop_storage_units()

    with subtest("retained Worker running and job entries block maintenance despite empty Docker"):
        stable = publication("status")
        before = storage_snapshot()
        for directory, name in [("running", ".unfinished.json.new"), ("jobs", "orphan-dispatch")]:
            path = storage_worker["stateDir"] + "/" + directory + "/" + name
            if directory == "running":
                storage.succeed("umask 077; printf incomplete > " + shlex.quote(path))
            else:
                storage.succeed("mkdir -m 0700 -- " + shlex.quote(path))
            try:
                queue_before = worker_queue_snapshot()
                assert storage.succeed("docker ps --all --quiet").strip() == ""
                storage.fail(worker_command("maintenance"))
                publication("begin", candidate_plan, "worker-retained-" + directory, stable["ledgerRevision"], success=False)
                assert worker_queue_snapshot() == queue_before
                assert storage_snapshot() == before and publication("status") == stable
                assert storage.succeed("docker ps --all --quiet").strip() == ""
            finally:
                storage.succeed(("rm -- " if directory == "running" else "rmdir -- ") + shlex.quote(path))
        assert json.loads(storage.succeed(worker_command("maintenance")))["jobsEntries"] == 0

    with subtest("legacy reserved-name Worker state is not exempt from maintenance"):
        orphan = "/var/lib/tentaflake-worker-state-volumes"
        storage.succeed("test ! -e " + shlex.quote(orphan))
        storage.succeed("install -d -m 0700 " + shlex.quote(orphan + "/running"))
        try:
            stable = publication("status")
            error = publication("begin", candidate_plan, "worker-legacy-state", stable["ledgerRevision"], success=False)
            assert "undeclared Worker state" in error, error
            assert publication("status") == stable
        finally:
            storage.succeed(shlex.join(["rmdir", orphan + "/running", orphan]))

    with subtest("uncertain Worker image loads require exact recovery with the backend stopped"):
        # Seed the durable post-crash state. Ledger subprocess tests separately
        # exercise actual death/write failures before and after runtime dispatch.
        uncertain_ledger = storage_snapshot()
        uncertain_ledger["revision"] += 1
        uncertain_ledger["workerImageLoad"].update({
            "outcome": "unknown", "claimedRevision": uncertain_ledger["revision"],
            "updatedRevision": uncertain_ledger["revision"],
        })
        storage.succeed(shlex.join(["bash", "-euo", "pipefail", "-c", 'printf %s "$1" > "$2"; sync "$2"', "--", json.dumps(uncertain_ledger), storage_ledger]))
        uncertain = publication("status")
        worker_refuses_mutations()
        publication("begin", candidate_plan, "worker-uncertain-load", uncertain["ledgerRevision"], success=False)
        claim = uncertain["workerImageLoad"]["claimedRevision"]
        def load_recovery(revision, claimed):
            return shlex.join(["tentaflake", "admission", "worker-load-recover", storage_plan, str(revision), str(claimed)])
        storage.fail(load_recovery(uncertain["ledgerRevision"] - 1, claim))
        storage.fail(load_recovery(uncertain["ledgerRevision"], claim - 1))
        storage.fail(load_recovery(uncertain["ledgerRevision"], claim))
        assert publication("status") == uncertain and storage_snapshot() == uncertain_ledger
        storage.succeed("timeout 150s systemctl stop " + shlex.join(docker_units))
        storage.succeed(load_recovery(uncertain["ledgerRevision"], claim))
        recovered = publication("status")
        assert recovered["workerImageLoad"]["outcome"] == "recovered"
        assert recovered["ledgerRevision"] == uncertain["ledgerRevision"] + 1
        assert recovered["published"] == uncertain["published"]
        assert storage_snapshot()["storage"] == uncertain_ledger["storage"]
        assert storage_snapshot()["receipts"] == uncertain_ledger["receipts"]
        storage.fail(load_recovery(recovered["ledgerRevision"], claim))
        assert publication("status") == recovered
        storage.succeed("timeout 150s systemctl start docker.service")
        storage.wait_for_unit("docker.service")

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
        storage_nix_evidence(storage_plan, candidate_plan)
        assert begun["outcome"] == "pending"
        pending = publication("status")
        assert pending["pending"] == begun and pending["published"] == stable["published"]
        assert pending["ledgerRevision"] == stable["ledgerRevision"] + 1
        worker_refuses_mutations()
        assert publication("status") == pending
        assert publication("begin", candidate_plan, "publish-capacity", stable["ledgerRevision"]) == begun
        publication("finish", "publish-capacity", stable["ledgerRevision"], success=False)
        publication("finish", "publish-capacity", pending["ledgerRevision"], success=False)
        assert publication("status") == pending
        refuse_storage_start(storage_workspace)
        stop_storage_units()
        assert publication("status") == pending
        assert storage_snapshot()["storage"] == charged["storage"]
        install_storage_generation(candidate_system, candidate_plan)
        storage_nix_evidence(storage_plan, candidate_plan)
        assert publication("status") == pending
        finished = publication("finish", "publish-capacity", pending["ledgerRevision"])
        storage_nix_evidence(storage_plan, candidate_plan)
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
        storage_nix_evidence(storage_plan, candidate_plan)
        pending = publication("status")
        assert pending["pending"] == begun
        install_storage_generation(base_system, storage_plan)
        storage_nix_evidence(storage_plan, candidate_plan)
        publication("cancel", "rollback-capacity", pending["ledgerRevision"], success=False)
        assert publication("status") == pending
        install_storage_generation(candidate_system, candidate_plan)
        storage_nix_evidence(storage_plan, candidate_plan)
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
            refuse_storage_start(unused_state)
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
            assert "publication requires retained runtime claims and Present storage; recover uncertain state before beginning" in error, error
            assert publication("status") == uncertain and uncertain["pending"] is None
            assert storage_snapshot() == uncertain_ledger
        finally:
            storage.succeed("rm -- " + shlex.quote(obstacle))
    for store_unit in ("nix-store.mount", r"nix-.ro\x2dstore.mount"):
        assert storage.succeed("systemctl show --value --property=DefaultDependencies " + shlex.quote(store_unit)).strip() == "no"
        assert "umount.target" not in storage.succeed("systemctl show --value --property=Conflicts " + shlex.quote(store_unit)).split()
    storage.shutdown()
  '';
}
