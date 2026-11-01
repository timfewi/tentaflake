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
from unittest.mock import Mock, patch
from types import SimpleNamespace

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
        # AF_UNIX paths must remain short under the launcher's long TMPDIR.
        self.temp = tempfile.TemporaryDirectory(prefix="tfa-", dir="/tmp")
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
            "fixture\tcoding\tagent-coding\tdocker-agent-coding.service\t/var/lib/tentaflake/agents/coding\ttrue\n"
            "fixture\tassistant\tagent-assistant\tdocker-agent-assistant.service\t/var/lib/tentaflake/agents/assistant\ttrue\n"
            "pi\twaiting\tagent-waiting\tdocker-agent-waiting.service\t/var/lib/unused\tfalse\n"
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
                "agent-coding": ["tentaflake-broker-llm-agent-coding.service"]
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
        self.config["policies"] = {"agent-coding": policy}
        self.config["worker_configs"] = {
            "agent-coding": str(self.root / "worker.json")
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
    response = {'schema_version':1,'agent':'agent-coding','job_id':args[3],
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
                    "agent": "agent-coding",
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
            ["agent-coding", "agent-assistant", "agent-waiting"],
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
                "/v1/agents/agent-coding/actions",
                {"action": "start", "request_id": "r1"},
            )[0],
            403,
        )
        self.assertEqual(self.request("GET", "/v1/agents/agent-coding/logs")[0], 403)
        self.assertEqual(self.mutations(), [])
        self.assertEqual(Path(self.socket).stat().st_mode & 0o777, 0o660)
        self.assertEqual(Path(self.config["state"]).stat().st_mode & 0o777, 0o700)

    def test_details_separate_declaration_from_observation_and_exclude_host_paths(self):
        self.start()
        status, details = self.request("GET", "/v1/agents/agent-coding/details")
        self.assertEqual(status, 200)
        self.assertEqual(len(details["policy_revision"]), 64)
        self.assertEqual(details["policy"]["research"]["readiness"], "unverified")
        self.assertEqual(details["policy"]["management"]["remote_grants"], "unverified")
        self.assertEqual(len(details["editable_fields"]), 7)
        self.assertFalse(details["capabilities"]["tool_decisions"])
        self.assertNotIn(str(self.root), json.dumps(details))
        self.assertNotIn("credential", json.dumps(details))
        _, unmanaged = self.request("GET", "/v1/agents/agent-assistant/details")
        self.assertFalse(unmanaged["policy"]["managed"])
        self.assertEqual(unmanaged["editable_fields"], [])
        self.assertNotEqual(unmanaged["policy_revision"], details["policy_revision"])

    def test_proposals_are_revision_bound_downloads_without_mutation(self):
        self.start()
        _, details = self.request("GET", "/v1/agents/agent-coding/details")
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
            "/v1/agents/agent-coding/config-proposals",
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
            'agents."agent-coding".llm.allowedModels = lib.mkForce', result["nix"]
        )
        self.assertIn("maxTimeoutSeconds = lib.mkForce 120;", result["nix"])
        self.assertEqual(self.mutations(), [])
        self.assertFalse((self.root / "worker-calls.jsonl").exists())
        self.assertFalse((self.root / result["filename"]).exists())
        self.assertEqual(
            self.request(
                "POST",
                "/v1/agents/agent-coding/config-proposals",
                {
                    "policy_revision": "0" * 64,
                    "changes": changes,
                },
            )[0],
            409,
        )

    def test_proposals_reject_policy_bypass_and_literal_injection(self):
        self.start()
        _, details = self.request("GET", "/v1/agents/agent-coding/details")
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
                        "/v1/agents/agent-coding/config-proposals",
                        {
                            "policy_revision": details["policy_revision"],
                            "changes": changes,
                        },
                    )[0],
                    400,
                )
        self.assertEqual(api.nix_string('a"\\${x}'), '"a\\"\\\\\\${x}"')
        _, unmanaged = self.request("GET", "/v1/agents/agent-assistant/details")
        self.assertEqual(
            self.request(
                "POST",
                "/v1/agents/agent-assistant/config-proposals",
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
            self.request("GET", "/v1/agents/agent-coding/tool-requests")[0], 403
        )
        self.close()
        self.start(allow_tool_requests=True)
        status, result = self.request("GET", "/v1/agents/agent-coding/tool-requests")
        self.assertEqual(status, 200)
        self.assertEqual(result["requests"], [self.worker_request])
        self.assertFalse(result["truncated"])
        self.assertEqual(
            self.request("GET", "/v1/agents/agent-assistant/tool-requests")[0], 403
        )
        bad = dict(self.worker_request, argv=["x" * 4097])
        (self.root / "requests.json").write_text(
            json.dumps(
                {
                    "schema_version": 1,
                    "agent": "agent-coding",
                    "requests": [bad],
                    "truncated": False,
                }
            )
        )
        self.assertEqual(
            self.request("GET", "/v1/agents/agent-coding/tool-requests")[0], 502
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
        path = "/v1/agents/agent-coding/tool-decisions"
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
                    "tentaflake-worker-agent-coding.service",
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
        path = "/v1/agents/agent-coding/tool-decisions"
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
        path = "/v1/agents/agent-coding/tool-decisions"
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
                "POST", "/v1/agents/agent-coding/tool-decisions", self.decision_body()
            )[0],
            403,
        )

    def test_post_publication_worker_error_stays_unknown_without_replay(self):
        self.start(allow_tool_requests=True, allow_tool_decisions=True)
        (self.root / "worker-uncertain").touch()
        path = "/v1/agents/agent-coding/tool-decisions"
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
            "docker-agent-coding.service",
            "missing",
            "--all",
            "..%2Fagent-coding",
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
                self.request("POST", "/v1/agents/agent-coding/actions", body)[0], 400
            )
        self.assertEqual(self.mutations(), [])
        status, result = self.request(
            "POST",
            "/v1/agents/agent-assistant/actions",
            {"action": "restart", "request_id": "r1"},
        )
        self.assertEqual(status, 200)
        self.assertEqual(result["state"], "succeeded")
        self.assertEqual(
            self.mutations(),
            [["restart", "--no-ask-password", "--", "docker-agent-assistant.service"]],
        )

    def test_stop_includes_only_configured_brokers(self):
        self.start(allow_actions=True)
        self.request(
            "POST",
            "/v1/agents/agent-coding/actions",
            {"action": "stop", "request_id": "r1"},
        )
        self.assertEqual(
            self.mutations(),
            [
                [
                    "stop",
                    "--no-ask-password",
                    "--",
                    "docker-agent-coding.service",
                    "tentaflake-broker-llm-agent-coding.service",
                ]
            ],
        )

    def test_stopped_scaffold_cannot_be_started(self):
        self.start(allow_actions=True)
        for action in ("start", "restart"):
            self.assertEqual(
                self.request(
                    "POST",
                    "/v1/agents/agent-waiting/actions",
                    {"action": action, "request_id": action},
                )[0],
                409,
            )
        self.assertEqual(self.mutations(), [])

    def test_receipts_survive_restart_and_conflicting_intent_is_rejected(self):
        self.start(allow_actions=True)
        request = {"action": "restart", "request_id": "durable"}
        first = self.request("POST", "/v1/agents/agent-coding/actions", request)
        self.assertEqual(
            self.request("POST", "/v1/agents/agent-coding/actions", request), first
        )
        self.close()
        self.start()
        self.assertEqual(
            self.request("POST", "/v1/agents/agent-coding/actions", request), first
        )
        self.assertEqual(len(self.mutations()), 1)
        self.assertEqual(
            self.request(
                "POST",
                "/v1/agents/agent-coding/actions",
                {"action": "stop", "request_id": "durable"},
            )[0],
            409,
        )
        self.assertEqual(
            self.request("POST", "/v1/agents/agent-assistant/actions", request)[0], 409
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
                "/v1/agents/agent-coding/actions",
                {"action": "restart", "request_id": "uncertain"},
            )
        self.assertEqual(status, 200)
        self.assertEqual(result["state"], "unknown")
        self.close()
        self.start()
        self.assertEqual(
            self.request(
                "POST",
                "/v1/agents/agent-coding/actions",
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
            self.service.action(self.service.agents["agent-coding"], request)
        self.assertEqual(len(self.mutations()), 1)
        self.close()
        self.start()
        status, response = self.request(
            "POST", "/v1/agents/agent-coding/actions", request
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
                    self.request("POST", "/v1/agents/agent-coding/actions", request)
                )
            )
            thread.start()
            try:
                self.assertTrue(entered.wait(1))
                same_status, same = self.request(
                    "POST", "/v1/agents/agent-coding/actions", request
                )
                self.assertEqual(same_status, 200)
                self.assertEqual(same["state"], "unknown")
                self.assertEqual(
                    self.request(
                        "POST",
                        "/v1/agents/agent-coding/actions",
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
            self.request("POST", "/v1/agents/agent-coding/actions", request)[0], 200
        )
        self.close()
        self.start(allow_actions=False)
        self.assertEqual(
            self.request("POST", "/v1/agents/agent-coding/actions", request)[0], 403
        )
        self.assertEqual(len(self.mutations()), 1)

    def test_failed_commands_do_not_expose_stderr(self):
        self.start(allow_actions=True)
        (self.root / "fail").touch()
        status, result = self.request(
            "POST",
            "/v1/agents/agent-coding/actions",
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
                    "/v1/agents/agent-coding/actions",
                    {"action": "start", "request_id": "r1"},
                )[0],
                503,
            )
        self.assertEqual(self.mutations(), [])

    def test_log_pagination_is_bounded_and_does_not_select_other_units(self):
        self.start(allow_logs=True)
        status, result = self.request("GET", "/v1/agents/agent-coding/logs?lines=2")
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
                self.request("GET", "/v1/agents/agent-coding/logs?" + query)[0], 400
            )

    def test_duplicate_json_fields_and_http_headers_are_rejected(self):
        self.start(allow_actions=True)
        for raw in (
            b"GET /v1/info HTTP/1.1\r\nHost: a\r\nHost: b\r\n\r\n",
            b"POST /v1/agents/agent-coding/actions HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n",
        ):
            with socket.socket(socket.AF_UNIX) as connection:
                connection.connect(self.socket)
                connection.sendall(raw)
                self.assertIn(b"400", connection.recv(4096).split(b"\r\n")[0])
        connection = UnixConnection(self.socket)
        connection.request(
            "POST",
            "/v1/agents/agent-coding/actions",
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
            connection.request("GET", "/v1/agents/agent-coding/logs?lines=32")
            response = connection.getresponse()
            payload = response.read()
            self.assertEqual(response.status, 200)
            self.assertLess(len(payload), 1024 * 1024)
            self.assertEqual(len(json.loads(payload)["entries"]), 32)
        finally:
            connection.close()

    def test_unsigned_linux_peer_uid_keeps_exact_grant(self):
        for uid in (2**31, 2**32 - 2):
            with self.subTest(uid=uid):
                service = api.Service({
                    **self.config,
                    "peer_grants": [{"uid": uid, "agents": ["agent-coding"], "permissions": ["read"]}],
                })
                try:
                    connection = Mock()
                    connection.getsockopt.return_value = api.struct.pack("iII", 123, uid, 2**32 - 2)
                    with patch.object(api, "receive", return_value=("GET", "/v1/info", None)), \
                         patch.object(api, "respond") as respond:
                        api.Handler(connection, None, SimpleNamespace(service=service))
                    self.assertEqual(respond.call_args.args[1], 200)
                    self.assertEqual(respond.call_args.args[2]["authorization"]["mode"], "exact-peer")
                finally:
                    service.close()

    def test_exact_peer_filters_inventory_and_denies_ungranted_operations(self):
        uid = api.os.geteuid()
        self.assertGreater(uid, 0, "Unix peer fixture requires an unprivileged test UID")
        self.start(
            allow_actions=True,
            allow_logs=True,
            peer_grants=[{"uid": uid, "agents": ["agent-coding"], "permissions": ["read"]}],
        )
        status, info = self.request("GET", "/v1/info")
        self.assertEqual(status, 200)
        self.assertEqual(info["authorization"], {"mode": "exact-peer", "resource_scoped": True})
        self.assertFalse(info["capabilities"]["actions"])
        self.assertFalse(info["capabilities"]["logs"])
        self.assertFalse(info["capabilities"]["config_proposals"])
        status, inventory = self.request("GET", "/v1/agents")
        self.assertEqual(status, 200)
        self.assertEqual([row["id"] for row in inventory["agents"]], ["agent-coding"])
        self.assertNotIn("agent-assistant", json.dumps(inventory))
        self.assertEqual(inventory["agents"][0]["capabilities"]["actions"], [])
        self.assertEqual(self.request("GET", "/v1/agents/agent-assistant/details")[0], 403)
        self.assertEqual(self.request("GET", "/v1/health")[0], 403)
        self.assertEqual(self.request("GET", "/v1/agents/agent-coding/logs")[0], 403)
        self.assertEqual(
            self.request("POST", "/v1/agents/agent-coding/actions",
                         {"action": "start", "request_id": "denied-peer"})[0],
            403,
        )
        self.assertEqual(self.mutations(), [])
        self.assertEqual(Path(self.socket).stat().st_mode & 0o777, 0o622)

    def test_exact_peer_permissions_have_no_inheritance_or_root_bypass(self):
        uid = api.os.geteuid()
        self.assertGreater(uid, 0, "Unix peer fixture requires an unprivileged test UID")
        self.start(
            allow_actions=True,
            allow_logs=True,
            peer_grants=[
                {"uid": uid, "agents": ["agent-coding"], "permissions": ["logs"]},
                {"uid": uid + 1, "agents": ["agent-assistant"], "permissions": ["actions"]},
            ],
        )
        self.assertEqual(self.request("GET", "/v1/agents")[0], 403)
        self.assertEqual(self.request("GET", "/v1/agents/agent-coding/logs")[0], 200)
        self.assertEqual(self.request("GET", "/v1/agents/agent-assistant/logs")[0], 403)
        for peer in (None, 0, uid + 2):
            with self.assertRaises(api.APIError) as denied:
                self.service.route("GET", "/v1/agents", None, peer_uid=peer)
            self.assertEqual(denied.exception.status, 403)
        with self.assertRaises(api.APIError) as denied:
            self.service.route(
                "POST", "/v1/agents/agent-coding/actions",
                {"action": "start", "request_id": "foreign"}, peer_uid=uid + 1,
            )
        self.assertEqual(denied.exception.status, 403)
        result = self.service.route(
            "POST", "/v1/agents/agent-assistant/actions",
            {"action": "start", "request_id": "owned"}, peer_uid=uid + 1,
        )
        self.assertEqual(result["state"], "succeeded")
        self.assertEqual(self.mutations(), [["start", "--no-ask-password", "--",
                                            "docker-agent-assistant.service"]])

    def test_empty_and_malformed_peer_grants_fail_closed(self):
        self.start(peer_grants=[])
        self.assertEqual(self.request("GET", "/v1/info")[0], 403)
        self.assertEqual(self.mutations(), [])
        self.close()
        uid = max(1, api.os.geteuid())
        valid = {"uid": uid, "agents": ["agent-coding"], "permissions": ["read"]}
        invalid = [
            "all",
            [valid, valid],
            [{**valid, "uid": True}],
            [{**valid, "uid": 0}],
            [{**valid, "agents": ["agent-absent"]}],
            [{**valid, "agents": ["agent-coding", "agent-coding"]}],
            [{**valid, "permissions": ["read", "root"]}],
            [{**valid, "permissions": ["read", "read"]}],
            [{**valid, "tenant": "self-asserted"}],
        ]
        for grants in invalid:
            with self.subTest(grants=grants):
                with self.assertRaisesRegex(ValueError, "peer grants"):
                    api.Service({**self.config, "peer_grants": grants})

    def start_runner_api(self, permissions=None):
        self.runner_uid = max(1, api.os.geteuid())
        inventory = Path(self.config["inventory"])
        inventory.write_text(inventory.read_text().replace("fixture\tcoding\t", "pi\tcoding\t"))
        self.start(
            runners={"agent-coding": {
                "socket": "/run/tentaflake-pi/agent-coding/control/runner.sock",
                "token": "/run/tentaflake-pi/agent-coding/api-token",
                "transport": "runsc-pi-v1",
            }},
            peer_grants=[{"uid": self.runner_uid, "agents": ["agent-coding"],
                          "permissions": permissions or ["read", "run_read", "run_submit", "run_stop"]}],
        )

    def runner_route(self, method, operation, body=None, aid="agent-coding"):
        return self.service.route(
            method, f"/v1/agents/{aid}/runner/{operation}", body,
            peer_uid=self.runner_uid,
        )

    def fake_runner(self, responses):
        """Native Linux Unix transport, not runsc VM evidence.

        Tests substitute privileged filesystem metadata and the expected outbound
        peer constant; actual SO_PEERCRED and request-byte delivery remain real.
        """
        path = self.root / f"runner-{time.monotonic_ns()}.sock"
        listener = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        listener.bind(str(path))
        listener.listen()
        listener.settimeout(1)
        observed = []
        def serve():
            try:
                for response in responses:
                    connection, _ = listener.accept()
                    with connection:
                        connection.settimeout(1)
                        raw = b""
                        while b"\r\n\r\n" not in raw:
                            chunk = connection.recv(4096)
                            if not chunk:
                                return
                            raw += chunk
                        header, data = raw.split(b"\r\n\r\n", 1)
                        fields = dict(line.split(b": ", 1) for line in header.split(b"\r\n")[1:])
                        while len(data) < int(fields[b"Content-Length"]):
                            chunk = connection.recv(4096)
                            if not chunk:
                                return
                            data += chunk
                        observed.append((header.split(b"\r\n")[0], data))
                        if callable(response):
                            response(connection)
                        elif response is not None:
                            connection.sendall(response)
            except (OSError, TimeoutError):
                pass
            finally:
                listener.close()
        thread = threading.Thread(target=serve)
        thread.start()
        self.addCleanup(thread.join)
        def files(_path, *, token=False):
            if token:
                return b"synthetic-fixture-token"
            descriptor = api.os.open(path, api.os.O_PATH | api.os.O_NOFOLLOW)
            return descriptor, f"/proc/self/fd/{descriptor}"
        return observed, files

    @staticmethod
    def runner_json(value, status=200):
        data = json.dumps(value).encode()
        return (f"HTTP/1.0 {status} Response\r\nContent-Type: application/json\r\n"
                f"Content-Length: {len(data)}\r\n\r\n").encode() + data

    def test_runner_scope_and_permissions_precede_all_io(self):
        self.start_runner_api(["read"])
        with patch.object(self.service, "runner_file") as files:
            for method, operation, body, aid in [
                ("GET", "capabilities", None, "agent-coding"),
                ("POST", "sessions", {"title": "test", "request_id": "s"}, "agent-coding"),
                ("POST", "runs/run-1/stop", {}, "agent-coding"),
                ("GET", "capabilities", None, "agent-assistant"),
            ]:
                with self.subTest(operation=operation, aid=aid):
                    with self.assertRaises(api.APIError) as error:
                        self.runner_route(method, operation, body, aid)
                    self.assertEqual(error.exception.status, 403)
            with self.assertRaises(api.APIError) as error:
                self.service.route("GET", "/v1/agents/agent-coding/runner/capabilities", None, peer_uid=0)
            self.assertEqual(error.exception.status, 403)
            files.assert_not_called()
        listed = self.service.list_agents(self.service.peer_grant(self.runner_uid))
        capability = listed["agents"][0]["capabilities"]["runner"]
        self.assertEqual(capability, {"read": False, "submit": False, "stop": False,
                                     "resource_scoped": True, "transport": "host-api"})
        self.assertNotIn("api-token", json.dumps(listed))
        self.assertNotIn("runner.sock", json.dumps(listed))

    def test_runner_real_unix_submission_receipt_and_lookup(self):
        self.start_runner_api()
        capabilities = {"available": True, "features": {"runs": True}}
        observed, files = self.fake_runner([
            self.runner_json(capabilities), self.runner_json({"run_id": "run-1"}),
            self.runner_json({"id": "run-1", "state": "completed"}),
        ])
        body = {"request_id": "request-1", "session_id": "session-1",
                "input": "synthetic task", "context_mode": "persistent"}
        with patch.object(self.service, "runner_file", side_effect=files), patch.object(api, "RUNSC_PI_PEER_UID", api.os.geteuid()):
            self.assertEqual(self.runner_route("POST", "runs", body), {"run_id": "run-1"})
            self.assertEqual(self.runner_route("POST", "runs", body), {"run_id": "run-1"})
            self.assertEqual(self.runner_route("GET", "runs?request_id=request-1")["state"], "completed")
            with self.assertRaises(api.APIError) as error:
                self.runner_route("POST", "runs", {**body, "input": "changed"})
            self.assertEqual(error.exception.status, 409)
        self.assertEqual([row[0] for row in observed], [
            b"GET /v1/capabilities HTTP/1.0", b"POST /v1/runs HTTP/1.0",
            b"GET /v1/runs?request_id=request-1 HTTP/1.0",
        ])

    def test_runner_uncertain_submission_is_not_replayed_after_restart(self):
        self.start_runner_api()
        observed, files = self.fake_runner([
            self.runner_json({"available": True, "features": {"runs": True}}), None,
        ])
        body = {"title": "synthetic session", "request_id": "session-1"}
        with patch.object(self.service, "runner_file", side_effect=files), patch.object(api, "RUNSC_PI_PEER_UID", api.os.geteuid()):
            with self.assertRaises(api.APIError):
                self.runner_route("POST", "sessions", body)
        self.close()
        self.start_runner_api()
        with patch.object(self.service, "runner_file") as files:
            with self.assertRaises(api.APIError) as error:
                self.runner_route("POST", "sessions", body)
            self.assertEqual(error.exception.status, 409)
            files.assert_not_called()
        self.assertEqual(len(observed), 2)

    def test_runner_rejects_paths_queries_and_bodies_before_io(self):
        self.start_runner_api()
        with patch.object(self.service, "runner_file") as files:
            for method, operation, body in [
                ("GET", "runs?request_id=x&request_id=y", None),
                ("GET", "runs?request_id=x&url=http://evil", None),
                ("GET", "capabilities?token=x", None),
                ("GET", "runs/../capabilities", None),
                ("GET", "runs%2Frun-1", None),
                ("POST", "sessions", {"title": "missing id"}),
                ("POST", "sessions", {"title": "test", "request_id": "x", "socket": "/evil"}),
                ("POST", "runs/run-1/stop", {"signal": 9}),
                ("POST", "runs", {"request_id": "x", "session_id": "s", "input": "x" * 65537, "context_mode": "persistent"}),
                ("POST", "runs", {"request_id": "x", "session_id": "s", "input": "x", "context_mode": "stateless"}),
            ]:
                with self.subTest(operation=operation):
                    with self.assertRaises(api.APIError):
                        self.runner_route(method, operation, body)
            files.assert_not_called()

    def test_runner_transport_refuses_peer_headers_size_timeout_and_private_errors(self):
        self.start_runner_api()
        cases = [
            (b"HTTP/1.0 302 Redirect\r\nLocation: http://evil\r\n\r\n", 502),
            (b"HTTP/1.0 200 OK\r\nContent-Type: application/json\r\nContent-Length: 9999999\r\n\r\n", 502),
            (b"HTTP/1.0 200 OK\r\nContent-Type: application/json\r\nTransfer-Encoding: chunked\r\n\r\n0\r\n\r\n", 502),
            (b"HTTP/1.0 200 OK\r\nContent-Type: application/json\r\nContent-Length: 2\r\nContent-Length: 2\r\n\r\n{}", 502),
            (self.runner_json({"error": "private token and path"}, 401), 502),
            (self.runner_json({"error": "private token and path"}, 404), 404),
            (lambda connection: time.sleep(0.1), 504),
        ]
        for response, status in cases:
            observed, files = self.fake_runner([response])
            with self.subTest(status=status), patch.object(self.service, "runner_file", side_effect=files), patch.object(api, "RUNSC_PI_PEER_UID", api.os.geteuid()), patch.object(api, "RUNNER_TIMEOUT", 0.05):
                with self.assertRaises(api.APIError) as error:
                    self.runner_route("GET", "capabilities")
                self.assertEqual(error.exception.status, status)
                self.assertNotIn("private token", str(error.exception))
            self.assertEqual(len(observed), 1)
        observed, files = self.fake_runner([self.runner_json({})])
        with patch.object(self.service, "runner_file", side_effect=files), patch.object(api, "RUNSC_PI_PEER_UID", api.os.geteuid() + 1):
            with self.assertRaises(api.APIError) as error:
                self.runner_route("GET", "capabilities")
            self.assertEqual(error.exception.status, 503)
        self.assertEqual(observed, [])

    def test_runner_events_are_bounded_json_frames(self):
        self.start_runner_api()
        observed, files = self.fake_runner([
            b"HTTP/1.0 200 OK\r\nContent-Type: text/event-stream\r\n\r\n: waiting\n\n",
            b'HTTP/1.0 200 OK\r\nContent-Type: text/event-stream\r\n\r\ndata: {"event":"run.completed","run_id":"run-1"}\n\n',
        ])
        with patch.object(self.service, "runner_file", side_effect=files), patch.object(api, "RUNSC_PI_PEER_UID", api.os.geteuid()):
            self.assertEqual(self.runner_route("GET", "runs/run-1/events"), {"events": []})
            self.assertEqual(self.runner_route("GET", "runs/run-1/events")["events"][0]["event"], "run.completed")
        self.assertEqual(len(observed), 2)

    def test_runner_descriptors_are_fixed_to_exact_inventory_id(self):
        descriptor = {"socket": "/run/tentaflake-pi/agent-coding/control/runner.sock",
                      "token": "/run/tentaflake-pi/agent-coding/api-token",
                      "transport": "runsc-pi-v1"}
        # Even an otherwise exact descriptor cannot admit a non-Pi inventory row.
        with self.assertRaisesRegex(ValueError, "runner inventory"):
            api.Service({**self.config, "runners": {"agent-coding": descriptor}})
        inventory = Path(self.config["inventory"])
        inventory.write_text(inventory.read_text().replace("fixture\tcoding\t", "pi\tcoding\t"))
        for runners in [
            {"agent-coding": {**descriptor, "socket": "/run/other.sock"}},
            {"agent-coding": {**descriptor, "token": "/run/tentaflake-pi/agent-assistant/api-token"}},
            {"agent-coding": {**descriptor, "transport": "native"}},
            {"agent-coding": {key: value for key, value in descriptor.items() if key != "transport"}},
            {"agent-coding": {**descriptor, "peer_uid": 0}},
            {"agent-waiting": descriptor},
            {"agent-absent": descriptor},
        ]:
            with self.subTest(runners=runners), self.assertRaisesRegex(ValueError, "runner inventory"):
                api.Service({**self.config, "runners": runners})

    def test_runner_credential_walk_checks_real_links_modes_and_identity(self):
        # Unprivileged fixture: remap only root and ownership metadata.
        # Traversal, no-follow opens, modes and hardlink counts are real.
        directory = self.root / "run/tentaflake-pi/agent-coding"
        directory.mkdir(parents=True, mode=0o700)
        credential = directory / "api-token"
        credential.write_text("a" * 64 + "\n")
        credential.chmod(0o440)
        original_open, original_fstat = api.os.open, api.os.fstat
        wrong_owner = False
        def opened(path, flags, *args, **kwargs):
            return original_open(self.root if path == "/" else path, flags, *args, **kwargs)
        def metadata(fd):
            value = original_fstat(fd)
            return SimpleNamespace(
                st_mode=value.st_mode, st_nlink=value.st_nlink, st_size=value.st_size,
                st_uid=123 if wrong_owner else 0, st_gid=10100,
            )
        path = "/run/tentaflake-pi/agent-coding/api-token"
        with patch.object(api.os, "open", side_effect=opened), patch.object(api.os, "fstat", side_effect=metadata):
            self.assertEqual(api.Service.runner_file(path, token=True), b"a" * 64)
            for value in ("short", "g" * 64 + "\n"):
                credential.chmod(0o600)
                credential.write_text(value)
                credential.chmod(0o440)
                with self.assertRaises(api.APIError):
                    api.Service.runner_file(path, token=True)
            credential.chmod(0o600)
            credential.write_text("a" * 64 + "\n")
            credential.chmod(0o644)
            with self.assertRaises(api.APIError):
                api.Service.runner_file(path, token=True)
            credential.chmod(0o440)
            wrong_owner = True
            with self.assertRaises(api.APIError):
                api.Service.runner_file(path, token=True)
            wrong_owner = False
            alias = directory / "alias"
            alias.hardlink_to(credential)
            with self.assertRaises(api.APIError):
                api.Service.runner_file(path, token=True)
            alias.unlink()
            credential.rename(alias)
            credential.symlink_to(alias)
            with self.assertRaises(OSError):
                api.Service.runner_file(path, token=True)
            credential.unlink()
            alias.rename(credential)
            directory.chmod(0o777)
            with self.assertRaises(api.APIError):
                api.Service.runner_file(path, token=True)
            directory.chmod(0o700)
            credential.unlink()
            with self.assertRaises(OSError):
                api.Service.runner_file(path, token=True)

    def test_runner_socket_inode_is_pinned_against_basename_replacement(self):
        directory = self.root / "run/tentaflake-pi/agent-coding"
        directory.mkdir(parents=True, mode=0o700)
        control = directory / "control"
        control.mkdir(mode=0o710)
        control.chmod(0o710)
        path = control / "runner.sock"
        listener = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        listener.bind(str(path))
        listener.listen()
        listener.settimeout(1)
        self.addCleanup(listener.close)
        path.chmod(0o620)
        original_open, original_fstat = api.os.open, api.os.fstat
        def opened(value, flags, *args, **kwargs):
            return original_open(self.root if value == "/" else value, flags, *args, **kwargs)
        def metadata(fd):
            value = original_fstat(fd)
            location = api.os.readlink(f"/proc/self/fd/{fd}")
            return SimpleNamespace(
                st_mode=value.st_mode, st_nlink=value.st_nlink, st_size=value.st_size,
                st_uid=10100 if location == str(control) or location.startswith(str(control) + "/") else 0,
                st_gid=10100,
            )
        with patch.object(api.os, "open", side_effect=opened), patch.object(api.os, "fstat", side_effect=metadata):
            descriptor, address = api.Service.runner_file("/run/tentaflake-pi/agent-coding/control/runner.sock")
            self.addCleanup(api.os.close, descriptor)
            path.rename(control / "retained.sock")
            path.symlink_to("/run/foreign-agent.sock")
            with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as client:
                client.settimeout(1)
                client.connect(address)
                accepted, _ = listener.accept()
                accepted.close()
            with self.assertRaises(api.APIError):
                api.Service.runner_file("/run/tentaflake-pi/agent-coding/control/runner.sock")

    def test_runner_unavailable_capability_prevents_mutation(self):
        self.start_runner_api()
        observed, files = self.fake_runner([
            self.runner_json({"available": False, "features": {"runs": False}}),
        ])
        with patch.object(self.service, "runner_file", side_effect=files), patch.object(api, "RUNSC_PI_PEER_UID", api.os.geteuid()):
            with self.assertRaises(api.APIError) as error:
                self.runner_route("POST", "sessions", {"title": "test", "request_id": "unavailable"})
            self.assertEqual(error.exception.status, 409)
        self.assertEqual([row[0] for row in observed], [b"GET /v1/capabilities HTTP/1.0"])

    def test_second_writer_and_unsafe_inventory_refused(self):
        self.start()
        with self.assertRaises(BlockingIOError):
            api.Service(self.config)
        self.close()
        Path(self.config["inventory"]).write_text(
            "fixture\tcoding\tagent-coding\t--all\t/tmp\ttrue\n"
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
