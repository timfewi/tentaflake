#!/usr/bin/env python3
"""Exercise projection evaluation with real Nix and a shallow, local Git clone."""

from contextlib import nullcontext
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch

from eval_flake import evaluate


class ShallowEvaluation(unittest.TestCase):
    def test_shallow_checkout_keeps_dirty_tracking_filtering_and_input_pin(self):
        environment = {key: value for key, value in os.environ.items() if not key.startswith("GIT_")}

        def run(*arguments, cwd):
            result = subprocess.run(
                arguments, cwd=cwd, env=environment, capture_output=True, text=True, timeout=90,
            )
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            return result

        def git(directory, *arguments):
            return run(
                "git", "-c", "user.name=CI fixture", "-c", "user.email=ci@example.invalid",
                "-c", "commit.gpgsign=false", *arguments, cwd=directory,
            ).stdout.strip()

        def commit(directory):
            git(directory, "add", ".")
            git(directory, "commit", "--quiet", "-m", "fixture")

        with tempfile.TemporaryDirectory(prefix="tentaflake-shallow-") as temporary:
            root = Path(temporary)
            pin = root / "pin"
            pin.mkdir()
            git(pin, "init", "--quiet")
            (pin / "flake.nix").write_text('{ outputs = _: { value = "locked"; }; }\n')
            commit(pin)

            source = root / "source"
            source.mkdir()
            git(source, "init", "--quiet")
            (source / ".gitignore").write_text("ignored.nix\n")
            (source / "tracked").write_text("committed\n")
            flake = '''{
  inputs.pin.url = @PIN@;
  outputs = { pin, ... }:
    assert pin.value == "locked";
    assert builtins.readFile ./tracked == "committed\\n"
      || throw "fixture tracked content mismatch";
    assert !(builtins.pathExists ./ignored.nix);
    assert !(builtins.pathExists ./untracked.nix);
    {
      checks.x86_64-linux.fixture = builtins.derivation {
        name = "projection-fixture";
        system = "x86_64-linux";
        builder = "/fixture-must-not-build";
      };
    };
}
'''.replace("@PIN@", json.dumps("git+" + pin.as_uri()))
            (source / "flake.nix").write_text(flake)
            commit(source)
            run("nix", "flake", "lock", cwd=source)
            commit(source)
            # The dependency's moving HEAD must not replace the source lock's pin.
            (pin / "flake.nix").write_text('{ outputs = _: { value = "unpinned"; }; }\n')
            commit(pin)

            checkout = root / "shallow checkout"
            git(root, "clone", "--quiet", "--depth=1", source.as_uri(), str(checkout))
            self.assertEqual(git(checkout, "rev-parse", "--is-shallow-repository"), "true")
            lock = (checkout / "flake.lock").read_bytes()
            (checkout / "ignored.nix").write_text("private fixture\n")
            (checkout / "untracked.nix").write_text("untracked fixture\n")

            def evaluate_checkout():
                # Keep only the disposable wrapper alive for lock inspection;
                # every Git/Nix subprocess is real, and the outer fixture cleans up.
                wrapper = root / "projection"
                wrapper.mkdir(exist_ok=True)
                (wrapper / "flake.lock").unlink(missing_ok=True)
                with patch("eval_flake.tempfile.TemporaryDirectory", return_value=nullcontext(str(wrapper))):
                    evaluate(checkout)
                locked = json.loads((wrapper / "flake.lock").read_text())["nodes"]["source"]["locked"]
                self.assertEqual(locked["type"], "git")
                self.assertIs(locked.get("shallow"), True)

            evaluate_checkout()

            # This must fail if Git's dirty tracked contents reach the evaluator.
            (checkout / "tracked").write_text("dirty\n")
            with self.assertRaises(subprocess.CalledProcessError) as failed:
                evaluate_checkout()
            self.assertIn("fixture tracked content mismatch", failed.exception.stderr)
            (checkout / "flake.nix").write_text(flake.replace('"committed\\n"', '"dirty\\n"'))
            evaluate_checkout()
            # Attribute-name inventory succeeds, but upstream output validation
            # must reject this malformed package rather than silently skip it.
            valid = (checkout / "flake.nix").read_text()
            (checkout / "flake.nix").write_text(valid.replace(
                "checks.x86_64-linux.fixture =",
                'packages.x86_64-linux.invalid = false; checks.x86_64-linux.fixture =',
            ))
            with self.assertRaises(subprocess.CalledProcessError) as failed:
                evaluate_checkout()
            self.assertEqual(failed.exception.cmd[:3], ["nix", "flake", "check"])
            self.assertEqual((checkout / "flake.lock").read_bytes(), lock)
            self.assertEqual(git(checkout, "rev-list", "--count", "HEAD"), "1")


if __name__ == "__main__":
    unittest.main()
