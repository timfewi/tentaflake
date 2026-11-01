#!/usr/bin/env python3
"""Prove adapter facts generate both artifacts and checks never rewrite them."""
import importlib.util
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

SCRIPT = Path(__file__).with_name("runtime-catalog-docs.py")
SPEC = importlib.util.spec_from_file_location("runtime_catalog_docs", SCRIPT)
catalog_docs = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(catalog_docs)


class RuntimeCatalog(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix="tentaflake-catalog-")
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        for name in ("lib", "adapters", "docs"):
            (self.root / name).mkdir()
        self.guide = self.root / "docs/agent-adapters.md"
        self.guide.write_text(
            "# Adapter fixture\n\n" + catalog_docs.START + "\nstale\n"
            + catalog_docs.END + "\n\nKeep this prose.\n"
        )
        self.catalog = {"schemaVersion": 1, "presets": {"fixture": {
            "identity": {"version": "1", "status": "definition"},
            "evidence": {"fixture": "source-regression", "vendorAcceptance": "pending"},
        }}}
        self.write_source()

    def write_source(self):
        # A pure Nix source fixture; no store or package realization is possible.
        payload = json.dumps(self.catalog)
        (self.root / "lib/runtimeCatalog.nix").write_text(
            "builtins.fromJSON " + json.dumps(payload) + "\n"
        )

    def run_generator(self, *arguments):
        return subprocess.run(
            [sys.executable, str(SCRIPT), "--root", str(self.root), *arguments],
            capture_output=True, text=True, timeout=45,
        )

    def test_generates_both_outputs_from_nix_and_detects_source_drift(self):
        result = self.run_generator()
        self.assertEqual(result.returncode, 0, result.stderr)
        output = self.root / "adapters/catalog.json"
        self.assertEqual(json.loads(output.read_text()), self.catalog)
        self.assertIn("| fixture | 1 | definition | source-regression | pending |", self.guide.read_text())
        self.assertTrue(self.guide.read_text().endswith("Keep this prose.\n"))
        self.assertEqual(self.run_generator("--check").returncode, 0)
        before = {path: path.read_bytes() for path in (output, self.guide)}
        self.catalog["presets"]["fixture"]["identity"]["version"] = "2"
        self.write_source()
        failed = self.run_generator("--check")
        self.assertNotEqual(failed.returncode, 0)
        self.assertIn("drifted", failed.stderr)
        self.assertEqual({path: path.read_bytes() for path in before}, before)

    def test_check_refuses_missing_artifact_without_writing(self):
        before = self.guide.read_bytes()
        self.assertNotEqual(self.run_generator("--check").returncode, 0)
        self.assertFalse((self.root / "adapters/catalog.json").exists())
        self.assertEqual(self.guide.read_bytes(), before)

    def test_generation_refuses_invalid_source_without_partial_writes(self):
        (self.root / "lib/runtimeCatalog.nix").write_text('throw "invalid fixture"\n')
        before = self.guide.read_bytes()
        failed = self.run_generator()
        self.assertNotEqual(failed.returncode, 0)
        self.assertNotIn("invalid fixture", failed.stderr)
        self.assertFalse((self.root / "adapters/catalog.json").exists())
        self.assertEqual(self.guide.read_bytes(), before)

    def test_generation_requires_one_marker_pair(self):
        self.guide.write_text(catalog_docs.START + catalog_docs.START + catalog_docs.END)
        before = self.guide.read_bytes()
        self.assertNotEqual(self.run_generator().returncode, 0)
        self.assertEqual(self.guide.read_bytes(), before)
        self.assertFalse((self.root / "adapters/catalog.json").exists())


if __name__ == "__main__":
    unittest.main()
