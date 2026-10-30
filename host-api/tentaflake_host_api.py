"""Optional local operator API. No model, guest, shell, or configuration authority."""

import argparse
import fcntl
import hashlib
import json
import os
import re
import selectors
import signal
import socketserver
import sqlite3
import stat
import subprocess
import threading
import time
from pathlib import Path
from urllib.parse import parse_qs, unquote, urlsplit

IDENTIFIER = re.compile(r"[A-Za-z0-9][A-Za-z0-9_.-]{0,127}\Z")
UNIT = re.compile(r"[A-Za-z0-9][A-Za-z0-9_.@:-]{0,200}\.service\Z")
MAX_BODY = 4096
MAX_HEADERS = 8192
MAX_OUTPUT = 512 * 1024
MAX_RESPONSE = 768 * 1024
MAX_RECEIPTS = 10000
JOB_ID = re.compile(r"[a-z0-9][a-z0-9_-]{0,63}\Z")
DIGEST = re.compile(r"[a-f0-9]{64}\Z")
MODEL = re.compile(r"[A-Za-z0-9][A-Za-z0-9._:/+-]{0,199}\Z")
WORKER_LIMITS = {
    "max_timeout_seconds": ("maxTimeoutSeconds", 86400),
    "max_queue_entries": ("maxQueueEntries", 4096),
    "max_queue_bytes": ("maxQueueBytes", 1024**3),
    "max_snapshot_bytes": ("maxSnapshotBytes", 16 * 1024**3),
    "max_log_bytes": ("maxLogBytes", 1024**3),
}
APPROVAL_CLASSES = [
    "external-reversible",
    "irreversible",
    "financial",
    "production",
    "communicative",
    "privileged",
]
STATES = {
    "active",
    "inactive",
    "failed",
    "activating",
    "deactivating",
    "reloading",
    "maintenance",
    "refreshing",
}


class APIError(Exception):
    def __init__(self, message, status=400):
        super().__init__(message)
        self.status = status


def decode(raw):
    def unique(pairs):
        result = {}
        for key, value in pairs:
            if key in result:
                raise ValueError("Repeated JSON field")
            result[key] = value
        return result

    return json.loads(raw, object_pairs_hook=unique)


def nix_string(value):
    """Literal Nix double-quoted string, including interpolation protection."""
    return json.dumps(value, ensure_ascii=False).replace("${", r"\${")


def unknown_policy():
    return {
        "managed": False,
        "authority": "nix",
        "auto_start": None,
        "llm": {
            "enabled": False,
            "allowed_models": [],
            "max_completion_tokens": None,
            "streaming": False,
            "strict_cost_envelope": False,
            "budgets": None,
        },
        "fetch": {"enabled": False, "allowed_hosts": []},
        "research": {"enabled": False, "readiness": "unverified"},
        "worker": {
            "enabled": False,
            "network": "none",
            "limits": None,
            "approval_required": APPROVAL_CLASSES,
        },
        "storage": {
            "workspace_quota_mib": None,
            "private_state_quota_mib": None,
            "resize": "migration-required",
        },
        "management": {
            "transport": "tailscale",
            "ssh_authorization": "disabled",
            "remote_grants": "unverified",
        },
    }


def command(argv, *, environment, timeout=10, maximum=MAX_OUTPUT):
    """Bound time and combined output, including inherited pipe writers."""
    process = subprocess.Popen(
        argv,
        stdin=subprocess.DEVNULL,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        env=environment,
        start_new_session=True,
    )
    deadline = time.monotonic() + timeout
    streams = {process.stdout: bytearray(), process.stderr: bytearray()}
    total = 0
    completed = False
    try:
        with selectors.DefaultSelector() as selector:
            for pipe in streams:
                os.set_blocking(pipe.fileno(), False)
                selector.register(pipe, selectors.EVENT_READ)
            while selector.get_map():
                remaining = deadline - time.monotonic()
                if remaining <= 0:
                    raise APIError("Host operation timed out", 504)
                for event, _ in selector.select(min(remaining, 0.1)):
                    chunk = os.read(event.fileobj.fileno(), 65536)
                    if not chunk:
                        selector.unregister(event.fileobj)
                        continue
                    total += len(chunk)
                    if total > maximum:
                        raise APIError("Host operation exceeded its output limit", 502)
                    streams[event.fileobj].extend(chunk)
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                raise APIError("Host operation timed out", 504)
            try:
                status = process.wait(timeout=remaining)
            except subprocess.TimeoutExpired as exc:
                raise APIError("Host operation timed out", 504) from exc
            completed = True
            return (
                status,
                bytes(streams[process.stdout]),
                bytes(streams[process.stderr]),
            )
    finally:
        # A child can exit while descendants retain its pipes or continue work.
        # The fixed subprocess process group belongs exclusively to this request.
        if not completed:
            try:
                os.killpg(process.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
        process.wait()
        process.stdout.close()
        process.stderr.close()


class Service:
    def __init__(self, config):
        self.config = config
        self.lock = threading.RLock()
        self.action_lock = threading.Lock()
        self.db = self.owner = None
        self.agents = {}
        for key in (
            "systemctl",
            "journalctl",
            "cli",
            "cli_config",
            "inventory",
            "state",
        ):
            if (
                not isinstance(config.get(key), str)
                or not Path(config[key]).is_absolute()
            ):
                raise ValueError(
                    "Host API requires absolute configured executable and data paths"
                )
        for key in ("allow_actions", "allow_logs"):
            if type(config.get(key)) is not bool:
                raise ValueError("Host API permissions must be explicit booleans")
        for key in ("allow_tool_requests", "allow_tool_decisions"):
            config.setdefault(key, False)
            if type(config[key]) is not bool:
                raise ValueError("Host API permissions must be explicit booleans")
        if config["allow_tool_decisions"] and not config["allow_tool_requests"]:
            raise ValueError("Worker decisions require request inspection")
        if config.get("backend") not in {"docker", "podman"} or config.get(
            "security_profile"
        ) not in {"dev", "balanced", "strict"}:
            raise ValueError("Invalid host configuration")
        if (
            not isinstance(config.get("host"), str)
            or not 1 <= len(config["host"]) <= 255
        ):
            raise ValueError("Invalid host name")
        inventory = Path(config["inventory"]).read_bytes()
        if len(inventory) > 256 * 1024:
            raise ValueError("Host inventory exceeds its limit")
        for line in inventory.decode().splitlines():
            if not line.strip() or line.startswith("#"):
                continue
            fields = line.split("\t")
            if len(fields) != 6:
                raise ValueError("Invalid host inventory")
            runtime, name, aid, unit, _state_dir, runnable = fields
            if (
                not IDENTIFIER.fullmatch(aid)
                or not UNIT.fullmatch(unit)
                or aid in self.agents
                or runnable not in {"true", "false"}
                or not runtime
                or not name
            ):
                raise ValueError("Invalid host inventory identity")
            self.agents[aid] = {
                "id": aid,
                "runtime": runtime,
                "name": name,
                "unit": unit,
                "runnable": runnable == "true",
            }
        if len(self.agents) > 256:
            raise ValueError("Host inventory exceeds its agent limit")
        self.policies = config.get("policies", {})
        self.worker_configs = config.get("worker_configs", {})
        if (
            not isinstance(self.policies, dict)
            or set(self.policies) - set(self.agents)
            or len(json.dumps(self.policies).encode()) > MAX_OUTPUT
            or not isinstance(self.worker_configs, dict)
            or any(
                aid not in self.agents
                or not isinstance(path, str)
                or not Path(path).is_absolute()
                for aid, path in self.worker_configs.items()
            )
            or (
                self.worker_configs
                and (
                    not isinstance(config.get("worker"), str)
                    or not Path(config["worker"]).is_absolute()
                )
            )
        ):
            raise ValueError("Invalid immutable policy or worker inventory")
        self.stop_units = config.get("stop_units", {})
        if not isinstance(self.stop_units, dict) or any(
            aid not in self.agents
            or not isinstance(units, list)
            or len(units) > 2
            or any(
                unit
                not in {
                    f"tentaflake-broker-llm-{aid}.service",
                    f"tentaflake-broker-fetch-{aid}.service",
                }
                for unit in units
            )
            for aid, units in self.stop_units.items()
        ):
            raise ValueError("Invalid broker stop inventory")
        # Commands never inherit client-controlled environment or caller PATH.
        self.environment = {
            "PATH": config.get("command_path", "/run/current-system/sw/bin"),
            "LANG": "C.UTF-8",
            "LC_ALL": "C.UTF-8",
            "SYSTEMD_COLORS": "0",
            "SYSTEMD_PAGER": "cat",
            "TENTAFLAKE_CONFIG": config["cli_config"],
        }
        state = Path(config["state"])
        state.mkdir(mode=0o700, parents=True, exist_ok=True)
        metadata = state.lstat()
        if (
            not stat.S_ISDIR(metadata.st_mode)
            or metadata.st_uid != os.geteuid()
            or metadata.st_mode & 0o077
        ):
            raise ValueError("Host API state must be an owned private directory")
        try:
            descriptor = os.open(
                state / "owner.lock", os.O_CREAT | os.O_RDWR | os.O_NOFOLLOW, 0o600
            )
            self.owner = os.fdopen(descriptor, "r+")
            if not stat.S_ISREG(os.fstat(descriptor).st_mode):
                raise ValueError("Invalid host API ownership lock")
            fcntl.flock(descriptor, fcntl.LOCK_EX | fcntl.LOCK_NB)
            database = state / "actions.sqlite3"
            if database.exists() or database.is_symlink():
                metadata = database.lstat()
                if (
                    not stat.S_ISREG(metadata.st_mode)
                    or metadata.st_uid != os.geteuid()
                    or metadata.st_mode & 0o077
                ):
                    raise ValueError("Invalid host API receipt database")
            self.db = sqlite3.connect(
                database, check_same_thread=False, isolation_level=None
            )
            os.chmod(database, 0o600)
            self.db.execute("PRAGMA journal_mode=WAL")
            self.db.execute("PRAGMA synchronous=FULL")
            self.db.execute(
                "CREATE TABLE IF NOT EXISTS actions (request_id TEXT PRIMARY KEY, agent_id TEXT NOT NULL, action TEXT NOT NULL, response TEXT NOT NULL)"
            )
            self.db.execute(
                "CREATE TABLE IF NOT EXISTS tool_decisions (request_id TEXT PRIMARY KEY, intent TEXT NOT NULL, response TEXT NOT NULL)"
            )
        except BaseException:
            self.close()
            raise

    def close(self):
        with self.lock:
            if self.db is not None:
                self.db.close()
                self.db = None
            if self.owner is not None:
                self.owner.close()
                self.owner = None

    def run(self, argv, **kwargs):
        try:
            return command(argv, environment=self.environment, **kwargs)
        except OSError as exc:
            raise APIError("Host operation is unavailable", 503) from exc

    def info(self):
        return {
            "schema_version": 1,
            "host": self.config["host"],
            "backend": self.config["backend"],
            "security_profile": self.config["security_profile"],
            "capabilities": {
                "actions": self.config["allow_actions"],
                "logs": self.config["allow_logs"],
                "chat": False,
                "details": True,
                "config_proposals": True,
                "tool_requests": self.config["allow_tool_requests"],
                "tool_decisions": self.config["allow_tool_decisions"],
            },
        }

    def states(self, agents):
        if not agents:
            return {}
        code, output, _ = self.run(
            [
                self.config["systemctl"],
                "show",
                "--no-pager",
                "--property=Id,ActiveState,LoadState",
                "--",
                *[agent["unit"] for agent in agents],
            ],
            timeout=5,
        )
        if code != 0:
            raise APIError("Host state is unavailable", 503)
        states = {}
        for block in output.decode("utf-8", "replace").strip().split("\n\n"):
            fields = dict(
                line.split("=", 1) for line in block.splitlines() if "=" in line
            )
            state = fields.get("ActiveState")
            states[fields.get("Id")] = (
                state
                if state in STATES and fields.get("LoadState") == "loaded"
                else "unknown"
            )
        return {agent["id"]: states.get(agent["unit"], "unknown") for agent in agents}

    def list_agents(self):
        states = self.states(list(self.agents.values()))
        return {
            "schema_version": 1,
            "agents": [
                {key: agent[key] for key in ("id", "name", "runtime", "runnable")}
                | {
                    "state": states[agent["id"]],
                    "capabilities": {
                        "actions": (
                            ["start", "stop", "restart"]
                            if agent["runnable"]
                            else ["stop"]
                        )
                        if self.config["allow_actions"]
                        else [],
                        "logs": self.config["allow_logs"],
                        "chat": False,
                    }
                    | self.management_capabilities(agent),
                }
                for agent in self.agents.values()
            ],
        }

    def policy(self, agent):
        return self.policies.get(agent["id"], unknown_policy())

    def editable_fields(self, agent):
        policy = self.policy(agent)
        if not policy["managed"]:
            return []
        return (
            (["llm.allowed_models"] if policy["llm"]["enabled"] else [])
            + (["fetch.allowed_hosts"] if policy["fetch"]["enabled"] else [])
            + (
                [f"worker.{field}" for field in WORKER_LIMITS]
                if policy["worker"]["enabled"]
                else []
            )
        )

    def management_capabilities(self, agent):
        worker = (
            agent["id"] in self.worker_configs
            and self.policy(agent)["worker"]["enabled"]
        )
        return {
            "details": True,
            "config_proposals": bool(self.editable_fields(agent)),
            "tool_requests": worker and self.config["allow_tool_requests"],
            "tool_decisions": worker and self.config["allow_tool_decisions"],
        }

    def details(self, agent):
        policy = self.policy(agent)
        revision = hashlib.sha256(
            json.dumps(
                {"agent_id": agent["id"], "policy": policy},
                sort_keys=True,
                separators=(",", ":"),
                ensure_ascii=False,
            ).encode()
        ).hexdigest()
        return {
            "schema_version": 1,
            "agent_id": agent["id"],
            "policy_revision": revision,
            "policy": policy,
            "editable_fields": self.editable_fields(agent),
            "capabilities": self.management_capabilities(agent),
        }

    def proposal(self, agent, body):
        details = self.details(agent)
        if not isinstance(body, dict) or set(body) != {"policy_revision", "changes"}:
            raise APIError("A policy_revision and changes object are required")
        revision = body["policy_revision"]
        if not isinstance(revision, str) or not DIGEST.fullmatch(revision):
            raise APIError("Invalid policy revision")
        if revision != details["policy_revision"]:
            raise APIError("Declared policy changed; inspect the current policy", 409)
        changes = body["changes"]
        if (
            not isinstance(changes, dict)
            or not changes
            or set(changes) - set(details["editable_fields"])
        ):
            raise APIError("Only advertised editable fields may be proposed")
        assignments = []
        for field, value in sorted(changes.items()):
            broker = f"tentaflake.broker.agents.{nix_string(agent['id'])}"
            if field == "llm.allowed_models":
                if (
                    not isinstance(value, list)
                    or not 1 <= len(value) <= 32
                    or any(
                        not isinstance(model, dict)
                        or set(model)
                        != {
                            "name",
                            "input_microusd_per_million",
                            "output_microusd_per_million",
                        }
                        or not isinstance(model["name"], str)
                        or not MODEL.fullmatch(model["name"])
                        or any(
                            type(model[key]) is not int
                            or not 1 <= model[key] <= 2**53 - 1
                            for key in (
                                "input_microusd_per_million",
                                "output_microusd_per_million",
                            )
                        )
                        for model in value
                    )
                    or len({model["name"] for model in value}) != len(value)
                ):
                    raise APIError(
                        "Models require unique exact names and positive explicit prices"
                    )
                models = "\n".join(
                    "    { name = "
                    + nix_string(model["name"])
                    + f"; inputMicrousdPerMillion = {model['input_microusd_per_million']};"
                    + f" outputMicrousdPerMillion = {model['output_microusd_per_million']}; }}"
                    for model in value
                )
                assignments.append(
                    f"  {broker}.llm.allowedModels = lib.mkForce [\n{models}\n  ];"
                )
            elif field == "fetch.allowed_hosts":
                if (
                    not isinstance(value, list)
                    or not 1 <= len(value) <= 64
                    or any(
                        not isinstance(host, str)
                        or not 1 <= len(host) <= 253
                        or any(
                            not re.fullmatch(
                                r"[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?", label
                            )
                            for label in host.split(".")
                        )
                        for host in value
                    )
                    or len(set(value)) != len(value)
                ):
                    raise APIError(
                        "Fetch hosts must be unique exact lowercase DNS names"
                    )
                hosts = " ".join(nix_string(host) for host in value)
                assignments.append(
                    f"  {broker}.fetch.allowedHosts = lib.mkForce [ {hosts} ];"
                )
            else:
                option, maximum = WORKER_LIMITS[field.removeprefix("worker.")]
                if type(value) is not int or not 1 <= value <= maximum:
                    raise APIError("Worker limit is outside the proposal bounds")
                assignments.append(
                    f"  tentaflake.worker.agents.{nix_string(agent['id'])}.{option} = lib.mkForce {value};"
                )
        return {
            "schema_version": 1,
            "agent_id": agent["id"],
            "policy_revision": revision,
            "validation": "structure-only",
            "activation": "explicit-host-update-required",
            "changes": changes,
            "filename": f"tentaflake-{agent['id']}-proposal.nix",
            "nix": f"# Declared policy revision: {revision}\n# Proposal only: evaluate and review the full host before activation.\n{{ lib, ... }}:\n{{\n"
            + "\n".join(assignments)
            + "\n}\n",
            "warnings": [
                "Structure and declared revision checked; full Nix evaluation and build have not run.",
                "The module replaces submitted options with lib.mkForce; review the complete host policy before applying.",
                "No host files, credentials or running configuration were changed.",
            ],
        }

    def worker_arguments(self, agent, operation):
        if not self.management_capabilities(agent)[operation]:
            raise APIError("This worker operation is not enabled", 403)
        return [self.config["worker"], "--config", self.worker_configs[agent["id"]]]

    @staticmethod
    def worker_error(diagnostic):
        # Only classify known categories; never publish worker paths or input.
        for marker, message, status in (
            (b"busy:", "Worker queue is busy; inspect it again", 409),
            (b"conflict:", "Worker request or decision changed", 409),
            (b"not found:", "Captured worker request was not found", 404),
            (b"limit:", "Worker inspection exceeded its safety bound", 503),
            (b"invalid:", "Worker state or decision is invalid", 409),
        ):
            if diagnostic.startswith(b"tentaflake-worker: " + marker):
                return APIError(message, status)
        return APIError("Worker operation is unavailable", 503)

    def tool_requests(self, agent):
        arguments = self.worker_arguments(agent, "tool_requests")
        code, output, diagnostic = self.run([*arguments, "requests"], timeout=5)
        if code:
            raise self.worker_error(diagnostic)
        try:
            result = decode(output)
            if (
                not isinstance(result, dict)
                or result.get("schema_version") != 1
                or result.get("agent") != agent["id"]
                or type(result.get("truncated")) is not bool
                or not isinstance(result.get("requests"), list)
                or len(result["requests"]) > 100
            ):
                raise ValueError("Invalid worker snapshot")
            rows = []
            seen = set()
            for row in result["requests"]:
                if (
                    not isinstance(row, dict)
                    or not isinstance(row.get("id"), str)
                    or not JOB_ID.fullmatch(row["id"])
                    or row["id"] in seen
                    or not isinstance(row.get("request_digest"), str)
                    or not DIGEST.fullmatch(row["request_digest"])
                    or row.get("action_class")
                    not in [*APPROVAL_CLASSES, "local-reversible", "forbidden"]
                    or row.get("cwd") != "/workspace"
                    or row.get("state") != "pending"
                    or type(row.get("timeout_seconds")) is not int
                    or row["timeout_seconds"] <= 0
                    or row.get("approval_status")
                    not in {
                        "pending",
                        "approved",
                        "denied",
                        "not-required",
                        "forbidden",
                    }
                    or type(row.get("can_approve")) is not bool
                    or type(row.get("can_deny")) is not bool
                    or not isinstance(row.get("argv"), list)
                    or not 1 <= len(row["argv"]) <= 128
                    or any(
                        not isinstance(arg, str)
                        or not 1 <= len(arg.encode()) <= 4096
                        or "\0" in arg
                        for arg in row["argv"]
                    )
                ):
                    raise ValueError("Invalid worker request")
                seen.add(row["id"])
                rows.append(
                    {
                        key: row[key]
                        for key in (
                            "id",
                            "request_digest",
                            "action_class",
                            "argv",
                            "cwd",
                            "timeout_seconds",
                            "state",
                            "approval_status",
                            "can_approve",
                            "can_deny",
                        )
                    }
                )
        except (ValueError, TypeError, UnicodeError) as exc:
            raise APIError("Worker returned an invalid snapshot", 502) from exc
        return {
            "schema_version": 1,
            "agent_id": agent["id"],
            "truncated": result["truncated"],
            "requests": rows,
        }

    @staticmethod
    def decision_receipt(response):
        if "_error" in response:
            raise APIError(response["_error"], response["_status"])
        return response

    def tool_decision(self, agent, body):
        arguments = self.worker_arguments(agent, "tool_decisions")
        if not isinstance(body, dict) or set(body) != {
            "job_id",
            "request_digest",
            "decision",
            "request_id",
        }:
            raise APIError(
                "A job_id, request_digest, decision and request_id are required"
            )
        job, digest, decision, rid = (
            body[key] for key in ("job_id", "request_digest", "decision", "request_id")
        )
        if (
            not isinstance(job, str)
            or not JOB_ID.fullmatch(job)
            or not isinstance(digest, str)
            or not DIGEST.fullmatch(digest)
            or not isinstance(decision, str)
            or decision not in {"approve", "deny"}
            or not isinstance(rid, str)
            or not IDENTIFIER.fullmatch(rid)
        ):
            raise APIError("Invalid worker decision")
        intent = json.dumps([agent["id"], job, digest, decision], separators=(",", ":"))
        with self.lock:
            previous = self.db.execute(
                "SELECT intent,response FROM tool_decisions WHERE request_id=?", (rid,)
            ).fetchone()
            if previous is not None:
                if previous[0] != intent:
                    raise APIError(
                        "Request ID already identifies a different decision", 409
                    )
                return self.decision_receipt(decode(previous[1]))
            if (
                self.db.execute("SELECT COUNT(*) FROM tool_decisions").fetchone()[0]
                >= MAX_RECEIPTS
            ):
                raise APIError(
                    "Worker decision receipt capacity reached; operator maintenance required",
                    503,
                )
            if not self.action_lock.acquire(blocking=False):
                raise APIError("Another host action is in progress", 409)
            response = {
                "schema_version": 1,
                "agent_id": agent["id"],
                "job_id": job,
                "request_digest": digest,
                "decision": decision,
                "request_id": rid,
                "state": "unknown",
                "drain_state": "not-requested",
            }
            try:
                self.db.execute(
                    "INSERT INTO tool_decisions VALUES (?,?,?)",
                    (rid, intent, json.dumps(response)),
                )
            except BaseException:
                self.action_lock.release()
                raise
        try:
            try:
                code, output, diagnostic = self.run(
                    [*arguments, "decide", job, decision, "--expected-digest", digest],
                    timeout=5,
                )
                if code:
                    # Only explicit pre-publication rejections prove no decision
                    # was committed. Crashes, signals and unknown diagnostics do not.
                    if code > 0 and any(
                        diagnostic.startswith(b"tentaflake-worker: " + marker)
                        for marker in (
                            b"busy:",
                            b"conflict:",
                            b"not found:",
                            b"limit:",
                            b"invalid:",
                        )
                    ):
                        error = self.worker_error(diagnostic)
                        response.update(
                            state="failed", _error=str(error), _status=error.status
                        )
                else:
                    result = decode(output)
                    expected = {
                        "schema_version": 1,
                        "agent": agent["id"],
                        "job_id": job,
                        "request_digest": digest,
                        "decision": decision,
                        "state": "recorded",
                        "drain_required": True,
                    }
                    if result != expected:
                        raise ValueError("Invalid worker decision receipt")
                    response["state"] = "recorded"
                    # Commit the recorded decision before attempting to schedule.
                    # A crash cannot downgrade it to an unrecorded authorization.
                    response["drain_state"] = "unknown"
                    with self.lock:
                        self.db.execute(
                            "UPDATE tool_decisions SET response=? WHERE request_id=?",
                            (json.dumps(response), rid),
                        )
                    code, _, _ = self.run(
                        [
                            self.config["systemctl"],
                            "start",
                            "--no-block",
                            "--no-ask-password",
                            "--",
                            f"tentaflake-worker-{agent['id']}.service",
                        ],
                        timeout=5,
                    )
                    if code == 0:
                        response["drain_state"] = "queued"
            except (APIError, ValueError, UnicodeError):
                # Dispatch may have committed. Unknown never becomes automatic replay.
                pass
            with self.lock:
                self.db.execute(
                    "UPDATE tool_decisions SET response=? WHERE request_id=?",
                    (json.dumps(response), rid),
                )
            return self.decision_receipt(response)
        finally:
            self.action_lock.release()

    def health(self):
        code, output, _ = self.run([self.config["cli"], "health", "--json"], timeout=15)
        if code not in {0, 1}:
            raise APIError("Host diagnostics are unavailable", 503)
        try:
            report = decode(output)
            if (
                not isinstance(report, dict)
                or type(report.get("problems")) is not int
                or report["problems"] < 0
                or (report["problems"] == 0) != (code == 0)
            ):
                raise ValueError("Invalid health report")
        except (ValueError, UnicodeError) as exc:
            raise APIError("Host diagnostics returned an invalid report", 502) from exc
        return {"schema_version": 1, "healthy": code == 0, "report": report}

    def logs(self, agent, query):
        if not self.config["allow_logs"]:
            raise APIError("Host logs are not enabled", 403)
        fields = parse_qs(query, keep_blank_values=True)
        if set(fields) - {"lines"} or any(
            len(values) != 1 for values in fields.values()
        ):
            raise APIError("Invalid log query")
        raw = fields.get("lines", ["100"])[0]
        if not re.fullmatch(r"[0-9]{1,3}", raw) or not 1 <= int(raw) <= 200:
            raise APIError("Log lines must be between 1 and 200")
        count = int(raw)
        code, output, _ = self.run(
            [
                self.config["journalctl"],
                "--no-pager",
                "--output=json",
                "--output-fields=__REALTIME_TIMESTAMP,MESSAGE",
                "-n",
                str(count + 1),
                "-u",
                agent["unit"],
            ]
        )
        if code != 0:
            raise APIError("Host logs are unavailable", 503)
        entries, truncated = [], False
        try:
            rows = [decode(line) for line in output.splitlines()]
            truncated = len(rows) > count
            for row in rows[-count:]:
                message = row.get("MESSAGE", "")
                timestamp = row.get("__REALTIME_TIMESTAMP", "")
                if not isinstance(message, str):
                    message = "[non-text journal message]"
                if not isinstance(timestamp, str) or not re.fullmatch(
                    r"[0-9]{1,20}", timestamp
                ):
                    timestamp = ""
                truncated |= len(message) > 4096
                entries.append({"timestamp": timestamp, "message": message[:4096]})
        except (ValueError, AttributeError, UnicodeError) as exc:
            raise APIError("Host logs returned invalid data", 502) from exc
        return {
            "schema_version": 1,
            "agent_id": agent["id"],
            "entries": entries,
            "truncated": truncated,
        }

    def action(self, agent, body):
        if not self.config["allow_actions"]:
            raise APIError("Host actions are not enabled", 403)
        if not isinstance(body, dict) or set(body) != {"action", "request_id"}:
            raise APIError("An action and request_id are required")
        action, rid = body["action"], body["request_id"]
        if (
            not isinstance(action, str)
            or action not in {"start", "stop", "restart"}
            or not isinstance(rid, str)
            or not IDENTIFIER.fullmatch(rid)
        ):
            raise APIError("Invalid host action")
        if not agent["runnable"] and action != "stop":
            raise APIError("This agent has no runnable container", 409)
        with self.lock:
            previous = self.db.execute(
                "SELECT agent_id,action,response FROM actions WHERE request_id=?",
                (rid,),
            ).fetchone()
            if previous is not None:
                if previous[:2] != (agent["id"], action):
                    raise APIError(
                        "Request ID already identifies a different action", 409
                    )
                return decode(previous[2])
            if (
                self.db.execute("SELECT COUNT(*) FROM actions").fetchone()[0]
                >= MAX_RECEIPTS
            ):
                raise APIError(
                    "Host action receipt capacity reached; operator maintenance required",
                    503,
                )
            if not self.action_lock.acquire(blocking=False):
                raise APIError("Another host action is in progress", 409)
            # Persist uncertainty before dispatch. A crash at any later point may
            # have applied the systemd job, so a retry never dispatches it again.
            response = {
                "schema_version": 1,
                "request_id": rid,
                "agent_id": agent["id"],
                "action": action,
                "state": "unknown",
                "observed_state": "unknown",
            }
            try:
                self.db.execute(
                    "INSERT INTO actions VALUES (?,?,?,?)",
                    (rid, agent["id"], action, json.dumps(response)),
                )
            except BaseException:
                self.action_lock.release()
                raise
        try:
            units = [agent["unit"]]
            if action == "stop":
                units.extend(self.stop_units.get(agent["id"], []))
            try:
                code, _, _ = self.run(
                    [
                        self.config["systemctl"],
                        action,
                        "--no-ask-password",
                        "--",
                        *units,
                    ],
                    timeout=15,
                )
                response["state"] = "succeeded" if code == 0 else "failed"
            except APIError:
                pass
            try:
                response["observed_state"] = self.states([agent])[agent["id"]]
            except APIError:
                pass
            with self.lock:
                self.db.execute(
                    "UPDATE actions SET response=? WHERE request_id=?",
                    (json.dumps(response), rid),
                )
            return response
        finally:
            self.action_lock.release()

    def route(self, method, target, body):
        parsed = urlsplit(target)
        path = unquote(parsed.path)
        if (
            parsed.scheme
            or parsed.netloc
            or parsed.fragment
            or not target.startswith("/")
        ):
            raise APIError("Invalid request target")
        if method == "GET" and not parsed.query:
            if path == "/v1/info":
                return self.info()
            if path == "/v1/agents":
                return self.list_agents()
            if path == "/v1/health":
                return self.health()
        match = re.fullmatch(
            r"/v1/agents/([A-Za-z0-9][A-Za-z0-9_.-]{0,127})/(logs|actions|details|config-proposals|tool-requests|tool-decisions)",
            path,
        )
        if match:
            agent = self.agents.get(match[1])
            if agent is None:
                raise APIError("Unknown host agent", 404)
            if method == "GET" and match[2] == "logs":
                return self.logs(agent, parsed.query)
            if method == "POST" and match[2] == "actions" and not parsed.query:
                return self.action(agent, body)
            if not parsed.query:
                handlers = {
                    ("GET", "details"): lambda: self.details(agent),
                    ("POST", "config-proposals"): lambda: self.proposal(agent, body),
                    ("GET", "tool-requests"): lambda: self.tool_requests(agent),
                    ("POST", "tool-decisions"): lambda: self.tool_decision(agent, body),
                }
                if (method, match[2]) in handlers:
                    return handlers[method, match[2]]()
        raise APIError("Host API route not found", 404)


def receive(connection):
    deadline = time.monotonic() + 5
    raw = bytearray()
    while b"\r\n\r\n" not in raw:
        remaining = deadline - time.monotonic()
        if remaining <= 0:
            raise APIError("Request timed out", 408)
        connection.settimeout(remaining)
        chunk = connection.recv(4096)
        if not chunk:
            raise APIError("Incomplete request")
        raw.extend(chunk)
        if len(raw) > MAX_HEADERS + MAX_BODY:
            raise APIError("Request exceeds size limit", 413)
    header, raw = bytes(raw).split(b"\r\n\r\n", 1)
    if len(header) > MAX_HEADERS:
        raise APIError("Request headers exceed size limit", 413)
    try:
        lines = header.decode("ascii").split("\r\n")
        method, target, protocol = lines[0].split(" ")
        if protocol != "HTTP/1.1" or method not in {"GET", "POST"}:
            raise ValueError("Unsupported request")
        headers = {}
        for line in lines[1:]:
            key, value = line.split(":", 1)
            key = key.lower()
            if (
                not re.fullmatch(r"[a-z0-9-]+", key)
                or key in headers
                or any(ord(char) < 32 and char != "\t" for char in value)
            ):
                raise ValueError("Invalid headers")
            headers[key] = value.strip()
        if "transfer-encoding" in headers:
            raise ValueError("Transfer encoding is unsupported")
        length_text = headers.get("content-length", "0")
        if not re.fullmatch(r"[0-9]{1,6}", length_text):
            raise ValueError("Invalid content length")
        length = int(length_text)
        if length > MAX_BODY or len(raw) > length:
            raise APIError("Invalid request body length", 413)
        if (
            method == "GET"
            and length
            or method == "POST"
            and headers.get("content-type", "").split(";")[0].strip()
            != "application/json"
        ):
            raise ValueError("Expected JSON POST or bodyless GET")
        while len(raw) < length:
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                raise APIError("Request timed out", 408)
            connection.settimeout(remaining)
            chunk = connection.recv(length - len(raw))
            if not chunk:
                raise ValueError("Incomplete body")
            raw += chunk
        return method, target, decode(raw) if raw else None
    except (ValueError, UnicodeError) as exc:
        raise APIError("Invalid HTTP or JSON request") from exc


def respond(connection, status, body):
    data = json.dumps(body, ensure_ascii=False, separators=(",", ":")).encode()
    if len(data) > MAX_RESPONSE:
        status = 502
        data = b'{"error":"Host response exceeded its size limit"}'
    connection.settimeout(5)
    connection.sendall(
        f"HTTP/1.1 {status} Response\r\nContent-Type: application/json\r\nContent-Length: {len(data)}\r\nConnection: close\r\nCache-Control: no-store\r\n\r\n".encode()
        + data
    )


class Handler(socketserver.BaseRequestHandler):
    def handle(self):
        try:
            method, target, body = receive(self.request)
            response = self.server.service.route(method, target, body)
            respond(self.request, 200, response)
        except APIError as exc:
            try:
                respond(self.request, exc.status, {"error": str(exc)})
            except OSError:
                pass
        except OSError:
            pass
        except Exception:  # noqa: BLE001 - never expose host paths or raw command diagnostics.
            try:
                respond(self.request, 503, {"error": "Host API is unavailable"})
            except OSError:
                pass


class Server(socketserver.ThreadingMixIn, socketserver.UnixStreamServer):
    daemon_threads = False
    request_queue_size = 16

    def __init__(self, path, service):
        self.service = service
        self.capacity = threading.BoundedSemaphore(8)
        self.path = Path(path)
        self.identity = None
        if self.path.exists() or self.path.is_symlink():
            metadata = self.path.lstat()
            if not stat.S_ISSOCK(metadata.st_mode) or metadata.st_uid != os.geteuid():
                raise ValueError("Refusing to replace a non-owned API socket")
            # Service's exclusive state lock already excludes another owner.
            self.path.unlink()
        super().__init__(str(path), Handler)
        os.chmod(path, 0o660)
        self.identity = self.path.stat().st_ino

    def process_request(self, request, client_address):
        if not self.capacity.acquire(blocking=False):
            try:
                respond(request, 503, {"error": "Host API concurrency limit reached"})
            except OSError:
                pass
            self.shutdown_request(request)
            return
        try:
            super().process_request(request, client_address)
        except BaseException:
            self.capacity.release()
            raise

    def process_request_thread(self, request, client_address):
        try:
            super().process_request_thread(request, client_address)
        finally:
            self.capacity.release()

    def server_close(self):
        super().server_close()
        if self.identity is not None:
            try:
                if self.path.lstat().st_ino == self.identity:
                    self.path.unlink()
            except FileNotFoundError:
                pass


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--config", required=True)
    parser.add_argument("--socket", default="/run/tentaflake-api/host.sock")
    args = parser.parse_args()
    os.umask(0o077)
    service = Service(decode(Path(args.config).read_bytes()))
    server = Server(args.socket, service)

    def stop(_signal, _frame):
        threading.Thread(target=server.shutdown, daemon=True).start()

    signal.signal(signal.SIGTERM, stop)
    signal.signal(signal.SIGINT, stop)
    try:
        server.serve_forever()
    finally:
        server.server_close()
        service.close()


if __name__ == "__main__":
    main()
