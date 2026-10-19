#!/usr/bin/env python3
"""Check complete projection coverage and the external Nix command boundary."""

from collections import Counter
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

from eval_flake import evaluation_plan


def inventory():
    return {
        "outputs": ["checks", "packages", "nixosConfigurations", "devShells", "futureOutput"],
        "checks": {"x86_64-linux": ["tentaflake"] + [f"check-{n}" for n in range(11)],
                   "aarch64-linux": ["another-check"]},
        "packages": {"x86_64-linux": ["installer-iso", "tool"], "aarch64-linux": ["tool"]},
        "hosts": ["installer-iso", "tentaflake"],
    }


class Evaluation(unittest.TestCase):
    def test_every_check_package_host_and_future_output_is_validated_once(self):
        source = inventory()
        actual = Counter()
        for selection in evaluation_plan(source):
            for output in ("checks", "packages"):
                for system, names in selection.get(output, {}).items():
                    if selection["kind"] == "checks":
                        self.assertLessEqual(len(names), 4)
                    actual.update((output, system, name) for name in names)
            if selection["kind"] == "host":
                actual[("nixosConfigurations", selection["name"])] += 1
                if selection["name"] == "tentaflake":
                    self.assertEqual(selection["checks"], {"x86_64-linux": ["tentaflake"]})
                if selection["name"] == "installer-iso":
                    self.assertEqual(selection["packages"], {"x86_64-linux": ["installer-iso"]})
            elif selection["kind"] == "output":
                actual[(selection["output"],)] += 1
        expected = Counter()
        for output in ("checks", "packages"):
            for system, names in source[output].items():
                expected.update((output, system, name) for name in names)
        expected.update(("nixosConfigurations", name) for name in source["hosts"])
        expected.update([("devShells",), ("futureOutput",)])
        self.assertEqual(actual, expected)
        self.assertEqual(source, inventory(), "planning must not mutate discovery evidence")

    def test_empty_output_groups_still_receive_upstream_validation(self):
        source = {"outputs": ["checks", "packages", "nixosConfigurations"],
                  "checks": {}, "packages": {}, "hosts": []}
        plan = evaluation_plan(source)
        self.assertEqual(len(plan), 3)
        self.assertIn({"kind": "output", "output": "checks"}, plan)
        self.assertIn({"kind": "output", "output": "nixosConfigurations"}, plan)

    def run_helper(self, fail=False):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            scripts = root / "scripts"
            scripts.mkdir()
            helper = scripts / "eval_flake.py"
            helper.write_text(Path(__file__).with_name("eval_flake.py").read_text())
            fake = root / "nix"
            fake.write_text(f"#!{sys.executable}\n" + '''
import json, os, pathlib, sys
args = sys.argv[1:]
log = pathlib.Path(os.environ["EVAL_TEST_LOG"])
record = {"args": args}
if args[:2] == ["flake", "lock"]:
    # Current Nix refuses to pin a dirty Git input without this opt-in. Only
    # the disposable wrapper may write its lock, never the source checkout.
    if "--allow-dirty-locks" not in args:
        print("fixture: dirty source requires --allow-dirty-locks", file=sys.stderr)
        sys.exit(19)
    record["flake"] = (pathlib.Path(args[-1][5:]) / "flake.nix").read_text()
elif args[:2] == ["flake", "check"]:
    record["selection"] = json.loads((pathlib.Path(args[-1][5:]) / "selection.json").read_text())
with log.open("a") as stream:
    stream.write(json.dumps(record) + "\\n")
if args[0] == "eval":
    print(os.environ["EVAL_TEST_INVENTORY"])
elif args[:2] == ["flake", "check"] and os.environ["EVAL_TEST_FAIL"] == "1":
    print("fixture output validation failed", file=sys.stderr)
    sys.exit(17)
''')
            fake.chmod(0o700)
            log = root / "calls.jsonl"
            result = subprocess.run(
                [sys.executable, str(helper)], capture_output=True, text=True, timeout=15,
                env=os.environ | {"PATH": str(root) + os.pathsep + os.environ["PATH"],
                                  "EVAL_TEST_LOG": str(log), "EVAL_TEST_FAIL": "1" if fail else "0",
                                  "EVAL_TEST_INVENTORY": json.dumps(inventory())},
            )
            calls = [json.loads(line) for line in log.read_text().splitlines()]
            self.assertIn(json.dumps("git+" + root.as_uri() + "?shallow=1"), calls[0]["flake"])
            return result, calls

    def test_real_entrypoint_uses_one_temporary_lock_and_only_no_build_validation(self):
        result, calls = self.run_helper()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual([call["args"][:2] for call in calls[:2]], [["flake", "lock"], ["eval", "--json"]])
        self.assertEqual(calls[0]["args"][:-1], ["flake", "lock", "--allow-dirty-locks"])
        checks = calls[2:]
        self.assertEqual([call["selection"] for call in checks], evaluation_plan(inventory()))
        for call in checks:
            self.assertEqual(call["args"][:-1], ["flake", "check", "--no-build", "--no-write-lock-file"])

    def test_failed_validation_stops_without_claiming_remaining_coverage(self):
        result, calls = self.run_helper(fail=True)
        self.assertEqual(result.returncode, 17)
        self.assertIn("fixture output validation failed", result.stderr)
        self.assertEqual(len(calls), 3)


if __name__ == "__main__":
    unittest.main()
