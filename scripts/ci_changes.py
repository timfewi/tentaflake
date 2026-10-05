#!/usr/bin/env python3
"""Select affected CI checks; unknown changes require the complete gate."""

import fnmatch
import json
import os
from pathlib import Path
import re
import subprocess
import sys


ROOT = Path(__file__).resolve().parents[1]
RULES = json.loads((ROOT / ".github/ci-paths.json").read_text())
CHECKS = {"runtime", "research", "evaluate", "generated", "rust", "cli", "broker",
          "worker", "devcontainer", "static", "selection"}
assert all(set(rule["checks"]) <= CHECKS for rule in RULES.values())


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


if __name__ == "__main__":
    if len(sys.argv) != 3:
        sys.exit("usage: ci_changes.py BASE_SHA HEAD_SHA")
    selected = select(*sys.argv[1:])
    for check in sorted(CHECKS):
        print(f"{check}={str(check in selected).lower()}")
    print(f"nix={str(bool(selected)).lower()}")
