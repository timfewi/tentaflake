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
        fs::write(&runtime, format!(r#"#!{}
[[ "$1" == --host=unix:///run/docker.sock && "$2" == --config=/var/empty ]] || exit 90
[[ ! ${{DOCKER_HOST+x}} && ! ${{DOCKER_CONTEXT+x}} && ! ${{DOCKER_CONFIG+x}} && ! ${{CONTAINER_HOST+x}} ]] || exit 91
shift 2
printf '%s\n' "$1" >> '{}'/calls
case "$1" in
image) printf '%s\n' 'sha256:{}';;
ps) exit 0;;
create) printf '%s\n' "$@" > '{}'/create-args; kill -STOP "$PPID"; : > '{}'/stopped; exit 77;;
*) exit 77;;
esac
"#, shell.display(), root.display(), "ab".repeat(32), root.display(), root.display())).unwrap();
        fs::set_permissions(&runtime, fs::Permissions::from_mode(0o700)).unwrap();
        let config = root.join("config.json");
        // Old generated configurations omit the new capacity fields. The real
        // binary must supply their defaults during upgrade.
        fs::write(&config, serde_json::to_vec(&serde_json::json!({
            "agent": "fixture", "backend": "docker", "runtime": runtime, "image": "fixture:offline",
            "workspace": root.join("workspace"), "state_dir": root.join("state"),
            "container_uid": tentaflake_admission::effective_uid(), "container_gid": tentaflake_admission::effective_gid(),
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
            .env("DOCKER_HOST", "tcp://untrusted.invalid:1234")
            .env("DOCKER_CONTEXT", "untrusted")
            .env("DOCKER_CONFIG", "/untrusted")
            .env("CONTAINER_HOST", "ssh://untrusted.invalid");
        command
    }

    fn runtime_script(&self, body: &str) {
        let runtime = self.root.join("fake-oci");
        let old = fs::read_to_string(&runtime).unwrap();
        let prefix = old.split("case \"$1\" in").next().unwrap();
        fs::write(runtime, format!("{prefix}{body}")).unwrap();
    }

    fn completed_runtime(&self, archive: &str) {
        let cid = "cd".repeat(32);
        self.runtime_script(&format!(
            r#"
case "$1" in
image) printf '%s\n' 'sha256:{}';;
ps) exit 0;;
create)
  [[ -f '{}'/state/jobs/job_1/dispatch.json ]] || exit 92
  printf '%s\n' '{cid}';;
start)
  [[ "$2" == '{cid}' && "$(<'{}'/state/jobs/job_1/container.json)" == *'{cid}'* ]] || exit 93;;
inspect)
  [[ "$3" == '{cid}' ]] || exit 94
  case "$2" in
    *Running*) if [[ -f '{}'/released ]]; then printf 'false\n'; else printf 'true\n'; fi;;
    *ExitCode*) printf '0\n';;
    *) exit 95;; esac;;
exec)
  [[ "$2" == '{cid}' ]] || exit 95
  if [[ "$3" == head ]]; then printf 'TFW1:{}:0\n'; else
    printf '%s\n' "$$" > '{}'/archive-pid
    {archive}
  fi;;
kill) [[ "$2" == --signal=USR1 && "$3" == '{cid}' ]] || exit 95; : > '{}'/released;;
logs) [[ "$2" == '{cid}' ]] || exit 96; printf 'ok\n';;
rm) [[ "$2" == --force && "$3" == '{cid}' ]] || exit 97;;
*) exit 98;;
esac
"#,
            "ab".repeat(32),
            self.root.display(),
            self.root.display(),
            self.root.display(),
            "ab".repeat(16),
            self.root.display(),
            self.root.display()
        ));
    }

    fn submit(&self) {
        fs::write(self.root.join("workspace/.tentaflake-worker/inbox/job_1.json"),
            br#"{"version":1,"id":"job_1","action_class":"local-reversible","argv":["true"],"timeout_seconds":1}"#).unwrap();
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
        // SAFETY: lock owns the fixture descriptor throughout this nonblocking flock call.
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
fn sigkill_at_dispatch_boundary_preserves_intent_and_restart_does_not_replay_or_cleanup() {
    let f = Fixture::new();
    f.submit();
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
    let create_args = fs::read_to_string(f.root.join("create-args")).unwrap();
    assert!(create_args.lines().any(|line| line == "--pull=never"));
    assert!(
        create_args
            .lines()
            .any(|line| line == format!("sha256:{}", "ab".repeat(32)))
    );
    assert!(!create_args.lines().any(|line| line == "fixture:offline"));
    let intent = fs::read(f.root.join("state/jobs/job_1/dispatch.json")).unwrap();
    for _ in 0..2 {
        let output = f.command(&["drain"]).output().unwrap();
        assert!(
            !output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(String::from_utf8_lossy(&output.stderr).contains("dispatch intent is unresolved"));
    }
    let calls = fs::read_to_string(f.root.join("calls")).unwrap();
    assert_eq!(calls.lines().filter(|s| *s == "create").count(), 1);
    assert_eq!(calls.lines().filter(|s| *s == "ps").count(), 0);
    assert_eq!(
        fs::read(f.root.join("state/jobs/job_1/dispatch.json")).unwrap(),
        intent
    );
    assert!(!f.root.join("state/results/job_1/result.json").exists());
    assert!(f.root.join("state/running/job_1.json").is_file());
    assert!(f.root.join("state/jobs/job_1/input").is_dir());
}

#[test]
fn confirmed_create_persists_identity_before_start_and_uses_cid_for_every_followup() {
    let f = Fixture::new();
    f.completed_runtime(r"for ((i=0; i<1024; i++)); do printf '\0'; done");
    f.submit();
    let output = f.command(&["drain"]).output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let result: serde_json::Value =
        serde_json::from_slice(&fs::read(f.root.join("state/results/job_1/result.json")).unwrap())
            .unwrap();
    assert_eq!(result["status"], "succeeded");
    let calls = fs::read_to_string(f.root.join("calls")).unwrap();
    assert!(calls.lines().any(|call| call == "exec"));
    assert!(calls.lines().any(|call| call == "kill"));
    assert_eq!(calls.lines().filter(|call| *call == "create").count(), 1);
    assert_eq!(calls.lines().filter(|call| *call == "rm").count(), 1);
    // This fixture sends a valid empty archive: the marker/release protocol
    // still runs, but there are no artifacts to publish.
    assert_eq!(result["artifacts_available"], false);
    assert!(!f.root.join("state/running/job_1.json").exists());
    assert!(!f.root.join("state/jobs/job_1").exists());
}

#[test]
fn failed_or_ambiguous_create_keeps_intent_and_does_not_guess_container_identity() {
    for reply in [
        "exit 77".to_owned(),
        "printf 'short-id\\n'".to_owned(),
        "printf 'untrusted diagnostic\\n' >&2; exit 77".to_owned(),
        format!("printf '{}\\n'; exit 77", "cd".repeat(32)),
        "while :; do printf aaaaaaaaaaaaaaaa; done".to_owned(),
    ] {
        let f = Fixture::new();
        f.runtime_script(&format!("case \"$1\" in\nimage) printf '%s\\n' 'sha256:{}';;\nps) exit 0;;\ncreate) {reply};;\n*) exit 99;;\nesac\n", "ab".repeat(32)));
        f.submit();
        let output = f.command(&["drain"]).output().unwrap();
        assert!(!output.status.success());
        assert!(!String::from_utf8_lossy(&output.stderr).contains("untrusted diagnostic"));
        assert!(f.root.join("state/jobs/job_1/dispatch.json").is_file());
        assert!(f.root.join("state/jobs/job_1/input").is_dir());
        assert!(f.root.join("state/running/job_1.json").is_file());
        assert!(!f.root.join("state/jobs/job_1/container.json").exists());
        let calls = fs::read(f.root.join("calls")).unwrap();
        assert!(!f.command(&["drain"]).output().unwrap().status.success());
        assert_eq!(fs::read(f.root.join("calls")).unwrap(), calls);
    }
}

#[test]
fn timed_out_create_preserves_durable_intent_without_recovery_runtime_calls() {
    let f = Fixture::new();
    let sleep = std::env::split_paths(&std::env::var_os("PATH").unwrap())
        .map(|path| path.join("sleep"))
        .find(|path| path.is_file())
        .unwrap();
    f.runtime_script(&format!("case \"$1\" in\nimage) printf '%s\\n' 'sha256:{}';;\nps) exit 0;;\ncreate) exec '{}' 30;;\n*) exit 99;;\nesac\n", "ab".repeat(32), sleep.display()));
    f.submit();
    let started = Instant::now();
    let output = f.command(&["drain"]).output().unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("timed out"));
    assert!(started.elapsed() < Duration::from_secs(20));
    assert!(f.root.join("state/jobs/job_1/dispatch.json").is_file());
    assert!(f.root.join("state/jobs/job_1/input").is_dir());
    assert!(f.root.join("state/running/job_1.json").is_file());
    let calls = fs::read(f.root.join("calls")).unwrap();
    assert!(!f.command(&["drain"]).output().unwrap().status.success());
    assert_eq!(fs::read(f.root.join("calls")).unwrap(), calls);
}

#[test]
fn missing_or_malformed_local_image_refuses_before_create() {
    for reply in [
        "exit 77",
        "printf 'fixture:tag\\n'",
        "printf 'sha256:abc\\n'",
    ] {
        let f = Fixture::new();
        f.runtime_script(&format!(
            "case \"$1\" in\nimage) {reply};;\n*) exit 99;;\nesac\n"
        ));
        f.submit();
        assert!(!f.command(&["drain"]).output().unwrap().status.success());
        assert_eq!(fs::read_to_string(f.root.join("calls")).unwrap(), "image\n");
        assert!(!f.root.join("state/jobs/job_1").exists());
    }
}

#[test]
fn podman_selects_local_mode_without_inherited_connection_or_user_config() {
    let f = Fixture::new();
    let mut config: serde_json::Value =
        serde_json::from_slice(&fs::read(&f.config).unwrap()).unwrap();
    config["backend"] = serde_json::json!("podman");
    fs::write(&f.config, serde_json::to_vec(&config).unwrap()).unwrap();
    let runtime = f.root.join("fake-oci");
    let old = fs::read_to_string(&runtime).unwrap();
    let shebang = old.lines().next().unwrap();
    fs::write(
        &runtime,
        format!(
            r#"{shebang}
[[ "$1" == --remote=false && "$2" == image ]] || exit 90
[[ "$HOME" == /var/empty && "$XDG_CONFIG_HOME" == /var/empty ]] || exit 91
[[ ! ${{CONTAINER_HOST+x}} && ! ${{CONTAINER_CONNECTION+x}} && ! ${{DOCKER_HOST+x}} ]] || exit 92
printf 'observed\n' > '{}'/local-podman
exit 77
"#,
            f.root.display()
        ),
    )
    .unwrap();
    f.submit();
    let output = f
        .command(&["drain"])
        .env("CONTAINER_CONNECTION", "untrusted")
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(f.root.join("local-podman").is_file());
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
    // SAFETY: The post-fork callback uses only async-signal-safe signal/setrlimit calls,
    // with a valid rlimit pointer, and returns without allocation.
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
fn artifact_stream_timeout_retains_dispatch_and_exact_identity() {
    let f = Fixture::new();
    let sleep = std::env::split_paths(&std::env::var_os("PATH").unwrap())
        .map(|path| path.join("sleep"))
        .find(|path| path.is_file())
        .unwrap();
    // A real, finite child exposes the old unbounded wait without leaving a
    // hanging test process when this regression is run against the old code.
    f.completed_runtime(&format!("exec '{}' 3", sleep.display()));
    f.submit();
    let output = f.command(&["drain"]).output().unwrap();
    assert!(
        !output.status.success(),
        "a stream outlived the job deadline"
    );
    assert!(String::from_utf8_lossy(&output.stderr).contains("timed out"));
    assert_retained_artifact_dispatch(&f);
}

fn assert_retained_artifact_dispatch(f: &Fixture) {
    let job = f.root.join("state/jobs/job_1");
    assert!(job.join("dispatch.json").is_file());
    assert!(job.join("input").is_dir());
    let cid: serde_json::Value =
        serde_json::from_slice(&fs::read(job.join("container.json")).unwrap()).unwrap();
    assert_eq!(cid["containerId"], "cd".repeat(32));
    assert!(f.root.join("state/running/job_1.json").is_file());
    assert!(!f.root.join("state/results/job_1/result.json").exists());
    assert!(!f.root.join("released").exists());
    let pid: libc::pid_t = fs::read_to_string(f.root.join("archive-pid"))
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    assert_eq!(
        // SAFETY: Signal zero only checks the test child PID; kill takes no pointers.
        unsafe { libc::kill(pid, 0) },
        -1,
        "archive client still exists"
    );
    assert_eq!(
        std::io::Error::last_os_error().raw_os_error(),
        Some(libc::ESRCH)
    );
    let calls = fs::read(f.root.join("calls")).unwrap();
    assert!(!f.command(&["drain"]).output().unwrap().status.success());
    assert_eq!(fs::read(f.root.join("calls")).unwrap(), calls);
}

#[test]
fn artifact_trailer_and_rejected_producer_cannot_escape_stream_bounds() {
    let sleep = std::env::split_paths(&std::env::var_os("PATH").unwrap())
        .map(|path| path.join("sleep"))
        .find(|path| path.is_file())
        .unwrap();
    for (archive, diagnostic) in [
        (
            format!(
                "for ((i=0; i<1024; i++)); do printf '\\0'; done; exec '{}' 3",
                sleep.display()
            ),
            "timed out",
        ),
        (
            "printf -v bytes '%65536s' ''; for ((i=0; i<16; i++)); do printf '%s' \"$bytes\"; done"
                .into(),
            "wire byte limit",
        ),
        (
            "for ((i=0; i<1024; i++)); do printf '\\0'; done; printf -v bytes '%65536s' ''; for ((i=0; i<16; i++)); do printf '%s' \"$bytes\"; done".into(),
            "wire byte limit",
        ),
        (
            "printf -v bytes '%16384s' ''; printf '%s' \"$bytes\" >&2".into(),
            "stderr limit",
        ),
    ] {
        let f = Fixture::new();
        f.completed_runtime(&archive);
        f.submit();
        let output = f.command(&["drain"]).output().unwrap();
        assert!(!output.status.success(), "{archive}");
        assert!(
            String::from_utf8_lossy(&output.stderr).contains(diagnostic),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_retained_artifact_dispatch(&f);
    }
}

#[test]
fn fully_observed_invalid_artifact_still_releases_completed_capsule() {
    let f = Fixture::new();
    f.completed_runtime("printf 'malformed tar header'");
    f.submit();
    let output = f.command(&["drain"]).output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let result: serde_json::Value =
        serde_json::from_slice(&fs::read(f.root.join("state/results/job_1/result.json")).unwrap())
            .unwrap();
    assert_eq!(result["status"], "succeeded");
    assert_eq!(result["artifacts_available"], false);
    assert!(
        fs::read_to_string(f.root.join("state/results/job_1/job.log"))
            .unwrap()
            .contains("artifact archive rejected")
    );
    assert!(f.root.join("released").exists());
    assert!(!f.root.join("state/running/job_1.json").exists());
    assert!(!f.root.join("state/jobs/job_1").exists());
}
