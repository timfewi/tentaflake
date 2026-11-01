#!/usr/bin/env python3
"""Exercise installed-host imports with the actual CLI and shared Nix validator."""
import fcntl
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import tomllib


def toml_value(value):
    """Encode the JSON-compatible fixture subset as TOML inline values."""
    if isinstance(value, dict):
        return "{ " + ", ".join(f"{json.dumps(key)} = {toml_value(item)}"
                               for key, item in value.items()) + " }"
    if isinstance(value, list):
        return "[" + ", ".join(toml_value(item) for item in value) + "]"
    return json.dumps(value, ensure_ascii=False)


def toml_document(value):
    return "".join(f"{json.dumps(key)} = {toml_value(item)}\n"
                   for key, item in value.items())


def main():
    binary, flake_dir, hostname = sys.argv[1:]
    flake = Path(flake_dir)
    destination = flake / "tentaflake.toml"
    with tempfile.TemporaryDirectory() as temporary:
        work = Path(temporary)
        config = work / "cli.conf"
        inventory = work / "agents.tsv"
        inventory.write_text("")
        config.write_text(f"backend=docker\nflake_dir={flake}\nhost_name=renamed-host\nconfiguration_name={hostname}\nagents_file={inventory}\nsecurity_profile=balanced\n")
        environment = {**os.environ, "TENTAFLAKE_CONFIG": str(config)}

        def run(*args, success=True, env=None):
            result = subprocess.run([binary, *args], env=env or environment,
                                    capture_output=True, text=True, timeout=75)
            assert (result.returncode == 0) == success, (args, result.stdout, result.stderr)
            return result

        def candidate(value):
            path = work / "incoming.toml"
            path.write_text(toml_document(value))
            return str(path)

        preset = tomllib.loads(run("agent", "template", "hermes", "new-preset").stdout)
        path = candidate(preset)
        before = destination.read_bytes()
        plan = json.loads(run("agent", "plan", path, "--json").stdout)
        assert plan["valid"] and not plan["imported"]
        assert plan["instances"][0]["vendorAcceptance"] == "pending"
        assert destination.read_bytes() == before
        run("agent", "validate", path)
        run("agent", "import", path)
        combined = tomllib.loads(destination.read_text())
        assert combined["agents"] == preset["agents"]
        assert set(combined) == {"schemaVersion", "agents"}

        # Reviewed generic argv executes only inside the offline capsule.
        image = "example.invalid/coding@sha256:" + "a" * 64
        stopped = tomllib.loads(run("agent", "template", "generic", "new-coding", "--image", image,
                                   "--", "fixture", "--json", "--hide").stdout)
        assert stopped["agents"][0]["definition"]["command"] == ["fixture", "--json", "--hide"]
        path = candidate(stopped)
        hidden = run("agent", "plan", path, "--hide", "--json").stdout
        assert "new-coding" not in hidden and "renamed-host" not in hidden and hostname not in hidden and str(flake) not in hidden
        assert json.loads(hidden)["valid"]
        run("agent", "import", path)
        assert tomllib.loads(destination.read_text())["agents"] == preset["agents"] + stopped["agents"]
        before = destination.read_bytes()
        run("agent", "import", path, success=False)
        assert destination.read_bytes() == before

        # Tracked TOML declarations reserve names even when active TSV is empty.
        assert inventory.read_text() == ""
        existing = tomllib.loads(run("agent", "template", "zeroclaw", "new-preset").stdout)
        denied = run("agent", "plan", candidate(existing), "--json", success=False)
        assert "agent identity conflicts with existing configuration or active inventory" in denied.stderr
        assert destination.read_bytes() == before

        # Distinct stopped names cannot share or contain state reserved by
        # desired declarations, even when the active inventory has no entries.
        reserved = tomllib.loads(run("agent", "template", "hermes", "desired-only").stdout)
        reserved_state = "/var/lib/tentaflake/agents/reserved-state/inner"
        reserved["agents"][0]["stateDir"] = reserved_state
        desired = tomllib.loads(before.decode())
        desired["agents"].extend(reserved["agents"])
        reserved_bytes = toml_document(desired).encode()
        destination.write_bytes(reserved_bytes)
        try:
            for regression, state_dir in (
                ("shared-state", "/var/lib/tentaflake/agents/new-preset"),
                ("nested-state", "/var/lib/tentaflake/agents/new-preset/child"),
                ("shared-desired-state", reserved_state),
                ("nested-desired-state", f"{reserved_state}/child"),
                ("parent-desired-state", reserved_state.rsplit("/", 1)[0]),
                ("shared-desired-workspace", f"{reserved_state}/workspace"),
            ):
                item = tomllib.loads(run("agent", "template", "hermes", regression).stdout)
                item["agents"][0]["stateDir"] = state_dir
                path = candidate(item)
                denied = json.loads(run("agent", "plan", path, "--json", success=False).stdout)
                assert not denied["valid"], regression
                assert any("paths must not overlap" in error for error in denied["errors"]), regression
                assert destination.read_bytes() == reserved_bytes, regression
                run("agent", "import", path, success=False)
                assert destination.read_bytes() == reserved_bytes, regression
                assert not (flake / "tentaflake.toml.pending").exists(), regression
        finally:
            destination.write_bytes(before)

        # Host-owned unmanaged OCI containers also reserve identities, even
        # without any matching declaration in TOML or the active TSV inventory.
        unmanaged = tomllib.loads(run("agent", "template", "pi", "unmanaged-fixture").stdout)
        denied = json.loads(run("agent", "plan", candidate(unmanaged), "--json", success=False).stdout)
        assert not denied["valid"] and "already exists" in denied["errors"][0]
        assert destination.read_bytes() == before

        # A removed Nix inventory must refuse without evaluating its contents
        # or publishing any change to the sole supported TOML inventory.
        obsolete = flake / "my-agents.nix"
        obsolete.write_text('throw "obsolete-evaluation-private-sentinel"\n')
        subprocess.run(["git", "-C", str(flake), "add", "my-agents.nix"], check=True)
        try:
            evaluation = subprocess.run(
                ["nix", "eval", "--no-write-lock-file",
                 f"{flake}#nixosConfigurations.{hostname}.config.tentaflake.agentInstances",
                 "--json"], capture_output=True, text=True, timeout=75)
            assert evaluation.returncode != 0
            assert (
                "error: tentaflake: manually migrate my-agents.nix to tentaflake.toml "
                "and remove the legacy file."
            ) in " ".join(evaluation.stderr.split())
            assert "obsolete-evaluation-private-sentinel" not in evaluation.stdout + evaluation.stderr
            fresh_legacy = tomllib.loads(run("agent", "template", "hermes", "legacy-refusal").stdout)
            result = run("agent", "import", candidate(fresh_legacy), success=False)
            assert "obsolete-evaluation-private-sentinel" not in result.stdout + result.stderr
            assert destination.read_bytes() == before
            assert obsolete.read_text() == 'throw "obsolete-evaluation-private-sentinel"\n'
        finally:
            obsolete.unlink()
            subprocess.run(["git", "-C", str(flake), "add", "-u", "my-agents.nix"], check=True)

        # JSON inventories are obsolete, including when a TOML inventory exists.
        # The CLI minimizes filenames; require the actual refusal and no paths.
        legacy_refusal = ("legacy inventory exists; explicitly migrate and archive it "
                          "before onboarding; no files changed")
        legacy_json = flake / "agents.json"
        legacy_bytes = b'{"schemaVersion":1,"agents":[]}'
        legacy_json.write_bytes(legacy_bytes)
        try:
            result = run("agent", "import", candidate(fresh_legacy), success=False)
            assert legacy_refusal in result.stderr
            assert str(flake) not in result.stdout + result.stderr
            assert legacy_json.read_bytes() == legacy_bytes
            assert destination.read_bytes() == before
            destination.unlink()
            result = run("agent", "import", candidate(fresh_legacy), success=False)
            assert legacy_refusal in result.stderr
            assert str(flake) not in result.stdout + result.stderr
            assert legacy_json.read_bytes() == legacy_bytes
            assert not destination.exists()
        finally:
            legacy_json.unlink()
            destination.write_bytes(before)

        # Legacy TOML refuses both alongside the new source and on its own.
        legacy_toml = flake / "agents.toml"
        legacy_toml.write_bytes(before)
        try:
            for present in (True, False):
                if not present:
                    destination.unlink()
                result = run("agent", "import", candidate(fresh_legacy), success=False)
                assert legacy_refusal in result.stderr
                assert str(flake) not in result.stdout + result.stderr
                assert legacy_toml.read_bytes() == before
                assert destination.exists() == present
                if present:
                    assert destination.read_bytes() == before
        finally:
            legacy_toml.unlink()
            destination.write_bytes(before)

        # Missing desired source must never become an empty inventory fallback.
        destination.unlink()
        try:
            run("agent", "plan", candidate(fresh_legacy), success=False)
            run("agent", "import", candidate(fresh_legacy), success=False)
            assert not destination.exists()
        finally:
            destination.write_bytes(before)

        # The old public input format must refuse, not silently convert.
        old_input = work / "incoming.json"
        old_input.write_text(json.dumps(fresh_legacy))
        run("agent", "validate", str(old_input), success=False)
        run("agent", "import", str(old_input), success=False)
        assert destination.read_bytes() == before

        invalid = [
            {**stopped, "schemaVersion": 2},
            {"hermes": []},
            {**stopped, "zeroclaw": []},
            {**stopped, "unknown": "refuse"},
            {"schemaVersion": 1, "agents": [
                {"adapter": "hermes", "name": "duplicate", "autoStart": False},
                {"adapter": "zeroclaw", "name": "duplicate", "autoStart": False}]},
            {"schemaVersion": 1, "agents": [{"adapter": "hermes", "name": "unsafe", "autoStart": True}]},
            {"schemaVersion": 1, "agents": [{"adapter": "hermes", "name": "unsafe", "autoStart": False,
                                              "extraVolumes": ["/home:/host:rw"]}]},
            {"schemaVersion": 1, "agents": [{"adapter": "hermes", "name": "unsafe", "autoStart": False,
                                              "settings": {"API_KEY": "credential-private-sentinel"}}]},
        ]
        for field, value in (("command", ["unsafe"]), ("definition", {"image": image}),
                             ("extraVolumes", ["/home:/host:rw"]), ("settings", {})):
            item = json.loads(json.dumps(stopped))
            item["agents"][0]["name"] = "invalid-coding"
            item["agents"][0][field] = value
            invalid.append(item)
        for change in ({"image": "example.invalid/agent:latest"}, {"capabilities": ["research"]},
                       {"ownership": {"uid": 0, "gid": 10000}}, {"volumes": ["/home:/host:rw"]}):
            item = json.loads(json.dumps(stopped))
            item["agents"][0]["name"] = "invalid-coding"
            item["agents"][0]["definition"].update(change)
            invalid.append(item)
        for item in invalid:
            denied = run("agent", "import", candidate(item), success=False)
            assert "credential-private-sentinel" not in denied.stdout + denied.stderr
            assert destination.read_bytes() == before

        # Invalid existing roots must remain byte-for-byte untouched.
        for invalid_existing in ({"hermes": []}, {**stopped, "unknown": True}):
            original = toml_document(invalid_existing).encode()
            destination.write_bytes(original)
            run("agent", "import", candidate(preset), success=False)
            assert destination.read_bytes() == original
        destination.write_bytes(before)

        # Unknown adapters in active inventory still reserve their common identity.
        inventory.write_text("future-runtime\treserved\tagent-reserved\tcustom.service\t/custom/state\ttrue\n")
        collision = tomllib.loads(run("agent", "template", "pi", "reserved").stdout)
        run("agent", "import", candidate(collision), success=False)
        assert destination.read_bytes() == before
        inventory.write_text("")

        # Do not echo a malformed secret-like value in TOML/Nix diagnostics.
        bad = work / "bad.toml"
        bad.write_text('credential-private-sentinel = invalid')
        result = run("agent", "validate", str(bad), success=False)
        assert "credential-private-sentinel" not in result.stdout + result.stderr

        with (flake / ".tentaflake.toml.lock").open("r+") as lock:
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
            fresh = tomllib.loads(run("agent", "template", "zeroclaw", "after-lock").stdout)
            result = run("agent", "import", candidate(fresh), success=False)
            assert "another agent import" in result.stderr
            assert destination.read_bytes() == before

        # Inject a concurrent operator edit at the subprocess boundary. This
        # tests publication conflict handling, not vendor/runtime acceptance.
        tool_dir = work / "tools"
        tool_dir.mkdir()
        validator = tool_dir / "nix"
        manual = b'schemaVersion = 1\nagents = []\n_securityNote = "operator edit"\n'
        validator.write_text(f"#!{sys.executable}\nfrom pathlib import Path\nimport json\n"
                             f"Path({str(destination)!r}).write_bytes({manual!r})\n"
                             "print(json.dumps({'schemaVersion':1,'valid':True,'instances':[],'errors':[]}))\n")
        validator.chmod(0o700)
        result = run("agent", "import", candidate(fresh), success=False,
                     env={**environment, "PATH": f"{tool_dir}:{environment['PATH']}"})
        assert "changed during validation" in result.stderr
        assert destination.read_bytes() == manual
        assert not (flake / "tentaflake.toml.pending").exists()
        print("PASS: preset/generic plan/import, state isolation, missing/legacy source refusal, redaction, locks and conflicts")


if __name__ == "__main__":
    main()
