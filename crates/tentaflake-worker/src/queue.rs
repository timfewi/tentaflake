//! Private queue ownership and durable transitions. All mutations hold `lock`.
use super::*;

pub(super) fn lock(config: &Config) -> Result<File> {
    open_path_no_symlinks(&config.state_dir)?;
    let path = config.state_dir.join(".queue.lock");
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
        .open(&path)
        .map_err(|e| format!("open queue lock: {e}"))?;
    if !file.metadata().map_err(|e| e.to_string())?.is_file() {
        return Err("queue lock must be a regular private file".into());
    }
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        return Err(format!(
            "queue is busy or cannot be locked: {}",
            io::Error::last_os_error()
        ));
    }
    Ok(file)
}

fn sync_directory(path: &Path) -> Result<()> {
    let fd = open_path_no_symlinks(path)?;
    let readable = open_dir_at(fd.as_raw_fd(), OsStr::new("."))
        .map_err(|e| format!("open directory for sync: {e}"))?;
    File::from(readable)
        .sync_all()
        .map_err(|e| format!("sync {}: {e}", path.display()))
}

fn read_private(path: &Path, max: usize) -> Result<Vec<u8>> {
    let mut file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
        .map_err(|e| format!("open {}: {e}", path.display()))?;
    if !file.metadata().map_err(|e| e.to_string())?.is_file() {
        return Err(format!("not a regular queue file: {}", path.display()));
    }
    read_bounded(&mut file, max).map_err(|e| format!("read {}: {e}", path.display()))
}

pub(super) fn read_request(config: &Config, directory: &str, job: &str) -> Result<JobRequest> {
    validate_identifier("job id", job, 64)?;
    let parent = config.state_dir.join(directory);
    open_path_no_symlinks(&parent)?;
    let content = read_private(
        &parent.join(format!("{job}.json")),
        config.max_request_bytes as usize,
    )?;
    let request: JobRequest =
        serde_json::from_slice(&content).map_err(|e| format!("parse private request: {e}"))?;
    validate_request(config, &request)?;
    if request.id != job {
        return Err("private request filename must equal <id>.json".into());
    }
    Ok(request)
}

pub(super) fn write_private(path: &Path, content: &[u8]) -> Result<()> {
    let parent = path.parent().ok_or("queue file needs a parent")?;
    open_path_no_symlinks(parent)?;
    let name = path.file_name().ok_or("queue file needs a name")?;
    let staging = parent.join(format!(".{}.new", name.to_string_lossy()));
    // A previous interrupted write is unpublished. Never overwrite the final
    // copy, including an approval that has already been granted.
    match fs::symlink_metadata(&staging) {
        Ok(metadata) if metadata.is_file() => {
            fs::remove_file(&staging)
                .map_err(|e| format!("remove incomplete queue staging: {e}"))?;
            sync_directory(parent)?;
        }
        Ok(_) => return Err("incomplete queue staging is not a regular file".into()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(e.to_string()),
    }
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(&staging)
        .map_err(|e| format!("create queue staging: {e}"))?;
    file.write_all(content)
        .and_then(|()| file.sync_all())
        .map_err(|e| format!("write queue staging: {e}"))?;
    // Same-filesystem hard-link publication is atomic and refuses replacement.
    // The agent inbox can be on a different filesystem, so it is not renamed.
    fs::hard_link(&staging, path).map_err(|e| format!("publish private queue file: {e}"))?;
    sync_directory(parent)?;
    fs::remove_file(staging).map_err(|e| format!("remove published queue staging: {e}"))?;
    sync_directory(parent)
}

pub(super) fn approved(config: &Config, job: &str) -> Result<bool> {
    let parent = config.state_dir.join("approvals");
    open_path_no_symlinks(&parent)?;
    let path = parent.join(job);
    match fs::symlink_metadata(&path) {
        Ok(_) => {
            if read_private(&path, 32)? != b"approved\n" {
                return Err("invalid host approval record".into());
            }
            Ok(true)
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(format!("inspect host approval: {e}")),
    }
}

pub(super) fn scan(fd: RawFd, max_entries: usize, max_bytes: u64) -> Result<(Vec<OsString>, u64)> {
    let mut names = Vec::new();
    let mut bytes = 0_u64;
    for entry in fs::read_dir(format!("/proc/self/fd/{fd}")).map_err(|e| e.to_string())? {
        let name = entry.map_err(|e| e.to_string())?.file_name();
        if names.len() >= max_entries {
            return Err("worker queue exceeds its aggregate entry ceiling".into());
        }
        let stat = fstatat_nofollow(fd, &name)?;
        let length = u64::try_from(stat.st_size).map_err(|_| "invalid queue entry size")?;
        bytes = bytes
            .checked_add(length)
            .ok_or("worker queue byte count overflow")?;
        if bytes > max_bytes {
            return Err("worker queue exceeds its aggregate byte ceiling".into());
        }
        names.push(name);
    }
    Ok((names, bytes))
}

pub(super) fn usage(config: &Config) -> Result<(usize, u64)> {
    let mut entries = 0;
    let mut bytes = 0_u64;
    for directory in ["pending", "running"] {
        let fd = open_path_no_symlinks(&config.state_dir.join(directory))?;
        let (names, length) = scan(
            fd.as_raw_fd(),
            config.max_queue_entries,
            config.max_queue_bytes,
        )?;
        entries += names.len();
        bytes = bytes
            .checked_add(length)
            .ok_or("worker queue byte count overflow")?;
        if entries > config.max_queue_entries || bytes > config.max_queue_bytes {
            return Err("private worker queue exceeds its aggregate capacity".into());
        }
    }
    Ok((entries, bytes))
}

pub(super) fn claim(config: &Config, request: &JobRequest) -> Result<()> {
    let pending = config.state_dir.join("pending");
    let running = config.state_dir.join("running");
    let name = format!("{}.json", request.id);
    // The per-agent lock owns both private directories. Existing running state
    // is never replaced, even if an operator restored conflicting old state.
    match fs::symlink_metadata(running.join(&name)) {
        Ok(_) => return Err("job already has a durable running claim".into()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(format!("inspect running claim: {e}")),
    }
    OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(pending.join(&name))
        .and_then(|f| f.sync_all())
        .map_err(|e| format!("sync request before claim: {e}"))?;
    fs::rename(pending.join(&name), running.join(&name)).map_err(|e| format!("claim job: {e}"))?;
    sync_directory(&running)?;
    sync_directory(&pending)
}

pub(super) fn terminal(config: &Config, request: &JobRequest) -> Result<bool> {
    let path = config.state_dir.join("results").join(&request.id);
    match fs::symlink_metadata(&path) {
        Ok(_) => {
            open_path_no_symlinks(&path)?;
            let result: JobResult =
                serde_json::from_slice(&read_private(&path.join("result.json"), 16 * 1024)?)
                    .map_err(|e| format!("invalid terminal result: {e}"))?;
            if result.version != 1
                || result.id != request.id
                || result.action_class != request.action_class
                || !matches!(
                    result.status.as_str(),
                    "succeeded"
                        | "failed"
                        | "timed-out"
                        | "runtime-error"
                        | "rejected"
                        | "denied"
                        | "interrupted"
                )
            {
                return Err("terminal result does not match its private request".into());
            }
            Ok(true)
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(format!("inspect terminal result: {e}")),
    }
}

pub(super) fn retire(config: &Config, directory: &str, request: &JobRequest) -> Result<()> {
    if !terminal(config, request)? {
        return Err("cannot retire a request without a durable terminal result".into());
    }
    let approvals = config.state_dir.join("approvals");
    match fs::remove_file(approvals.join(&request.id)) {
        Ok(()) => sync_directory(&approvals)?,
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(format!("consume approval: {e}")),
    }
    let parent = config.state_dir.join(directory);
    fs::remove_file(parent.join(format!("{}.json", request.id)))
        .map_err(|e| format!("retire request: {e}"))?;
    sync_directory(&parent)
}

pub(super) fn recover(config: &Config) -> Result<()> {
    for directory in ["pending", "approvals"] {
        let parent = config.state_dir.join(directory);
        open_path_no_symlinks(&parent)?;
        for entry in fs::read_dir(&parent).map_err(|e| e.to_string())? {
            let entry = entry.map_err(|e| e.to_string())?;
            let name = entry.file_name();
            let Some(name) = name.to_str() else {
                continue;
            };
            let staged = name.strip_prefix('.').and_then(|s| s.strip_suffix(".new"));
            let staged_job = if directory == "pending" {
                staged.and_then(|s| s.strip_suffix(".json"))
            } else {
                staged
            };
            if staged_job.is_some_and(|job| validate_identifier("job id", job, 64).is_ok()) {
                if !entry.file_type().map_err(|e| e.to_string())?.is_file() {
                    return Err("incomplete queue staging is not a regular file".into());
                }
                fs::remove_file(entry.path()).map_err(|e| e.to_string())?;
            }
        }
        sync_directory(&parent)?;
    }
    let running = config.state_dir.join("running");
    let fd = open_path_no_symlinks(&running)?;
    let (names, _) = scan(
        fd.as_raw_fd(),
        config.max_queue_entries,
        config.max_queue_bytes,
    )?;
    for name in names {
        let Some(job) = name.to_str().and_then(|s| s.strip_suffix(".json")) else {
            continue;
        };
        let request = read_request(config, "running", job)?;
        cleanup_existing_worker_container(config, &format!("tfw-{}-{job}", config.agent))?;
        for path in [
            config.state_dir.join("jobs").join(job),
            config.state_dir.join("results").join(format!(".{job}.tmp")),
        ] {
            match fs::symlink_metadata(&path) {
                Ok(_) => {
                    open_path_no_symlinks(&path)?;
                    fs::remove_dir_all(&path)
                        .map_err(|e| format!("remove interrupted staging: {e}"))?;
                    sync_directory(path.parent().ok_or("staging needs parent")?)?;
                }
                Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                Err(e) => return Err(e.to_string()),
            }
        }
        if !terminal(config, &request)? {
            finish_without_run(
                config,
                &request,
                "interrupted",
                "execution outcome is unknown; review before submitting a new job ID",
            )?;
        }
        audit(
            config,
            job,
            "recovered",
            Some(request.action_class),
            "abandoned claim retired without dispatch replay",
        )?;
        retire(config, "running", &request)?;
    }
    Ok(())
}

pub(super) fn publish_result(staging: &Path, destination: &Path) -> Result<()> {
    // Sync all published files, including imported artifacts, before the
    // terminal rename. No request can retire on a partially durable result.
    fn sync_tree(path: &Path) -> Result<()> {
        for entry in fs::read_dir(path).map_err(|e| e.to_string())? {
            let entry = entry.map_err(|e| e.to_string())?;
            let kind = entry.file_type().map_err(|e| e.to_string())?;
            if kind.is_dir() {
                sync_tree(&entry.path())?;
            } else if kind.is_file() {
                File::open(entry.path())
                    .and_then(|f| f.sync_all())
                    .map_err(|e| e.to_string())?;
            } else {
                return Err("result staging contains a link or special file".into());
            }
        }
        sync_directory(path)
    }
    sync_tree(staging)?;
    match fs::symlink_metadata(destination) {
        Ok(_) => return Err("refusing to replace an existing terminal result".into()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(format!("inspect terminal destination: {e}")),
    }
    fs::rename(staging, destination).map_err(|e| format!("publish result: {e}"))?;
    sync_directory(destination.parent().ok_or("result needs parent")?)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fixture {
        root: PathBuf,
        config: Config,
    }

    impl Fixture {
        fn new(name: &str) -> Self {
            let root = Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../target/test-tmp")
                .join(format!(
                    "queue-{name}-{}-{}",
                    std::process::id(),
                    SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .unwrap()
                        .as_nanos()
                ));
            fs::create_dir_all(&root).unwrap();
            let root = root.canonicalize().unwrap();
            let workspace = root.join("workspace");
            fs::create_dir_all(workspace.join(".tentaflake-worker/inbox")).unwrap();
            let shell = std::env::split_paths(&std::env::var_os("PATH").unwrap())
                .map(|p| p.join("bash"))
                .find(|p| p.is_file())
                .expect("contributor tools include bash");
            let runtime = root.join("fake-oci");
            fs::write(&runtime, format!("#!{}\nprintf '%s\\n' \"$*\" >> '{}'/calls\ncase \"$1\" in ps) exit 0;; *) exit 77;; esac\n", shell.display(), root.display())).unwrap();
            fs::set_permissions(&runtime, fs::Permissions::from_mode(0o700)).unwrap();
            let config = Config {
                agent: "fixture".into(),
                backend: "docker".into(),
                runtime,
                image: "fixture:offline".into(),
                workspace,
                state_dir: root.join("state"),
                container_uid: unsafe { libc::geteuid() },
                container_gid: unsafe { libc::getegid() },
                max_request_bytes: 65536,
                max_inbox_entries: 256,
                max_inbox_bytes: 8 * 1024 * 1024,
                max_queue_entries: 256,
                max_queue_bytes: 8 * 1024 * 1024,
                max_snapshot_bytes: 65536,
                max_snapshot_entries: 64,
                max_log_bytes: 1024,
                max_timeout_seconds: 5,
                memory: "32m".into(),
                memory_swap: "32m".into(),
                cpus: "1".into(),
                pids_limit: 16,
                workspace_tmpfs_size: "32m".into(),
                tmp_tmpfs_size: "16m".into(),
            };
            ensure_state_layout(&config).unwrap();
            Self { root, config }
        }

        fn request(&self, class: ActionClass) -> JobRequest {
            JobRequest {
                version: 1,
                id: "job_1".into(),
                action_class: class,
                argv: vec!["true".into()],
                timeout_seconds: 1,
            }
        }

        fn pending(&self, request: &JobRequest) {
            write_private(
                &pending_path(&self.config, &request.id),
                &serde_json::to_vec(request).unwrap(),
            )
            .unwrap();
        }

        fn calls(&self) -> String {
            fs::read_to_string(self.root.join("calls")).unwrap_or_default()
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn queue_lock_excludes_competing_worker_and_operator() {
        let f = Fixture::new("lock");
        let first = lock(&f.config).unwrap();
        assert!(lock(&f.config).unwrap_err().contains("busy"));
        drop(first);
        let _next = lock(&f.config).unwrap();
    }

    #[test]
    fn incomplete_private_staging_never_replaces_a_published_request() {
        let f = Fixture::new("staging");
        let path = pending_path(&f.config, "job_1");
        fs::write(path.parent().unwrap().join(".job_1.json.new"), b"{partial").unwrap();
        let request = f.request(ActionClass::LocalReversible);
        f.pending(&request);
        let before = fs::read(&path).unwrap();
        assert!(write_private(&path, b"replacement").is_err());
        assert_eq!(fs::read(&path).unwrap(), before);
        assert_eq!(
            read_request(&f.config, "pending", "job_1").unwrap().id,
            "job_1"
        );
    }

    #[test]
    fn terminal_pending_is_retired_without_runtime_dispatch() {
        let f = Fixture::new("terminal");
        let request = f.request(ActionClass::LocalReversible);
        f.pending(&request);
        finish_without_run(&f.config, &request, "denied", "fixture terminal result").unwrap();
        drain(&f.config).unwrap();
        assert!(!pending_path(&f.config, &request.id).exists());
        assert_eq!(f.calls(), "");
        assert!(terminal(&f.config, &request).unwrap());
    }

    #[test]
    fn failed_dispatch_keeps_claim_and_restart_never_creates_again() {
        let f = Fixture::new("dispatch");
        let request = f.request(ActionClass::LocalReversible);
        f.pending(&request);
        assert!(drain(&f.config).is_err());
        assert!(!pending_path(&f.config, &request.id).exists());
        assert!(f.config.state_dir.join("running/job_1.json").is_file());
        assert_eq!(
            f.calls()
                .lines()
                .filter(|s| s.starts_with("create "))
                .count(),
            1
        );
        drain(&f.config).unwrap();
        drain(&f.config).unwrap();
        assert_eq!(
            f.calls()
                .lines()
                .filter(|s| s.starts_with("create "))
                .count(),
            1
        );
        let result: JobResult = serde_json::from_slice(
            &fs::read(f.config.state_dir.join("results/job_1/result.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(result.status, "interrupted");
        assert!(!f.config.state_dir.join("running/job_1.json").exists());
    }

    #[test]
    fn legacy_abandoned_snapshot_is_not_reexecuted() {
        let f = Fixture::new("legacy");
        let request = f.request(ActionClass::LocalReversible);
        f.pending(&request);
        fs::create_dir_all(f.config.state_dir.join("jobs/job_1/input")).unwrap();
        drain(&f.config).unwrap();
        assert!(!f.calls().contains("create "));
        assert!(terminal(&f.config, &request).unwrap());
        assert!(!f.config.state_dir.join("jobs/job_1").exists());
    }

    #[test]
    fn damaged_legacy_pending_preserves_complete_inbox_copy() {
        let f = Fixture::new("damaged");
        let request = f.request(ActionClass::LocalReversible);
        fs::write(pending_path(&f.config, &request.id), b"{partial").unwrap();
        let inbox = f
            .config
            .workspace
            .join(".tentaflake-worker/inbox/job_1.json");
        let content = serde_json::to_vec(&request).unwrap();
        fs::write(&inbox, &content).unwrap();
        assert!(drain(&f.config).is_err());
        assert_eq!(fs::read(inbox).unwrap(), content);
        assert_eq!(f.calls(), "");
    }

    #[test]
    fn approval_requires_exact_private_record_and_cannot_change_terminal_job() {
        let f = Fixture::new("approval");
        let request = f.request(ActionClass::Communicative);
        f.pending(&request);
        let approval = f.config.state_dir.join("approvals/job_1");
        let outside = f.root.join("outside");
        fs::write(&outside, b"approved\n").unwrap();
        symlink(&outside, &approval).unwrap();
        assert!(approved(&f.config, "job_1").is_err());
        fs::remove_file(&approval).unwrap();
        fs::write(&approval, b"yes").unwrap();
        assert!(approved(&f.config, "job_1").is_err());
        fs::remove_file(&approval).unwrap();
        finish_without_run(&f.config, &request, "denied", "fixture").unwrap();
        assert!(
            approve(&f.config, "job_1")
                .unwrap_err()
                .contains("terminal")
        );
        assert!(deny(&f.config, "job_1").unwrap_err().contains("terminal"));
        assert_eq!(f.calls(), "");
    }

    #[test]
    fn private_request_identity_and_size_are_validated() {
        let mut f = Fixture::new("identity");
        let request = f.request(ActionClass::LocalReversible);
        f.pending(&request);
        fs::rename(
            pending_path(&f.config, "job_1"),
            pending_path(&f.config, "other"),
        )
        .unwrap();
        assert!(
            read_request(&f.config, "pending", "other")
                .unwrap_err()
                .contains("filename")
        );
        f.config.max_request_bytes = 8;
        assert!(read_request(&f.config, "pending", "other").is_err());
    }

    #[test]
    fn queue_overload_preserves_unaccepted_request_and_existing_approval() {
        let mut f = Fixture::new("queue-capacity");
        f.config.max_queue_entries = 1;
        let request = f.request(ActionClass::Communicative);
        f.pending(&request);
        let mut second = f.request(ActionClass::LocalReversible);
        second.id = "job_2".into();
        let inbox = f
            .config
            .workspace
            .join(".tentaflake-worker/inbox/job_2.json");
        fs::write(&inbox, serde_json::to_vec(&second).unwrap()).unwrap();
        assert!(drain(&f.config).unwrap_err().contains("full"));
        assert!(pending_path(&f.config, "job_1").is_file());
        assert!(inbox.is_file());
        assert!(!pending_path(&f.config, "job_2").exists());
        assert_eq!(f.calls(), "");
        assert!(
            fs::read_to_string(f.config.state_dir.join("audit.jsonl"))
                .unwrap()
                .contains("queue-overload")
        );
        f.config.max_queue_entries = 2;
        f.config.max_queue_bytes = fs::metadata(pending_path(&f.config, "job_1"))
            .unwrap()
            .len();
        assert!(drain(&f.config).unwrap_err().contains("full"));
        assert!(inbox.is_file());
    }

    #[test]
    fn inbox_scan_bounds_entries_and_bytes_including_ignored_files() {
        let mut f = Fixture::new("inbox-capacity");
        let inbox = f.config.workspace.join(".tentaflake-worker/inbox");
        fs::write(inbox.join("ignored-one"), b"one").unwrap();
        fs::write(inbox.join("ignored-two"), b"two").unwrap();
        f.config.max_inbox_entries = 1;
        assert!(drain(&f.config).unwrap_err().contains("entry ceiling"));
        f.config.max_inbox_entries = 2;
        f.config.max_inbox_bytes = 5;
        assert!(drain(&f.config).unwrap_err().contains("byte ceiling"));
        f.config.max_inbox_bytes = 6;
        drain(&f.config).unwrap();
        assert_eq!(fs::read_dir(inbox).unwrap().count(), 2);
    }

    #[test]
    fn pending_and_running_share_one_capacity_budget() {
        let mut f = Fixture::new("combined-capacity");
        let request = f.request(ActionClass::Communicative);
        f.pending(&request);
        claim(&f.config, &request).unwrap();
        let mut second = f.request(ActionClass::Communicative);
        second.id = "job_2".into();
        f.pending(&second);
        assert_eq!(usage(&f.config).unwrap().0, 2);
        f.config.max_queue_entries = 1;
        assert!(usage(&f.config).unwrap_err().contains("aggregate capacity"));
    }

    #[test]
    fn recovery_refuses_foreign_container_and_retains_ambiguous_claim() {
        let f = Fixture::new("foreign");
        let request = f.request(ActionClass::LocalReversible);
        f.pending(&request);
        claim(&f.config, &request).unwrap();
        let shell = std::env::split_paths(&std::env::var_os("PATH").unwrap())
            .map(|p| p.join("bash"))
            .find(|p| p.is_file())
            .unwrap();
        fs::write(&f.config.runtime, format!("#!{}\ncase \"$1\" in ps) echo tfw-fixture-job_1;; inspect) echo another-agent;; *) exit 77;; esac\n", shell.display())).unwrap();
        assert!(drain(&f.config).unwrap_err().contains("unowned"));
        assert!(f.config.state_dir.join("running/job_1.json").is_file());
        assert!(!terminal(&f.config, &request).unwrap());
    }
}
