//! Read-only, bounded queue prerequisites. This snapshot grants no dispatch.
use super::*;
use std::collections::{BTreeMap, BTreeSet};
use std::os::unix::fs::MetadataExt;

// Observation ceilings keep even unusually large configured queues bounded.
// Exceeding them refuses the probe; it never changes the worker's job policy.
const MAX_ENTRIES: usize = 4096;
const MAX_BYTES: u64 = 64 * 1024 * 1024;
const MAX_REQUEST_BYTES: u64 = 1024 * 1024;

#[derive(Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Snapshot {
    schema_version: u8,
    agent: String,
    observed_unix_seconds: u64,
    inbox: Usage,
    pending: Usage,
    running: Usage,
    limits: Limits,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Limits {
    inbox_entries: usize,
    inbox_bytes: u64,
    queue_entries: usize,
    queue_bytes: u64,
}

#[derive(Debug, Default, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct Usage {
    entries: usize,
    bytes: u64,
}

#[derive(Debug, PartialEq, Eq)]
struct Identity {
    device: u64,
    inode: u64,
    uid: u32,
    gid: u32,
    mode: u32,
    links: u64,
    bytes: u64,
    modified: (i64, i64),
    changed: (i64, i64),
}

impl Identity {
    fn read(file: &File) -> Result<Self> {
        let m = file
            .metadata()
            .map_err(|_| "worker probe metadata unavailable")?;
        Ok(Self {
            device: m.dev(),
            inode: m.ino(),
            uid: m.uid(),
            gid: m.gid(),
            mode: m.mode(),
            links: m.nlink(),
            bytes: m.len(),
            modified: (m.mtime(), m.mtime_nsec()),
            changed: (m.ctime(), m.ctime_nsec()),
        })
    }

    fn directory(&self, owner: u32, modes: &[u32]) -> Result<()> {
        if self.mode & libc::S_IFMT != libc::S_IFDIR
            || self.uid != owner
            || !modes.contains(&(self.mode & 0o7777))
        {
            return Err("worker probe directory ownership or mode mismatch".into());
        }
        Ok(())
    }
}

fn child(parent: &File, name: &str) -> Result<File> {
    open_dir_at(parent.as_raw_fd(), OsStr::new(name))
        .map(File::from)
        .map_err(|_| "worker probe layout missing or unsafe".into())
}

fn unchanged(file: &File, before: &Identity) -> Result<()> {
    if Identity::read(file)? != *before {
        return Err("worker probe source changed during observation".into());
    }
    Ok(())
}

fn open_root(path: &Path) -> Result<File> {
    open_path_no_symlinks(path)
        .map(File::from)
        .map_err(|_| "worker probe root missing or unsafe".into())
}

pub(super) fn collect(config: &Config) -> Result<Snapshot> {
    if unsafe { libc::geteuid() } != 0 {
        return Err("worker probe requires the root state owner".into());
    }
    let state = open_root(&config.state_dir)?;
    let workspace = open_root(&config.workspace)?;
    let state_identity = Identity::read(&state)?;
    state_identity.directory(0, &[0o750])?;
    let workspace_identity = Identity::read(&workspace)?;
    let (snapshot, _lock) = observe(config, &state, &workspace)?;
    // Retain the shared queue lock through both final path reopens.
    unchanged(&open_root(&config.state_dir)?, &state_identity)?;
    unchanged(&open_root(&config.workspace)?, &workspace_identity)?;
    Ok(snapshot)
}

// The public entrypoint authenticates the root-owned absolute path. Keeping
// this observer relative to held, owned directories also lets filesystem tests
// exercise the same modes/traversal from their explicit private fixture FDs.
fn observe(config: &Config, state: &File, workspace: &File) -> Result<(Snapshot, File)> {
    let deadline = Instant::now() + Duration::from_secs(2);
    let state_identity = Identity::read(state)?;
    let owner = state_identity.uid;
    state_identity.directory(owner, &[0o750])?;
    if state_identity.gid != config.container_gid {
        return Err("worker probe state group mismatch".into());
    }
    let workspace_identity = Identity::read(workspace)?;
    if workspace_identity.uid != config.container_uid
        || workspace_identity.gid != config.container_gid
        || workspace_identity.mode & libc::S_IFMT != libc::S_IFDIR
        || workspace_identity.mode & 0o700 != 0o700
        || workspace_identity.mode & 0o002 != 0
    {
        return Err("worker probe workspace ownership or mode mismatch".into());
    }
    let lock = open_file_at(state.as_raw_fd(), OsStr::new(".queue.lock"))
        .map(File::from)
        .map_err(|_| "worker probe queue lock missing or unsafe")?;
    let lock_identity = Identity::read(&lock)?;
    if lock_identity.uid != owner
        || lock_identity.mode != (libc::S_IFREG | 0o600)
        || lock_identity.links != 1
        || lock_identity.bytes != 0
    {
        return Err("worker probe queue lock ownership or mode mismatch".into());
    }
    if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_SH | libc::LOCK_NB) } != 0 {
        return Err("worker probe queue busy or unavailable".into());
    }

    let mut directories = Vec::new();
    for name in ["pending", "running", "approvals", "jobs", "results"] {
        let file = child(state, name)?;
        let identity = Identity::read(&file)?;
        if name == "results" {
            identity.directory(owner, &[0o750, 0o2750])?;
            if identity.gid != config.container_gid {
                return Err("worker probe result group mismatch".into());
            }
        } else {
            identity.directory(owner, &[0o700])?;
        }
        directories.push((name, file, identity));
    }
    let control = child(workspace, ".tentaflake-worker")?;
    let control_identity = Identity::read(&control)?;
    control_identity.directory(config.container_uid, &[0o700])?;
    let inbox = child(&control, "inbox")?;
    let inbox_identity = Identity::read(&inbox)?;
    inbox_identity.directory(config.container_uid, &[0o770])?;
    if control_identity.gid != config.container_gid || inbox_identity.gid != config.container_gid {
        return Err("worker probe inbox group mismatch".into());
    }

    let scan = || -> Result<_> {
        let pending = requests(config, &directories[0].1, Some(owner), deadline)?;
        let running = requests(config, &directories[1].1, Some(owner), deadline)?;
        let incoming = requests(config, &inbox, None, deadline)?;
        let queue_entries = pending
            .len()
            .checked_add(running.len())
            .ok_or("worker probe entry count overflow")?;
        let queue_bytes = usage(&pending)?
            .bytes
            .checked_add(usage(&running)?.bytes)
            .ok_or("worker probe byte count overflow")?;
        if queue_entries > config.max_queue_entries
            || queue_bytes > config.max_queue_bytes
            || queue_entries
                .checked_add(incoming.len())
                .is_none_or(|n| n > MAX_ENTRIES)
            || queue_bytes
                .checked_add(usage(&incoming)?.bytes)
                .is_none_or(|n| n > MAX_BYTES)
        {
            return Err("worker probe exceeds queue or observation capacity".into());
        }
        let mut ids = BTreeSet::new();
        for name in pending.keys().chain(running.keys()).chain(incoming.keys()) {
            if !ids.insert(name) {
                return Err("worker probe conflicting request identities".into());
            }
        }
        Ok((pending, running, incoming))
    };
    let observed = scan()?;
    // Inbox writers do not hold the private queue lock. Refuse observable
    // rewrites/renames instead of calling a mixed scan a coherent snapshot.
    if scan()? != observed {
        return Err("worker probe queue changed during observation".into());
    }
    for (name, file, identity) in &directories {
        unchanged(file, identity)?;
        unchanged(&child(state, name)?, identity)?;
    }
    unchanged(&child(workspace, ".tentaflake-worker")?, &control_identity)?;
    unchanged(&child(&control, "inbox")?, &inbox_identity)?;
    unchanged(state, &state_identity)?;
    unchanged(workspace, &workspace_identity)?;
    let current_lock = open_file_at(state.as_raw_fd(), OsStr::new(".queue.lock"))
        .map(File::from)
        .map_err(|_| "worker probe queue lock changed")?;
    unchanged(&current_lock, &lock_identity)?;
    if Instant::now() >= deadline {
        return Err("worker probe observation deadline".into());
    }
    let observed_unix_seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| "worker probe clock unavailable")?
        .as_secs();
    Ok((
        Snapshot {
            schema_version: 1,
            agent: config.agent.clone(),
            observed_unix_seconds,
            pending: usage(&observed.0)?,
            running: usage(&observed.1)?,
            inbox: usage(&observed.2)?,
            limits: Limits {
                inbox_entries: config.max_inbox_entries,
                inbox_bytes: config.max_inbox_bytes,
                queue_entries: config.max_queue_entries,
                queue_bytes: config.max_queue_bytes,
            },
        },
        lock,
    ))
}

fn usage(records: &BTreeMap<OsString, Identity>) -> Result<Usage> {
    Ok(Usage {
        entries: records.len(),
        bytes: records.values().try_fold(0_u64, |total, item| {
            total
                .checked_add(item.bytes)
                .ok_or("worker probe byte count overflow")
        })?,
    })
}

fn requests(
    config: &Config,
    directory: &File,
    private_owner: Option<u32>,
    deadline: Instant,
) -> Result<BTreeMap<OsString, Identity>> {
    if Instant::now() >= deadline {
        return Err("worker probe observation deadline".into());
    }
    let (max_entries, max_bytes) = if private_owner.is_some() {
        (config.max_queue_entries, config.max_queue_bytes)
    } else {
        (config.max_inbox_entries, config.max_inbox_bytes)
    };
    let (names, _) = queue::scan(
        directory.as_raw_fd(),
        max_entries.min(MAX_ENTRIES),
        max_bytes.min(MAX_BYTES),
    )
    .map_err(|_| "worker probe queue unreadable or over capacity")?;
    let mut records = BTreeMap::new();
    for name in names {
        if Instant::now() >= deadline {
            return Err("worker probe observation deadline".into());
        }
        let job = name
            .to_str()
            .and_then(|s| s.strip_suffix(".json"))
            .ok_or("worker probe incomplete or unrecognized queue entry")?;
        validate_identifier("job id", job, 64)?;
        let mut file = open_file_at(directory.as_raw_fd(), &name)
            .map(File::from)
            .map_err(|_| "worker probe queue entry missing or unsafe")?;
        let identity = Identity::read(&file)?;
        if identity.mode & libc::S_IFMT != libc::S_IFREG
            || identity.links != 1
            || identity.uid != private_owner.unwrap_or(config.container_uid)
            || (private_owner.is_some() && identity.mode & 0o7777 != 0o600)
            || (private_owner.is_none()
                && (identity.gid != config.container_gid || identity.mode & 0o002 != 0))
            || identity.bytes > config.max_request_bytes.min(MAX_REQUEST_BYTES)
        {
            return Err("worker probe queue entry ownership, mode or size mismatch".into());
        }
        let bytes = read_bounded(&mut file, identity.bytes as usize)
            .map_err(|_| "worker probe queue entry changed or unreadable")?;
        let request: JobRequest =
            serde_json::from_slice(&bytes).map_err(|_| "worker probe invalid request")?;
        validate_request(config, &request).map_err(|_| "worker probe invalid request")?;
        if request.id != job || bytes.len() as u64 != identity.bytes {
            return Err("worker probe request identity or size mismatch".into());
        }
        unchanged(&file, &identity)?;
        if records.insert(name, identity).is_some() {
            return Err("worker probe duplicate directory entry".into());
        }
    }
    Ok(records)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fixture {
        root: PathBuf,
        config: Config,
    }

    impl Fixture {
        fn new() -> Self {
            let root = Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../target/test-tmp")
                .join(format!(
                    "probe-{}-{}",
                    std::process::id(),
                    SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .unwrap()
                        .as_nanos()
                ));
            fs::create_dir_all(&root).unwrap();
            let root = root.canonicalize().unwrap();
            let workspace = root.join("workspace");
            let state_dir = root.join("state");
            for (path, mode) in [
                (workspace.clone(), 0o700),
                (state_dir.clone(), 0o750),
                (workspace.join(".tentaflake-worker"), 0o700),
                (workspace.join(".tentaflake-worker/inbox"), 0o770),
            ] {
                fs::create_dir(&path).unwrap();
                fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
            }
            for name in ["pending", "running", "approvals", "jobs", "results"] {
                let path = state_dir.join(name);
                fs::create_dir(&path).unwrap();
                // Use the supported operator-created layout here. Nix build
                // sandboxes reject setting setgid; the VM exercises tmpfiles'
                // actual 02750 layout through the public root probe.
                fs::set_permissions(
                    path,
                    fs::Permissions::from_mode(if name == "results" { 0o750 } else { 0o700 }),
                )
                .unwrap();
            }
            let lock = state_dir.join(".queue.lock");
            fs::write(&lock, []).unwrap();
            fs::set_permissions(lock, fs::Permissions::from_mode(0o600)).unwrap();
            let config = Config {
                agent: "fixture".into(),
                backend: "docker".into(),
                runtime: root.join("must-not-execute"),
                image: "fixture:offline".into(),
                workspace,
                state_dir,
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
            Self { root, config }
        }

        fn run(&self) -> Result<Snapshot> {
            let state = open_root(&self.config.state_dir)?;
            let workspace = open_root(&self.config.workspace)?;
            observe(&self.config, &state, &workspace).map(|(snapshot, _)| snapshot)
        }

        fn request(&self, directory: &str, job: &str) -> PathBuf {
            let parent = if directory == "inbox" {
                self.config.workspace.join(".tentaflake-worker/inbox")
            } else {
                self.config.state_dir.join(directory)
            };
            let path = parent.join(format!("{job}.json"));
            let request = serde_json::json!({"version":1,"id":job,"action_class":"communicative",
                "argv":["private-job-content"],"timeout_seconds":1});
            fs::write(&path, serde_json::to_vec(&request).unwrap()).unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
            path
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn snapshot_counts_actual_requests_without_dispatch_repair_or_disclosure() {
        let f = Fixture::new();
        let files = [
            f.request("pending", "first"),
            f.request("running", "second"),
            f.request("inbox", "third"),
        ];
        let before = files
            .iter()
            .map(|p| {
                (
                    Identity::read(&File::open(p).unwrap()).unwrap(),
                    fs::read(p).unwrap(),
                )
            })
            .collect::<Vec<_>>();
        let group = unsafe { libc::getegid() };
        let snapshot = f.run().unwrap();
        assert_eq!(snapshot.pending.entries, 1);
        assert_eq!(snapshot.running.entries, 1);
        assert_eq!(snapshot.inbox.entries, 1);
        assert_eq!(snapshot.pending.bytes, before[0].1.len() as u64);
        assert_eq!(snapshot.running.bytes, before[1].1.len() as u64);
        assert_eq!(snapshot.inbox.bytes, before[2].1.len() as u64);
        assert!(snapshot.observed_unix_seconds > 0);
        let json = serde_json::to_value(snapshot).unwrap();
        assert_eq!(
            json.as_object()
                .unwrap()
                .keys()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            [
                "agent",
                "inbox",
                "limits",
                "observedUnixSeconds",
                "pending",
                "running",
                "schemaVersion"
            ]
        );
        let text = json.to_string();
        for private in [
            "private-job-content",
            "first",
            "second",
            "third",
            f.root.to_str().unwrap(),
        ] {
            assert!(!text.contains(private));
        }
        for (path, (identity, bytes)) in files.iter().zip(before) {
            assert_eq!(fs::read(path).unwrap(), bytes);
            unchanged(&File::open(path).unwrap(), &identity).unwrap();
        }
        assert_eq!(unsafe { libc::getegid() }, group);
        assert!(!f.config.state_dir.join("audit.jsonl").exists());
        assert!(
            fs::read_dir(f.config.state_dir.join("jobs"))
                .unwrap()
                .next()
                .is_none()
        );
        assert!(
            fs::read_dir(f.config.state_dir.join("results"))
                .unwrap()
                .next()
                .is_none()
        );
    }

    #[test]
    fn missing_layout_or_lock_is_not_created() {
        let f = Fixture::new();
        let running = f.config.state_dir.join("running");
        fs::remove_dir(&running).unwrap();
        assert!(f.run().is_err());
        assert!(!running.exists());
        fs::create_dir(&running).unwrap();
        fs::set_permissions(&running, fs::Permissions::from_mode(0o700)).unwrap();
        fs::remove_file(f.config.state_dir.join(".queue.lock")).unwrap();
        assert!(f.run().unwrap_err().contains("queue lock missing"));
        assert!(!f.config.state_dir.join(".queue.lock").exists());
    }

    #[test]
    fn busy_queue_is_refused_without_consuming_approval_or_recovering_claim() {
        let f = Fixture::new();
        let request = f.request("running", "uncertain");
        let before = fs::read(&request).unwrap();
        let lock = File::open(f.config.state_dir.join(".queue.lock")).unwrap();
        assert_eq!(
            unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) },
            0
        );
        assert!(f.run().unwrap_err().contains("busy"));
        assert_eq!(fs::read(request).unwrap(), before);
        drop(lock);
        assert_eq!(f.run().unwrap().running.entries, 1);
    }

    #[test]
    fn wrong_modes_symlinks_and_hardlinks_are_refused_without_repair() {
        let f = Fixture::new();
        let pending = f.config.state_dir.join("pending");
        fs::set_permissions(&pending, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(f.run().is_err());
        assert_eq!(fs::metadata(&pending).unwrap().mode() & 0o7777, 0o755);
        fs::set_permissions(&pending, fs::Permissions::from_mode(0o700)).unwrap();
        let path = f.request("pending", "unsafe");
        let saved = f.root.join("saved");
        fs::rename(&path, &saved).unwrap();
        symlink(&saved, &path).unwrap();
        assert!(f.run().is_err());
        assert!(path.is_symlink());
        fs::remove_file(&path).unwrap();
        fs::hard_link(&saved, &path).unwrap();
        assert!(f.run().is_err());
        assert_eq!(fs::metadata(saved).unwrap().nlink(), 2);
    }

    #[test]
    fn state_owner_and_child_identity_are_checked_from_held_fds() {
        let f = Fixture::new();
        let state = open_root(&f.config.state_dir).unwrap();
        let identity = Identity::read(&state).unwrap();
        assert!(
            identity
                .directory(identity.uid.wrapping_add(1), &[0o750])
                .is_err()
        );
        let old = child(&state, "pending").unwrap();
        let before = Identity::read(&old).unwrap();
        fs::rename(f.config.state_dir.join("pending"), f.root.join("moved")).unwrap();
        fs::create_dir(f.config.state_dir.join("pending")).unwrap();
        assert!(unchanged(&child(&state, "pending").unwrap(), &before).is_err());
        fs::remove_dir(f.config.state_dir.join("pending")).unwrap();
        symlink(f.root.join("moved"), f.config.state_dir.join("pending")).unwrap();
        assert!(f.run().is_err());
    }

    #[test]
    fn combined_private_and_inbox_capacities_are_enforced() {
        let mut f = Fixture::new();
        f.request("pending", "one");
        f.request("running", "two");
        f.config.max_queue_entries = 1;
        assert!(f.run().is_err());
        f.config.max_queue_entries = 2;
        f.config.max_queue_bytes = 1;
        assert!(f.run().is_err());
        f.config.max_queue_bytes = 1024 * 1024;
        f.request("inbox", "three");
        f.request("inbox", "four");
        f.config.max_inbox_entries = 1;
        assert!(f.run().is_err());
        f.config.max_inbox_entries = 2;
        assert!(f.run().is_ok());
    }

    #[test]
    fn malformed_partial_and_conflicting_requests_are_not_recovered() {
        let f = Fixture::new();
        let path = f.request("pending", "same");
        let staging = f.config.state_dir.join("pending/.same.json.new");
        fs::write(&staging, "partial").unwrap();
        assert!(f.run().is_err());
        assert_eq!(fs::read(&staging).unwrap(), b"partial");
        fs::remove_file(staging).unwrap();
        f.request("inbox", "same");
        assert!(f.run().unwrap_err().contains("conflicting"));
        fs::remove_file(
            f.config
                .workspace
                .join(".tentaflake-worker/inbox/same.json"),
        )
        .unwrap();
        fs::write(&path, r#"{"version":1,"id":"wrong","action_class":"communicative","argv":["private"],"timeout_seconds":1}"#).unwrap();
        assert!(f.run().unwrap_err().contains("identity"));
        fs::write(&path, "private-malformed-request").unwrap();
        let error = f.run().unwrap_err();
        assert!(!error.contains("private-malformed-request"));
        assert_eq!(fs::read(path).unwrap(), b"private-malformed-request");
    }

    #[test]
    fn special_files_nonempty_locks_and_expired_observations_are_refused() {
        let f = Fixture::new();
        let pending = open_root(&f.config.state_dir.join("pending")).unwrap();
        assert!(
            requests(
                &f.config,
                &pending,
                Some(unsafe { libc::geteuid() }),
                Instant::now()
            )
            .unwrap_err()
            .contains("deadline")
        );
        let path = f.config.state_dir.join("pending/fifo.json");
        let name = CString::new(path.as_os_str().as_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
        assert!(f.run().is_err());
        fs::remove_file(path).unwrap();
        let lock = f.config.state_dir.join(".queue.lock");
        fs::write(&lock, b"unexpected-lock-content").unwrap();
        assert!(f.run().unwrap_err().contains("queue lock"));
        assert_eq!(fs::read(lock).unwrap(), b"unexpected-lock-content");
    }
}
