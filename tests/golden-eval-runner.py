#!/usr/bin/env python3
"""Run the versioned Tentaflake host-policy golden evaluation corpus in the VM."""

import json
import os
import re
import shlex
import subprocess
import time
from pathlib import Path

CORPUS_PATH = Path("/etc/tentaflake/golden-evals.json")
WORKSPACE = Path("/var/lib/hermes-test/workspace")
INBOX = WORKSPACE / ".tentaflake-worker" / "inbox"
STATE_DIR = Path("/var/lib/tentaflake-worker-hermes-test")
PENDING_DIR = STATE_DIR / "pending"
RESULTS_DIR = STATE_DIR / "results"
AUDIT_PATH = STATE_DIR / "audit.jsonl"
WORKER_CONFIG = Path("/etc/tentaflake/workers/hermes-test.json")
WORKER_UNIT = "tentaflake-worker-hermes-test.service"
WORKER_PATH_UNIT = "tentaflake-worker-hermes-test.path"
ACTION_CLASSES = {
    "local-reversible",
    "external-reversible",
    "irreversible",
    "financial",
    "production",
    "communicative",
    "privileged",
    "forbidden",
}
CASE_ID = re.compile(r"^[a-z0-9][a-z0-9_-]{0,63}$")


def fail(message):
    raise AssertionError(message)


def wait_for(predicate, description, timeout=30):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        value = predicate()
        if value:
            return value
        time.sleep(0.2)
    fail(f"timed out waiting for {description}")


def load_corpus():
    suite = json.loads(CORPUS_PATH.read_text(encoding="utf-8"))
    if suite.get("schema_version") != 1:
        fail("golden eval schema_version must be 1")
    if suite.get("suite") != "tentaflake-agent-host-policy":
        fail("golden eval suite name changed unexpectedly")
    marker = suite.get("sensitive_marker")
    if not isinstance(marker, str) or not marker:
        fail("golden eval suite must declare a non-empty sensitive_marker")
    cases = suite.get("cases")
    if not isinstance(cases, list) or not cases:
        fail("golden eval suite must contain cases")

    ids = set()
    classes = set()
    for case in cases:
        if not isinstance(case, dict):
            fail("golden eval case must be an object")
        case_id = case.get("id")
        action_class = case.get("action_class")
        decision = case.get("decision")
        if not isinstance(case_id, str) or not CASE_ID.fullmatch(case_id):
            fail(f"invalid golden eval case id: {case_id!r}")
        if case_id in ids:
            fail(f"duplicate golden eval case id: {case_id}")
        if action_class not in ACTION_CLASSES:
            fail(f"{case_id}: unknown action class {action_class!r}")
        if action_class in classes:
            fail(f"{case_id}: each action class must have exactly one v1 case")
        if decision not in {"auto", "approve", "deny"}:
            fail(f"{case_id}: unknown decision {decision!r}")
        if action_class in {"local-reversible", "forbidden"} and decision != "auto":
            fail(f"{case_id}: only automatic handling is valid for {action_class}")
        if action_class not in {"local-reversible", "forbidden"} and decision == "auto":
            fail(f"{case_id}: approval-required action must not run automatically")
        if not isinstance(case.get("expected_events"), list):
            fail(f"{case_id}: expected_events must be a list")
        ids.add(case_id)
        classes.add(action_class)
    if classes != ACTION_CLASSES:
        missing = sorted(ACTION_CLASSES - classes)
        extra = sorted(classes - ACTION_CLASSES)
        fail(f"golden eval action-class matrix mismatch: missing={missing}, extra={extra}")
    return suite, cases, marker


def write_job(case, sensitive_marker, padding_bytes=0):
    case_id = case["id"]
    script = [
        "set -eu",
        "test ! -e /run/docker.sock",
    ]
    if case.get("assert_offline"):
        script.append("! wget -T 1 -q https://1.1.1.1/ -O /tmp/direct-egress")
    script.extend(
        [
            "mkdir artifacts",
            f"printf '%s\\n' {shlex.quote(case_id)} > artifacts/marker",
        ]
    )
    padding = []
    while padding_bytes > 0:
        chunk = min(padding_bytes, 4096)
        padding.append("x" * chunk)
        padding_bytes -= chunk
    request = {
        "version": 1,
        "id": case_id,
        "action_class": case["action_class"],
        "argv": ["sh", "-c", "; ".join(script), sensitive_marker, *padding],
        "timeout_seconds": 5,
    }
    temporary = INBOX / f".{case_id}.tmp"
    destination = INBOX / f"{case_id}.json"
    with temporary.open("x", encoding="utf-8") as stream:
        json.dump(request, stream, separators=(",", ":"))
        stream.write("\n")
        stream.flush()
        os.fsync(stream.fileno())
    os.replace(temporary, destination)


def result_path(case_id):
    return RESULTS_DIR / case_id / "result.json"


def load_result(case):
    path = result_path(case["id"])
    wait_for(path.is_file, f"result for {case['id']}")
    return json.loads(path.read_text(encoding="utf-8"))


def expect_pending_without_side_effect(case):
    case_id = case["id"]
    pending = PENDING_DIR / f"{case_id}.json"
    wait_for(
        lambda: pending.is_file() or result_path(case_id).is_file(),
        f"pending decision for {case_id}",
    )
    if result_path(case_id).is_file():
        fail(f"{case_id}: approval-required action produced a result before operator decision")
    artifact = RESULTS_DIR / case_id / "artifacts" / "marker"
    if artifact.exists():
        fail(f"{case_id}: approval-required action produced an artifact before operator decision")


def worker(command, case_id, expect_success):
    completed = subprocess.run(
        ["tentaflake-worker", "--config", str(WORKER_CONFIG), command, case_id],
        check=False,
        text=True,
        capture_output=True,
    )
    succeeded = completed.returncode == 0
    if succeeded != expect_success:
        outcome = "unexpectedly succeeded" if succeeded else "failed"
        fail(
            f"{case_id}: {command} {outcome}\n"
            f"stdout:\n{completed.stdout}\nstderr:\n{completed.stderr}"
        )


def assert_result(case):
    case_id = case["id"]
    result = load_result(case)
    if result.get("id") != case_id:
        fail(f"{case_id}: result id mismatch: {result!r}")
    if result.get("action_class") != case["action_class"]:
        fail(f"{case_id}: result action_class mismatch: {result!r}")
    if result.get("status") != case["expected_status"]:
        fail(f"{case_id}: result status mismatch: {result!r}")
    if result.get("artifacts_available") is not case["artifacts_available"]:
        fail(f"{case_id}: artifact projection mismatch: {result!r}")

    artifact = RESULTS_DIR / case_id / "artifacts" / "marker"
    if case["artifacts_available"]:
        if artifact.read_text(encoding="utf-8").strip() != case_id:
            fail(f"{case_id}: artifact marker is missing or changed")
    elif artifact.exists():
        fail(f"{case_id}: rejected or denied action exposed an artifact")


def audit_events(case_id):
    if not AUDIT_PATH.is_file():
        return []
    return [
        item
        for item in (
            json.loads(line)
            for line in AUDIT_PATH.read_text(encoding="utf-8").splitlines()
            if line
        )
        if item.get("job") == case_id
    ]


def assert_audit(case):
    expected = set(case["expected_events"])

    def observed():
        return {item.get("event") for item in audit_events(case["id"])}

    wait_for(
        lambda: expected <= observed(),
        f"audit events {sorted(expected)} for {case['id']}",
    )
    events = observed()
    if not expected <= events:
        fail(f"{case['id']}: missing audit events {sorted(expected - events)}")


def run_case(case, sensitive_marker):
    case_id = case["id"]
    write_job(case, sensitive_marker)
    decision = case["decision"]
    if decision == "auto":
        assert_result(case)
    else:
        expect_pending_without_side_effect(case)
        worker(decision, case_id, expect_success=True)
        assert_result(case)
        if decision == "approve":
            worker("approve", case_id, expect_success=False)
    assert_audit(case)


def systemctl_checked(arguments, description):
    completed = subprocess.run(
        ["systemctl", *arguments],
        check=False,
        text=True,
        capture_output=True,
    )
    if completed.returncode != 0:
        fail(f"{description} failed:\n{completed.stdout}\n{completed.stderr}")


def worker_restart_count():
    completed = subprocess.run(
        ["systemctl", "show", "--value", "--property=NRestarts", WORKER_UNIT],
        check=False,
        text=True,
        capture_output=True,
    )
    if completed.returncode != 0:
        fail(f"cannot inspect worker restart count:\n{completed.stderr}")
    value = completed.stdout.strip()
    if not value.isdecimal():
        fail(f"worker restart count is not numeric: {value!r}")
    return int(value)


def assert_inbox_junk_does_not_starve_valid_work(sensitive_marker):
    case = {
        "id": "golden-inbox-fairness",
        "action_class": "local-reversible",
        "expected_status": "succeeded",
        "artifacts_available": True,
        "expected_events": ["request-accepted", "completed"],
    }
    # Stop the watcher before constructing the fixture so its first activation
    # observes the complete over-limit inbox rather than a timing-dependent
    # partial directory while Python is still creating entries.
    systemctl_checked(["stop", WORKER_PATH_UNIT], "stop worker path watcher")
    systemctl_checked(["stop", WORKER_UNIT], "stop worker service")
    restarts_before = worker_restart_count()
    for index in range(1024):
        (INBOX / f"golden-junk-{index:04}.json").mkdir()
    write_job(case, sensitive_marker)
    systemctl_checked(["start", WORKER_PATH_UNIT], "start worker path watcher")
    systemctl_checked(
        ["is-active", "--quiet", WORKER_PATH_UNIT], "verify worker path watcher"
    )
    # PathChanged intentionally does not retrigger merely because preserved junk
    # remains. Start the boot-enabled worker explicitly for this pre-existing
    # fixture; --no-block allows its expected bounded continuation restart.
    systemctl_checked(
        ["--no-block", "start", WORKER_UNIT], "start worker for pre-existing inbox"
    )
    assert_result(case)
    assert_audit(case)
    wait_for(
        lambda: worker_restart_count() > restarts_before,
        "a completed systemd retry after the bounded inbox scan",
    )
    wait_for(
        lambda: not (STATE_DIR / "inbox.cursor").exists(),
        "inbox cursor convergence after the retry",
    )
    if not (INBOX / "golden-junk-0000.json").is_dir():
        fail("inbox junk was unexpectedly deleted")
    if not (INBOX / "golden-junk-1023.json").is_dir():
        fail("inbox junk did not remain available for bounded scanning")
    wait_for(
        lambda: subprocess.run(
            ["systemctl", "show", "--value", "--property=ActiveState", WORKER_UNIT],
            check=False,
            text=True,
            capture_output=True,
        ).stdout.strip()
        == "inactive",
        "worker inactivity after inbox convergence",
    )
    audit_size = AUDIT_PATH.stat().st_size
    time.sleep(2)
    if AUDIT_PATH.stat().st_size != audit_size:
        fail("preserved inbox junk caused repeated worker audit growth")
    for index in range(1024):
        (INBOX / f"golden-junk-{index:04}.json").rmdir()


def assert_private_pending_capacity(sensitive_marker):
    config = json.loads(WORKER_CONFIG.read_text(encoding="utf-8"))
    limit = config.get("max_pending_requests")
    if not isinstance(limit, int) or limit < 1:
        fail("golden worker fixture must declare a positive max_pending_requests")
    if limit > 32:
        fail("golden worker fixture must keep max_pending_requests small")

    # As with the inbox fixture, construct the whole burst while the watcher is
    # stopped so VM scheduling cannot make this an accidental sequence of small
    # queue admissions.
    systemctl_checked(["stop", WORKER_PATH_UNIT], "stop worker path watcher")
    systemctl_checked(["stop", WORKER_UNIT], "stop worker service")
    cases = [
        {
            "id": f"golden-pending-{index:04}",
            "action_class": "external-reversible",
        }
        for index in range(limit + 1)
    ]
    for case in cases:
        write_job(case, sensitive_marker)
    systemctl_checked(["start", WORKER_PATH_UNIT], "start worker path watcher")

    accepted = cases[:limit]
    rejected = cases[-1]
    wait_for(
        lambda: all((PENDING_DIR / f"{case['id']}.json").is_file() for case in accepted)
        and not (PENDING_DIR / f"{rejected['id']}.json").exists()
        and not (INBOX / f"{rejected['id']}.json").exists(),
        "bounded private pending queue admission",
    )
    rejected_events = audit_events(rejected["id"])
    if not any(
        event.get("event") == "request-rejected"
        and event.get("detail") == "private pending queue capacity reached"
        for event in rejected_events
    ):
        fail(f"{rejected['id']}: missing private queue capacity rejection")

    for case in accepted:
        worker("deny", case["id"], expect_success=True)
        result = load_result(case)
        if result.get("status") != "denied":
            fail(f"{case['id']}: capacity fixture cleanup did not deny the request")
    wait_for(
        lambda: not list(PENDING_DIR.glob("golden-pending-*.json")),
        "private pending queue cleanup",
    )


def assert_private_pending_byte_capacity(sensitive_marker):
    config = json.loads(WORKER_CONFIG.read_text(encoding="utf-8"))
    byte_limit = config.get("max_pending_bytes")
    request_limit = config.get("max_request_bytes")
    if not isinstance(byte_limit, int) or not isinstance(request_limit, int):
        fail("golden worker fixture must declare private queue byte limits")
    padding_bytes = byte_limit // 2
    if padding_bytes <= 0 or padding_bytes + 4096 > request_limit:
        fail("golden worker fixture cannot exercise private queue byte capacity")

    systemctl_checked([ "stop", WORKER_PATH_UNIT ], "stop worker path watcher")
    systemctl_checked([ "stop", WORKER_UNIT ], "stop worker service")
    cases = [
        {
            "id": f"golden-pending-bytes-{index}",
            "action_class": "external-reversible",
        }
        for index in range(2)
    ]
    for case in cases:
        write_job(case, sensitive_marker, padding_bytes=padding_bytes)
    systemctl_checked([ "start", WORKER_PATH_UNIT ], "start worker path watcher")

    accepted, rejected = cases
    wait_for(
        lambda: (PENDING_DIR / f"{accepted['id']}.json").is_file()
        and not (PENDING_DIR / f"{rejected['id']}.json").exists()
        and not (INBOX / f"{rejected['id']}.json").exists(),
        "private pending byte-capacity admission",
    )
    wait_for(
        lambda: any(
            event.get("event") == "request-rejected"
            and event.get("detail") == "private pending queue capacity reached"
            for event in audit_events(rejected["id"])
        ),
        "private queue byte-capacity rejection audit",
    )
    worker("deny", accepted["id"], expect_success=True)
    if load_result(accepted).get("status") != "denied":
        fail(f"{accepted['id']}: byte-capacity fixture cleanup did not deny the request")


def assert_atomic_claim_race(sensitive_marker):
    case = {
        "id": "golden-claim-race",
        "action_class": "external-reversible",
    }
    write_job(case, sensitive_marker)
    expect_pending_without_side_effect(case)

    commands = [
        subprocess.Popen(
            [
                "tentaflake-worker",
                "--config",
                str(WORKER_CONFIG),
                "deny",
                case["id"],
            ],
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
        )
        for _ in range(2)
    ]
    outcomes = []
    for command in commands:
        stdout, stderr = command.communicate(timeout=30)
        outcomes.append((command.returncode, stdout, stderr))
    successes = sum(returncode == 0 for returncode, _, _ in outcomes)
    if successes != 1:
        fail(f"{case['id']}: expected exactly one atomic claimant, got {outcomes!r}")

    result = load_result(case)
    if result.get("status") != "denied":
        fail(f"{case['id']}: atomic claimant did not publish one denied result")
    denied_events = [
        event for event in audit_events(case["id"]) if event.get("event") == "denied"
    ]
    if len(denied_events) != 1:
        fail(f"{case['id']}: expected one terminal denied audit event, got {denied_events!r}")


def assert_state_capacity_admission(sensitive_marker):
    config = json.loads(WORKER_CONFIG.read_text(encoding="utf-8"))
    required = (
        2 * config["max_snapshot_bytes"]
        + config["max_log_bytes"]
        + config["max_pending_requests"] * 4096
        + 16 * 1024 * 1024
    )
    stats = os.statvfs(STATE_DIR)
    block_size = stats.f_frsize or stats.f_bsize
    available = stats.f_bavail * block_size
    shortfall = available - required + 1
    if shortfall <= 0:
        fail("golden worker state fixture unexpectedly lacks its baseline reservation")

    filler = RESULTS_DIR / "golden-state-capacity-fill"
    completed = subprocess.run(
        ["fallocate", "--length", str(shortfall), str(filler)],
        check=False,
        text=True,
        capture_output=True,
    )
    if completed.returncode != 0:
        fail(
            "cannot create worker-state capacity fixture:\n"
            f"stdout:\n{completed.stdout}\nstderr:\n{completed.stderr}"
        )
    try:
        after = os.statvfs(STATE_DIR)
        after_available = after.f_bavail * (after.f_frsize or after.f_bsize)
        if after_available >= required:
            fail("worker-state capacity fixture did not consume the reserved headroom")

        case = {
            "id": "golden-state-capacity",
            "action_class": "local-reversible",
        }
        write_job(case, sensitive_marker)
        result = load_result(case)
        if result.get("status") != "rejected":
            fail(f"{case['id']}: exhausted state capacity did not reject before execution")
        wait_for(
            lambda: any(
                event.get("event") == "rejected"
                and str(event.get("detail", "")).startswith(
                    "worker state capacity rejected execution:"
                )
                for event in audit_events(case["id"])
            ),
            "worker-state capacity rejection audit",
        )
    finally:
        filler.unlink(missing_ok=True)


def main():
    suite, cases, sensitive_marker = load_corpus()
    if not INBOX.is_dir() or not STATE_DIR.is_dir():
        fail("golden eval fixture paths are unavailable")

    for case in cases:
        run_case(case, sensitive_marker)
    assert_inbox_junk_does_not_starve_valid_work(sensitive_marker)
    assert_private_pending_capacity(sensitive_marker)
    assert_private_pending_byte_capacity(sensitive_marker)
    assert_atomic_claim_race(sensitive_marker)
    assert_state_capacity_admission(sensitive_marker)

    audit = AUDIT_PATH.read_text(encoding="utf-8")
    if sensitive_marker in audit:
        fail("worker audit leaked the golden sensitive marker")
    print(f"{suite['suite']} v{suite['schema_version']}: {len(cases)} cases passed")


if __name__ == "__main__":
    main()
