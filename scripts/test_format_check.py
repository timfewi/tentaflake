#!/usr/bin/env python3
"""Exercise the real formatter gate without changing unrelated source or index entries."""

import os
from pathlib import Path
import subprocess
import tempfile
import unittest


class FormatCheck(unittest.TestCase):
    def test_tracked_and_untracked_odd_filenames_are_rejected_without_writes(self):
        root = Path(__file__).resolve().parents[1]
        environment = {key: value for key, value in os.environ.items() if not key.startswith("GIT_")}

        def git(*arguments, **kwargs):
            return subprocess.run(
                ["git", "--literal-pathspecs", "-C", str(root), *arguments],
                check=True, env=environment, **kwargs,
            )

        fd, name = tempfile.mkstemp(prefix="-fmt check \n", suffix=".nix", dir=root)
        os.close(fd)
        path = Path(name)
        relative = path.name
        stage_attempted = False
        try:
            # A missing tracked file could theoretically share the random name.
            # Never remove or replace such an existing index entry.
            self.assertEqual(git("ls-files", "-z", "--", relative, stdout=subprocess.PIPE).stdout, b"")
            for tracked in (False, True):
                with self.subTest(tracked=tracked):
                    if tracked:
                        path.write_bytes(b"{}\n")
                        stage_attempted = True
                        git("add", "--", relative)
                    original = b"{probe=1;}\n"
                    path.write_bytes(original)
                    result = subprocess.run(
                        ["just", "fmt-check"], cwd=root, capture_output=True, text=True,
                        env=environment,
                    )
                    diagnostics = result.stdout + result.stderr
                    self.assertNotEqual(result.returncode, 0, diagnostics)
                    self.assertIn("not formatted", diagnostics)
                    # Accept either a literal or escaped newline in diagnostics.
                    self.assertIn(relative.rsplit("\n", 1)[-1], diagnostics)
                    self.assertEqual(path.read_bytes(), original)
                    if tracked:
                        self.assertEqual(git("show", f":{relative}", stdout=subprocess.PIPE).stdout, b"{}\n")
        finally:
            try:
                if stage_attempted:
                    git("rm", "--cached", "-f", "--ignore-unmatch", "--", relative,
                        stdout=subprocess.DEVNULL)
            finally:
                path.unlink(missing_ok=True)


if __name__ == "__main__":
    unittest.main()
