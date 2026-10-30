# Exercise the real API sandbox, worker binary and systemd path activation.
# No vendor runtime, OCI image, credentials or external service is needed.
{ self, pkgs }:
pkgs.testers.runNixOSTest {
  name = "tentaflake-worker-drain-wakeup";

  nodes.machine =
    { lib, ... }:
    {
      imports = [ self.nixosModules.default ];
      virtualisation = {
        cores = 2;
        memorySize = 1536;
        oci-containers = {
          backend = "podman";
          containers.hermes-wakeup = {
            image = "unused-worker-fixture:latest";
            autoStart = false;
          };
        };
      };
      tentaflake = {
        hostName = "worker-wakeup-fixture";
        adminUser = "operator";
        boot.enable = false;
        modernConsole.enable = false;
        nixSettings.enable = false;
        hostApi = {
          enable = true;
          allowToolRequests = true;
          allowToolDecisions = true;
        };
        worker = {
          imageReference = "unused-worker-fixture:latest";
          agents.hermes-wakeup = {
            enable = true;
            workspace = "/var/lib/worker-wakeup/workspace";
          };
        };
      };
      environment.systemPackages = [ pkgs.curl ];
      systemd.services = {
        # Replace the whole dependency so even its image derivation is unused.
        tentaflake-worker-image = lib.mkForce {
          serviceConfig = {
            Type = "oneshot";
            RemainAfterExit = true;
            ExecStart = "${pkgs.coreutils}/bin/true";
          };
        };
        # Any accidental controller execution is a fixture failure, not a pull.
        podman-hermes-wakeup.serviceConfig.ExecStart = lib.mkForce "${pkgs.coreutils}/bin/false";
        tentaflake-worker-hermes-wakeup.serviceConfig = {
          TimeoutStartSec = "120s";
          ExecStartPost = pkgs.writeShellScript "hold-worker-after-drain" ''
            set -eu
            state=/var/lib/tentaflake-worker-hermes-wakeup
            count=0
            if test -f "$state/test-drains"; then
              count=$(cat "$state/test-drains")
            fi
            echo "$((count + 1))" > "$state/test-drains"
            if test -e "$state/test-hold"; then
              touch "$state/test-held"
              while test -e "$state/test-hold"; do
                sleep 0.1
              done
              rm "$state/test-held"
            fi
          '';
        };
      };
    };

  testScript = ''
    import json
    import shlex

    # The persistence subtest reboots; omit QEMU's default -no-reboot.
    machine.start(allow_reboot=True)
    unit = "tentaflake-worker-hermes-wakeup.service"
    watcher = "tentaflake-worker-hermes-wakeup.path"
    state = "/var/lib/tentaflake-worker-hermes-wakeup"
    inbox = "/var/lib/worker-wakeup/workspace/.tentaflake-worker/inbox"
    marker = state + "/.drain-request"
    endpoint = "http://localhost/v1/agents/hermes-wakeup/"

    def api(method, resource, body=None, status=200):
        command = "curl --silent --show-error --unix-socket /run/tentaflake-api/host.sock"
        command += " -X " + method + " -w '\\n%{http_code}'"
        if body is not None:
            command += " -H 'Content-Type: application/json' --data " + shlex.quote(json.dumps(body))
        output = machine.succeed(command + " " + endpoint + resource)
        payload, actual = output.rsplit("\n", 1)
        assert int(actual) == status, output
        return json.loads(payload)

    def wait_api():
        machine.wait_for_unit("tentaflake-api.service")
        # Type=simple becoming active precedes Python binding the socket.
        machine.wait_until_succeeds(
            "curl --fail --silent --unix-socket /run/tentaflake-api/host.sock http://localhost/v1/info"
        )

    def inactive():
        machine.wait_until_succeeds(
            "test $(systemctl show -p ActiveState --value " + unit + ") = inactive"
        )

    def count():
        return int(machine.succeed("cat " + state + "/test-drains"))

    def capture_and_hold(job):
        inactive()
        before = count()
        machine.succeed("touch " + state + "/test-hold")
        request = {
            "version": 1, "id": job, "action_class": "communicative",
            "argv": ["sh", "-c", "exit 99"], "timeout_seconds": 5,
        }
        # Publish atomically from outside the watched inbox.
        machine.succeed(
            "printf %s " + shlex.quote(json.dumps(request)) + " > " + state + "/test-request; "
            "mv " + state + "/test-request " + inbox + "/" + job + ".json"
        )
        machine.wait_until_succeeds("test -e " + state + "/test-held")
        assert count() == before + 1
        assert machine.succeed("systemctl show -p SubState --value " + unit).strip() == "start-post"
        machine.succeed("test -z \"$(ls -A " + inbox + ")\"")
        rows = api("GET", "tool-requests")["requests"]
        row = next(row for row in rows if row["id"] == job)
        assert row["approval_status"] == "pending", row
        invocation = machine.succeed("systemctl show -p InvocationID --value " + unit)
        body = {
            "job_id": job, "request_digest": row["request_digest"],
            "decision": "deny", "request_id": "deny-" + job,
        }
        return before, invocation, body

    def decide_while_held(invocation, body):
        receipt = api("POST", "tool-decisions", body)
        assert receipt["state"] == "recorded", receipt
        assert receipt["drain_state"] == "queued", receipt
        # systemctl start succeeded but coalesced into the *old* activating job.
        assert machine.succeed("systemctl show -p InvocationID --value " + unit) == invocation
        assert machine.succeed("systemctl show -p SubState --value " + unit).strip() == "start-post"
        machine.succeed("test -f " + marker)
        machine.fail("test -e " + state + "/results/" + body["job_id"])
        assert api("POST", "tool-decisions", body) == receipt
        api("POST", "tool-decisions", dict(body, decision="approve"), status=409)
        return receipt

    def terminal(job):
        result = state + "/results/" + job + "/result.json"
        machine.wait_until_succeeds("test -f " + result)
        assert json.loads(machine.succeed("cat " + result))["status"] == "denied"
        inactive()
        machine.succeed("test ! -e " + marker + " && test ! -e " + state + "/pending/" + job + ".json")

    wait_api()
    machine.wait_for_unit(watcher)
    machine.wait_until_succeeds("test -f " + state + "/test-drains")
    inactive()

    with subtest("coalesced API start cannot lose the decision wakeup"):
        before, invocation, body = capture_and_hold("race")
        receipt = decide_while_held(invocation, body)
        machine.succeed("rm " + state + "/test-hold")
        # Without the marker/PathExists fix this waits forever: inbox is empty,
        # the API start joined the old drain, and pending is not watched.
        # No manual service start, retry dispatch or new inbox event follows.
        terminal("race")
        assert count() == before + 2
        completed = count()
        assert api("POST", "tool-decisions", body) == receipt
        machine.succeed("test ! -e " + marker)
        assert count() == completed

    with subtest("durable marker and API receipt survive service restart and reboot"):
        before, invocation, body = capture_and_hold("recovery")
        machine.succeed("systemctl stop " + watcher)
        receipt = decide_while_held(invocation, body)
        machine.succeed("rm " + state + "/test-hold")
        inactive()
        assert count() == before + 1
        machine.succeed("test -f " + marker)
        machine.succeed("systemctl restart tentaflake-api.service")
        wait_api()
        assert api("POST", "tool-decisions", body) == receipt
        assert count() == before + 1
        machine.succeed("test -f " + marker)
        boot_id = machine.succeed("cat /proc/sys/kernel/random/boot_id").strip()
        machine.execute("systemctl reboot >&2 &", check_return=False)
        machine.connected = False
        wait_api()
        machine.wait_for_unit(watcher)
        assert machine.succeed("cat /proc/sys/kernel/random/boot_id").strip() != boot_id
        terminal("recovery")
        assert api("POST", "tool-decisions", body) == receipt
        machine.succeed("test ! -e " + marker)

    with subtest("stale marker is consumed without a restart loop"):
        before = count()
        machine.succeed("touch " + marker)
        machine.wait_until_succeeds("test $(cat " + state + "/test-drains) -gt " + str(before))
        inactive()
        machine.succeed("test ! -e " + marker)
        assert count() == before + 1
        machine.succeed("sleep 2")
        assert count() == before + 1

    machine.succeed("test $(systemctl show -p ActiveState --value podman-hermes-wakeup.service) = inactive")
  '';
}
