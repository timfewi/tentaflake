#!/usr/bin/env python3
"""Unit tests for the VM-side Golden Eval runner."""

import copy
import json
import re
import runpy
import subprocess
import tempfile
import unittest
from pathlib import Path
from unittest import mock

RUNNER_PATH = Path(__file__).with_name("golden-eval-runner.py")
CORPUS_PATH = Path(__file__).with_name("golden-evals.json")


class GoldenEvalRunnerTests(unittest.TestCase):
    def setUp(self):
        self.runner = runpy.run_path(str(RUNNER_PATH))
        self.corpus = json.loads(CORPUS_PATH.read_text(encoding="utf-8"))

    def validate(self, suite=None):
        if suite is None:
            suite = self.corpus
        return self.runner["validate_corpus"](suite)

    def mutated(self, mutator):
        suite = copy.deepcopy(self.corpus)
        mutator(suite)
        return suite

    def assert_invalid(self, mutator, message):
        with self.assertRaisesRegex(AssertionError, re.escape(message)):
            self.validate(self.mutated(mutator))

    def test_repository_corpus_is_valid(self):
        suite, cases, marker = self.validate()

        self.assertIs(suite, self.corpus)
        self.assertEqual(len(cases), 8)
        self.assertEqual(
            marker,
            "GOLDEN_PROMPT_MARKER_MUST_NOT_APPEAR_IN_AUDIT",
        )

    def test_multiple_cases_per_action_class_are_allowed(self):
        suite = copy.deepcopy(self.corpus)
        extra = copy.deepcopy(suite["cases"][0])
        extra["id"] = "local-reversible-second-scenario"
        suite["cases"].append(extra)

        _, cases, _ = self.validate(suite)

        self.assertEqual(len(cases), 9)

    def test_every_action_class_remains_required(self):
        def remove_forbidden(suite):
            suite["cases"] = [
                case
                for case in suite["cases"]
                if case["action_class"] != "forbidden"
            ]

        self.assert_invalid(
            remove_forbidden,
            "golden eval action-class matrix is missing ['forbidden']",
        )

    def test_suite_and_case_fields_are_closed(self):
        self.assert_invalid(
            lambda suite: suite.__setitem__("unexpected", True),
            "golden eval suite fields mismatch: missing=[], "
            "unexpected=['unexpected']",
        )
        self.assert_invalid(
            lambda suite: suite["cases"][0].pop("expected_status"),
            "golden eval case 0 fields mismatch: "
            "missing=['expected_status'], unexpected=[]",
        )
        self.assert_invalid(
            lambda suite: suite["cases"][0].__setitem__("unexpected", True),
            "golden eval case 0 fields mismatch: missing=[], "
            "unexpected=['unexpected']",
        )

    def test_schema_rejects_wrong_json_types(self):
        cases = [
            (
                lambda suite: suite.__setitem__("schema_version", True),
                "golden eval schema_version must be integer 1",
            ),
            (
                lambda suite: suite.__setitem__("description", 1),
                "golden eval suite must declare a non-empty description",
            ),
            (
                lambda suite: suite["cases"][0].__setitem__(
                    "artifacts_available", "yes"
                ),
                "local-reversible-runs-offline: "
                "artifacts_available must be a boolean",
            ),
            (
                lambda suite: suite["cases"][0].__setitem__(
                    "assert_offline", 1
                ),
                "local-reversible-runs-offline: "
                "assert_offline must be a boolean",
            ),
            (
                lambda suite: suite["cases"][0].__setitem__(
                    "expected_events", "completed"
                ),
                "local-reversible-runs-offline: "
                "expected_events must be a non-empty list",
            ),
            (
                lambda suite: suite["cases"][0].__setitem__(
                    "expected_events", ["request-accepted", 1]
                ),
                "local-reversible-runs-offline: "
                "expected_events must contain non-empty strings",
            ),
        ]
        for mutator, message in cases:
            with self.subTest(message=message):
                self.assert_invalid(mutator, message)

    def test_policy_projection_is_strict_and_ordered(self):
        self.assert_invalid(
            lambda suite: suite["cases"][0].__setitem__(
                "expected_events", ["completed", "request-accepted"]
            ),
            "local-reversible-runs-offline: expected policy projection "
            "('succeeded', True, ['request-accepted', 'completed']), "
            "got ('succeeded', True, ['completed', 'request-accepted'])",
        )
        self.assert_invalid(
            lambda suite: suite["cases"][0].__setitem__(
                "expected_status", "rejected"
            ),
            "local-reversible-runs-offline: expected policy projection "
            "('succeeded', True, ['request-accepted', 'completed']), "
            "got ('rejected', True, ['request-accepted', 'completed'])",
        )

    def audit_case(self):
        return {
            "id": "audit-case",
            "expected_events": [
                "approval-required",
                "approved",
                "completed",
            ],
        }

    def assert_audit_with(self, events):
        audit = self.runner["assert_audit"]
        records = [{"job": "audit-case", "event": event} for event in events]
        with mock.patch.dict(
            audit.__globals__,
            {"audit_events": lambda _case_id: records},
        ):
            audit(self.audit_case())

    def test_audit_transition_accepts_exact_order(self):
        self.assert_audit_with(
            ["approval-required", "approved", "completed"]
        )

    def test_audit_transition_rejects_reversed_order(self):
        with self.assertRaisesRegex(AssertionError, "audit transition mismatch"):
            self.assert_audit_with(
                ["completed", "approved", "approval-required"]
            )

    def test_audit_transition_rejects_extra_terminal_event(self):
        with self.assertRaisesRegex(AssertionError, "audit transition mismatch"):
            self.assert_audit_with(
                [
                    "approval-required",
                    "approved",
                    "completed",
                    "rejected",
                ]
            )

    def test_audit_transition_rejects_duplicate_event(self):
        with self.assertRaisesRegex(AssertionError, "audit transition mismatch"):
            self.assert_audit_with(
                [
                    "approval-required",
                    "approved",
                    "approved",
                    "completed",
                ]
            )

    def test_worker_requires_the_exact_expected_error(self):
        worker = self.runner["worker"]
        expected = "tentaflake-worker: pending job replay does not exist"
        completed = subprocess.CompletedProcess(
            args=[],
            returncode=1,
            stdout="",
            stderr=expected + "\n",
        )
        with mock.patch.object(
            worker.__globals__["subprocess"],
            "run",
            return_value=completed,
        ):
            worker(
                "approve",
                "replay",
                expect_success=False,
                expected_stderr=expected,
            )
            with self.assertRaisesRegex(
                AssertionError,
                "returned the wrong error",
            ):
                worker(
                    "approve",
                    "replay",
                    expect_success=False,
                    expected_stderr="different error",
                )

    def test_approved_case_checks_the_duplicate_approval_error(self):
        run_case = self.runner["run_case"]
        case = copy.deepcopy(self.corpus["cases"][1])
        calls = []

        def record_worker(
            command,
            case_id,
            expect_success,
            expected_stderr=None,
        ):
            calls.append(
                (command, case_id, expect_success, expected_stderr)
            )

        with mock.patch.dict(
            run_case.__globals__,
            {
                "write_job": lambda *_args: None,
                "expect_pending_without_side_effect": lambda _case: None,
                "worker": record_worker,
                "assert_result": lambda _case: None,
                "assert_audit": lambda _case: None,
            },
        ):
            run_case(case, "marker")

        case_id = case["id"]
        self.assertEqual(
            calls,
            [
                ("approve", case_id, True, None),
                (
                    "approve",
                    case_id,
                    False,
                    "tentaflake-worker: "
                    f"pending job {case_id} does not exist",
                ),
            ],
        )

    def test_offline_job_asserts_loopback_only_without_public_probe(self):
        write_job = self.runner["write_job"]
        case = copy.deepcopy(self.corpus["cases"][0])

        with tempfile.TemporaryDirectory() as directory:
            inbox = Path(directory)
            with mock.patch.dict(write_job.__globals__, {"INBOX": inbox}):
                write_job(case, "sensitive-marker")
            request = json.loads(
                (inbox / f"{case['id']}.json").read_text(encoding="utf-8")
            )

        script = request["argv"][2]
        self.assertIn("set -- /sys/class/net/*", script)
        self.assertIn('test "$#" -eq 1', script)
        self.assertIn('test "${1##*/}" = lo', script)
        self.assertNotIn("wget", script)
        self.assertNotIn("1.1.1.1", script)
        self.assertNotRegex(script, r"https?://")


if __name__ == "__main__":
    unittest.main()
