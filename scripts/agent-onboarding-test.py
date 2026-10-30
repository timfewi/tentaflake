#!/usr/bin/env python3
"""Exercise installed-host imports with the actual CLI and shared Nix validator."""
import fcntl
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile


def main():
    binary, flake_dir, hostname = sys.argv[1:]
    flake = Path(flake_dir)
    destination = flake / "agents.json"
    with tempfile.TemporaryDirectory() as temporary:
        work = Path(temporary)
        config = work / "cli.conf"
        inventory = work / "agents.tsv"
        inventory.write_text("")
        config.write_text(f"backend=docker\nflake_dir={flake}\nhost_name={hostname}\nagents_file={inventory}\nsecurity_profile=balanced\n")
        environment = {**os.environ, "TENTAFLAKE_CONFIG": str(config)}

        def run(*args, success=True, env=None):
            result = subprocess.run([binary, *args], env=env or environment,
                                    capture_output=True, text=True, timeout=75)
            assert (result.returncode == 0) == success, (args, result.stdout, result.stderr)
            return result

        def candidate(value):
            path = work / "incoming.json"
            path.write_text(json.dumps(value))
            return str(path)

        preset = json.loads(run("agent", "template", "hermes", "new-preset").stdout)
        path = candidate(preset)
        before = destination.read_bytes()
        plan = json.loads(run("agent", "plan", path, "--json").stdout)
        assert plan["valid"] and not plan["imported"]
        assert plan["instances"][0]["vendorAcceptance"] == "pending"
        assert destination.read_bytes() == before
        run("agent", "validate", path)
        run("agent", "import", path)
        combined = json.loads(destination.read_bytes())
        assert combined["agents"] == preset["agents"]
        assert set(combined) == {"schemaVersion", "agents"}

        image = "example.invalid/coding@sha256:" + "a" * 64
        generic = json.loads(run("agent", "template", "generic", "new-coding", "--image", image,
                                 "--", "fixture", "--json", "--hide").stdout)
        assert generic["agents"][0]["definition"]["command"] == ["fixture", "--json", "--hide"]
        path = candidate(generic)
        hidden = run("agent", "plan", path, "--hide", "--json").stdout
        assert "new-coding" not in hidden and hostname not in hidden and str(flake) not in hidden
        assert json.loads(hidden)["valid"]
        run("agent", "import", path)
        assert json.loads(destination.read_bytes())["agents"] == preset["agents"] + generic["agents"]
        before = destination.read_bytes()
        run("agent", "import", path, success=False)
        assert destination.read_bytes() == before

        # Tracked JSON declarations reserve names even when active TSV is empty.
        assert inventory.read_text() == ""
        existing = json.loads(run("agent", "template", "zeroclaw", "new-preset").stdout)
        denied = run("agent", "plan", candidate(existing), "--json", success=False)
        assert "agent identity conflicts with existing configuration or active inventory" in denied.stderr
        assert destination.read_bytes() == before

        # Host-owned unmanaged OCI containers also reserve identities, even
        # without any matching declaration in JSON or the active TSV inventory.
        unmanaged = json.loads(run("agent", "template", "pi", "unmanaged-fixture").stdout)
        denied = json.loads(run("agent", "plan", candidate(unmanaged), "--json", success=False).stdout)
        assert not denied["valid"] and "already exists" in denied["errors"][0]
        assert destination.read_bytes() == before

        # A removed Nix inventory must refuse without evaluating its contents
        # or publishing any change to the sole supported JSON inventory.
        obsolete = flake / "my-agents.nix"
        obsolete.write_text('throw "obsolete-evaluation-private-sentinel"\n')
        subprocess.run(["git", "-C", str(flake), "add", "my-agents.nix"], check=True)
        try:
            evaluation = subprocess.run(
                ["nix", "eval", "--no-write-lock-file",
                 f"{flake}#nixosConfigurations.{hostname}.config.tentaflake.agentInstances",
                 "--json"], capture_output=True, text=True, timeout=75)
            assert evaluation.returncode != 0
            assert "Manually migrate declarations to agents.json" in evaluation.stderr
            assert "obsolete-evaluation-private-sentinel" not in evaluation.stdout + evaluation.stderr
            fresh_legacy = json.loads(run("agent", "template", "hermes", "legacy-refusal").stdout)
            result = run("agent", "import", candidate(fresh_legacy), success=False)
            assert "obsolete-evaluation-private-sentinel" not in result.stdout + result.stderr
            assert destination.read_bytes() == before
            assert obsolete.read_text() == 'throw "obsolete-evaluation-private-sentinel"\n'
        finally:
            obsolete.unlink()
            subprocess.run(["git", "-C", str(flake), "add", "-u", "my-agents.nix"], check=True)

        invalid = [
            {**generic, "schemaVersion": 2},
            {"hermes": []},
            {**generic, "zeroclaw": []},
            {**generic, "unknown": "refuse"},
            {"schemaVersion": 1, "agents": [
                {"adapter": "hermes", "name": "duplicate", "autoStart": False},
                {"adapter": "zeroclaw", "name": "duplicate", "autoStart": False}]},
            {"schemaVersion": 1, "agents": [{"adapter": "hermes", "name": "unsafe", "autoStart": True}]},
            {"schemaVersion": 1, "agents": [{"adapter": "hermes", "name": "unsafe", "autoStart": False,
                                              "extraVolumes": ["/home:/host:rw"]}]},
            {"schemaVersion": 1, "agents": [{"adapter": "hermes", "name": "unsafe", "autoStart": False,
                                              "settings": {"API_KEY": "credential-private-sentinel"}}]},
        ]
        for change in ({"image": "example.invalid/agent:latest"}, {"capabilities": ["research"]},
                       {"ownership": {"uid": 0, "gid": 10000}}, {"volumes": ["/home:/host:rw"]}):
            item = json.loads(json.dumps(generic))
            item["agents"][0]["name"] = "invalid-coding"
            item["agents"][0]["definition"].update(change)
            invalid.append(item)
        for item in invalid:
            denied = run("agent", "import", candidate(item), success=False)
            assert "credential-private-sentinel" not in denied.stdout + denied.stderr
            assert destination.read_bytes() == before

        # Invalid existing roots must remain byte-for-byte untouched.
        for invalid_existing in ({"hermes": []}, {**generic, "unknown": True}):
            original = json.dumps(invalid_existing).encode()
            destination.write_bytes(original)
            run("agent", "import", candidate(preset), success=False)
            assert destination.read_bytes() == original
        destination.write_bytes(before)

        # Unknown adapters in active inventory still reserve their common identity.
        inventory.write_text("future-runtime\treserved\tagent-reserved\tcustom.service\t/custom/state\ttrue\n")
        collision = json.loads(run("agent", "template", "pi", "reserved").stdout)
        run("agent", "import", candidate(collision), success=False)
        assert destination.read_bytes() == before
        inventory.write_text("")

        # Do not echo a malformed secret-like value in JSON/Nix diagnostics.
        bad = work / "bad.json"
        bad.write_text('{"credential-private-sentinel": invalid}')
        result = run("agent", "validate", str(bad), success=False)
        assert "credential-private-sentinel" not in result.stdout + result.stderr

        with (flake / ".agents.json.lock").open("r+") as lock:
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
            fresh = json.loads(run("agent", "template", "zeroclaw", "after-lock").stdout)
            result = run("agent", "import", candidate(fresh), success=False)
            assert "another agent import" in result.stderr
            assert destination.read_bytes() == before

        # Inject a concurrent operator edit at the subprocess boundary. This
        # tests publication conflict handling, not vendor/runtime acceptance.
        tool_dir = work / "tools"
        tool_dir.mkdir()
        validator = tool_dir / "nix"
        manual = b'{"schemaVersion":1,"agents":[],"_securityNote":"operator edit"}'
        validator.write_text(f"#!{sys.executable}\nfrom pathlib import Path\nimport json\n"
                             f"Path({str(destination)!r}).write_bytes({manual!r})\n"
                             "print(json.dumps({'schemaVersion':1,'valid':True,'instances':[],'errors':[]}))\n")
        validator.chmod(0o700)
        result = run("agent", "import", candidate(fresh), success=False,
                     env={**environment, "PATH": f"{tool_dir}:{environment['PATH']}"})
        assert "changed during validation" in result.stderr
        assert destination.read_bytes() == manual
        assert not (flake / "agents.json.pending").exists()
        print("PASS: installed preset/generic plan, import, redaction, refusals, lock and conflict recovery")


if __name__ == "__main__":
    main()
