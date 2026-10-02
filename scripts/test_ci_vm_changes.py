#!/usr/bin/env python3
"""Exercise CI decisions with actual Git histories, rather than mocked paths."""

import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch

from ci_vm_changes import select


class VmSelection(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.repository = Path(self.temporary.name)
        self.git("init", "--quiet")
        self.lock = {
            "version": 7,
            "nodes": {
                "tentaflake-research": {"locked": {"rev": "old"}},
                "nixpkgs": {"locked": {"rev": "old"}},
                "root": {"inputs": {"nixpkgs": "nixpkgs", "tentaflake-research": "tentaflake-research"}},
            },
        }
        self.write("flake.lock", json.dumps(self.lock))
        self.write("modules/security.nix", "{}\n")
        self.base = self.commit()

    def git(self, *arguments):
        environment = {key: value for key, value in os.environ.items() if not key.startswith("GIT_")}
        return subprocess.check_output(
            ["git", "-c", "user.name=CI fixture", "-c", "user.email=ci@example.invalid",
             "-c", "commit.gpgsign=false", "-C", str(self.repository), *arguments],
            env=environment, stderr=subprocess.DEVNULL,
        ).decode().strip()

    def write(self, name, value="fixture\n"):
        path = self.repository / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(value)

    def commit(self):
        self.git("add", ".")
        self.git("commit", "--quiet", "--allow-empty", "-m", "fixture")
        return self.git("rev-parse", "HEAD")

    def test_docs_and_contributor_tools_do_not_boot_vms(self):
        for name in ("README.md", "docs/new.md", "justfile", ".devcontainer/devcontainer.json"):
            self.write(name)
        self.assertEqual(select(self.base, self.commit(), self.repository), set())

    def test_runtime_and_research_changes_select_only_their_suite(self):
        for name, expected in (("crates/tentaflake-worker/src/main.rs", {"runtime"}),
                               ("installer/installer.sh", {"runtime"}),
                               ("tests/integration.nix", {"runtime"}),
                               ("modules/research.nix", {"research"}),
                               ("lib/researchClient.nix", {"research"}),
                               ("tests/research-integration.nix", {"research"})):
            with self.subTest(name=name):
                base = self.git("rev-parse", "HEAD")
                self.write(name)
                self.assertEqual(select(base, self.commit(), self.repository), expected)

    def test_shared_security_and_unknown_paths_require_both(self):
        for name in ("modules/security.nix", "lib/constants.nix", "lib/control-data.md", "flake.nix",
                     "adapters/default.nix", "adapters/hermes.nix", "adapters/zeroclaw.nix",
                     "adapters/openclaw.nix", "tests/agent-adapters.nix",
                     ".github/vm-paths.json", ".github/workflows/check.yml", "new-component/config"):
            with self.subTest(name=name):
                base = self.git("rev-parse", "HEAD")
                self.write(name, "changed\n")
                self.assertEqual(select(base, self.commit(), self.repository), {"runtime", "research"})

    def test_research_only_lock_pin_and_other_input_changes(self):
        self.lock["nodes"]["tentaflake-research"]["locked"]["rev"] = "new"
        self.write("flake.lock", json.dumps(self.lock))
        research_head = self.commit()
        self.assertEqual(select(self.base, research_head, self.repository), {"research"})
        self.lock["nodes"]["nixpkgs"]["locked"]["rev"] = "new"
        self.write("flake.lock", json.dumps(self.lock))
        self.assertEqual(select(research_head, self.commit(), self.repository), {"runtime", "research"})

    def test_invalid_base_or_lock_requires_both(self):
        self.assertEqual(select("0" * 40, self.base, self.repository), {"runtime", "research"})
        self.assertEqual(select("--invalid", self.base, self.repository), {"runtime", "research"})
        self.write("flake.lock", "invalid JSON")
        self.assertEqual(select(self.base, self.commit(), self.repository), {"runtime", "research"})

    def test_deleted_and_renamed_runtime_paths_cannot_hide_in_docs(self):
        self.git("mv", "modules/security.nix", "security.md")
        self.assertEqual(select(self.base, self.commit(), self.repository), {"runtime", "research"})

    def test_newlines_and_inherited_git_metadata_preserve_selection(self):
        self.write("crates/tentaflake-cli/src/new\nREADME.md")
        head = self.commit()
        self.assertEqual(select(self.base, head, self.repository), {"runtime"})
        with patch.dict(os.environ, {"GIT_DIR": "/unrelated/git/metadata"}):
            self.assertEqual(select(self.base, head, self.repository), {"runtime"})


if __name__ == "__main__":
    unittest.main()
