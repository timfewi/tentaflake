#!/usr/bin/env python3
"""Select affected checks and apply the automatic/explicit workload boundary."""

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
          "worker", "devcontainer", "static", "selection"}
assert all(set(rule["checks"]) <= CHECKS for rule in RULES.values())
FAST_CHECKS = {"rust", "cli", "broker", "worker", "static", "selection"}
BUILD_CHECKS = {"runtime", "research", "evaluate", "generated", "cli", "broker",
                "worker", "devcontainer"}


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


def execution(affected, mode):
    if mode not in {"fast", "full"}:
        raise ValueError("mode must be fast or full")
    selected = affected if mode == "full" else affected & FAST_CHECKS
    return {check: check in selected for check in sorted(CHECKS)} | {
        "nix": bool(selected - {"selection"}),
        "builds": mode == "full" and bool(affected & BUILD_CHECKS),
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("base")
    parser.add_argument("head")
    parser.add_argument("--mode", choices=("fast", "full"), default="fast")
    parser.add_argument("--summary", type=Path)
    arguments = parser.parse_args()
    affected = select(arguments.base, arguments.head)
    for check, enabled in execution(affected, arguments.mode).items():
        print(f"{check}={str(enabled).lower()}")
    if arguments.summary:
        with arguments.summary.open("a") as summary:
            summary.write(f"## Check workload: {arguments.mode}\n\n")
            summary.write("Affected inventory: " + (", ".join(sorted(affected)) or "none") + ".\n\n")
            summary.write(
                "PR/push runs only routing, whitespace, selected lint and Rust unit checks. "
                "Configuration/generated-flake evaluation, Research closure preparation, "
                "package/image/system builds and VM tests require an explicit dispatch.\n\n"
                "An explicit run uses the comparison SHA; missing or invalid comparison "
                "history selects the full inventory. Skipped builds are not runtime evidence. "
                "Before merging, obtain the build/runtime evidence required by the change "
                "for the current revision. Selection alone does not prove a check passed.\n"
            )


if __name__ == "__main__":
    main()
