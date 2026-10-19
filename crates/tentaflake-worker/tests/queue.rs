use std::fs::{self, File};
use std::os::fd::AsRawFd;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

struct Fixture {
    root: PathBuf,
    config: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/test-tmp")
            .join(format!(
                "queue-cli-{}-{}",
                std::process::id(),
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
        fs::create_dir_all(root.join("workspace/.tentaflake-worker/inbox")).unwrap();
        fs::create_dir(root.join("state")).unwrap();
        let root = root.canonicalize().unwrap();
        let shell = std::env::split_paths(&std::env::var_os("PATH").unwrap())
            .map(|p| p.join("bash"))
            .find(|p| p.is_file())
            .expect("pinned contributor shell contains bash");
        let runtime = root.join("fake-oci");
        fs::write(&runtime, format!("#!{}\nprintf '%s\\n' \"$1\" >> '{}'/calls\ncase \"$1\" in\nps) exit 0;;\ncreate) kill -STOP \"$PPID\"; : > '{}'/stopped; exit 77;;\n*) exit 77;;\nesac\n", shell.display(), root.display(), root.display())).unwrap();
        fs::set_permissions(&runtime, fs::Permissions::from_mode(0o700)).unwrap();
        let config = root.join("config.json");
        // Old generated configurations omit the new capacity fields. The real
        // binary must supply their defaults during upgrade.
        fs::write(&config, serde_json::to_vec(&serde_json::json!({
            "agent": "fixture", "backend": "docker", "runtime": runtime, "image": "fixture:offline",
            "workspace": root.join("workspace"), "state_dir": root.join("state"),
            "container_uid": unsafe { libc::geteuid() }, "container_gid": unsafe { libc::getegid() },
            "max_request_bytes": 65536, "max_snapshot_bytes": 65536, "max_snapshot_entries": 64,
            "max_log_bytes": 1024, "max_timeout_seconds": 5, "memory": "32m", "memory_swap": "32m",
            "cpus": "1", "pids_limit": 16, "workspace_tmpfs_size": "32m", "tmp_tmpfs_size": "16m"
        })).unwrap()).unwrap();
        Self { root, config }
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_tentaflake-worker"));
        command.arg("--config").arg(&self.config).args(args);
        command
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[test]
fn actual_commands_reject_competing_queue_owner_without_mutation() {
    let f = Fixture::new();
    let lock = File::create(f.root.join("state/.queue.lock")).unwrap();
    assert_eq!(
        unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) },
        0
    );
    for args in [&["drain"][..], &["approve", "job_1"], &["deny", "job_1"]] {
        let output = f.command(args).output().unwrap();
        assert!(!output.status.success());
        assert!(String::from_utf8_lossy(&output.stderr).contains("busy"));
    }
    assert_eq!(
        fs::read_dir(f.root.join("state/approvals"))
            .unwrap()
            .count(),
        0
    );
    assert!(!f.root.join("calls").exists());
}

#[test]
fn sigkill_at_dispatch_boundary_preserves_claim_and_restart_does_not_replay() {
    let f = Fixture::new();
    fs::write(f.root.join("workspace/.tentaflake-worker/inbox/job_1.json"),
        br#"{"version":1,"id":"job_1","action_class":"local-reversible","argv":["true"],"timeout_seconds":1}"#).unwrap();
    let mut child = f
        .command(&["drain"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    // The fake OCI process stops only its requesting worker, writes a marker,
    // and exits. No container or lingering fake-runtime process is created.
    let deadline = Instant::now() + Duration::from_secs(5);
    while !f.root.join("stopped").exists() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    let reached_boundary = f.root.join("stopped").exists();
    child.kill().unwrap();
    child.wait().unwrap();
    assert!(
        reached_boundary,
        "worker did not reach the stopped dispatch boundary"
    );
    assert!(f.root.join("state/running/job_1.json").is_file());
    for _ in 0..2 {
        let output = f.command(&["drain"]).output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let calls = fs::read_to_string(f.root.join("calls")).unwrap();
    assert_eq!(calls.lines().filter(|s| *s == "create").count(), 1);
    let result: serde_json::Value =
        serde_json::from_slice(&fs::read(f.root.join("state/results/job_1/result.json")).unwrap())
            .unwrap();
    assert_eq!(result["status"], "interrupted");
    assert!(!f.root.join("state/running/job_1.json").exists());
    assert!(!f.root.join("state/jobs/job_1").exists());
}

#[test]
fn interrupted_private_write_keeps_inbox_and_restart_captures_complete_request() {
    let f = Fixture::new();
    let inbox = f.root.join("workspace/.tentaflake-worker/inbox/job_1.json");
    let request = br#"{"version":1,"id":"job_1","action_class":"communicative","argv":["true"],"timeout_seconds":1}"#;
    fs::write(&inbox, request).unwrap();
    let mut command = f.command(&["drain"]);
    // Limit only this child, forcing a partial private staging write. Parent
    // tests and the host retain their limits; no filesystem is filled.
    unsafe {
        command.pre_exec(|| {
            libc::signal(libc::SIGXFSZ, libc::SIG_IGN);
            let limit = libc::rlimit {
                rlim_cur: 64,
                rlim_max: 64,
            };
            if libc::setrlimit(libc::RLIMIT_FSIZE, &limit) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let output = command.output().unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("write queue staging"));
    assert!(!f.root.join("state/pending/job_1.json").exists());
    assert_eq!(fs::read(&inbox).unwrap(), request);
    assert!(f.root.join("state/pending/.job_1.json.new").is_file());
    let output = f.command(&["drain"]).output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        fs::read(f.root.join("state/pending/job_1.json")).unwrap(),
        request
    );
    assert!(!f.root.join("state/pending/.job_1.json.new").exists());
    assert!(
        !f.root.join("calls").exists(),
        "unapproved request must never dispatch"
    );
}
