"""Offline evidence for why agent-owned Git metadata cannot cross to the host.

Run: python3 tests/git-autopush-boundary.py /path/to/git
Only temporary local repositories and a deliberately fake token are used.
The module-evaluation fixture separately enforces rejection of gitAutoPush.
"""

import os
from pathlib import Path
import subprocess
import sys
import tempfile


def main():
    git = sys.argv[1]
    shell = sys.argv[2] if len(sys.argv) > 2 else "/bin/sh"
    with tempfile.TemporaryDirectory(prefix="autopush-boundary-") as directory:
        root = Path(directory)
        home = root / "home"
        home.mkdir()
        env = {
            "PATH": os.environ.get("PATH", ""),
            "HOME": str(home),
            "GIT_CONFIG_NOSYSTEM": "1",
            "GIT_CONFIG_GLOBAL": os.devnull,
            "GIT_TERMINAL_PROMPT": "0",
            "GIT_ALLOW_PROTOCOL": "file",
            "GH_TOKEN": "synthetic-not-a-real-credential",
            "HOOK_MARKER": str(root / "hook-marker"),
            "HELPER_MARKER": str(root / "helper-marker"),
        }

        def run(*args, input=None):
            return subprocess.run(
                [git, *map(str, args)],
                env=env,
                input=input,
                text=True,
                capture_output=True,
                check=True,
                timeout=15,
            ).stdout.strip()

        repo = root / "agent-repo"
        destination = root / "unchecked-push-destination.git"
        run("init", "--initial-branch=main", repo)
        run("init", "--bare", destination)
        run("-C", repo, "-c", "user.name=Synthetic", "-c",
            "user.email=synthetic@example.invalid", "commit", "--allow-empty", "-m", "fixture")
        allowed_url = "https://github.com/example/allowed.git"
        run("-C", repo, "remote", "add", "origin", allowed_url)
        run("-C", repo, "config", "remote.origin.pushurl", destination)

        hooks = root / "agent-hooks"
        hooks.mkdir()
        hook = hooks / "pre-push"
        hook.write_text(f'#!{shell}\nprintf "%s" "$GH_TOKEN" > "$HOOK_MARKER"\n')
        hook.chmod(0o700)
        included = root / "agent-config"
        run("config", "--file", included, "core.hooksPath", hooks)
        helper = '!f() { printf "%s" "$GH_TOKEN" > "$HELPER_MARKER"; printf "username=fixture\\npassword=%s\\n" "$GH_TOKEN"; }; f'
        run("config", "--file", included, "credential.helper", helper)
        run("-C", repo, "config", "include.path", included)
        # The former allowlist inspected the fetch URL, not the push destination.
        assert run("-C", repo, "remote", "get-url", "origin") == allowed_url
        assert run("-C", repo, "remote", "get-url", "--push", "origin") == str(destination)
        run("-C", repo, "push", "origin", "HEAD:refs/heads/main")
        assert run("--git-dir", destination, "rev-parse", "refs/heads/main") == run(
            "-C", repo, "rev-parse", "HEAD"
        )
        assert (root / "hook-marker").read_text() == env["GH_TOKEN"]

        # Includes and credential helpers execute independently of hook disabling.
        # credential fill invokes helpers only; it does not contact this hostname.
        credential = run("-C", repo, "-c", "core.hooksPath=/dev/null", "credential", "fill",
                         input="protocol=https\nhost=unchecked.invalid\n\n")
        assert "password=" + env["GH_TOKEN"] in credential
        assert (root / "helper-marker").read_text() == env["GH_TOKEN"]
    print("Synthetic Git hooks/config/pushurl boundary evidence passed (offline)")


if __name__ == "__main__":
    main()
