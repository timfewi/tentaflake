#!/usr/bin/env python3
"""Interrupt one real VM creation supervisor; never manufacture ledger state."""

import json
import os
import re
import select
import signal
import stat
import subprocess
import sys
import time


LEDGER = "/var/lib/tentaflake-admission/ledger.json"
SYSTEMCTL = "/run/current-system/sw/bin/systemctl"
PROPERTIES = ("Id", "LoadState", "ActiveState", "SubState", "MainPID", "ControlPID",
              "InvocationID", "ControlGroup", "Type", "KillMode", "Restart")


def require(condition, message):
    if not condition:
        raise RuntimeError(message)


def read(path, limit):
    with open(path, "rb") as stream:
        data = stream.read(limit + 1)
    require(len(data) <= limit, "observation exceeds fixture bound")
    return data.decode("utf-8")


def process(pid):
    fields = read(f"/proc/{pid}/stat", 8192).rsplit(") ", 1)[1].split()
    return fields[0], int(fields[19])  # Linux stat fields 3 and 22.


def run(operation, environment, path):
    require(os.geteuid() == 0, "creation interruption requires VM root")
    require(re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9_.:-]{0,127}", operation), "invalid operation")
    require(re.fullmatch(r"env-[0-9a-f]{32}", environment), "invalid environment")
    require(path == LEDGER, "unexpected fixture ledger path")
    unit = f"tentaflake-environment-create@{operation}.service"
    deadline = time.monotonic() + 240
    boot = read("/proc/sys/kernel/random/boot_id", 64).strip()

    def remaining():
        value = deadline - time.monotonic()
        require(value > 0, "creation interruption deadline exceeded")
        return value

    def systemctl(*args):
        result = subprocess.run([SYSTEMCTL, *args], check=True, stdout=subprocess.PIPE,
                                stderr=subprocess.DEVNULL, timeout=min(10, remaining()),
                                env={"LC_ALL": "C"})
        require(len(result.stdout) <= 8192, "oversized systemd observation")
        return result.stdout.decode("utf-8")

    def snapshot():
        remaining()
        with os.fdopen(os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK), "rb") as stream:
            metadata = os.fstat(stream.fileno())
            require(stat.S_ISREG(metadata.st_mode) and metadata.st_uid == 0
                    and stat.S_IMODE(metadata.st_mode) == 0o600 and metadata.st_nlink == 1,
                    "unsafe fixture ledger")
            raw = stream.read(8 * 1024 * 1024 + 1)
        require(len(raw) <= 8 * 1024 * 1024, "oversized fixture ledger")
        ledger = json.loads(raw)
        matches = [item for item in ledger["api"]["operations"]
                   if item["mutation"]["operationId"] == operation]
        require(len(matches) == 1 and matches[0]["environmentId"] == environment,
                "creation operation does not match environment")
        rows = [row for row in ledger["storage"] if row["allocation"]["binding"] is not None
                and row["allocation"]["binding"]["instanceId"] == environment]
        require(len(rows) == 2 and {row["allocation"]["kind"] for row in rows} == {"state", "workspace"}
                and all(row["release"] is None for row in rows), "unexpected native storage receipts")
        return matches[0], all(row["allocation"]["state"] == "present" for row in rows)

    def identity(execution, stopped=False):
        supervisor = execution["supervisor"]
        require(execution["bootId"] == boot and supervisor["unit"] == unit,
                "creation execution is not from this boot and unit")
        pid, ticks = supervisor["pid"], supervisor["startTimeTicks"]
        invocation = supervisor["invocationId"]
        require(type(pid) is int and 1 < pid < 2**31 and type(ticks) is int and ticks > 0
                and re.fullmatch(r"[0-9a-f]{32}", invocation) and invocation != "0" * 32,
                "invalid recorded supervisor")
        lines = systemctl("show", "--property=" + ",".join(PROPERTIES), "--", unit).splitlines()
        values = dict(line.split("=", 1) for line in lines)
        require(len(lines) == len(values) and set(values) == set(PROPERTIES), "invalid systemd fields")
        require(values["Id"] == unit and values["LoadState"] == "loaded"
                and values["ActiveState"] == "activating" and values["SubState"] == "start"
                and values["MainPID"] == str(pid) and values["ControlPID"] == "0"
                and values["InvocationID"] == invocation and values["Type"] == "oneshot"
                and values["KillMode"] == "control-group" and values["Restart"] == "no",
                "systemd supervisor identity changed")
        group = values["ControlGroup"]
        require(group.startswith("/") and group != "/"
                and read(f"/proc/{pid}/cgroup", 8192).strip() == "0::" + group,
                "supervisor cgroup mismatch")
        state, actual_ticks = process(pid)
        require(actual_ticks == ticks and (not stopped or state == "T"), "supervisor process changed")
        require(read("/proc/sys/kernel/random/boot_id", 64).strip() == boot, "boot changed")
        return pid

    initial, _ = snapshot()
    require(initial["state"] == "queued" and initial["execution"] is None, "operation is not queued")
    systemctl("start", "--no-block", "--", unit)
    pidfd, execution = None, None
    try:
        while True:
            recorded, present = snapshot()
            require(recorded["state"] in ("queued", "unknown"), "interruption window missed")
            if recorded["state"] == "unknown":
                if execution is None:
                    execution = recorded["execution"]
                    pid = identity(execution)
                    candidate = os.pidfd_open(pid)
                    try:
                        require(process(pid)[1] == execution["supervisor"]["startTimeTicks"],
                                "supervisor changed while opening pidfd")
                    except BaseException:
                        os.close(candidate)
                        raise
                    pidfd = candidate
                require(recorded["execution"] == execution, "recorded execution changed")
                if present:
                    signal.pidfd_send_signal(pidfd, signal.SIGSTOP)
                    while process(pid)[0] != "T":
                        time.sleep(min(0.002, remaining()))
                    identity(execution, stopped=True)
                    recorded, present = snapshot()
                    require(recorded["state"] == "unknown" and present and recorded["execution"] == execution,
                            "interruption window missed after stop")
                    break
            time.sleep(min(0.002, remaining()))
    finally:
        if pidfd is not None:
            try:
                try:
                    signal.pidfd_send_signal(pidfd, signal.SIGKILL)
                except ProcessLookupError:
                    pass
                # Cleanup still gets its own bound after the observation deadline.
                require(select.select([pidfd], [], [], 10)[0], "supervisor did not exit")
            finally:
                os.close(pidfd)
    return {"operationId": operation, "environmentId": environment, "execution": execution}


if __name__ == "__main__":
    try:
        require(len(sys.argv) == 4, "expected operation, environment and fixed ledger arguments")
        print(json.dumps(run(*sys.argv[1:]), separators=(",", ":")))
    except RuntimeError as error:
        sys.exit("creation interruption fixture failed: " + str(error))
    except Exception:
        # Avoid raw subprocess, path, JSON or process diagnostics in VM logs.
        sys.exit("creation interruption fixture failed; preserve ledger and storage")
