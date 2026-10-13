#!/usr/bin/env python3
"""Exercise generated quota helpers on private temporary files, without mounts."""

from pathlib import Path
import re
import shlex
import subprocess
import sys
import tempfile


def run(
    helpers: dict[str, Path],
    kind: str,
    root: Path,
    expected: str,
    evidence: str | None = None,
    foreign_owner: Path | None = None,
) -> None:
    tools = root.parent / "queries"
    tools.mkdir()
    metadata = tools / "image-metadata"
    # Model host UID0 without privileges. Permission bits are real, every query
    # stays inside the virtual host, and foreign ownership has a refusal case.
    metadata.write_text(
        f"#!{sys.executable}\n"
        "import os, stat, sys\nfrom pathlib import Path\n"
        f"root = Path({str(root)!r})\n"
        f"foreign = {str(foreign_owner) if foreign_owner else None!r}\n"
        "path = Path(sys.argv[1])\n"
        "assert path.is_relative_to(root), path\n"
        "metadata = path.stat()\n"
        "assert metadata.st_uid == os.geteuid(), path\n"
        "uid = 65534 if str(path) == foreign else 0\n"
        "print(f'{uid}:{stat.S_IMODE(metadata.st_mode):o}')\n"
    )
    metadata.chmod(0o700)

    def replace_once(script: str, old: str, new: str) -> str:
        assert script.count(old) == 1, f"expected one helper boundary: {old}"
        return script.replace(old, new, 1)

    def query(script: str, name: str, value: str) -> str:
        executable = tools / name
        executable.write_text(f"#!{sys.executable}\nprint({value!r})\n")
        executable.chmod(0o700)
        script, count = re.subn(rf"/nix/store/[^\s]+/bin/{name}", str(executable), script)
        assert count == 1, f"expected one pinned {name} query"
        return script

    def relocate(kind: str) -> str:
        script = helpers[kind].read_text().replace("/var/lib", str(root / "var/lib"))
        if kind != "owner":
            script = replace_once(script, "stat -c '%u:%a'", shlex.quote(str(metadata)))
            # This private directory represents /. Check every actual ancestor
            # inside it; the test runner's /tmp is outside the virtual host.
            script = replace_once(
                script,
                '[ "$image_parent" = / ] && break',
                f'[ "$image_parent" = {shlex.quote(str(root))} ] && break',
            )
        if evidence == "bind":
            # A ReadWritePaths bind is not the managed loop image.
            script = query(script, "findmnt", "/dev/vda[ordinary-directory]")
        elif evidence == "wrong-size":
            # Only mount metadata is supplied. The real helper must inspect the
            # temporary image length before privileged ownership changes.
            script = query(script, "findmnt", "/dev/loop999999")
            script = query(
                script, "losetup", str(root / "var/lib/tentaflake-workspace-volumes/state/hermes-quota.img")
            )
        return script

    relocated = relocate(kind)
    if kind == "create":
        verify = tools / "verify"
        verify.write_text(relocate("verify"))
        verify.chmod(0o700)
        relocated = replace_once(
            relocated, f"exec {helpers['verify']}", f"exec {shlex.quote(str(verify))}"
        )
    result = subprocess.run(
        ["bash", "-eu", "-c", relocated], capture_output=True, text=True, timeout=15
    )
    assert result.returncode != 0, "unsafe source was accepted"
    assert expected in result.stderr, result.stderr


def check(helpers: dict[str, Path], kind: str, case: str, expected: str) -> None:
    with tempfile.TemporaryDirectory(prefix="tentaflake-quota-") as temporary:
        root = Path(temporary) / "host"
        root.mkdir(mode=0o700)
        target = "hermes-quota" if kind == "owner" else "generic-quota/state"
        source = root / "var/lib" / target
        other = root / "unrelated"
        other.mkdir(mode=0o755)
        marker = other / "marker"
        marker.write_text("preserve unrelated data\n")
        before = (other.stat().st_mode, marker.read_bytes())
        source.parent.mkdir(parents=True)
        image = root / f"var/lib/tentaflake-workspace-volumes/state/{target.split('/')[0]}.img"
        staging = image.with_suffix(".img.new")
        if case == "source-link":
            source.symlink_to(other, target_is_directory=True)
        elif case == "source-file":
            source.write_text("keep this file\n")
        elif case == "ancestor-link":
            source.parent.rmdir()
            source.parent.symlink_to(other, target_is_directory=True)
            (other / source.name).mkdir(mode=0o755)
        elif case != "verify-missing":
            source.mkdir()
            if case == "child-link":
                (source / "skills").symlink_to(other, target_is_directory=True)
            elif case in {"nonempty", "nonempty-existing-image", "namespace-bind"}:
                (source / "data").write_text("existing state\n")
                if case in {"nonempty-existing-image", "namespace-bind"}:
                    image.parent.mkdir(parents=True)
                    with image.open("wb") as output:
                        output.truncate(128 * 1024 * 1024)
            elif case == "worker-scaffold":
                (source / ".tentaflake-worker/inbox").mkdir(parents=True)
            elif case == "incomplete-image":
                image.parent.mkdir(parents=True)
                staging.write_bytes(b"incomplete")
            elif case in {"foreign-owner", "writable-ancestor"}:
                image.parent.mkdir(parents=True)
                if case == "writable-ancestor":
                    image.parent.parent.chmod(0o777)
            elif case in {"image-link", "image-directory", "wrong-size", "wrong-format", "owner-wrong-size", "existing-image", "final-and-staging"}:
                image.parent.mkdir(parents=True)
                if case == "image-link":
                    image.symlink_to(marker)
                elif case == "image-directory":
                    image.mkdir()
                else:
                    with image.open("wb") as output:
                        output.write(b"not a Btrfs image")
                        output.truncate(128 * 1024 * 1024 if case in {"wrong-format", "existing-image", "final-and-staging"} else 1024)
                if case == "final-and-staging":
                    staging.write_bytes(b"incomplete")
        evidence = "bind" if case == "namespace-bind" else "wrong-size" if case == "owner-wrong-size" else None
        paths_before = set(root.rglob("*"))
        run(helpers, kind, root, expected, evidence=evidence, foreign_owner=image.parent if case == "foreign-owner" else None)
        assert (other.stat().st_mode, marker.read_bytes()) == before, case
        if case in {"verify-missing", "existing-image", "final-and-staging", "foreign-owner", "writable-ancestor"}:
            assert set(root.rglob("*")) == paths_before, f"{case}: refusal changed paths"
        if case == "ancestor-link":
            assert (other / source.name).stat().st_mode & 0o777 == 0o755, case
        if case == "verify-missing":
            assert not source.exists() and not image.exists() and not staging.exists()
        if case == "nonempty":
            assert (source / "data").read_text() == "existing state\n"
            assert not image.exists()
        if case in {"nonempty-existing-image", "namespace-bind"}:
            assert (source / "data").read_text() == "existing state\n"
            assert image.stat().st_size == 128 * 1024 * 1024
        if case == "worker-scaffold":
            assert not image.exists(), "state must not tolerate workspace scaffolding"
        if case in {"incomplete-image", "final-and-staging"}:
            assert staging.read_bytes() == b"incomplete", case
        if case in {"wrong-size", "wrong-format", "owner-wrong-size", "existing-image", "final-and-staging"}:
            with image.open("rb") as output:
                assert output.read(17) == b"not a Btrfs image", case
            assert image.stat().st_size == (128 * 1024 * 1024 if case in {"wrong-format", "existing-image", "final-and-staging"} else 1024)


def main() -> None:
    helpers = dict(zip(["create", "verify", "owner"], map(Path, sys.argv[1:]), strict=True))
    cases = [
        ("create", "source-link", "refusing symlink"),
        ("create", "ancestor-link", "refusing symlink"),
        ("owner", "source-link", "refusing symlink"),
        ("owner", "source-file", "refusing symlink or non-directory"),
        ("owner", "child-link", "refusing symlink"),
        ("create", "nonempty", "refusing to hide non-empty state"),
        ("verify", "nonempty-existing-image", "refusing to hide non-empty state"),
        ("verify", "namespace-bind", "refusing to hide non-empty state"),
        ("create", "worker-scaffold", "refusing to hide non-empty state"),
        ("verify", "image-link", "image is not a regular file"),
        ("verify", "image-directory", "image is not a regular file"),
        ("create", "incomplete-image", "incomplete image exists"),
        ("verify", "wrong-size", "state image size drift"),
        ("verify", "wrong-format", "state image is not Btrfs"),
        ("owner", "owner-wrong-size", "state image size drift"),
        ("verify", "verify-missing", "state image is missing"),
        ("verify", "incomplete-image", "incomplete image exists"),
        ("verify", "final-and-staging", "incomplete image exists"),
        ("create", "final-and-staging", "incomplete image exists"),
        ("create", "existing-image", "refusing to recreate existing state image"),
        ("create", "image-link", "refusing to recreate existing state image"),
        ("create", "image-directory", "refusing to recreate existing state image"),
        ("create", "foreign-owner", "image directory must be root-owned"),
        ("verify", "writable-ancestor", "image directory must be root-owned"),
    ]
    for kind, case, expected in cases:
        check(helpers, kind, case, expected)
    print(f"{len(cases)} generated quota helper refusal checks passed")


if __name__ == "__main__":
    main()
