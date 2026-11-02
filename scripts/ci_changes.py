#!/usr/bin/env python3
"""Select the checks, builds and VM suites affected by a change."""

import argparse
import fnmatch
import json
import os
from pathlib import Path
import re
import subprocess


ROOT = Path(__file__).resolve().parents[1]
RULES = json.loads((ROOT / ".github/ci-paths.json").read_text())
CHECKS = {"runtime", "research", "evaluate", "generated", "rust", "cli", "broker",
          "worker", "control", "integrations", "activity", "workloads", "pi-durable",
          "devcontainer", "static", "selection"}
assert all(set(rule["checks"]) <= CHECKS for rule in RULES.values())
BUILD_CHECKS = {"runtime", "research", "evaluate", "generated", "cli", "broker",
                "worker", "control", "integrations", "activity", "workloads",
                "pi-durable", "devcontainer"}


def git(repository, *arguments):
    # A caller's Git metadata must not redirect the explicit repository.
    environment = {key: value for key, value in os.environ.items() if not key.startswith("GIT_")}
    return subprocess.check_output(
        ["git", "-C", str(repository), *arguments], env=environment, stderr=subprocess.DEVNULL
    )


def lock_checks(repository, base, head):
    before, after = (
        json.loads(git(repository, "show", f"{revision}:flake.lock"))
        for revision in (base, head)
    )
    if before == after:
        return set()
    # A research-only pin has no effect on the host's other locked inputs.
    del before["nodes"]["tentaflake-research"]
    del after["nodes"]["tentaflake-research"]
    return set(RULES["research"]["checks"]) if before == after else CHECKS.copy()


def select(base, head, repository=ROOT):
    if any(re.fullmatch(r"[0-9a-fA-F]{40}", revision) is None for revision in (base, head)):
        return CHECKS.copy()
    try:
        paths = git(repository, "diff", "--name-only", "--no-renames", "-z", base, head, "--")
        required = set()
        for encoded in paths.split(b"\0"):
            if not encoded:
                continue
            path = os.fsdecode(encoded)
            if path == "flake.lock":
                required.update(lock_checks(repository, base, head))
                continue
            # A documentation suffix cannot hide a file inside runtime code.
            for rule in RULES.values():
                if any(fnmatch.fnmatchcase(path, pattern) for pattern in rule["paths"]):
                    required.update(rule["checks"])
                    break
            else:
                required.update(CHECKS)
        return required
    except (subprocess.CalledProcessError, OSError, KeyError, TypeError, ValueError):
        return CHECKS.copy()


def execution(affected):
    # Every run, automatic or dispatched, executes all affected gates.
    return {check: check in affected for check in sorted(CHECKS)} | {
        "nix": bool(affected - {"selection"}),
        "builds": bool(affected & BUILD_CHECKS),
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("base")
    parser.add_argument("head")
    parser.add_argument("--summary", type=Path)
    arguments = parser.parse_args()
    affected = select(arguments.base, arguments.head)
    for check, enabled in execution(affected).items():
        print(f"{check}={str(enabled).lower()}")
    if arguments.summary:
        with arguments.summary.open("a") as summary:
            summary.write("## Affected checks\n\n")
            summary.write("Affected inventory: " + (", ".join(sorted(affected)) or "none") + ".\n\n")
            summary.write(
                "Every PR, push and dispatch runs the affected lint, unit, evaluation, "
                "generated-flake, package/image/system build and VM gates. Missing or "
                "invalid comparison history selects the full inventory. Skipped builds "
                "are not runtime evidence; selection alone does not prove a check passed.\n"
            )


if __name__ == "__main__":
    main()
