#!@python@/bin/python3
"""Start the native server; the Generic capsule owns isolation and credentials.

Provider keys follow official configuration documentation and the pinned binary's
effective config response. Native model/tool/session acceptance is a separate gate.
"""

import ipaddress
import json
import os
from pathlib import Path
import re
import stat
import sys
from urllib.parse import urlsplit


CODEX = "@codex@/bin/codex"
PRIVATE_NETWORKS = tuple(
    ipaddress.IPv4Network(value)
    for value in ("10.0.0.0/8", "172.16.0.0/12", "192.168.0.0/16")
)


def broker_url(environment):
    """Validate the existing four-key model transport without exposing its token."""
    token = environment.get("OPENAI_API_KEY", "")
    value = environment.get("OPENAI_BASE_URL", "")
    try:
        url = urlsplit(value)
        address = ipaddress.IPv4Address(url.hostname or "")
        port = url.port
    except ValueError:
        raise ValueError("invalid broker environment") from None
    if (
        re.fullmatch(r"[0-9a-f]{64}", token) is None
        or environment.get("TENTAFLAKE_BROKER_TOKEN") != token
        or environment.get("TENTAFLAKE_LLM_BASE_URL") != value
        or not any(address in network for network in PRIVATE_NETWORKS)
        or port is None
        or not 1 <= port <= 65535
        or value != f"http://{address}:{port}/v1"
    ):
        raise ValueError("invalid broker environment")
    return value


def private_child(parent, name):
    """Preserve existing private state; never follow a replacement symlink."""
    parent_fd = os.open(parent, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
    try:
        try:
            os.mkdir(name, mode=0o700, dir_fd=parent_fd)
        except FileExistsError:
            pass
        child = os.open(
            name, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW, dir_fd=parent_fd
        )
    finally:
        os.close(parent_fd)
    metadata = os.fstat(child)
    if metadata.st_uid != os.geteuid() or stat.S_IMODE(metadata.st_mode) != 0o700:
        os.close(child)
        raise ValueError("state directory is not private and owned")
    return child


def prepare(environment, model, state, temporary):
    """Internal path arguments let filesystem tests use owned temporary fixtures."""
    if re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9._:/-]{0,127}", model) is None:
        raise ValueError("an explicit model identifier is required")
    url = broker_url(environment)
    home = state / ".codex"
    runtime = temporary / "tentaflake-codex"
    os.close(private_child(state, ".codex"))
    directory = private_child(temporary, "tentaflake-codex")
    try:
        try:
            os.stat("server.sock", dir_fd=directory, follow_symlinks=False)
        except FileNotFoundError:
            pass
        else:
            # A fresh container has a fresh tmpfs. Never unlink another server
            # or pretend that an old socket proves its former process stopped.
            raise ValueError("server socket already exists")
    finally:
        os.close(directory)
    settings = {
        "model": model,
        "model_provider": "tentaflake",
        "model_providers.tentaflake.name": "Tentaflake broker",
        "model_providers.tentaflake.base_url": url,
        "model_providers.tentaflake.env_key": "OPENAI_API_KEY",
        "model_providers.tentaflake.wire_api": "responses",
    }
    arguments = [CODEX]
    for key, value in settings.items():
        arguments.extend(["-c", f"{key}={json.dumps(value)}"])
    arguments.extend(
        ["app-server", "--strict-config", "--listen", f"unix://{runtime}/server.sock"]
    )
    child_environment = dict(environment, HOME=str(state), CODEX_HOME=str(home))
    return arguments, child_environment


def main():
    try:
        if len(sys.argv) != 2 or os.geteuid() == 0 or os.environ.get("HOME") != "/state":
            raise ValueError("unsupported invocation")
        os.umask(0o077)
        arguments, environment = prepare(
            os.environ, sys.argv[1], Path("/state"), Path("/tmp")
        )
        os.execve(CODEX, arguments, environment)
    except (OSError, ValueError):
        # Neither environment values, filesystem paths nor native arguments are
        # needed to diagnose a wrapper refusal. Native stderr belongs to Codex.
        print("Codex reference startup refused", file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
