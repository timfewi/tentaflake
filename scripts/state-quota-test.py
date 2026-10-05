#!/usr/bin/env python3
"""Exercise generated quota helpers on private temporary files, without mounts."""

from pathlib import Path
import subprocess
import sys
import tempfile


def run(script: str, root: Path, expected: str) -> None:
    # Relocate only the public fixture paths, including the guard's stop path.
    relocated = script.replace("/var/lib", str(root / "lib"))
    result = subprocess.run(
        ["bash", "-eu", "-c", relocated], capture_output=True, text=True, timeout=15
    )
    assert result.returncode != 0, "unsafe source was accepted"
    assert expected in result.stderr, result.stderr


def check(script: str, target: str, case: str, expected: str) -> None:
    with tempfile.TemporaryDirectory(prefix="tentaflake-quota-") as temporary:
        root = Path(temporary)
        source = root / "lib" / target
        other = root / "unrelated"
        other.mkdir(mode=0o755)
        marker = other / "marker"
        marker.write_text("preserve unrelated data\n")
        before = (other.stat().st_mode, marker.read_bytes())
        source.parent.mkdir(parents=True)
        image = root / "lib/tentaflake-workspace-volumes/state/generic-quota.img"
        if case == "source-link":
            source.symlink_to(other, target_is_directory=True)
        elif case == "source-file":
            source.write_text("keep this file\n")
        elif case == "ancestor-link":
            source.parent.rmdir()
            source.parent.symlink_to(other, target_is_directory=True)
            (other / source.name).mkdir(mode=0o755)
        else:
            source.mkdir()
            if case == "child-link":
                (source / "skills").symlink_to(other, target_is_directory=True)
            elif case in {"nonempty", "nonempty-existing-image"}:
                (source / "data").write_text("existing state\n")
                if case == "nonempty-existing-image":
                    image.parent.mkdir(parents=True)
                    with image.open("wb") as output:
                        output.truncate(128 * 1024 * 1024)
            elif case == "worker-scaffold":
                (source / ".tentaflake-worker/inbox").mkdir(parents=True)
            elif case == "incomplete-image":
                image.parent.mkdir(parents=True)
                image.with_suffix(".img.new").write_bytes(b"incomplete")
            elif case in {"image-link", "image-directory", "wrong-size", "wrong-format"}:
                image.parent.mkdir(parents=True)
                if case == "image-link":
                    image.symlink_to(marker)
                elif case == "image-directory":
                    image.mkdir()
                else:
                    with image.open("wb") as output:
                        output.write(b"not a Btrfs image")
                        output.truncate(128 * 1024 * 1024 if case == "wrong-format" else 1024)
        run(script, root, expected)
        assert (other.stat().st_mode, marker.read_bytes()) == before, case
        if case == "ancestor-link":
            assert (other / source.name).stat().st_mode & 0o777 == 0o755, case
        if case == "nonempty":
            assert (source / "data").read_text() == "existing state\n"
            assert not image.exists()
        if case == "nonempty-existing-image":
            assert (source / "data").read_text() == "existing state\n"
            assert image.stat().st_size == 128 * 1024 * 1024
        if case == "worker-scaffold":
            assert not image.exists(), "state must not tolerate workspace scaffolding"
        if case in {"wrong-size", "wrong-format"}:
            with image.open("rb") as output:
                assert output.read(17) == b"not a Btrfs image", case
            assert image.stat().st_size == (128 * 1024 * 1024 if case == "wrong-format" else 1024)


def main() -> None:
    prepare, owner = (Path(path).read_text() for path in sys.argv[1:])
    for case in ["source-link", "ancestor-link"]:
        check(prepare, "generic-quota/state", case, "refusing symlink")
    check(owner, "hermes-quota", "source-link", "refusing symlink")
    check(owner, "hermes-quota", "source-file", "refusing symlink or non-directory")
    check(owner, "hermes-quota", "child-link", "refusing symlink")
    for case in ["nonempty", "nonempty-existing-image", "worker-scaffold"]:
        check(prepare, "generic-quota/state", case, "refusing to hide non-empty state")
    for case in ["image-link", "image-directory"]:
        check(prepare, "generic-quota/state", case, "image is not a regular file")
    check(prepare, "generic-quota/state", "incomplete-image", "incomplete image exists")
    check(prepare, "generic-quota/state", "wrong-size", "state image size drift")
    check(prepare, "generic-quota/state", "wrong-format", "state image is not Btrfs")
    print("13 generated quota helper refusal checks passed")


if __name__ == "__main__":
    main()
