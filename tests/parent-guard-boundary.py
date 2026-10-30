"""Exercise the generated parent guard only inside a disposable VM temp tree."""

import os
from pathlib import Path
import stat
import subprocess
import sys
import tempfile


PARENTS = ("var", "var/lib", "var/lib/tentaflake", "var/lib/tentaflake/agents")


def guard(script, root):
    return subprocess.run(
        [script],
        env={**os.environ, "TENTAFLAKE_PARENT_TEST_ROOT": str(root)},
        text=True,
        capture_output=True,
        timeout=5,
        check=False,
    )


def snapshot(directory):
    return {
        str(path.relative_to(directory)): (
            path.lstat().st_uid,
            path.lstat().st_gid,
            stat.S_IMODE(path.lstat().st_mode),
            path.read_bytes() if path.is_file() else None,
        )
        for path in [directory, *directory.rglob("*")]
    }


def main():
    assert os.geteuid() == 0, "Probe requires root inside the disposable VM"
    script = sys.argv[1]
    with tempfile.TemporaryDirectory(prefix="tentaflake-parent-guard-") as temporary:
        root = Path(temporary)
        result = guard(script, root)
        assert result.returncode == 0, result.stderr
        for relative in PARENTS:
            info = (root / relative).stat()
            assert stat.S_ISDIR(info.st_mode)
            assert info.st_uid == 0 and info.st_gid == 0
            assert stat.S_IMODE(info.st_mode) == 0o755

    for relative in PARENTS:
        for kind in ("symlink", "file", "owner", "group-writable", "world-writable"):
            with tempfile.TemporaryDirectory(prefix="tentaflake-parent-guard-") as temporary:
                root = Path(temporary)
                target = root / relative
                target.parent.mkdir(parents=True, exist_ok=True)
                outside = root / "outside"
                outside.mkdir(mode=0o755)
                (outside / "sentinel").write_bytes(b"untouched\n")
                if kind == "symlink":
                    target.symlink_to(outside, target_is_directory=True)
                elif kind == "file":
                    target.write_bytes(b"not a directory\n")
                else:
                    target.mkdir(mode=0o755)
                    if kind == "owner":
                        os.chown(target, 10000, 10000)
                    else:
                        target.chmod(0o775 if kind == "group-writable" else 0o757)
                before = snapshot(outside)
                target_before = target.lstat()
                result = guard(script, root)
                assert result.returncode == 1, (relative, kind, result)
                expected = (
                    "refusing unsafe agent parent"
                    if kind in ("symlink", "file")
                    else "agent parents must be root-owned and not group/world writable"
                )
                assert expected in result.stderr, (relative, kind, result.stderr)
                assert snapshot(outside) == before, (relative, kind)
                target_after = target.lstat()
                assert (target_after.st_uid, target_after.st_gid, target_after.st_mode) == (
                    target_before.st_uid, target_before.st_gid, target_before.st_mode
                ), (relative, kind)
    print("Generated parent guard passed isolated creation and 20 refusal cases")


if __name__ == "__main__":
    main()
