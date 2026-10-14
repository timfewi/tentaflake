use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

struct Fixture {
    root: PathBuf,
    config: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/test-tmp")
            .join(format!(
                "probe-cli-{}-{}",
                std::process::id(),
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
        fs::create_dir_all(root.join("workspace")).unwrap();
        fs::create_dir(root.join("state")).unwrap();
        let root = root.canonicalize().unwrap();
        // Invalid commands used to initialize/chmod private queue directories
        // before rejecting their arguments. Preserve this untouched layout.
        fs::set_permissions(root.join("state"), fs::Permissions::from_mode(0o755)).unwrap();
        let config = root.join("config.json");
        fs::write(&config, serde_json::to_vec(&serde_json::json!({
            "agent": "fixture", "backend": "docker", "runtime": root.join("must-not-execute"),
            "image": "fixture:offline", "workspace": root.join("workspace"),
            "state_dir": root.join("state"),
            "container_uid": unsafe { libc::geteuid() }, "container_gid": unsafe { libc::getegid() },
            "max_request_bytes": 65536, "max_snapshot_bytes": 65536, "max_snapshot_entries": 64,
            "max_log_bytes": 1024, "max_timeout_seconds": 5, "memory": "32m", "memory_swap": "32m",
            "cpus": "1", "pids_limit": 16, "workspace_tmpfs_size": "32m", "tmp_tmpfs_size": "16m"
        })).unwrap()).unwrap();
        Self { root, config }
    }

    fn command(&self, config: &Path, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_tentaflake-worker"))
            .arg("--config")
            .arg(config)
            .args(args)
            .output()
            .unwrap()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn identity(path: &Path) -> (u64, u32, i64, i64, i64, i64) {
    let m = fs::metadata(path).unwrap();
    (
        m.ino(),
        m.mode(),
        m.mtime(),
        m.mtime_nsec(),
        m.ctime(),
        m.ctime_nsec(),
    )
}

#[test]
fn actual_cli_rejects_all_malformed_commands_before_layout_mutation() {
    let f = Fixture::new();
    let state = f.root.join("state");
    let before = identity(&state);
    for args in [
        &["drain", "extra"][..],
        &["approve"],
        &["approve", "../invalid"],
        &["approve", "valid", "extra"],
        &["deny"],
        &["deny", "valid", "extra"],
        &["probe", "extra"],
        &["unknown"],
        &[],
    ] {
        let output = f.command(&f.config, args);
        assert!(!output.status.success(), "unexpected success for {args:?}");
        assert!(output.stdout.is_empty());
        assert_eq!(identity(&state), before);
        assert_eq!(fs::read_dir(&state).unwrap().count(), 0);
        assert_eq!(fs::read_dir(f.root.join("workspace")).unwrap().count(), 0);
    }
}

#[test]
fn malformed_cli_precedes_even_configuration_access() {
    let f = Fixture::new();
    let missing = f.root.join("missing-config.json");
    for args in [
        &["probe", "extra"][..],
        &["approve"],
        &["deny", "../invalid"],
    ] {
        let output = f.command(&missing, args);
        assert!(!output.status.success());
        let error = String::from_utf8(output.stderr).unwrap();
        assert!(
            !error.contains("missing-config"),
            "configuration was opened before parsing: {error}"
        );
        assert!(!missing.exists());
    }
}

#[test]
fn actual_probe_refuses_incomplete_layout_without_repair_or_partial_snapshot() {
    let f = Fixture::new();
    let state = f.root.join("state");
    let before = identity(&state);
    let output = f.command(&f.config, &["probe"]);
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).contains("worker probe"));
    assert_eq!(identity(&state), before);
    assert_eq!(fs::read_dir(&state).unwrap().count(), 0);
    assert_eq!(fs::read_dir(f.root.join("workspace")).unwrap().count(), 0);
}
