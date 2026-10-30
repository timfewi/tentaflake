use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt, symlink};
use std::path::PathBuf;
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(0);
const ARCHIVE: &str = "tentaflake-backup-fixture-20261030T000000Z.tar.gz";

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "tentaflake-backup-{}-{}-{}",
            std::process::id(),
            NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&root).unwrap();
        fs::create_dir(root.join("state")).unwrap();
        fs::create_dir(root.join("bin")).unwrap();
        let locate = |name: &str| {
            let output = Command::new("sh")
                .args(["-c", &format!("command -v {name}")])
                .output()
                .unwrap();
            assert!(output.status.success());
            String::from_utf8(output.stdout).unwrap().trim().to_owned()
        };
        let shell = locate("sh");
        for name in ["stat", "mv", "ln"] {
            symlink(locate(name), root.join("bin").join(name)).unwrap();
        }
        for (name, script) in [
            ("date", "printf '20261030T000000Z\\n'"),
            (
                "sudo",
                r#"printf '%s\n' "$*" >> "$FIXTURE_ROOT/sudo.log"
[ "$1" = tar ] || exit 91
exec "$@""#,
            ),
            (
                "tar",
                r#"[ "$1" = czf ] || exit 92
# Also emulate the old pathname-opening tar invocation for baseline reproduction.
if [ "$2" != - ]; then exec > "$2"; fi
stat -Lc '%a' /proc/self/fd/3 3>&1 > "$FIXTURE_ROOT/mode"
printf 'synthetic-private-state\n'
if [ "$FIXTURE_REPLACE" = 1 ]; then
  mv "$FIXTURE_ARCHIVE" "$FIXTURE_ARCHIVE.moved"
  ln -s "$FIXTURE_ROOT/target" "$FIXTURE_ARCHIVE"
fi
printf 'synthetic-final-bytes\n'
exit "$FIXTURE_TAR_EXIT""#,
            ),
        ] {
            let path = root.join("bin").join(name);
            fs::write(&path, format!("#!{shell}\n{script}\n")).unwrap();
            fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
        }
        fs::write(
            root.join("agents.tsv"),
            format!(
                "hermes\tbackup-fixture\tbackup-fixture\tbackup-fixture.service\t{}\ttrue\n",
                root.join("state").display()
            ),
        )
        .unwrap();
        fs::write(
            root.join("cli.conf"),
            format!(
                "backend=docker\nflake_dir=/tmp\nhost_name=fixture-host\nagents_file={}\n",
                root.join("agents.tsv").display()
            ),
        )
        .unwrap();
        Self(root)
    }

    fn run(&self, exit: &str, replace: bool) -> Output {
        Command::new(env!("CARGO_BIN_EXE_tentaflake"))
            .env_clear()
            .env("PATH", self.0.join("bin"))
            .env("TENTAFLAKE_CONFIG", self.0.join("cli.conf"))
            .env("FIXTURE_ROOT", &self.0)
            .env("FIXTURE_ARCHIVE", self.0.join(ARCHIVE))
            .env("FIXTURE_TAR_EXIT", exit)
            .env("FIXTURE_REPLACE", if replace { "1" } else { "0" })
            .current_dir(&self.0)
            .args(["backup", "backup-fixture"])
            .output()
            .unwrap()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn backup_refuses_existing_files_and_symlinks_before_sudo() {
    for link in [false, true] {
        let fixture = Fixture::new();
        let target = fixture.0.join("target");
        fs::write(&target, "untouched").unwrap();
        let archive = fixture.0.join(ARCHIVE);
        if link {
            symlink(&target, &archive).unwrap();
        } else {
            fs::write(&archive, "untouched").unwrap();
        }
        let output = fixture.run("0", false);
        assert!(!output.status.success());
        assert_eq!(fs::read_to_string(target).unwrap(), "untouched");
        assert_eq!(fs::read_to_string(archive).unwrap(), "untouched");
        assert!(!fixture.0.join("sudo.log").exists());
    }
}

#[test]
fn backup_is_private_before_writing_and_never_passes_output_path_to_sudo() {
    let fixture = Fixture::new();
    let output = fixture.run("0", false);
    assert!(output.status.success(), "{output:?}");
    assert_eq!(fs::read_to_string(fixture.0.join("mode")).unwrap(), "600\n");
    let archive = fs::metadata(fixture.0.join(ARCHIVE)).unwrap();
    assert_eq!(archive.permissions().mode() & 0o777, 0o600);
    assert_eq!(archive.uid(), fs::metadata(&fixture.0).unwrap().uid());
    assert_eq!(
        fs::read_to_string(fixture.0.join("sudo.log")).unwrap(),
        format!("tar czf - -C {} state\n", fixture.0.display())
    );
}

#[test]
fn failed_backup_retains_private_partial_output() {
    let fixture = Fixture::new();
    let output = fixture.run("7", false);
    assert_eq!(output.status.code(), Some(7));
    let archive = fixture.0.join(ARCHIVE);
    assert_eq!(
        fs::metadata(&archive).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert!(
        fs::read_to_string(archive)
            .unwrap()
            .contains("synthetic-private-state")
    );
    assert_eq!(fs::read_to_string(fixture.0.join("mode")).unwrap(), "600\n");
    assert!(
        String::from_utf8(output.stderr)
            .unwrap()
            .contains("private partial output retained")
    );
}

#[test]
fn backup_keeps_writing_held_file_and_never_removes_replacement_on_failure() {
    let fixture = Fixture::new();
    fs::write(fixture.0.join("target"), "untouched").unwrap();
    let output = fixture.run("7", true);
    assert_eq!(output.status.code(), Some(7));
    assert_eq!(
        fs::read_to_string(fixture.0.join("target")).unwrap(),
        "untouched"
    );
    assert!(
        fs::symlink_metadata(fixture.0.join(ARCHIVE))
            .unwrap()
            .file_type()
            .is_symlink()
    );
    let moved = fixture.0.join(format!("{ARCHIVE}.moved"));
    assert_eq!(
        fs::metadata(&moved).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert!(
        fs::read_to_string(moved)
            .unwrap()
            .ends_with("synthetic-final-bytes\n")
    );
}

#[test]
fn spawn_failure_retains_only_private_empty_output() {
    let fixture = Fixture::new();
    fs::remove_file(fixture.0.join("bin/sudo")).unwrap();
    let output = fixture.run("0", false);
    assert!(!output.status.success());
    assert!(
        String::from_utf8(output.stderr)
            .unwrap()
            .contains("private output retained")
    );
    let archive = fs::metadata(fixture.0.join(ARCHIVE)).unwrap();
    assert_eq!(archive.len(), 0);
    assert_eq!(archive.permissions().mode() & 0o777, 0o600);
}
