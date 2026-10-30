//! Private queue ownership and durable transitions. All mutations hold `lock`.
use super::*;

pub(super) const DRAIN_REQUEST: &str = ".drain-request";

// Called at drain entry with the queue lock held. A later decision cannot
// publish its wakeup until this invocation has finished all queue processing.
pub(super) fn consume_drain_request(config: &Config) -> Result<()> {
    match fs::remove_file(config.state_dir.join(DRAIN_REQUEST)) {
        Ok(()) => sync_directory(&config.state_dir),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("consume drain request: {error}")),
    }
}

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

pub(super) fn sync_directory(path: &Path) -> Result<()> {
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

// Empty ID receipts outlive result retention; running claims reserve full
// worst-case capacity, while completed trees are charged their actual usage.
#[derive(Debug)]
pub(super) struct RetainedUsage {
    receipts: Vec<String>,
    running: Vec<bool>,
    bytes: u64,
    entries: u64,
    results: usize,
}

fn job_storage_budget(config: &Config) -> CopyBudget {
    CopyBudget {
        bytes_left: config
            .max_snapshot_bytes
            .saturating_mul(2)
            .saturating_add(config.max_log_bytes as u64)
            .saturating_add(16 * 1024),
        entries_left: config
            .max_snapshot_entries
            .saturating_mul(2)
            .saturating_add(8),
    }
}

// Publication can stop before the link or between link and staging unlink.
// Recover only empty private staging files whose link count proves either
// state; never remove a published ID receipt or an unexplained hardlink.
fn recover_reservation_staging(config: &Config) -> Result<()> {
    let parent = config.state_dir.join("reservations");
    let fd = open_path_no_symlinks(&parent)?;
    let names = list_fd_dir(fd.as_raw_fd(), 8192, false)?;
    let mut staging = Vec::new();
    for name in &names {
        let raw = name.to_str().ok_or("invalid retained reservation name")?;
        if !raw.starts_with('.') {
            continue;
        }
        let job = raw
            .strip_prefix('.')
            .and_then(|s| s.strip_suffix(".new"))
            .ok_or("unknown reservation staging")?;
        validate_identifier("reservation staging", job, 64)?;
        let stat = fstatat_nofollow(fd.as_raw_fd(), name)?;
        if stat.st_mode & libc::S_IFMT != libc::S_IFREG
            || stat.st_size != 0
            || stat.st_mode & 0o077 != 0
            || stat.st_uid != unsafe { libc::geteuid() }
        {
            return Err("unsafe reservation staging".into());
        }
        if names.iter().any(|name| name == OsStr::new(job)) {
            let published = fstatat_nofollow(fd.as_raw_fd(), OsStr::new(job))?;
            if stat.st_nlink != 2
                || published.st_nlink != 2
                || stat.st_dev != published.st_dev
                || stat.st_ino != published.st_ino
            {
                return Err("reservation staging does not match published receipt".into());
            }
        } else if stat.st_nlink != 1 {
            return Err("reservation staging has unexplained hardlinks".into());
        }
        staging.push(name);
    }
    for name in &staging {
        unlink_at(fd.as_raw_fd(), name)?;
    }
    if !staging.is_empty() {
        sync_directory(&parent)?;
    }
    Ok(())
}

pub(super) fn check_retained(config: &Config) -> Result<RetainedUsage> {
    recover_reservation_staging(config)?;
    let fd = open_path_no_symlinks(&config.state_dir.join("reservations"))?;
    let names = list_fd_dir(fd.as_raw_fd(), 4096, false)?;
    let mut reserved = Vec::with_capacity(names.len());
    for name in names {
        let job = name.to_str().ok_or("invalid retained reservation name")?;
        validate_identifier("retained reservation", job, 64)?;
        let stat = fstatat_nofollow(fd.as_raw_fd(), &name)?;
        if stat.st_mode & libc::S_IFMT != libc::S_IFREG
            || stat.st_nlink != 1
            || stat.st_size != 0
            || stat.st_mode & 0o022 != 0
            || stat.st_uid != unsafe { libc::geteuid() }
        {
            return Err("unsafe retained reservation".into());
        }
        reserved.push(job.to_owned());
    }
    reserved.sort_unstable();
    let full = job_storage_budget(config);
    let mut budgets: Vec<CopyBudget> = reserved
        .iter()
        .map(|_| job_storage_budget(config))
        .collect();
    let mut running = vec![false; reserved.len()];
    let mut occupied = vec![false; reserved.len()];
    let mut occupied_count = 0;
    let mut total_bytes = 0_u64;
    let mut total_entries = 0_u64;
    for directory in ["running", "results", "jobs"] {
        let fd = open_path_no_symlinks(&config.state_dir.join(directory))?;
        let names = list_fd_dir(
            fd.as_raw_fd(),
            (config.max_retained_results as u64).saturating_mul(2),
            false,
        )?;
        for name in names {
            let raw = name.to_str().ok_or("invalid retained state name")?;
            let job = if directory == "running" {
                raw.strip_suffix(".json").ok_or("unknown running state")?
            } else if directory == "results" && raw.starts_with('.') {
                raw.strip_prefix('.')
                    .and_then(|s| s.strip_suffix(".tmp"))
                    .ok_or("unknown result staging")?
            } else {
                raw
            };
            let index = reserved
                .binary_search_by(|id| id.as_str().cmp(job))
                .map_err(|_| "unreserved legacy worker state; operator reconciliation required")?;
            if !occupied[index] {
                occupied_count += 1;
                occupied[index] = true;
            }
            if occupied_count > config.max_retained_results {
                return Err("retained result count ceiling exceeded".into());
            }
            let stat = fstatat_nofollow(fd.as_raw_fd(), &name)?;
            if stat.st_uid != unsafe { libc::geteuid() }
                || stat.st_mode & 0o022 != 0
                || (directory == "jobs" && stat.st_mode & 0o077 != 0)
            {
                return Err("unsafe retained state ownership or permissions".into());
            }
            if directory == "running" {
                if stat.st_mode & libc::S_IFMT != libc::S_IFREG
                    || stat.st_nlink != 1
                    || stat.st_size < 0
                    || stat.st_size as u64 > config.max_request_bytes
                {
                    return Err("unsafe retained running claim".into());
                }
                running[index] = true;
                total_bytes = total_bytes
                    .checked_add(full.bytes_left)
                    .ok_or("retained size overflow")?;
                total_entries = total_entries
                    .checked_add(full.entries_left)
                    .ok_or("retained entry overflow")?;
                if total_bytes > config.max_retained_result_bytes
                    || total_entries > config.max_retained_entries
                {
                    return Err("retained running reservations exceed aggregate capacity".into());
                }
                continue;
            }
            if stat.st_mode & libc::S_IFMT != libc::S_IFDIR {
                return Err("retained state must be a directory".into());
            }
            let before_bytes = budgets[index].bytes_left;
            let before_entries = budgets[index].entries_left;
            let child = open_dir_at(fd.as_raw_fd(), &name).map_err(|e| e.to_string())?;
            scan_retained_tree(
                child.as_raw_fd(),
                &mut budgets[index],
                0,
                directory == "jobs",
            )?;
            if !running[index] {
                total_bytes = total_bytes
                    .checked_add(before_bytes - budgets[index].bytes_left)
                    .ok_or("retained size overflow")?;
                total_entries = total_entries
                    .checked_add(before_entries - budgets[index].entries_left)
                    .ok_or("retained entry overflow")?;
                if total_bytes > config.max_retained_result_bytes
                    || total_entries > config.max_retained_entries
                {
                    return Err("retained trees exceed aggregate capacity".into());
                }
            }
        }
    }
    Ok(RetainedUsage {
        results: occupied_count,
        receipts: reserved,
        running,
        bytes: total_bytes,
        entries: total_entries,
    })
}

fn scan_retained_tree(
    fd: RawFd,
    budget: &mut CopyBudget,
    depth: usize,
    snapshot: bool,
) -> Result<()> {
    // One wrapper (input, artifacts-export, or artifacts) surrounds every
    // producer tree below the job/result directory scanned at depth zero.
    if depth > MAX_TREE_DEPTH + 1 {
        return Err("retained directory depth exceeds ceiling".into());
    }
    for name in list_fd_dir(fd, budget.entries_left, false)? {
        budget.entries_left = budget
            .entries_left
            .checked_sub(1)
            .ok_or("retained entry ceiling exceeded")?;
        let stat = fstatat_nofollow(fd, &name)?;
        // Snapshot descendants preserve source modes. Their root-owned 0700
        // job ancestor, checked above, prevents controller access; result
        // trees have no such private ancestor and must remain non-writable.
        if stat.st_uid != unsafe { libc::geteuid() }
            || (!snapshot
                && stat.st_mode & libc::S_IFMT != libc::S_IFLNK
                && stat.st_mode & 0o022 != 0)
        {
            return Err("unsafe retained entry ownership or permissions".into());
        }
        match stat.st_mode & libc::S_IFMT {
            libc::S_IFDIR => {
                let child = open_dir_at(fd, &name).map_err(|e| e.to_string())?;
                scan_retained_tree(child.as_raw_fd(), budget, depth + 1, snapshot)?;
            }
            libc::S_IFREG if stat.st_nlink == 1 => {
                let size = u64::try_from(stat.st_size).map_err(|_| "invalid retained size")?;
                budget.bytes_left = budget
                    .bytes_left
                    .checked_sub(size)
                    .ok_or("retained byte ceiling exceeded")?;
            }
            libc::S_IFLNK if snapshot && stat.st_size <= 4096 => {}
            _ => return Err("unsafe retained entry".into()),
        }
    }
    Ok(())
}

pub(super) fn reserve_result(config: &Config, job: &str) -> Result<()> {
    let usage = check_retained(config)?;
    let existing = usage.receipts.iter().position(|id| id == job);
    if existing.is_some_and(|index| usage.running[index]) {
        return Ok(());
    }
    let full = job_storage_budget(config);
    if usage.results >= config.max_retained_results
        || full.bytes_left > config.max_retained_result_bytes.saturating_sub(usage.bytes)
        || full.entries_left > config.max_retained_entries.saturating_sub(usage.entries)
        || (existing.is_none() && usage.receipts.len() >= 4096)
    {
        return Err("retained worker capacity is full; no results or receipts were removed".into());
    }
    if existing.is_none() {
        write_private(&config.state_dir.join("reservations").join(job), b"")?;
    }
    Ok(())
}

pub(super) fn claim(config: &Config, request: &JobRequest) -> Result<()> {
    reserve_result(config, &request.id)?;
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
    for directory in ["pending", "approvals", "decisions"] {
        let parent = config.state_dir.join(directory);
        if directory == "decisions"
            && matches!(fs::symlink_metadata(&parent), Err(error) if error.kind() == io::ErrorKind::NotFound)
        {
            continue;
        }
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
                max_retained_results: 1024,
                max_retained_result_bytes: default_retained_result_bytes(),
                max_retained_entries: default_retained_entries(),
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
        assert!(drain(&f.config).unwrap_err().contains("unreserved legacy"));
        assert_eq!(f.calls(), "");
        assert!(pending_path(&f.config, &request.id).exists());
        assert!(f.config.state_dir.join("jobs/job_1/input").is_dir());
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

    #[test]
    fn aggregate_byte_reservation_is_exact_and_archival_releases_capacity() {
        let mut f = Fixture::new("aggregate-retained");
        let full = job_storage_budget(&f.config).bytes_left;
        f.config.max_retained_result_bytes = full + 100;
        reserve_result(&f.config, "old").unwrap();
        let result = f.config.state_dir.join("results/old");
        fs::create_dir(&result).unwrap();
        fs::write(result.join("artifact"), [0_u8; 101]).unwrap();
        assert!(
            reserve_result(&f.config, "next")
                .unwrap_err()
                .contains("capacity is full")
        );
        fs::write(result.join("artifact"), [0_u8; 100]).unwrap();
        let mut request = f.request(ActionClass::LocalReversible);
        request.id = "next".into();
        f.pending(&request);
        claim(&f.config, &request).unwrap();
        assert_eq!(check_retained(&f.config).unwrap().bytes, full + 100);
        assert!(reserve_result(&f.config, "third").is_err());
        // Explicit operator archival preserves the artifact outside active state.
        fs::rename(&result, f.root.join("archived-result")).unwrap();
        assert!(job_exists(&f.config, "old"));
        assert_eq!(check_retained(&f.config).unwrap().bytes, full);
        assert_eq!(
            fs::read(f.root.join("archived-result/artifact"))
                .unwrap()
                .len(),
            100
        );
        assert_eq!(f.calls(), "");
    }

    #[test]
    fn retained_slots_stop_dispatch_without_losing_receipts() {
        let mut f = Fixture::new("retained-slots");
        f.config.max_retained_results = 2;
        for id in ["job_1", "job_2"] {
            let mut request = f.request(ActionClass::LocalReversible);
            request.id = id.into();
            f.pending(&request);
            assert!(drain(&f.config).is_err()); // fake OCI create fails
            drain(&f.config).unwrap(); // interrupted, never replayed
        }
        let receipt = fs::read(f.config.state_dir.join("results/job_1/result.json")).unwrap();
        let mut third = f.request(ActionClass::LocalReversible);
        third.id = "job_3".into();
        f.pending(&third);
        let calls = f.calls();
        for _ in 0..3 {
            assert!(drain(&f.config).unwrap_err().contains("capacity is full"));
        }
        assert_eq!(f.calls(), calls);
        assert_eq!(
            fs::read(f.config.state_dir.join("results/job_1/result.json")).unwrap(),
            receipt
        );
        assert!(pending_path(&f.config, "job_3").exists());
        assert_eq!(check_retained(&f.config).unwrap().results, 2);
        assert_eq!(
            fs::read_dir(f.config.state_dir.join("reservations"))
                .unwrap()
                .count(),
            2
        );
    }

    #[test]
    fn retained_scan_enforces_exact_bytes_entries_and_unsafe_state() {
        let mut f = Fixture::new("retained-scan");
        f.config.max_retained_results = 1;
        reserve_result(&f.config, "job_1").unwrap();
        let result = f.config.state_dir.join("results/job_1");
        fs::create_dir(&result).unwrap();
        let path = result.join("artifact");
        let ceiling = 2 * f.config.max_snapshot_bytes + f.config.max_log_bytes as u64 + 16 * 1024;
        let file = File::create(&path).unwrap();
        file.set_len(ceiling).unwrap();
        check_retained(&f.config).unwrap();
        file.set_len(ceiling + 1).unwrap();
        assert!(
            check_retained(&f.config)
                .unwrap_err()
                .contains("byte ceiling")
        );
        file.set_len(0).unwrap();
        let entries = 2 * f.config.max_snapshot_entries + 8;
        for n in 1..entries {
            fs::write(result.join(format!("entry_{n}")), b"").unwrap();
        }
        check_retained(&f.config).unwrap();
        fs::write(result.join("overflow"), b"").unwrap();
        assert!(check_retained(&f.config).is_err());
        fs::remove_file(result.join("overflow")).unwrap();
        fs::remove_file(&path).unwrap();
        symlink(&f.root, &path).unwrap();
        assert!(check_retained(&f.config).unwrap_err().contains("unsafe"));
        assert!(
            fs::symlink_metadata(&path)
                .unwrap()
                .file_type()
                .is_symlink()
        );
    }

    #[test]
    fn aggregate_entries_reserve_the_full_inflight_tree() {
        let mut f = Fixture::new("aggregate-entries");
        let full = job_storage_budget(&f.config).entries_left;
        f.config.max_retained_entries = full + 1;
        reserve_result(&f.config, "old").unwrap();
        let result = f.config.state_dir.join("results/old");
        fs::create_dir(&result).unwrap();
        fs::write(result.join("one"), b"").unwrap();
        fs::write(result.join("two"), b"").unwrap();
        assert!(
            reserve_result(&f.config, "next")
                .unwrap_err()
                .contains("capacity is full")
        );
        fs::rename(result.join("two"), f.root.join("archived-entry")).unwrap();
        reserve_result(&f.config, "next").unwrap();
        assert_eq!(check_retained(&f.config).unwrap().entries, 1);
        assert!(result.join("one").exists());
        assert!(f.root.join("archived-entry").exists());
    }

    #[test]
    fn reservation_publication_crashes_recover_without_losing_receipts() {
        for linked in [false, true] {
            let f = Fixture::new("reservation-publication");
            let parent = f.config.state_dir.join("reservations");
            let staging = parent.join(".job_1.new");
            let receipt = parent.join("job_1");
            fs::write(&staging, b"").unwrap();
            fs::set_permissions(&staging, fs::Permissions::from_mode(0o600)).unwrap();
            if linked {
                fs::hard_link(&staging, &receipt).unwrap();
            }
            drain(&f.config).unwrap();
            assert!(!staging.exists());
            assert_eq!(receipt.exists(), linked);
            if linked {
                assert!(fs::read(&receipt).unwrap().is_empty());
                assert!(job_exists(&f.config, "job_1"));
            }
            assert_eq!(f.calls(), "");
        }
    }

    #[test]
    fn unsafe_reservation_staging_is_preserved_and_refused() {
        for case in [
            "symlink",
            "nonempty",
            "unrelated-link",
            "mismatched-receipt",
        ] {
            let f = Fixture::new("unsafe-reservation");
            let parent = f.config.state_dir.join("reservations");
            let staging = parent.join(".job_1.new");
            let outside = f.root.join("outside");
            fs::write(&outside, b"").unwrap();
            fs::set_permissions(&outside, fs::Permissions::from_mode(0o600)).unwrap();
            match case {
                "symlink" => symlink(&outside, &staging).unwrap(),
                "unrelated-link" => fs::hard_link(&outside, &staging).unwrap(),
                _ => {
                    fs::write(
                        &staging,
                        if case == "nonempty" {
                            b"x".as_slice()
                        } else {
                            b"".as_slice()
                        },
                    )
                    .unwrap();
                    fs::set_permissions(&staging, fs::Permissions::from_mode(0o600)).unwrap();
                    if case == "mismatched-receipt" {
                        fs::write(parent.join("job_1"), b"").unwrap();
                    }
                }
            }
            assert!(drain(&f.config).is_err(), "{case}");
            assert!(fs::symlink_metadata(&staging).is_ok());
            assert!(outside.exists());
            assert_eq!(f.calls(), "");
        }
    }

    #[test]
    fn writable_source_modes_do_not_block_private_snapshot_recovery() {
        let f = Fixture::new("snapshot-modes");
        let request = f.request(ActionClass::LocalReversible);
        f.pending(&request);
        claim(&f.config, &request).unwrap();
        let job = f.config.state_dir.join("jobs/job_1");
        let input = job.join("input");
        fs::create_dir(&job).unwrap();
        fs::set_permissions(&job, fs::Permissions::from_mode(0o700)).unwrap();
        fs::create_dir(&input).unwrap();
        fs::set_permissions(&input, fs::Permissions::from_mode(0o775)).unwrap();
        fs::write(input.join("source"), b"fixture").unwrap();
        fs::set_permissions(input.join("source"), fs::Permissions::from_mode(0o664)).unwrap();
        drain(&f.config).unwrap();
        assert!(!job.exists());
        assert!(!f.calls().contains("create "));
        let result: JobResult = serde_json::from_slice(
            &fs::read(f.config.state_dir.join("results/job_1/result.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(result.status, "interrupted");
        assert!(!f.config.state_dir.join("running/job_1.json").exists());
    }

    #[test]
    fn deepest_imported_artifacts_remain_valid_after_publication() {
        let mut f = Fixture::new("artifact-depth");
        f.config.max_snapshot_entries = 512;
        let request = f.request(ActionClass::LocalReversible);
        reserve_result(&f.config, &request.id).unwrap();
        let staging = f.config.state_dir.join("results/.job_1.tmp");
        let artifacts = staging.join("artifacts");
        fs::create_dir_all(&artifacts).unwrap();
        let relative: PathBuf = std::iter::repeat_n("d", MAX_TREE_DEPTH).collect();
        let mut builder = tar::Builder::new(Vec::new());
        let mut header = tar::Header::new_gnu();
        header.set_entry_type(tar::EntryType::dir());
        header.set_mode(0o750);
        header.set_size(0);
        header.set_cksum();
        builder
            .append_data(&mut header, &relative, io::empty())
            .unwrap();
        let archive = builder.into_inner().unwrap();
        import_artifact_tar(archive.as_slice(), &artifacts, 1024, 512).unwrap();
        publish_result(&staging, &f.config.state_dir.join("results/job_1")).unwrap();
        check_retained(&f.config).unwrap();
        assert!(
            f.config
                .state_dir
                .join("results/job_1/artifacts")
                .join(relative)
                .is_dir()
        );
    }

    #[test]
    fn snapshot_depth_boundary_recovers_and_overflow_never_dispatches() {
        for extra in [0, 1] {
            let mut f = Fixture::new("snapshot-depth");
            f.config.max_snapshot_entries = 512;
            let relative: PathBuf = std::iter::repeat_n("d", MAX_TREE_DEPTH + extra).collect();
            fs::create_dir_all(f.config.workspace.join(&relative)).unwrap();
            let request = f.request(ActionClass::LocalReversible);
            f.pending(&request);
            if extra == 0 {
                claim(&f.config, &request).unwrap();
                let job = f.config.state_dir.join("jobs/job_1");
                let input = job.join("input");
                fs::create_dir(&job).unwrap();
                fs::set_permissions(&job, fs::Permissions::from_mode(0o700)).unwrap();
                fs::create_dir(&input).unwrap();
                let source = open_path_no_symlinks(&f.config.workspace).unwrap();
                let mut budget = CopyBudget {
                    bytes_left: 65536,
                    entries_left: 512,
                };
                copy_directory_fd(source.as_raw_fd(), &input, &mut budget, true).unwrap();
                check_retained(&f.config).unwrap();
            }
            drain(&f.config).unwrap();
            let result: JobResult = serde_json::from_slice(
                &fs::read(f.config.state_dir.join("results/job_1/result.json")).unwrap(),
            )
            .unwrap();
            assert_eq!(
                result.status,
                if extra == 0 {
                    "interrupted"
                } else {
                    "rejected"
                }
            );
            assert!(!f.calls().contains("create "));
            assert!(!f.config.state_dir.join("jobs/job_1").exists());
            drain(&f.config).unwrap();
        }
    }

    #[test]
    fn repeated_audit_failures_do_not_grow_metadata() {
        let f = Fixture::new("audit-bound");
        let path = f.config.state_dir.join("audit.jsonl");
        let file = File::create(&path).unwrap();
        file.set_len(1024 * 1024).unwrap();
        for _ in 0..3 {
            assert!(audit(&f.config, "unknown", "rejected", None, "fixture").is_err());
        }
        assert_eq!(file.metadata().unwrap().len(), 1024 * 1024);
    }
}
