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

    fn success(&self, args: &[&str]) -> serde_json::Value {
        let output = self.command(args).output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice(&output.stdout).unwrap()
    }

    fn capture(&self) -> serde_json::Value {
        let request = serde_json::json!({
            "version":1,"id":"job_1","action_class":"communicative",
            "argv":["printf", "%s\\n", "literal\nargument", "☃"],"timeout_seconds":1
        });
        fs::write(
            self.root
                .join("workspace/.tentaflake-worker/inbox/job_1.json"),
            serde_json::to_vec(&request).unwrap(),
        )
        .unwrap();
        let output = self.command(&["drain"]).output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        request
    }

    fn request_digest(&self) -> String {
        self.success(&["requests"])["requests"][0]["request_digest"]
            .as_str()
            .unwrap()
            .to_owned()
    }

    fn decide(&self, decision: &str, digest: &str) -> std::process::Output {
        self.command(&["decide", "job_1", decision, "--expected-digest", digest])
            .output()
            .unwrap()
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
    lock.set_permissions(fs::Permissions::from_mode(0o600))
        .unwrap();
    assert_eq!(
        unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) },
        0
    );
    for args in [
        &["drain"][..],
        &["approve", "job_1"],
        &["deny", "job_1"],
        &["requests"],
        &[
            "decide",
            "job_1",
            "approve",
            "--expected-digest",
            &"0".repeat(64),
        ],
    ] {
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

#[test]
fn inspection_never_initializes_state_and_only_displays_captured_requests() {
    let f = Fixture::new();
    fs::remove_dir(f.root.join("state")).unwrap();
    let row = f.success(&["requests"]);
    assert_eq!(row["initialized"], false);
    assert_eq!(row["requests"], serde_json::json!([]));
    assert!(!f.root.join("state").exists());
    let original = f.capture();
    // A later agent-controlled file is never the approval display's source.
    fs::write(
        f.root.join("workspace/.tentaflake-worker/inbox/job_1.json"),
        b"not the captured request",
    )
    .unwrap();
    let mut config: serde_json::Value =
        serde_json::from_slice(&fs::read(&f.config).unwrap()).unwrap();
    config["container_gid"] = serde_json::json!(unsafe { libc::getegid() }.saturating_add(1));
    fs::write(&f.config, serde_json::to_vec(&config).unwrap()).unwrap();
    let lock = f.root.join("state/.queue.lock");
    fs::set_permissions(&lock, fs::Permissions::from_mode(0o400)).unwrap();
    let row = f.success(&["requests"]);
    assert_eq!(row["requests"][0]["argv"], original["argv"]);
    assert_eq!(row["requests"][0]["approval_status"], "pending");
    assert_eq!(row["requests"][0]["can_approve"], true);
    assert_eq!(row["truncated"], false);
    assert!(!f.root.join("state/decisions").exists());
    assert!(!f.root.join("calls").exists());
}

#[test]
fn decisions_are_record_only_idempotent_and_first_choice_wins() {
    for decision in ["approve", "deny"] {
        let f = Fixture::new();
        f.capture();
        let digest = f.request_digest();
        let first = f.decide(decision, &digest);
        assert!(
            first.status.success(),
            "{}",
            String::from_utf8_lossy(&first.stderr)
        );
        let retry = f.decide(decision, &digest);
        assert!(retry.status.success());
        assert_eq!(retry.stdout, first.stdout);
        let receipt: serde_json::Value = serde_json::from_slice(&first.stdout).unwrap();
        assert_eq!(receipt["state"], "recorded");
        assert_eq!(receipt["drain_required"], true);
        let other = if decision == "approve" {
            "deny"
        } else {
            "approve"
        };
        let conflict = f.decide(other, &digest);
        assert!(!conflict.status.success());
        assert!(String::from_utf8_lossy(&conflict.stderr).contains("conflict:"));
        let legacy = f.command(&[other, "job_1"]).output().unwrap();
        assert!(!legacy.status.success());
        assert!(f.root.join("state/pending/job_1.json").is_file());
        assert!(!f.root.join("state/approvals/job_1").exists());
        assert!(!f.root.join("state/results/job_1").exists());
        assert!(
            !f.root.join("calls").exists(),
            "recording must never invoke OCI"
        );
        if decision == "deny" {
            assert!(f.command(&["drain"]).output().unwrap().status.success());
            assert!(!f.root.join("state/pending/job_1.json").exists());
            assert_eq!(f.decide(decision, &digest).stdout, first.stdout);
            assert!(
                !f.root.join("calls").exists(),
                "draining denial must not invoke OCI"
            );
        }
    }
}

#[test]
fn changed_captured_request_never_inherits_an_old_approval() {
    for field in ["argv", "action_class", "timeout_seconds"] {
        let f = Fixture::new();
        let mut request = f.capture();
        let digest = f.request_digest();
        assert!(f.decide("approve", &digest).status.success());
        request[field] = match field {
            "argv" => serde_json::json!(["different-command"]),
            "action_class" => serde_json::json!("financial"),
            _ => serde_json::json!(2),
        };
        fs::write(
            f.root.join("state/pending/job_1.json"),
            serde_json::to_vec(&request).unwrap(),
        )
        .unwrap();
        for output in [
            f.decide("approve", &digest),
            f.command(&["drain"]).output().unwrap(),
            f.command(&["requests"]).output().unwrap(),
        ] {
            assert!(!output.status.success());
            assert!(String::from_utf8_lossy(&output.stderr).contains("conflict:"));
        }
        assert!(!f.root.join("calls").exists());
    }
}

#[test]
fn forbidden_invalid_and_stale_decisions_do_not_create_records() {
    let f = Fixture::new();
    let mut request = f.capture();
    let stale = f.decide("approve", &"0".repeat(64));
    assert!(!stale.status.success());
    assert!(String::from_utf8_lossy(&stale.stderr).contains("conflict:"));
    let invalid = f.decide("execute", &f.request_digest());
    assert!(!invalid.status.success());
    request["action_class"] = serde_json::json!("forbidden");
    fs::write(
        f.root.join("state/pending/job_1.json"),
        serde_json::to_vec(&request).unwrap(),
    )
    .unwrap();
    let row = f.success(&["requests"]);
    assert_eq!(row["requests"][0]["can_approve"], false);
    assert!(!f.decide("approve", &f.request_digest()).status.success());
    assert!(!f.root.join("state/decisions").exists());
    assert!(!f.root.join("calls").exists());
}

#[test]
fn review_refuses_links_oversized_files_and_shared_private_directories() {
    use std::os::unix::fs::symlink;
    for case in ["link", "large", "permissions", "running"] {
        let f = Fixture::new();
        f.capture();
        let digest = f.request_digest();
        let path = f.root.join("state/pending/job_1.json");
        match case {
            "link" => {
                fs::rename(&path, f.root.join("elsewhere")).unwrap();
                symlink(f.root.join("elsewhere"), &path).unwrap();
            }
            "large" => fs::write(&path, vec![b' '; 65537]).unwrap(),
            "permissions" => fs::set_permissions(
                f.root.join("state/pending"),
                fs::Permissions::from_mode(0o770),
            )
            .unwrap(),
            _ => fs::copy(&path, f.root.join("state/running/job_1.json"))
                .map(|_| ())
                .unwrap(),
        }
        assert!(
            !f.command(&["requests"]).output().unwrap().status.success(),
            "{case}"
        );
        assert!(!f.decide("approve", &digest).status.success(), "{case}");
        assert!(!f.root.join("state/decisions").exists());
        assert!(!f.root.join("calls").exists());
    }
}

#[test]
fn list_bounds_rows_and_bytes_without_truncating_any_argv() {
    let f = Fixture::new();
    let original = f.capture();
    let pending = f.root.join("state/pending");
    for index in 2..=110 {
        let mut request = original.clone();
        request["id"] = serde_json::json!(format!("job_{index}"));
        let path = pending.join(format!("job_{index}.json"));
        fs::write(&path, serde_json::to_vec(&request).unwrap()).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
    }
    let rows = f.success(&["requests"]);
    assert_eq!(rows["requests"].as_array().unwrap().len(), 100);
    assert_eq!(rows["truncated"], true);
    for row in rows["requests"].as_array().unwrap() {
        assert_eq!(row["argv"], original["argv"]);
    }
    for index in 1..=110 {
        let mut request = original.clone();
        request["id"] = serde_json::json!(format!("job_{index}"));
        request["argv"] = serde_json::json!(["x".repeat(4096), "y".repeat(4096)]);
        fs::write(
            pending.join(format!("job_{index}.json")),
            serde_json::to_vec(&request).unwrap(),
        )
        .unwrap();
    }
    let output = f.command(&["requests"]).output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stdout.len() <= 256 * 1024 + 1);
    let rows: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(rows["truncated"], true);
    assert!(rows["requests"].as_array().unwrap().len() < 100);
    for row in rows["requests"].as_array().unwrap() {
        assert_eq!(
            row["argv"],
            serde_json::json!(["x".repeat(4096), "y".repeat(4096)])
        );
    }
}

#[test]
fn interrupted_receipt_publication_remains_retryable_and_recovers_without_execution() {
    let f = Fixture::new();
    f.capture();
    let digest = f.request_digest();
    let first = f.decide("deny", &digest);
    assert!(first.status.success());
    let final_path = f.root.join("state/decisions/job_1");
    let staging = f.root.join("state/decisions/.job_1.new");
    fs::hard_link(&final_path, &staging).unwrap();
    let retry = f.decide("deny", &digest);
    assert!(
        retry.status.success(),
        "{}",
        String::from_utf8_lossy(&retry.stderr)
    );
    assert_eq!(retry.stdout, first.stdout);
    assert_eq!(
        f.success(&["requests"])["requests"][0]["approval_status"],
        "denied"
    );
    assert!(f.command(&["drain"]).output().unwrap().status.success());
    assert!(!staging.exists());
    assert!(final_path.is_file());
    assert_eq!(f.decide("deny", &digest).stdout, first.stdout);
    assert!(!f.root.join("calls").exists());
}

#[test]
fn racing_opposite_decisions_record_only_one_choice() {
    let f = Fixture::new();
    f.capture();
    let digest = f.request_digest();
    let mut approve = f.command(&["decide", "job_1", "approve", "--expected-digest", &digest]);
    let mut deny = f.command(&["decide", "job_1", "deny", "--expected-digest", &digest]);
    let first = approve
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let second = deny
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let results = [
        first.wait_with_output().unwrap(),
        second.wait_with_output().unwrap(),
    ];
    assert_eq!(
        results
            .iter()
            .filter(|result| result.status.success())
            .count(),
        1
    );
    let receipt: serde_json::Value =
        serde_json::from_slice(&fs::read(f.root.join("state/decisions/job_1")).unwrap()).unwrap();
    let winner = receipt["decision"].as_str().unwrap();
    assert!(f.decide(winner, &digest).status.success());
    assert!(
        !f.decide(
            if winner == "approve" {
                "deny"
            } else {
                "approve"
            },
            &digest
        )
        .status
        .success()
    );
    assert!(!f.root.join("calls").exists());
}

#[test]
fn lost_decision_acknowledgement_is_uncertain_and_exact_retry_is_safe() {
    let f = Fixture::new();
    f.capture();
    let digest = f.request_digest();
    let failed_stdout = fs::OpenOptions::new()
        .write(true)
        .open("/dev/full")
        .unwrap();
    let result = f
        .command(&["decide", "job_1", "deny", "--expected-digest", &digest])
        .stdout(failed_stdout)
        .output()
        .unwrap();
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("uncertain:"));
    let receipt: serde_json::Value =
        serde_json::from_slice(&fs::read(f.root.join("state/decisions/job_1")).unwrap()).unwrap();
    assert_eq!(receipt["decision"], "deny");
    assert!(f.decide("deny", &digest).status.success());
    assert!(!f.root.join("calls").exists());
}

#[test]
fn request_scan_counts_ignored_entries_before_reporting_any_rows() {
    let f = Fixture::new();
    f.capture();
    fs::write(f.root.join("state/pending/ignored"), b"staging").unwrap();
    let mut config: serde_json::Value =
        serde_json::from_slice(&fs::read(&f.config).unwrap()).unwrap();
    config["max_queue_entries"] = serde_json::json!(1);
    fs::write(&f.config, serde_json::to_vec(&config).unwrap()).unwrap();
    let output = f.command(&["requests"]).output().unwrap();
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).contains("limit:"));
    assert!(!f.root.join("calls").exists());
}
