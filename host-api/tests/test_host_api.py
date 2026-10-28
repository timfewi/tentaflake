"""Real Unix HTTP contract, fixed command authority and durable action receipts."""

import http.client
import importlib.util
import json
import socket
import sys
import tempfile
import threading
import time
import unittest
from pathlib import Path
from unittest.mock import patch

SOURCE = Path(__file__).resolve().parents[1] / "tentaflake_host_api.py"
SPEC = importlib.util.spec_from_file_location("host_api", SOURCE)
api = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(api)


class UnixConnection(http.client.HTTPConnection):
    def __init__(self, path):
        super().__init__("localhost", timeout=3)
        self.path = path

    def connect(self):
        self.sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        self.sock.settimeout(self.timeout)
        self.sock.connect(self.path)


class HostAPI(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="tentaflake-api-test-")
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.calls = self.root / "calls.jsonl"
        systemctl = self.root / "systemctl"
        systemctl.write_text(
            f"#!{sys.executable}\n"
            + """
import json
from pathlib import Path
import sys
root = Path(__file__).parent
args = sys.argv[1:]
with (root / 'calls.jsonl').open('a') as output:
    output.write(json.dumps(args) + '\\n')
if args[0] == 'show':
    for unit in args[args.index('--')+1:]:
        print(f'Id={unit}\\nActiveState=inactive\\nLoadState=loaded\\n')
elif args[0] in {'start', 'stop', 'restart'}:
    if (root / 'fail').exists():
        print('synthetic private diagnostic', file=sys.stderr)
        sys.exit(1)
else:
    sys.exit(2)
"""
        )
        systemctl.chmod(0o700)
        journalctl = self.root / "journalctl"
        journalctl.write_text(
            f"#!{sys.executable}\n"
            + """
import json
import sys
for number in range(int(sys.argv[sys.argv.index('-n')+1])):
    print(json.dumps({'MESSAGE':f'fixture {number}', '__REALTIME_TIMESTAMP':'123456789'}))
"""
        )
        journalctl.chmod(0o700)
        cli = self.root / "tentaflake"
        cli.write_text(
            f"#!{sys.executable}\n"
            + """
import json
import sys
from pathlib import Path
failed = (Path(__file__).parent / 'unhealthy').exists()
print(json.dumps({'host':'fixture','problems':int(failed),'disk_percent':10,'failed_systemd_units':failed,'failed_agents':[],'unknown_agents':[]}))
sys.exit(int(failed))
"""
        )
        cli.chmod(0o700)
        inventory = self.root / "agents.tsv"
        inventory.write_text(
            "hermes\tcoding\thermes-coding\tdocker-hermes-coding.service\t/var/lib/hermes-coding\ttrue\n"
            "zeroclaw\tcoding\tzeroclaw-coding\tdocker-zeroclaw-coding.service\t/var/lib/zeroclaw-coding\ttrue\n"
            "pi\twaiting\tpi-waiting\ttentaflake-refusal-pi-waiting.service\t/var/lib/unused\tfalse\n"
        )
        self.config = {
            "systemctl": str(systemctl),
            "journalctl": str(journalctl),
            "cli": str(cli),
            "cli_config": str(self.root / "cli.conf"),
            "inventory": str(inventory),
            "state": str(self.root / "state"),
            "host": "fixture",
            "backend": "docker",
            "security_profile": "balanced",
            "allow_actions": False,
            "allow_logs": False,
            "stop_units": {
                "hermes-coding": ["tentaflake-broker-llm-hermes-coding.service"]
            },
        }
        policy = api.unknown_policy()
        policy.update(managed=True, auto_start=False)
        policy["llm"].update(
            enabled=True,
            allowed_models=[
                {
                    "name": "model-a",
                    "input_microusd_per_million": 2,
                    "output_microusd_per_million": 3,
                }
            ],
            max_completion_tokens=4096,
            budgets={
                "max_concurrency": 4,
                "max_requests_per_minute": 30,
                "daily_request_budget": 1000,
                "daily_token_budget": 1000000,
                "daily_cost_microusd": 10000000,
            },
        )
        policy["fetch"].update(enabled=True, allowed_hosts=["example.org"])
        policy["worker"].update(
            enabled=True,
            limits={
                "max_timeout_seconds": 900,
                "max_queue_entries": 256,
                "max_queue_bytes": 8 * 1024**2,
                "max_snapshot_bytes": 1024**3,
                "max_log_bytes": 1024**2,
            },
        )
        self.config["policies"] = {"hermes-coding": policy}
        self.config["worker_configs"] = {
            "hermes-coding": str(self.root / "worker.json")
        }
        worker = self.root / "worker"
        worker.write_text(
            f"#!{sys.executable}\n"
            + """
import json
from pathlib import Path
import sys
root = Path(__file__).parent
args = sys.argv[1:]
with (root / 'worker-calls.jsonl').open('a') as output:
    output.write(json.dumps(args) + '\\n')
if (root / 'worker-uncertain').exists():
    (root / 'committed-decision').touch()
    print('tentaflake-worker: uncertain: decision publication or output failed', file=sys.stderr)
    sys.exit(1)
if (root / 'worker-fail').exists():
    print('tentaflake-worker: conflict: private request /secret/path', file=sys.stderr)
    sys.exit(1)
if args[2] == 'requests':
    print((root / 'requests.json').read_text())
elif args[2] == 'decide':
    response = {'schema_version':1,'agent':'hermes-coding','job_id':args[3],
        'request_digest':args[6], 'decision':args[4], 'state':'recorded', 'drain_required':True}
    if (root / 'worker-invalid').exists():
        response['agent'] = 'another-agent'
    print(json.dumps(response))
else:
    sys.exit(2)
"""
        )
        worker.chmod(0o700)
        self.config["worker"] = str(worker)
        self.worker_request = {
            "id": "job-1",
            "request_digest": "a" * 64,
            "action_class": "external-reversible",
            "argv": ["printf", "${untrusted}\ntext"],
            "cwd": "/workspace",
            "timeout_seconds": 30,
            "state": "pending",
            "approval_status": "pending",
            "can_approve": True,
            "can_deny": True,
        }
        (self.root / "requests.json").write_text(
            json.dumps(
                {
                    "schema_version": 1,
                    "agent": "hermes-coding",
                    "initialized": True,
                    "requests": [self.worker_request],
                    "truncated": False,
                }
            )
        )
        self.service = None
        self.server = None
        self.thread = None
        self.addCleanup(self.close)

    def start(self, **overrides):
        self.config.update(overrides)
        self.service = api.Service(self.config)
        self.socket = str(self.root / "host.sock")
        self.server = api.Server(self.socket, self.service)
        self.thread = threading.Thread(
            target=self.server.serve_forever, kwargs={"poll_interval": 0.01}
        )
        self.thread.start()

    def close(self):
        if self.server:
            self.server.shutdown()
            self.server.server_close()
            self.thread.join()
            self.server = None
        if self.service:
            self.service.close()
            self.service = None

    def request(self, method, path, body=None):
        connection = UnixConnection(self.socket)
        try:
            connection.request(
                method,
                path,
                json.dumps(body) if body is not None else None,
                headers={"Content-Type": "application/json"},
            )
            response = connection.getresponse()
            return response.status, json.loads(response.read())
        finally:
            connection.close()

    def mutations(self):
        if not self.calls.exists():
            return []
        return [
            row
            for line in self.calls.read_text().splitlines()
            if (row := json.loads(line))[0] in {"start", "stop", "restart"}
        ]

    def test_read_only_default_and_no_chat_claim(self):
        self.start()
        status, info = self.request("GET", "/v1/info")
        self.assertEqual(status, 200)
        self.assertEqual(
            info["capabilities"],
            {
                "actions": False,
                "logs": False,
                "chat": False,
                "details": True,
                "config_proposals": True,
                "tool_requests": False,
                "tool_decisions": False,
            },
        )
        status, result = self.request("GET", "/v1/agents")
        self.assertEqual(status, 200)
        self.assertEqual(
            [row["id"] for row in result["agents"]],
            ["hermes-coding", "zeroclaw-coding", "pi-waiting"],
        )
        self.assertTrue(all(row["state"] == "inactive" for row in result["agents"]))
        self.assertTrue(
            all(row["capabilities"]["actions"] == [] for row in result["agents"])
        )
        self.assertNotIn("unit", json.dumps(result))
        self.assertNotIn("/var/lib", json.dumps(result))
        self.assertEqual(
            self.request(
                "POST",
                "/v1/agents/hermes-coding/actions",
                {"action": "start", "request_id": "r1"},
            )[0],
            403,
        )
        self.assertEqual(self.request("GET", "/v1/agents/hermes-coding/logs")[0], 403)
        self.assertEqual(self.mutations(), [])
        self.assertEqual(Path(self.socket).stat().st_mode & 0o777, 0o660)
        self.assertEqual(Path(self.config["state"]).stat().st_mode & 0o777, 0o700)

    def test_details_separate_declaration_from_observation_and_exclude_host_paths(self):
        self.start()
        status, details = self.request("GET", "/v1/agents/hermes-coding/details")
        self.assertEqual(status, 200)
        self.assertEqual(len(details["policy_revision"]), 64)
        self.assertEqual(details["policy"]["research"]["readiness"], "unverified")
        self.assertEqual(details["policy"]["management"]["remote_grants"], "unverified")
        self.assertEqual(len(details["editable_fields"]), 7)
        self.assertFalse(details["capabilities"]["tool_decisions"])
        self.assertNotIn(str(self.root), json.dumps(details))
        self.assertNotIn("credential", json.dumps(details))
        _, unmanaged = self.request("GET", "/v1/agents/zeroclaw-coding/details")
        self.assertFalse(unmanaged["policy"]["managed"])
        self.assertEqual(unmanaged["editable_fields"], [])
        self.assertNotEqual(unmanaged["policy_revision"], details["policy_revision"])

    def test_proposals_are_revision_bound_downloads_without_mutation(self):
        self.start()
        _, details = self.request("GET", "/v1/agents/hermes-coding/details")
        changes = {
            "llm.allowed_models": [
                {
                    "name": "provider/model-a",
                    "input_microusd_per_million": 7,
                    "output_microusd_per_million": 11,
                }
            ],
            "fetch.allowed_hosts": ["example.org"],
            "worker.max_timeout_seconds": 120,
        }
        status, result = self.request(
            "POST",
            "/v1/agents/hermes-coding/config-proposals",
            {
                "policy_revision": details["policy_revision"],
                "changes": changes,
            },
        )
        self.assertEqual(status, 200)
        self.assertEqual(result["changes"], changes)
        self.assertEqual(result["validation"], "structure-only")
        self.assertEqual(result["activation"], "explicit-host-update-required")
        self.assertIn(
            'agents."hermes-coding".llm.allowedModels = lib.mkForce', result["nix"]
        )
        self.assertIn("maxTimeoutSeconds = lib.mkForce 120;", result["nix"])
        self.assertEqual(self.mutations(), [])
        self.assertFalse((self.root / "worker-calls.jsonl").exists())
        self.assertFalse((self.root / result["filename"]).exists())
        self.assertEqual(
            self.request(
                "POST",
                "/v1/agents/hermes-coding/config-proposals",
                {
                    "policy_revision": "0" * 64,
                    "changes": changes,
                },
            )[0],
            409,
        )

    def test_proposals_reject_policy_bypass_and_literal_injection(self):
        self.start()
        _, details = self.request("GET", "/v1/agents/hermes-coding/details")
        for changes in (
            {"worker.enable": False},
            {"llm.providerCredentialFile": "/secret"},
            {"worker.max_timeout_seconds": True},
            {"worker.max_timeout_seconds": 86401},
            {"worker.max_snapshot_bytes": 16 * 1024**3 + 1},
            {"fetch.allowed_hosts": ["*.example.org"]},
            {"fetch.allowed_hosts": ["a.example\nmalicious"]},
            {"fetch.allowed_hosts": ["${builtins.readFile /secret}"]},
            {
                "llm.allowed_models": [
                    {
                        "name": "${builtins.readFile /secret}",
                        "input_microusd_per_million": 1,
                        "output_microusd_per_million": 1,
                    }
                ]
            },
            {
                "llm.allowed_models": [
                    {
                        "name": "model",
                        "input_microusd_per_million": 0,
                        "output_microusd_per_million": 1,
                    }
                ]
            },
            {"llm.allowed_models": [{"name": "model"}]},
        ):
            with self.subTest(changes=changes):
                self.assertEqual(
                    self.request(
                        "POST",
                        "/v1/agents/hermes-coding/config-proposals",
                        {
                            "policy_revision": details["policy_revision"],
                            "changes": changes,
                        },
                    )[0],
                    400,
                )
        self.assertEqual(api.nix_string('a"\\${x}'), '"a\\"\\\\\\${x}"')
        _, unmanaged = self.request("GET", "/v1/agents/zeroclaw-coding/details")
        self.assertEqual(
            self.request(
                "POST",
                "/v1/agents/zeroclaw-coding/config-proposals",
                {
                    "policy_revision": unmanaged["policy_revision"],
                    "changes": {"worker.max_queue_entries": 2},
                },
            )[0],
            400,
        )

    def test_worker_requests_require_separate_permission_and_preserve_full_review_text(
        self,
    ):
        self.start()
        self.assertEqual(
            self.request("GET", "/v1/agents/hermes-coding/tool-requests")[0], 403
        )
        self.close()
        self.start(allow_tool_requests=True)
        status, result = self.request("GET", "/v1/agents/hermes-coding/tool-requests")
        self.assertEqual(status, 200)
        self.assertEqual(result["requests"], [self.worker_request])
        self.assertFalse(result["truncated"])
        self.assertEqual(
            self.request("GET", "/v1/agents/zeroclaw-coding/tool-requests")[0], 403
        )
        bad = dict(self.worker_request, argv=["x" * 4097])
        (self.root / "requests.json").write_text(
            json.dumps(
                {
                    "schema_version": 1,
                    "agent": "hermes-coding",
                    "requests": [bad],
                    "truncated": False,
                }
            )
        )
        self.assertEqual(
            self.request("GET", "/v1/agents/hermes-coding/tool-requests")[0], 502
        )
        self.assertEqual(self.mutations(), [])

    def decision_body(self, **overrides):
        return {
            "job_id": "job-1",
            "request_digest": "a" * 64,
            "decision": "approve",
            "request_id": "approval-1",
        } | overrides

    def test_worker_decisions_are_durable_intent_bound_and_schedule_only_fixed_worker(
        self,
    ):
        self.start(allow_tool_requests=True, allow_tool_decisions=True)
        path = "/v1/agents/hermes-coding/tool-decisions"
        status, receipt = self.request("POST", path, self.decision_body())
        self.assertEqual(status, 200)
        self.assertEqual(receipt["state"], "recorded")
        self.assertEqual(receipt["drain_state"], "queued")
        self.assertEqual(
            self.mutations(),
            [
                [
                    "start",
                    "--no-block",
                    "--no-ask-password",
                    "--",
                    "tentaflake-worker-hermes-coding.service",
                ]
            ],
        )
        self.close()
        self.start()
        self.assertEqual(
            self.request("POST", path, self.decision_body()), (200, receipt)
        )
        self.assertEqual(
            len((self.root / "worker-calls.jsonl").read_text().splitlines()), 1
        )
        self.assertEqual(len(self.mutations()), 1)
        self.assertEqual(
            self.request("POST", path, self.decision_body(decision="deny"))[0], 409
        )
        self.assertEqual(
            self.request("POST", path, self.decision_body(job_id="../job"))[0], 400
        )

    def test_worker_scheduling_failure_preserves_recorded_authorization(self):
        self.start(allow_tool_requests=True, allow_tool_decisions=True)
        (self.root / "fail").touch()
        path = "/v1/agents/hermes-coding/tool-decisions"
        _, receipt = self.request("POST", path, self.decision_body(decision="deny"))
        self.assertEqual(receipt["state"], "recorded")
        self.assertEqual(receipt["drain_state"], "unknown")
        (self.root / "fail").unlink()
        self.assertEqual(
            self.request("POST", path, self.decision_body(decision="deny")),
            (200, receipt),
        )
        self.assertEqual(len(self.mutations()), 1)

    def test_uncertain_worker_dispatch_never_replays_and_known_conflicts_are_safe(self):
        self.start(allow_tool_requests=True, allow_tool_decisions=True)
        (self.root / "worker-invalid").touch()
        path = "/v1/agents/hermes-coding/tool-decisions"
        _, receipt = self.request("POST", path, self.decision_body())
        self.assertEqual(receipt["state"], "unknown")
        self.assertEqual(receipt["drain_state"], "not-requested")
        (self.root / "worker-invalid").unlink()
        self.close()
        self.start()
        self.assertEqual(
            self.request("POST", path, self.decision_body()), (200, receipt)
        )
        self.assertEqual(
            len((self.root / "worker-calls.jsonl").read_text().splitlines()), 1
        )
        (self.root / "worker-fail").touch()
        status, failure = self.request(
            "POST", path, self.decision_body(request_id="approval-2")
        )
        self.assertEqual(status, 409)
        self.assertNotIn("/secret", str(failure))
        (self.root / "worker-fail").unlink()
        self.assertEqual(
            self.request("POST", path, self.decision_body(request_id="approval-2")),
            (status, failure),
        )
        self.assertEqual(self.mutations(), [])

    def test_worker_decisions_need_inspection_and_declared_worker(self):
        with self.assertRaises(ValueError):
            api.Service(self.config | {"allow_tool_decisions": True})
        self.start(allow_tool_requests=True)
        self.assertEqual(
            self.request(
                "POST", "/v1/agents/hermes-coding/tool-decisions", self.decision_body()
            )[0],
            403,
        )

    def test_post_publication_worker_error_stays_unknown_without_replay(self):
        self.start(allow_tool_requests=True, allow_tool_decisions=True)
        (self.root / "worker-uncertain").touch()
        path = "/v1/agents/hermes-coding/tool-decisions"
        status, receipt = self.request("POST", path, self.decision_body())
        self.assertEqual(status, 200)
        self.assertEqual(receipt["state"], "unknown")
        self.assertTrue((self.root / "committed-decision").exists())
        (self.root / "worker-uncertain").unlink()
        self.close()
        self.start()
        self.assertEqual(
            self.request("POST", path, self.decision_body()), (status, receipt)
        )
        self.assertEqual(
            len((self.root / "worker-calls.jsonl").read_text().splitlines()), 1
        )
        self.assertEqual(self.mutations(), [])

    def test_health_preserves_unhealthy_exit_one_as_evidence(self):
        self.start()
        self.assertTrue(self.request("GET", "/v1/health")[1]["healthy"])
        (self.root / "unhealthy").touch()
        status, result = self.request("GET", "/v1/health")
        self.assertEqual(status, 200)
        self.assertFalse(result["healthy"])
        self.assertEqual(result["report"]["problems"], 1)

    def test_only_exact_agent_id_and_fixed_action_reach_systemd(self):
        self.start(allow_actions=True)
        for aid in (
            "coding",
            "docker-hermes-coding.service",
            "missing",
            "--all",
            "..%2Fhermes-coding",
        ):
            with self.subTest(aid=aid):
                self.assertEqual(
                    self.request(
                        "POST",
                        f"/v1/agents/{aid}/actions",
                        {"action": "restart", "request_id": "r1"},
                    )[0],
                    404,
                )
        for body in (
            {"action": "exec", "request_id": "r1"},
            {"action": "stop", "request_id": "r1", "unit": "ssh.service"},
            {"action": "stop", "request_id": "../escape"},
            {"action": ["stop"], "request_id": "r1"},
        ):
            self.assertEqual(
                self.request("POST", "/v1/agents/hermes-coding/actions", body)[0], 400
            )
        self.assertEqual(self.mutations(), [])
        status, result = self.request(
            "POST",
            "/v1/agents/zeroclaw-coding/actions",
            {"action": "restart", "request_id": "r1"},
        )
        self.assertEqual(status, 200)
        self.assertEqual(result["state"], "succeeded")
        self.assertEqual(
            self.mutations(),
            [["restart", "--no-ask-password", "--", "docker-zeroclaw-coding.service"]],
        )

    def test_stop_includes_only_configured_brokers(self):
        self.start(allow_actions=True)
        self.request(
            "POST",
            "/v1/agents/hermes-coding/actions",
            {"action": "stop", "request_id": "r1"},
        )
        self.assertEqual(
            self.mutations(),
            [
                [
                    "stop",
                    "--no-ask-password",
                    "--",
                    "docker-hermes-coding.service",
                    "tentaflake-broker-llm-hermes-coding.service",
                ]
            ],
        )

    def test_stopped_scaffold_cannot_be_started(self):
        self.start(allow_actions=True)
        for action in ("start", "restart"):
            self.assertEqual(
                self.request(
                    "POST",
                    "/v1/agents/pi-waiting/actions",
                    {"action": action, "request_id": action},
                )[0],
                409,
            )
        self.assertEqual(self.mutations(), [])

    def test_receipts_survive_restart_and_conflicting_intent_is_rejected(self):
        self.start(allow_actions=True)
        request = {"action": "restart", "request_id": "durable"}
        first = self.request("POST", "/v1/agents/hermes-coding/actions", request)
        self.assertEqual(
            self.request("POST", "/v1/agents/hermes-coding/actions", request), first
        )
        self.close()
        self.start()
        self.assertEqual(
            self.request("POST", "/v1/agents/hermes-coding/actions", request), first
        )
        self.assertEqual(len(self.mutations()), 1)
        self.assertEqual(
            self.request(
                "POST",
                "/v1/agents/hermes-coding/actions",
                {"action": "stop", "request_id": "durable"},
            )[0],
            409,
        )
        self.assertEqual(
            self.request("POST", "/v1/agents/zeroclaw-coding/actions", request)[0], 409
        )

    def test_unknown_receipt_is_never_replayed_after_failure_or_restart(self):
        self.start(allow_actions=True)
        original = self.service.run

        def fail_action(argv, **kwargs):
            if argv[1] == "restart":
                raise api.APIError("Host operation timed out", 504)
            return original(argv, **kwargs)

        with patch.object(self.service, "run", side_effect=fail_action):
            status, result = self.request(
                "POST",
                "/v1/agents/hermes-coding/actions",
                {"action": "restart", "request_id": "uncertain"},
            )
        self.assertEqual(status, 200)
        self.assertEqual(result["state"], "unknown")
        self.close()
        self.start()
        self.assertEqual(
            self.request(
                "POST",
                "/v1/agents/hermes-coding/actions",
                {"action": "restart", "request_id": "uncertain"},
            )[1],
            result,
        )
        self.assertEqual(self.mutations(), [])

    def test_crash_after_dispatch_retains_unknown_receipt(self):
        self.start(allow_actions=True)
        original = self.service.run

        def crash_after_dispatch(argv, **kwargs):
            result = original(argv, **kwargs)
            if argv[1] == "restart":
                raise SystemExit("synthetic interruption after systemd accepted")
            return result

        request = {"action": "restart", "request_id": "interrupted"}
        with (
            patch.object(self.service, "run", side_effect=crash_after_dispatch),
            self.assertRaises(SystemExit),
        ):
            self.service.action(self.service.agents["hermes-coding"], request)
        self.assertEqual(len(self.mutations()), 1)
        self.close()
        self.start()
        status, response = self.request(
            "POST", "/v1/agents/hermes-coding/actions", request
        )
        self.assertEqual(status, 200)
        self.assertEqual(response["state"], "unknown")
        self.assertEqual(response["observed_state"], "unknown")
        self.assertEqual(len(self.mutations()), 1)

    def test_concurrent_actions_do_not_queue_late_dispatch(self):
        self.start(allow_actions=True)
        entered, release = threading.Event(), threading.Event()
        original = self.service.run
        responses = []

        def hold_action(argv, **kwargs):
            if argv[1] == "restart":
                entered.set()
                if not release.wait(3):
                    raise RuntimeError("fixture did not release action")
            return original(argv, **kwargs)

        request = {"action": "restart", "request_id": "first"}
        with patch.object(self.service, "run", side_effect=hold_action):
            thread = threading.Thread(
                target=lambda: responses.append(
                    self.request("POST", "/v1/agents/hermes-coding/actions", request)
                )
            )
            thread.start()
            try:
                self.assertTrue(entered.wait(1))
                same_status, same = self.request(
                    "POST", "/v1/agents/hermes-coding/actions", request
                )
                self.assertEqual(same_status, 200)
                self.assertEqual(same["state"], "unknown")
                self.assertEqual(
                    self.request(
                        "POST",
                        "/v1/agents/hermes-coding/actions",
                        {"action": "stop", "request_id": "second"},
                    )[0],
                    409,
                )
                self.assertEqual(self.request("GET", "/v1/info")[0], 200)
            finally:
                release.set()
                thread.join()
        self.assertEqual(responses[0][1]["state"], "succeeded")
        self.assertEqual(len(self.mutations()), 1)

    def test_operator_must_reenable_actions_even_for_old_receipts(self):
        self.start(allow_actions=True)
        request = {"action": "restart", "request_id": "previous-permission"}
        self.assertEqual(
            self.request("POST", "/v1/agents/hermes-coding/actions", request)[0], 200
        )
        self.close()
        self.start(allow_actions=False)
        self.assertEqual(
            self.request("POST", "/v1/agents/hermes-coding/actions", request)[0], 403
        )
        self.assertEqual(len(self.mutations()), 1)

    def test_failed_commands_do_not_expose_stderr(self):
        self.start(allow_actions=True)
        (self.root / "fail").touch()
        status, result = self.request(
            "POST",
            "/v1/agents/hermes-coding/actions",
            {"action": "restart", "request_id": "failed"},
        )
        self.assertEqual(status, 200)
        self.assertEqual(result["state"], "failed")
        self.assertNotIn("private", json.dumps(result))

    def test_receipt_capacity_fails_closed(self):
        self.start(allow_actions=True)
        with patch.object(api, "MAX_RECEIPTS", 0):
            self.assertEqual(
                self.request(
                    "POST",
                    "/v1/agents/hermes-coding/actions",
                    {"action": "start", "request_id": "r1"},
                )[0],
                503,
            )
        self.assertEqual(self.mutations(), [])

    def test_log_pagination_is_bounded_and_does_not_select_other_units(self):
        self.start(allow_logs=True)
        status, result = self.request("GET", "/v1/agents/hermes-coding/logs?lines=2")
        self.assertEqual(status, 200)
        self.assertEqual(len(result["entries"]), 2)
        self.assertEqual(result["entries"][0]["message"], "fixture 1")
        self.assertTrue(result["truncated"])
        for query in (
            "lines=0",
            "lines=201",
            "lines=-1",
            "lines=1&lines=2",
            "unit=ssh.service",
            "lines=x",
            "lines=",
        ):
            self.assertEqual(
                self.request("GET", "/v1/agents/hermes-coding/logs?" + query)[0], 400
            )

    def test_duplicate_json_fields_and_http_headers_are_rejected(self):
        self.start(allow_actions=True)
        for raw in (
            b"GET /v1/info HTTP/1.1\r\nHost: a\r\nHost: b\r\n\r\n",
            b"POST /v1/agents/hermes-coding/actions HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n",
        ):
            with socket.socket(socket.AF_UNIX) as connection:
                connection.connect(self.socket)
                connection.sendall(raw)
                self.assertIn(b"400", connection.recv(4096).split(b"\r\n")[0])
        connection = UnixConnection(self.socket)
        connection.request(
            "POST",
            "/v1/agents/hermes-coding/actions",
            '{"action":"start","action":"stop","request_id":"r"}',
            headers={"Content-Type": "application/json"},
        )
        self.assertEqual(connection.getresponse().status, 400)
        connection.close()
        self.assertEqual(self.mutations(), [])

    def test_unicode_logs_stay_within_bridge_response_limit(self):
        journal = Path(self.config["journalctl"])
        journal.write_text(
            f"#!{sys.executable}\n"
            + "import json\nfor _ in range(32):\n    print(json.dumps({'MESSAGE': '\\U0001f4ac'*3000, '__REALTIME_TIMESTAMP':'123456789'}, ensure_ascii=False))\n"
        )
        self.start(allow_logs=True)
        connection = UnixConnection(self.socket)
        try:
            connection.request("GET", "/v1/agents/hermes-coding/logs?lines=32")
            response = connection.getresponse()
            payload = response.read()
            self.assertEqual(response.status, 200)
            self.assertLess(len(payload), 1024 * 1024)
            self.assertEqual(len(json.loads(payload)["entries"]), 32)
        finally:
            connection.close()

    def test_second_writer_and_unsafe_inventory_refused(self):
        self.start()
        with self.assertRaises(BlockingIOError):
            api.Service(self.config)
        self.close()
        Path(self.config["inventory"]).write_text(
            "hermes\tcoding\thermes-coding\t--all\t/tmp\ttrue\n"
        )
        with self.assertRaisesRegex(ValueError, "identity"):
            api.Service(self.config)

    def test_untrusted_socket_path_is_not_removed(self):
        self.service = api.Service(self.config)
        path = self.root / "preserve"
        path.write_text("operator file")
        with self.assertRaises(ValueError):
            api.Server(str(path), self.service)
        self.assertEqual(path.read_text(), "operator file")

    def test_process_deadline_and_output_ceiling(self):
        environment = {"PATH": "/usr/bin:/bin"}
        started = time.monotonic()
        with self.assertRaises(api.APIError) as failure:
            api.command(
                [sys.executable, "-c", "import time;time.sleep(5)"],
                environment=environment,
                timeout=0.1,
            )
        self.assertEqual(failure.exception.status, 504)
        self.assertLess(time.monotonic() - started, 1)
        with self.assertRaises(api.APIError) as failure:
            api.command(
                [sys.executable, "-c", "print('x'*10000)"],
                environment=environment,
                maximum=100,
            )
        self.assertEqual(failure.exception.status, 502)

    def test_inherited_pipe_writer_cannot_extend_deadline(self):
        started = time.monotonic()
        with self.assertRaises(api.APIError) as failure:
            api.command(
                [
                    sys.executable,
                    "-c",
                    "import os,time;pid=os.fork();time.sleep(5) if pid==0 else None",
                ],
                environment={},
                timeout=0.1,
            )
        self.assertEqual(failure.exception.status, 504)
        self.assertLess(time.monotonic() - started, 1)


if __name__ == "__main__":
    unittest.main()
