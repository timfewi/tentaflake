#!/usr/bin/env python3
"""docs/ holds only inventoried product guides; .scratch/ notes are never tracked."""

from pathlib import Path
import re
import subprocess
import sys


ROOT = Path(__file__).resolve().parents[1]
GUIDE = ROOT / "docs/18-documentation.md"
ROW = re.compile(r"^\| `(docs/[^`]+)` \| .+ \|$")


def inventory(text):
    try:
        block = text.split("<!-- guide-inventory:start -->", 1)[1].split("<!-- guide-inventory:end -->", 1)[0]
    except IndexError:
        raise SystemExit(f"{GUIDE.relative_to(ROOT)}: guide inventory markers are missing")
    listed = [match.group(1) for line in block.splitlines() if (match := ROW.match(line))]
    duplicates = sorted({path for path in listed if listed.count(path) > 1})
    return set(listed), duplicates


def tracked(path):
    # Outside a Git checkout (e.g. a source tarball) only the inventory is checked.
    result = subprocess.run(["git", "-C", str(ROOT), "ls-files", "-z", "--", path],
                            capture_output=True, check=False)
    return [name for name in result.stdout.decode().split("\0") if name] if result.returncode == 0 else []


def main():
    listed, duplicates = inventory(GUIDE.read_text())
    present = {str(path.relative_to(ROOT)) for path in (ROOT / "docs").rglob("*") if path.is_file()}
    failures = [f"{path}: listed more than once" for path in duplicates]
    failures += [f"{path}: not in the guide inventory; product guides only, working notes go to .scratch/"
                 for path in sorted(present - listed)]
    failures += [f"{path}: inventoried but missing" for path in sorted(listed - present)]
    failures += [f"{path}: .scratch/ is local working space and must never be tracked"
                 for path in tracked(".scratch")]
    if failures:
        print("\n".join(failures), file=sys.stderr)
        return 1
    print(f"docs inventory: {len(present)} guides match {GUIDE.relative_to(ROOT)}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
