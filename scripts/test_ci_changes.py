#!/usr/bin/env python3
"""Exercise CI decisions with actual Git histories, rather than mocked paths."""

import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

from ci_changes import CHECKS, select


class CiSelection(unittest.TestCase):
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

    def vms(self, base, head, repository):
        return select(base, head, repository) & {"runtime", "research"}

    def test_docs_and_contributor_tools_do_not_boot_vms(self):
        for name in ("README.md", "docs/new.md", "justfile", ".devcontainer/devcontainer.json"):
            self.write(name)
        self.assertEqual(self.vms(self.base, self.commit(), self.repository), set())

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
                self.assertEqual(self.vms(base, self.commit(), self.repository), expected)

    def test_shared_security_and_unknown_paths_require_both(self):
        for name in ("modules/security.nix", "lib/constants.nix", "lib/control-data.md", "flake.nix",
                     "adapters/default.nix", "adapters/hermes.nix", "adapters/zeroclaw.nix",
                     "adapters/openclaw.nix", "tests/agent-adapters.nix",
                     "new-component/config"):
            with self.subTest(name=name):
                base = self.git("rev-parse", "HEAD")
                self.write(name, "changed\n")
                self.assertEqual(self.vms(base, self.commit(), self.repository), {"runtime", "research"})

    def test_research_only_lock_pin_and_other_input_changes(self):
        self.lock["nodes"]["tentaflake-research"]["locked"]["rev"] = "new"
        self.write("flake.lock", json.dumps(self.lock))
        research_head = self.commit()
        self.assertEqual(self.vms(self.base, research_head, self.repository), {"research"})
        self.lock["nodes"]["nixpkgs"]["locked"]["rev"] = "new"
        self.write("flake.lock", json.dumps(self.lock))
        self.assertEqual(self.vms(research_head, self.commit(), self.repository), {"runtime", "research"})

    def test_invalid_base_or_lock_requires_both(self):
        self.assertEqual(select("0" * 40, self.base, self.repository), CHECKS)
        self.assertEqual(select("--invalid", self.base, self.repository), CHECKS)
        self.write("flake.lock", "invalid JSON")
        self.assertEqual(select(self.base, self.commit(), self.repository), CHECKS)

    def test_deleted_and_renamed_runtime_paths_cannot_hide_in_docs(self):
        self.git("mv", "modules/security.nix", "security.md")
        self.assertEqual(self.vms(self.base, self.commit(), self.repository), {"runtime", "research"})

    def test_newlines_and_inherited_git_metadata_preserve_selection(self):
        self.write("crates/tentaflake-cli/src/new\nREADME.md")
        head = self.commit()
        self.assertEqual(self.vms(self.base, head, self.repository), {"runtime"})
        with patch.dict(os.environ, {"GIT_DIR": "/unrelated/git/metadata"}):
            self.assertEqual(self.vms(self.base, head, self.repository), {"runtime"})

    def test_docs_select_no_nix_or_research_checks(self):
        for name in ("README.md", "AGENTS.md", "docs/roadmap.md", ".agents/skills/example/SKILL.md"):
            self.write(name)
        self.assertEqual(select(self.base, self.commit(), self.repository), set())

    def test_each_rust_crate_builds_only_its_package_without_research(self):
        for crate in ("cli", "broker", "worker"):
            with self.subTest(crate=crate):
                base = self.git("rev-parse", "HEAD")
                self.write(f"crates/tentaflake-{crate}/src/main.rs")
                self.assertEqual(select(base, self.commit(), self.repository),
                                 {"runtime", crate, "rust", "static"})

    def test_shared_rust_inputs_require_all_rust_packages_not_research(self):
        self.write("Cargo.toml")
        self.assertEqual(select(self.base, self.commit(), self.repository),
                         {"runtime", "cli", "broker", "worker", "rust", "static"})

    def test_ci_control_changes_use_regressions_without_vms_or_research(self):
        for name in (".github/ci-paths.json", ".github/vm-paths.json", ".github/workflows/check.yml",
                     "scripts/ci_changes.py", "scripts/ci_vm_changes.py",
                     "scripts/test_ci_changes.py", "scripts/test_ci_vm_changes.py",
                     "tests/ci-vm-selection.nix"):
            with self.subTest(name=name):
                base = self.git("rev-parse", "HEAD")
                self.write(name)
                self.assertEqual(select(base, self.commit(), self.repository), {"selection", "static"})

    def test_devcontainer_changes_do_not_build_agents_or_research(self):
        self.write(".devcontainer/devcontainer.json")
        self.assertEqual(select(self.base, self.commit(), self.repository), {"devcontainer", "static"})

    def test_research_only_changes_do_not_build_rust_packages(self):
        self.lock["nodes"]["tentaflake-research"]["locked"]["rev"] = "new"
        self.write("flake.lock", json.dumps(self.lock))
        self.assertEqual(select(self.base, self.commit(), self.repository),
                         {"research", "evaluate", "generated", "static"})

    def test_unknown_paths_require_all_checks(self):
        self.write("new-component/config")
        self.assertEqual(select(self.base, self.commit(), self.repository), CHECKS)

    def test_shared_nix_inputs_select_the_complete_gate(self):
        self.write("modules/security.nix", "changed\n")
        self.assertEqual(select(self.base, self.commit(), self.repository), CHECKS)

    def test_cli_emits_false_build_flags_for_a_docs_commit(self):
        scripts = Path(__file__).parent
        self.write("scripts/ci_changes.py", (scripts / "ci_changes.py").read_text())
        self.write(".github/ci-paths.json", (scripts.parent / ".github/ci-paths.json").read_text())
        base = self.commit()
        self.write("README.md")
        head = self.commit()
        result = subprocess.run([sys.executable, str(self.repository / "scripts/ci_changes.py"),
                                 base, head], check=True, capture_output=True, text=True)
        actual = dict(line.split("=", 1) for line in result.stdout.splitlines())
        self.assertEqual(actual, {check: "false" for check in CHECKS} | {"nix": "false"})

    def test_first_parent_whitespace_check_detects_a_bad_docs_merge(self):
        self.git("checkout", "-b", "docs-change")
        self.write("README.md", "trailing whitespace \n")
        head = self.commit()
        self.git("checkout", "--detach", self.base)
        self.git("merge", "--no-ff", "--quiet", "-m", "fixture merge", head)
        with self.assertRaises(subprocess.CalledProcessError):
            self.git("show", "--format=", "--check", "--first-parent", "HEAD")


if __name__ == "__main__":
    unittest.main()
