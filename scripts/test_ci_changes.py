#!/usr/bin/env python3
"""Exercise CI decisions with actual Git histories, rather than mocked paths."""

import json
import os
from pathlib import Path
import re
import shlex
import subprocess
import sys
import tempfile
import textwrap
import tomllib
import unittest
from unittest.mock import patch

from ci_changes import CHECKS, execution, select


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

    def test_real_formatter_regression_stays_in_the_static_gate(self):
        self.write("scripts/test_format_check.py")
        affected = select(self.base, self.commit(), self.repository)
        self.assertEqual(affected, {"static"})
        self.assertFalse(execution(affected)["builds"])


    def test_evaluator_changes_run_evaluation_builds(self):
        self.write("scripts/eval_flake.py")
        self.write("scripts/test_eval_flake.py")
        self.write("scripts/test_eval_flake_nix.py")
        affected = select(self.base, self.commit(), self.repository)
        self.assertEqual(affected, {"evaluate", "selection", "static"})
        self.assertTrue(execution(affected)["evaluate"])
        self.assertTrue(execution(affected)["builds"])

    def test_catalog_generator_changes_run_metadata_and_evaluation_gates(self):
        for name in ("scripts/runtime-catalog-docs.py", "scripts/test_runtime_catalog.py"):
            with self.subTest(name=name):
                base = self.git("rev-parse", "HEAD")
                self.write(name)
                affected = select(base, self.commit(), self.repository)
                self.assertEqual(affected, {"static", "selection", "evaluate", "cli", "rust"})
                selected = execution(affected)
                self.assertTrue(selected["static"])
                self.assertTrue(selected["cli"])
                self.assertTrue(selected["evaluate"])
                self.assertTrue(selected["builds"])
                self.assertFalse(selected["runtime"])
                self.assertFalse(selected["research"])


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
                     "adapters/openclaw.nix", "adapters/pi-durable.nix", "tests/agent-adapters.nix",
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

    def test_new_crates_and_packages_select_their_units_and_builds(self):
        for crate in ("control", "integrations", "activity", "workloads"):
            expected = {crate, "rust", "static"}
            # Activity is a path dependency of both dispatching services.
            if crate == "activity":
                expected.update({"broker", "worker", "runtime"})
            for name in (f"crates/tentaflake-{crate}/src/lib.rs",
                         f"pkgs/tentaflake-{crate}/default.nix"):
                with self.subTest(name=name):
                    base = self.git("rev-parse", "HEAD")
                    self.write(name)
                    affected = select(base, self.commit(), self.repository)
                    self.assertEqual(affected, expected)
                    selected = execution(affected)
                    self.assertTrue(selected[crate])
                    self.assertTrue(selected["rust"])
                    self.assertTrue(selected["builds"])
                    self.assertEqual(selected["runtime"], crate == "activity")
                    self.assertFalse(selected["research"])

    def test_workload_module_and_vm_tests_select_workload_gates(self):
        for name in ("modules/workload-mapping.nix", "tests/workload-mapping-vm.nix",
                     "tests/workload-mapping-scale-vm.nix"):
            with self.subTest(name=name):
                base = self.git("rev-parse", "HEAD")
                self.write(name)
                self.assertEqual(select(base, self.commit(), self.repository),
                                 {"workloads", "evaluate", "static"})

    def test_pi_durable_sources_select_non_vm_gates(self):
        for name in ("pkgs/tentaflake-pi-durable/default.nix", "pkgs/tentaflake-pi-durable/engine.mjs",
                     "pkgs/tentaflake-pi-durable/test/sdk-child.mjs",
                     "pkgs/tentaflake-pi-durable/test/sdk-recovery.test.mjs",
                     "lib/piDurableTools.nix", "lib/piDurableTransport.nix",
                     "lib/piDurableProvisioning.nix", "modules/pi-durable-provisioning.nix",
                     "tests/pi-durable-provisioning.nix",
                     "tests/fixtures/pi-durable-provisioning/tentaflake.toml",
                     "tests/pi-durable-runtime-evaluation.nix", "tests/pi-durable-runtime-vm.nix",
                     "tests/local-image-proof.nix"):
            with self.subTest(name=name):
                base = self.git("rev-parse", "HEAD")
                self.write(name)
                affected = select(base, self.commit(), self.repository)
                self.assertEqual(affected, {"pi-durable", "evaluate", "static"})
                selected = execution(affected)
                self.assertTrue(selected["pi-durable"])
                self.assertTrue(selected["evaluate"])
                self.assertTrue(selected["builds"])
                self.assertFalse(selected["runtime"])
                self.assertFalse(selected["research"])

    def test_consumer_example_selects_the_actual_composition_fixture(self):
        self.write("examples/consumer-flake.nix")
        affected = select(self.base, self.commit(), self.repository)
        self.assertEqual(affected, {"generated", "static"})
        self.assertTrue(execution(affected)["generated"])
        self.assertTrue(execution(affected)["builds"])

    def test_control_evaluation_fixture_selects_its_build(self):
        self.write("tests/control-evaluation.nix")
        affected = select(self.base, self.commit(), self.repository)
        self.assertEqual(affected, {"evaluate", "static"})
        self.assertTrue(execution(affected)["evaluate"])
        self.assertTrue(execution(affected)["builds"])

    def test_shared_rust_inputs_require_all_rust_packages_not_research(self):
        self.write("Cargo.toml")
        self.assertEqual(select(self.base, self.commit(), self.repository),
                         {"runtime", "cli", "broker", "worker", "control", "integrations",
                          "activity", "workloads", "rust", "static"})

    def test_worker_module_uses_runtime_and_worker_gates_without_research(self):
        self.write("modules/worker.nix", "changed\n")
        head = self.commit()
        self.assertEqual(select(self.base, head, self.repository),
                         {"runtime", "worker", "rust", "static"})
        self.write("modules/security.nix", "changed\n")
        self.assertEqual(select(head, self.commit(), self.repository), CHECKS)
        self.assertEqual(select(self.base, self.git("rev-parse", "HEAD"), self.repository), CHECKS)

    def test_worker_wakeup_fixture_selects_worker_runtime_gates(self):
        self.write("tests/worker-drain-wakeup.nix", "changed\n")
        self.assertEqual(select(self.base, self.commit(), self.repository),
                         {"runtime", "worker", "rust", "static"})

    def test_ci_control_changes_use_regressions_without_vms_or_research(self):
        for name in (".github/ci-paths.json", ".github/vm-paths.json", ".github/workflows/check.yml",
                     "scripts/ci_changes.py", "scripts/ci_vm_changes.py",
                     "scripts/test_ci_changes.py", "scripts/test_ci_vm_changes.py",
                     "scripts/test_docs_inventory.py", "tests/ci-vm-selection.nix"):
            with self.subTest(name=name):
                base = self.git("rev-parse", "HEAD")
                self.write(name)
                self.assertEqual(select(base, self.commit(), self.repository), {"selection", "static"})

    def test_onboarding_validation_uses_installed_fixtures_without_research(self):
        for name in ("lib/agentPlan.nix", "lib/agentPlanEntry.nix", "tests/agent-onboarding.nix",
                     "scripts/agent-onboarding-test.py"):
            with self.subTest(name=name):
                base = self.git("rev-parse", "HEAD")
                self.write(name)
                self.assertEqual(select(base, self.commit(), self.repository), {"generated", "static"})

    def test_operator_onboarding_uses_real_cli_fixture_without_booting_vms(self):
        self.write("crates/tentaflake-cli/src/onboarding.rs")
        self.assertEqual(select(self.base, self.commit(), self.repository), {"cli", "rust", "generated", "static"})
        base = self.git("rev-parse", "HEAD")
        self.write("crates/tentaflake-cli/src/main.rs")
        head = self.commit()
        self.assertEqual(select(base, head, self.repository), {"runtime", "cli", "rust", "static"})
        self.assertEqual(select(self.base, head, self.repository), {"runtime", "cli", "rust", "generated", "static"})

    def test_shared_agent_parser_retains_runtime_and_schema_evaluation_without_research(self):
        self.write("lib/systemFromData.nix")
        self.assertEqual(select(self.base, self.commit(), self.repository),
                         {"runtime", "pi-durable", "evaluate", "generated", "static"})

    def test_system_schema_selects_cli_corpus_and_schema_evaluation(self):
        # The CLI embeds schema/system-v1*.json; the Nix parser reads it too.
        for name in ("schema/system-v1.json", "schema/system-v1-fixtures.json"):
            with self.subTest(name=name):
                base = self.git("rev-parse", "HEAD")
                self.write(name)
                self.assertEqual(select(base, self.commit(), self.repository),
                                 {"runtime", "pi-durable", "evaluate", "generated", "cli", "rust", "static"})

    def test_devcontainer_changes_do_not_build_agents_or_research(self):
        self.write(".devcontainer/devcontainer.json")
        self.assertEqual(select(self.base, self.commit(), self.repository), {"devcontainer", "static"})

    def test_host_api_changes_select_protocol_and_configuration_checks(self):
        for name in ("host-api/tentaflake_host_api/server.py", "lib/hostApiPolicy.nix", "modules/host-api.nix",
                     "pkgs/tentaflake-host-api/default.nix", "tests/host-api-evaluation.nix"):
            with self.subTest(name=name):
                base = self.git("rev-parse", "HEAD")
                self.write(name)
                affected = select(base, self.commit(), self.repository)
                self.assertEqual(affected, {"evaluate", "static"})
                self.assertTrue(execution(affected)["static"])
                self.assertTrue(execution(affected)["evaluate"])
                self.assertTrue(execution(affected)["builds"])

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
        self.assertEqual(actual, {check: "false" for check in CHECKS} |
                         {"nix": "false", "builds": "false"})

    def test_unknown_history_selects_the_full_inventory(self):
        affected = select("", self.base, self.repository)
        self.assertTrue(all(execution(affected).values()))

    def test_comparison_selects_only_affected_builds(self):
        self.write("modules/research.nix")
        affected = select(self.base, self.commit(), self.repository)
        selected = execution(affected)
        self.assertEqual({key for key in CHECKS if selected[key]},
                         {"research", "evaluate", "generated", "static"})
        self.assertTrue(selected["builds"])

    def test_crate_changes_select_unit_build_and_runtime_gates(self):
        self.write("crates/tentaflake-broker/src/main.rs")
        selected = execution(select(self.base, self.commit(), self.repository))
        self.assertTrue(selected["broker"])
        self.assertTrue(selected["rust"])
        self.assertFalse(selected["cli"])
        self.assertFalse(selected["worker"])
        self.assertTrue(selected["builds"])
        self.assertTrue(selected["runtime"])

    def test_unknown_paths_and_rename_require_full_inventory(self):
        self.git("mv", "modules/security.nix", "security.md")
        self.write("new-component/config")
        affected = select(self.base, self.commit(), self.repository)
        self.assertTrue(all(execution(affected).values()))

    def test_docs_and_ci_metadata_never_build(self):
        for name in ("README.md", ".github/workflows/check.yml", "justfile", "lib/devshell.nix"):
            base = self.git("rev-parse", "HEAD")
            self.write(name)
            affected = select(base, self.commit(), self.repository)
            self.assertFalse(execution(affected)["builds"])

    def test_cli_unknown_history_selects_builds_and_writes_summary(self):
        script = Path(__file__).with_name("ci_changes.py")
        summary = self.repository / "summary.md"
        result = subprocess.run(
            [sys.executable, str(script), "", "0" * 40, "--summary", str(summary)],
            check=True, capture_output=True, text=True)
        actual = dict(line.split("=", 1) for line in result.stdout.splitlines())
        for output in ("builds", "evaluate", "runtime", "research"):
            self.assertEqual(actual[output], "true")
        self.assertIn("Skipped builds are not runtime evidence", summary.read_text())

    def test_removed_mode_option_is_rejected(self):
        script = Path(__file__).with_name("ci_changes.py")
        result = subprocess.run([sys.executable, str(script), "", "0" * 40, "--mode", "fast"],
                                capture_output=True, text=True)
        self.assertNotEqual(result.returncode, 0)

    def test_first_parent_whitespace_check_detects_a_bad_docs_merge(self):
        self.git("checkout", "-b", "docs-change")
        self.write("README.md", "trailing whitespace \n")
        head = self.commit()
        self.git("checkout", "--detach", self.base)
        self.git("merge", "--no-ff", "--quiet", "-m", "fixture merge", head)
        with self.assertRaises(subprocess.CalledProcessError):
            self.git("show", "--format=", "--check", "--first-parent", "HEAD")


class WorkflowPolicy(unittest.TestCase):
    def run_step(self, name, selected):
        """Execute the real selection shell with Nix replaced by an argv recorder."""
        workflow = (Path(__file__).resolve().parent.parent / ".github/workflows/check.yml").read_text()
        steps = [step for step in re.split(r"(?m)^      - ", workflow)
                 if step.startswith(f"name: {name}\n")]
        self.assertEqual(len(steps), 1)
        environment = os.environ.copy()
        for variable, check in re.findall(
                r"(?m)^          (CHECK_[A-Z_]+): \$\{\{ needs\.changes\.outputs\.([a-z][a-z-]*) \}\}$",
                steps[0]):
            self.assertRegex(
                workflow,
                rf"(?m)^      {check}: \$\{{\{{ steps\.filter\.outputs\.{check} \}}\}}$",
            )
            environment[variable] = str(selected[check]).lower()
        script = textwrap.dedent(steps[0].split("        run: |\n", 1)[1])
        result = subprocess.run(
            ["bash", "-euo", "pipefail", "-c",
             'nix() { printf "%s\\n" "$*"; }\n' + script],
            env=environment, check=True, capture_output=True, text=True,
        )
        return [shlex.split(line) for line in result.stdout.splitlines()]

    def test_every_workspace_member_executes_its_cargo_unit_suite(self):
        root = Path(__file__).resolve().parent.parent
        members = tomllib.loads((root / "Cargo.toml").read_text())["workspace"]["members"]
        for member in members:
            package = Path(member).name
            check = package.removeprefix("tentaflake-")
            with self.subTest(package=package):
                self.assertIn(check, CHECKS)
                calls = self.run_step("Rust lint and unit tests", execution({check, "rust"}))
                self.assertEqual(calls, [
                    ["develop", "--command", "cargo", "clippy", "--package", package,
                     "--all-targets", "--", "-D", "warnings"],
                    ["develop", "--command", "cargo", "test", "--package", package],
                ])

    def test_each_rust_package_executes_its_explicit_build(self):
        for crate in ("cli", "broker", "worker", "control", "integrations", "activity", "workloads"):
            with self.subTest(crate=crate):
                calls = self.run_step(
                    "nix build (non-VM checks)", execution({crate, "rust"}))
                expected = {f".#checks.x86_64-linux.tentaflake-{crate}",
                            ".#checks.x86_64-linux.rust-package-sources"}
                if crate == "worker":
                    expected.add(".#checks.x86_64-linux.tentaflake-worker-image")
                self.assertEqual({arg for call in calls for arg in call[2:]}, expected)
                self.assertTrue(all(call[:2] == ["build", "--no-link"] for call in calls))
                self.assertTrue(all(len(call[2:]) <= 4 for call in calls))

    def test_pi_durable_changes_execute_recovery_module_graph_and_image_builds(self):
        calls = self.run_step(
            "nix build (non-VM checks)", execution({"pi-durable", "evaluate", "static"}))
        targets = {arg for call in calls for arg in call[2:]}
        self.assertTrue({
            ".#checks.x86_64-linux.tentaflake-pi-durable",
            ".#checks.x86_64-linux.tentaflake-pi-durable-recovery",
            ".#checks.x86_64-linux.pi-durable-runtime-evaluation",
            ".#checks.x86_64-linux.pi-durable-provisioning",
            ".#checks.x86_64-linux.pi-durable-runtime-module-graph",
            ".#checks.x86_64-linux.local-image-proof",
            ".#packages.x86_64-linux.tentaflake-pi-durable-image",
        } <= targets)
        self.assertTrue(all(len(call[2:]) <= 4 for call in calls))

    def test_full_inventory_executes_complete_non_vm_builds_in_bounded_groups(self):
        selected = execution(CHECKS)
        calls = self.run_step("Prepare exact evaluation closures and source filesets", selected)
        calls += self.run_step("nix build (non-VM checks)", selected)
        self.assertTrue(all(call[:2] == ["build", "--no-link"] for call in calls))
        self.assertTrue(all(1 <= len(call[2:]) <= 4 for call in calls))
        actual = {arg.rsplit(".", 1)[-1] for call in calls for arg in call[2:]}
        self.assertEqual(actual, {
            "tentaflake-cli", "tentaflake-broker", "tentaflake-worker",
            "tentaflake-control", "tentaflake-integrations", "tentaflake-activity",
            "tentaflake-workloads", "tentaflake-worker-image", "tentaflake-pi-durable",
            "tentaflake-pi-durable-recovery", "tentaflake-pi-durable-image",
            "pi-durable-runtime-evaluation", "pi-durable-runtime-module-graph",
            "pi-durable-provisioning",
            "local-image-proof", "host-api", "host-api-evaluation", "control-evaluation",
            "devcontainer-cli", "ci-vm-selection", "rust-package-sources", "system-schema",
            "image-pinning", "hardening-journald", "agent-adapters", "module-evaluation",
            "research-policy",
        })

    def test_component_vms_follow_pi_durable_and_workload_selection(self):
        for selected, expected in (
            ({"pi-durable"}, ["pi-durable-runtime-vm"]),
            ({"workloads", "rust"}, ["workload-mapping-vm", "workload-mapping-scale-vm"]),
            (CHECKS, ["pi-durable-runtime-vm", "workload-mapping-vm", "workload-mapping-scale-vm"]),
        ):
            with self.subTest(selected=sorted(selected)):
                calls = self.run_step("nix build (component VM tests)", execution(selected))
                self.assertEqual([call[-1].rsplit(".", 1)[-1] for call in calls], expected)
                self.assertTrue(all(call[:3] == ["build", "--no-link", "-L"] for call in calls))

    def test_optional_cache_setup_is_bounded_and_nonfatal(self):
        workflow = (Path(__file__).resolve().parent.parent / ".github/workflows/check.yml").read_text()
        # Inspect the small step block; actionlint separately validates YAML.
        steps = [step for step in re.split(r"(?m)^      - ", workflow)
                 if "uses: DeterminateSystems/magic-nix-cache-action@" in step]
        self.assertEqual(len(steps), 1)
        timeout = re.search(r"(?m)^        timeout-minutes: ([1-9][0-9]*)$", steps[0])
        self.assertIsNotNone(timeout)
        self.assertLessEqual(int(timeout.group(1)), 3)
        self.assertRegex(steps[0], r"(?m)^        continue-on-error: true$")


if __name__ == "__main__":
    unittest.main()
