use serde::{Deserialize, Serialize};
use std::ffi::{CStr, CString, OsStr, OsString};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt, symlink};
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

type Result<T> = std::result::Result<T, String>;

const RESOLVE_NO_MAGICLINKS: u64 = 0x02;
const RESOLVE_NO_SYMLINKS: u64 = 0x04;
const COMPLETION_MARKER: &str = "/workspace/.tentaflake-worker-complete";
const MAX_INBOX_ENTRIES: u64 = 1024;
const MAX_INBOX_SCAN_ENTRIES: u64 = 1024;
const MAX_PENDING_REQUESTS_LIMIT: u64 = 4096;
const MAX_PENDING_BYTES_LIMIT: u64 = 512 * 1024 * 1024;
const MAX_READY_JOBS_PER_DRAIN_LIMIT: u64 = 128;
const INBOX_CURSOR_FILE: &str = "inbox.cursor";
const STATE_LAYOUT_MARKER_FILE: &str = ".tentaflake-worker-state-v1";
const STATE_LAYOUT_MARKER: &[u8] = b"tentaflake-worker-state-v1\n";
const PENDING_FILE_ALLOCATION_BYTES: u64 = 4096;
const PENDING_METADATA_INODES: u64 = 4;
const STATE_CONTROL_RESERVE_BYTES: u64 = 16 * 1024 * 1024;
const STATE_CONTROL_RESERVE_INODES: u64 = 1024;
const WORKSPACE_INIT_SCRIPT: &str = r#"
cp -R --no-preserve=ownership /input/. /workspace/
cd /workspace
rm -f .tentaflake-worker-complete
trap '' USR1
set +e
"$@"
status=$?
nonce="$(od -An -N16 -tx1 /dev/urandom | tr -d ' \n')"
trap 'exit "$status"' USR1
printf 'TFW1:%s:%s\n' "$nonce" "$status" > .tentaflake-worker-complete
while :; do
  sleep 3600 &
  wait "$!" || true
done
"#;

#[repr(C)]
struct OpenHow {
    flags: u64,
    mode: u64,
    resolve: u64,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Config {
    agent: String,
    backend: String,
    runtime: PathBuf,
    image: String,
    workspace: PathBuf,
    state_dir: PathBuf,
    container_uid: u32,
    container_gid: u32,
    max_request_bytes: u64,
    max_pending_requests: u64,
    max_pending_bytes: u64,
    max_ready_jobs_per_drain: u64,
    max_snapshot_bytes: u64,
    max_snapshot_entries: u64,
    max_log_bytes: usize,
    max_timeout_seconds: u64,
    memory: String,
    memory_swap: String,
    cpus: String,
    pids_limit: u32,
    workspace_tmpfs_size: String,
    tmp_tmpfs_size: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
enum ActionClass {
    LocalReversible,
    ExternalReversible,
    Irreversible,
    Financial,
    Production,
    Communicative,
    Privileged,
    Forbidden,
}

impl ActionClass {
    fn needs_approval(self) -> bool {
        !matches!(self, Self::LocalReversible | Self::Forbidden)
    }
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct JobRequest {
    version: u8,
    id: String,
    action_class: ActionClass,
    argv: Vec<String>,
    #[serde(default = "default_timeout")]
    timeout_seconds: u64,
}

#[derive(Debug, Serialize)]
struct JobResult {
    version: u8,
    id: String,
    action_class: ActionClass,
    status: String,
    exit_code: Option<i32>,
    timed_out: bool,
    artifacts_available: bool,
    log_truncated: bool,
    completed_unix_seconds: u64,
    message: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct CompletionMarker {
    raw: String,
    exit_code: i32,
}

struct ArtifactExport {
    available: bool,
    diagnostic: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct InboxDirectoryStamp {
    device: u64,
    inode: u64,
    change_seconds: i64,
    change_nanoseconds: i64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct InboxCursor {
    offset: libc::c_long,
    stamp: InboxDirectoryStamp,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct InboxCursorRead {
    cursor: Option<InboxCursor>,
    reset: bool,
}

struct InboxScan {
    names: Vec<OsString>,
    deferred: bool,
    ignored_entries: u64,
    scanned_entries: u64,
    next_cursor: Option<libc::c_long>,
    directory_stamp: InboxDirectoryStamp,
    directory_changed: bool,
    stale_cursor: bool,
}

struct PendingUsage {
    requests: u64,
    bytes: u64,
}

struct WorkerStateLock {
    _file: File,
}

impl WorkerStateLock {
    fn acquire(state_dir: &Path) -> Result<Self> {
        let path = state_dir.join("worker.lock");
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .mode(0o600)
            .open(&path)
            .map_err(|error| format!("open worker state lock {}: {error}", path.display()))?;
        let result = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) };
        if result == 0 {
            Ok(Self { _file: file })
        } else {
            Err(format!(
                "lock worker state {}: {}",
                path.display(),
                io::Error::last_os_error()
            ))
        }
    }
}

impl PendingUsage {
    fn has_capacity_for(&self, config: &Config, request_bytes: u64) -> bool {
        self.requests < config.max_pending_requests
            && request_bytes <= config.max_pending_bytes.saturating_sub(self.bytes)
    }

    fn reserve(&mut self, request_bytes: u64) -> Result<()> {
        self.requests = self
            .requests
            .checked_add(1)
            .ok_or_else(|| "pending request count overflow".to_string())?;
        self.bytes = self
            .bytes
            .checked_add(request_bytes)
            .ok_or_else(|| "pending byte count overflow".to_string())?;
        Ok(())
    }
}

#[derive(Debug, PartialEq, Eq)]
enum WorkerGroupAction {
    Keep,
    Adopt,
}

#[derive(Debug, Serialize)]
struct AuditEvent<'a> {
    timestamp_unix_seconds: u64,
    agent: &'a str,
    job: &'a str,
    event: &'a str,
    action_class: Option<ActionClass>,
    detail: &'a str,
}

fn default_timeout() -> u64 {
    300
}

fn main() {
    if let Err(error) = run() {
        eprintln!("tentaflake-worker: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let mut args = std::env::args().skip(1);
    if args.next().as_deref() != Some("--config") {
        return Err("usage: tentaflake-worker --config FILE drain|approve|deny [JOB]".into());
    }
    let config_path = args
        .next()
        .ok_or_else(|| "--config requires a path".to_string())?;
    let command = args
        .next()
        .ok_or_else(|| "missing drain, approve, or deny command".to_string())?;
    let config: Config = serde_json::from_reader(
        File::open(&config_path).map_err(|error| format!("open {config_path}: {error}"))?,
    )
    .map_err(|error| format!("parse {config_path}: {error}"))?;
    validate_config(&config)?;
    enter_worker_group(&config)?;
    ensure_state_layout(&config)?;
    let _state_lock = WorkerStateLock::acquire(&config.state_dir)?;
    recover_inflight(&config)?;

    match command.as_str() {
        "drain" => {
            reject_extra_args(args)?;
            drain(&config)
        }
        "approve" => {
            let job = required_job(args)?;
            approve(&config, &job)
        }
        "deny" => {
            let job = required_job(args)?;
            deny(&config, &job)
        }
        _ => Err(format!("unknown command: {command}")),
    }
}

fn enter_worker_group(config: &Config) -> Result<()> {
    let effective_uid = unsafe { libc::geteuid() };
    let effective_gid = unsafe { libc::getegid() };
    match worker_group_action(effective_uid, effective_gid, config.container_gid)? {
        WorkerGroupAction::Keep => Ok(()),
        WorkerGroupAction::Adopt => {
            let result = unsafe { libc::setegid(config.container_gid) };
            if result == 0 && unsafe { libc::getegid() } == config.container_gid {
                Ok(())
            } else {
                Err(format!(
                    "adopt worker group {}: {}",
                    config.container_gid,
                    io::Error::last_os_error()
                ))
            }
        }
    }
}

fn worker_group_action(
    effective_uid: u32,
    effective_gid: u32,
    container_gid: u32,
) -> Result<WorkerGroupAction> {
    if effective_gid == container_gid {
        Ok(WorkerGroupAction::Keep)
    } else if effective_uid == 0 {
        Ok(WorkerGroupAction::Adopt)
    } else {
        Err(format!(
            "worker must run as root or with effective group {container_gid}; current group is {effective_gid}"
        ))
    }
}

fn required_job(mut args: impl Iterator<Item = String>) -> Result<String> {
    let job = args.next().ok_or_else(|| "missing job id".to_string())?;
    reject_extra_args(args)?;
    validate_identifier("job id", &job, 64)?;
    Ok(job)
}

fn reject_extra_args(mut args: impl Iterator<Item = String>) -> Result<()> {
    if let Some(value) = args.next() {
        Err(format!("unexpected argument: {value}"))
    } else {
        Ok(())
    }
}

fn validate_config(config: &Config) -> Result<()> {
    validate_identifier("agent", &config.agent, 48)?;
    if !matches!(config.backend.as_str(), "docker" | "podman") {
        return Err("backend must be docker or podman".into());
    }
    if !config.runtime.is_absolute()
        || !config.workspace.is_absolute()
        || !config.state_dir.is_absolute()
    {
        return Err("runtime, workspace, and state_dir must be absolute paths".into());
    }
    if config.workspace.as_os_str().as_bytes().contains(&b',') {
        return Err("workspace may not contain a comma".into());
    }
    if config.image.is_empty() || config.image.contains(char::is_whitespace) {
        return Err("image must be one non-empty OCI reference".into());
    }
    if config.max_request_bytes == 0
        || config.max_pending_requests == 0
        || config.max_pending_bytes == 0
        || config.max_ready_jobs_per_drain == 0
        || config.max_snapshot_bytes == 0
        || config.max_snapshot_entries == 0
        || config.max_log_bytes == 0
        || config.max_timeout_seconds == 0
        || config.pids_limit == 0
    {
        return Err("worker limits must all be positive".into());
    }
    if config.max_pending_bytes < config.max_request_bytes {
        return Err("max_pending_bytes must accommodate one max_request_bytes request".into());
    }
    if config.max_pending_requests > MAX_PENDING_REQUESTS_LIMIT
        || config.max_pending_bytes > MAX_PENDING_BYTES_LIMIT
        || config.max_ready_jobs_per_drain > MAX_READY_JOBS_PER_DRAIN_LIMIT
        || config.max_ready_jobs_per_drain > config.max_pending_requests
    {
        return Err("configured private queue limits exceed the worker safety bounds".into());
    }
    Ok(())
}

fn validate_identifier(kind: &str, value: &str, max: usize) -> Result<()> {
    let valid = !value.is_empty()
        && value.len() <= max
        && value.bytes().enumerate().all(|(index, byte)| {
            byte.is_ascii_lowercase()
                || byte.is_ascii_digit()
                || (index > 0 && matches!(byte, b'-' | b'_'))
        });
    if valid {
        Ok(())
    } else {
        Err(format!(
            "{kind} must be lowercase ASCII, begin with a letter or digit, and be at most {max} bytes"
        ))
    }
}

fn validate_request(config: &Config, request: &JobRequest) -> Result<()> {
    if request.version != 1 {
        return Err("request version must be 1".into());
    }
    validate_identifier("job id", &request.id, 64)?;
    if request.argv.is_empty() || request.argv.len() > 128 {
        return Err("argv must contain between 1 and 128 entries".into());
    }
    if request
        .argv
        .iter()
        .any(|value| value.is_empty() || value.len() > 4096 || value.contains('\0'))
    {
        return Err("argv contains an empty, NUL, or overlong value".into());
    }
    if request.timeout_seconds == 0 || request.timeout_seconds > config.max_timeout_seconds {
        return Err(format!(
            "timeout_seconds must be between 1 and {}",
            config.max_timeout_seconds
        ));
    }
    Ok(())
}

fn require_directory(path: &Path, label: &str) -> Result<()> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| format!("stat {label} {}: {error}", path.display()))?;
    if metadata.is_dir() && !metadata.file_type().is_symlink() {
        Ok(())
    } else {
        Err(format!(
            "{label} is not a safe directory: {}",
            path.display()
        ))
    }
}

fn verify_state_layout_marker(config: &Config) -> Result<()> {
    let path = config.state_dir.join(STATE_LAYOUT_MARKER_FILE);
    let metadata = fs::symlink_metadata(&path)
        .map_err(|error| format!("stat state layout marker {}: {error}", path.display()))?;
    if !metadata.file_type().is_file()
        || metadata.file_type().is_symlink()
        || metadata.len() != u64::try_from(STATE_LAYOUT_MARKER.len()).unwrap_or(u64::MAX)
    {
        return Err(format!(
            "worker state layout marker is unsafe or missing: {}",
            path.display()
        ));
    }
    let mut file = File::open(&path)
        .map_err(|error| format!("open state layout marker {}: {error}", path.display()))?;
    let content = read_bounded(&mut file, STATE_LAYOUT_MARKER.len())
        .map_err(|error| format!("read state layout marker {}: {error}", path.display()))?;
    if content.as_slice() == STATE_LAYOUT_MARKER {
        Ok(())
    } else {
        Err(format!(
            "worker state layout marker has unexpected content: {}",
            path.display()
        ))
    }
}

fn ensure_state_layout(config: &Config) -> Result<()> {
    require_directory(&config.state_dir, "worker state root")?;
    verify_state_layout_marker(config)?;
    for name in ["pending", "inflight", "jobs"] {
        require_directory(&config.state_dir.join(name), "worker state directory")?;
    }
    let results = config.state_dir.join("results");
    require_directory(&results, "worker result directory")?;
    harden_shared_directory(&results)
}

fn harden_shared_directory(path: &Path) -> Result<()> {
    let metadata =
        fs::symlink_metadata(path).map_err(|error| format!("stat {}: {error}", path.display()))?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(format!(
            "refusing non-directory result path {}",
            path.display()
        ));
    }
    let current = metadata.permissions().mode() & 0o7777;
    let desired = hardened_shared_directory_mode(current);
    if current != desired {
        fs::set_permissions(path, fs::Permissions::from_mode(desired))
            .map_err(|error| format!("chmod {}: {error}", path.display()))?;
    }
    Ok(())
}

fn hardened_shared_directory_mode(current: u32) -> u32 {
    0o750 | (current & 0o2000)
}

fn drain(config: &Config) -> Result<()> {
    let workspace = open_path_no_symlinks(&config.workspace)?;
    let control = match open_dir_at(workspace.as_raw_fd(), OsStr::new(".tentaflake-worker")) {
        Ok(fd) => fd,
        Err(error) if error.raw_os_error() == Some(libc::ENOENT) => return Ok(()),
        Err(error) => return Err(format!("open worker control directory: {error}")),
    };
    let inbox = match open_dir_at(control.as_raw_fd(), OsStr::new("inbox")) {
        Ok(fd) => fd,
        Err(error) if error.raw_os_error() == Some(libc::ENOENT) => return Ok(()),
        Err(error) => return Err(format!("open worker inbox: {error}")),
    };

    let inbox_deferred = ingest(config, inbox.as_raw_fd())?;
    let ready_deferred = process_ready(config, workspace.as_raw_fd())?;
    if inbox_deferred || ready_deferred {
        return Err("worker continuation required; restarting after the bounded batch".into());
    }
    Ok(())
}

fn ingest(config: &Config, inbox_fd: RawFd) -> Result<bool> {
    let cursor = read_inbox_cursor(config, inbox_fd)?;
    let mut cursor_reset = cursor.reset;
    let mut scan = collect_inbox_requests(inbox_fd, config.max_request_bytes, cursor.cursor)?;
    if scan.stale_cursor {
        cursor_reset = true;
        scan = collect_inbox_requests(inbox_fd, config.max_request_bytes, None)?;
    }
    if cursor_reset {
        audit(
            config,
            "unknown",
            "inbox-cursor-reset",
            None,
            "discarded a stale, corrupt, or mutation-invalidated inbox cursor and restarted the scan",
        )?;
    }
    if scan.ignored_entries > 0 {
        audit(
            config,
            "unknown",
            "inbox-ignored",
            None,
            &format!(
                "ignored {} non-request, non-regular, unreadable, or oversized inbox entries",
                scan.ignored_entries
            ),
        )?;
    }
    if scan.deferred {
        audit(
            config,
            "unknown",
            "inbox-overflow",
            None,
            &format!(
                "raw inbox scan or eligible request limit reached after {} entries; remaining requests will be retried",
                scan.scanned_entries
            ),
        )?;
    }
    let mut inbox_mutated = false;
    let mut pending_usage = read_pending_usage(config)?;
    for name in scan.names {
        let stat = match fstatat_nofollow(inbox_fd, &name) {
            Ok(stat) => stat,
            Err(error) => {
                audit(
                    config,
                    "unknown",
                    "request-rejected",
                    None,
                    &format!("cannot inspect inbox entry: {error}"),
                )?;
                continue;
            }
        };
        if stat.st_mode & libc::S_IFMT != libc::S_IFREG {
            audit(
                config,
                "unknown",
                "request-rejected",
                None,
                "inbox entry changed into a non-regular file",
            )?;
            continue;
        }
        let file = match open_file_at(inbox_fd, &name) {
            Ok(file) => file,
            Err(error) => {
                audit(
                    config,
                    "unknown",
                    "request-rejected",
                    None,
                    &format!("cannot safely open inbox entry: {error}"),
                )?;
                continue;
            }
        };
        let mut file = unsafe { File::from_raw_fd(file.into_raw_fd()) };
        let metadata = match file.metadata() {
            Ok(metadata) => metadata,
            Err(error) => {
                audit(
                    config,
                    "unknown",
                    "request-rejected",
                    None,
                    &format!("cannot inspect opened inbox entry: {error}"),
                )?;
                continue;
            }
        };
        if !metadata.is_file() {
            audit(
                config,
                "unknown",
                "request-rejected",
                None,
                "inbox entry changed into a non-regular file",
            )?;
            continue;
        }
        let content = match read_bounded(&mut file, config.max_request_bytes as usize) {
            Ok(content) => content,
            Err(error) => {
                audit(
                    config,
                    "unknown",
                    "request-rejected",
                    None,
                    &format!("cannot read bounded inbox entry: {error}"),
                )?;
                continue;
            }
        };
        let parsed: Result<JobRequest> = serde_json::from_slice(&content)
            .map_err(|error| format!("invalid request JSON: {error}"));
        let request = match parsed.and_then(|request| {
            validate_request(config, &request)?;
            let expected = format!("{}.json", request.id);
            if name.as_bytes() != expected.as_bytes() {
                return Err("request filename must equal <id>.json".into());
            }
            Ok(request)
        }) {
            Ok(request) => request,
            Err(error) => {
                audit(config, "unknown", "request-rejected", None, &error)?;
                unlink_at(inbox_fd, &name)?;
                inbox_mutated = true;
                continue;
            }
        };

        if job_exists(config, &request.id) {
            audit(
                config,
                &request.id,
                "request-rejected",
                Some(request.action_class),
                "job id already exists",
            )?;
            unlink_at(inbox_fd, &name)?;
            inbox_mutated = true;
            continue;
        }

        let request_bytes = u64::try_from(content.len())
            .map_err(|_| "request length does not fit the pending queue counter".to_string())?;
        if !pending_usage.has_capacity_for(config, request_bytes) {
            audit(
                config,
                &request.id,
                "request-rejected",
                Some(request.action_class),
                "private pending queue capacity reached",
            )?;
            unlink_at(inbox_fd, &name)?;
            inbox_mutated = true;
            continue;
        }

        let pending = pending_path(config, &request.id);
        write_new_private(&pending, &content)?;
        pending_usage.reserve(request_bytes)?;
        unlink_at(inbox_fd, &name)?;
        inbox_mutated = true;
        audit(
            config,
            &request.id,
            if request.action_class.needs_approval() {
                "approval-required"
            } else {
                "request-accepted"
            },
            Some(request.action_class),
            "request moved into private worker state",
        )?;
    }

    let mut must_rescan = scan.directory_changed || (scan.deferred && inbox_mutated);
    let cursor_to_persist = if scan.deferred && !must_rescan {
        scan.next_cursor.map(|offset| InboxCursor {
            offset,
            stamp: scan.directory_stamp,
        })
    } else {
        None
    };
    let cursor_persisted = write_inbox_cursor(config, inbox_fd, cursor_to_persist)?;
    if cursor_to_persist.is_some() && !cursor_persisted {
        must_rescan = true;
    }
    if must_rescan {
        audit(
            config,
            "unknown",
            "inbox-cursor-reset",
            None,
            "inbox changed while a bounded scan was in progress; retrying from the beginning",
        )?;
    }
    Ok(scan.deferred || must_rescan)
}

fn process_ready(config: &Config, workspace_fd: RawFd) -> Result<bool> {
    let pending = list_pending_entries(config)?;
    let pending_count = pending.len();
    let mut processed = 0_u64;
    for (index, entry) in pending.into_iter().enumerate() {
        let path = entry.path();
        let file_type = entry
            .file_type()
            .map_err(|error| format!("inspect pending entry {}: {error}", path.display()))?;
        if path.extension() != Some(OsStr::new("json")) || !file_type.is_file() {
            return Err(format!(
                "private pending queue contains an unsafe entry: {}",
                path.display()
            ));
        }
        let request = read_pending_file(config, &path)?;
        if request.action_class.needs_approval() {
            continue;
        }
        let Some(claimed_path) = claim_pending(config, &request.id)? else {
            continue;
        };
        let claimed = read_pending_file(config, &claimed_path)?;
        complete_claimed_request(config, workspace_fd, &claimed)?;
        processed += 1;
        if processed >= config.max_ready_jobs_per_drain {
            return Ok(index + 1 < pending_count);
        }
    }
    Ok(false)
}

fn complete_claimed_request(
    config: &Config,
    workspace_fd: RawFd,
    request: &JobRequest,
) -> Result<()> {
    if request.action_class == ActionClass::Forbidden {
        finish_without_run(
            config,
            request,
            "rejected",
            "action class is always forbidden",
        )?;
        remove_inflight(config, &request.id)?;
        audit(
            config,
            &request.id,
            "rejected",
            Some(request.action_class),
            "action class is always forbidden",
        )?;
        return Ok(());
    }

    match ensure_execution_capacity(config) {
        Ok(()) => execute(config, workspace_fd, request)?,
        Err(error) => {
            let detail = format!("worker state capacity rejected execution: {error}");
            finish_without_run(config, request, "rejected", &detail)?;
            remove_inflight(config, &request.id)?;
            audit(
                config,
                &request.id,
                "rejected",
                Some(request.action_class),
                &detail,
            )?;
            return Ok(());
        }
    }

    remove_inflight(config, &request.id)
}

fn approve(config: &Config, job: &str) -> Result<()> {
    let request = read_pending(config, job)?;
    if !request.action_class.needs_approval() || request.action_class == ActionClass::Forbidden {
        return Err("this action class cannot be approved".into());
    }
    let claimed_path =
        claim_pending(config, job)?.ok_or_else(|| format!("pending job {job} does not exist"))?;
    let claimed = read_pending_file(config, &claimed_path)?;
    if !claimed.action_class.needs_approval() || claimed.action_class == ActionClass::Forbidden {
        return Err("claimed action class cannot be approved".into());
    }
    audit(
        config,
        job,
        "approved",
        Some(claimed.action_class),
        "host operator approval atomically claimed the request",
    )?;
    let workspace = open_path_no_symlinks(&config.workspace)?;
    complete_claimed_request(config, workspace.as_raw_fd(), &claimed)
}

fn deny(config: &Config, job: &str) -> Result<()> {
    let claimed_path =
        claim_pending(config, job)?.ok_or_else(|| format!("pending job {job} does not exist"))?;
    let request = read_pending_file(config, &claimed_path)?;
    finish_without_run(
        config,
        &request,
        "denied",
        "host operator denied the atomically claimed request",
    )?;
    remove_inflight(config, job)?;
    audit(
        config,
        job,
        "denied",
        Some(request.action_class),
        "host operator denied the atomically claimed request",
    )
}

fn read_pending(config: &Config, job: &str) -> Result<JobRequest> {
    let path = pending_path(config, job);
    read_pending_file(config, &path).map_err(|error| {
        if !path.exists() {
            format!("pending job {job} does not exist")
        } else {
            error
        }
    })
}

fn read_pending_file(config: &Config, path: &Path) -> Result<JobRequest> {
    let metadata =
        fs::symlink_metadata(path).map_err(|error| format!("stat {}: {error}", path.display()))?;
    if !metadata.file_type().is_file() || metadata.len() > config.max_request_bytes {
        return Err(format!("pending request is unsafe: {}", path.display()));
    }
    let mut file = File::open(path).map_err(|error| format!("open {}: {error}", path.display()))?;
    let content = read_bounded(&mut file, config.max_request_bytes as usize)
        .map_err(|error| format!("read {}: {error}", path.display()))?;
    let request: JobRequest = serde_json::from_slice(&content)
        .map_err(|error| format!("parse {}: {error}", path.display()))?;
    validate_request(config, &request)?;
    let expected = format!("{}.json", request.id);
    if path.file_name() != Some(OsStr::new(&expected)) {
        return Err(format!(
            "pending filename does not match request id: {}",
            path.display()
        ));
    }
    Ok(request)
}

fn pending_path(config: &Config, job: &str) -> PathBuf {
    config.state_dir.join("pending").join(format!("{job}.json"))
}

fn inflight_path(config: &Config, job: &str) -> PathBuf {
    config
        .state_dir
        .join("inflight")
        .join(format!("{job}.json"))
}

fn rename_no_replace(from: &Path, to: &Path) -> io::Result<()> {
    let from = CString::new(from.as_os_str().as_bytes())
        .map_err(|_| io::Error::from_raw_os_error(libc::EINVAL))?;
    let to = CString::new(to.as_os_str().as_bytes())
        .map_err(|_| io::Error::from_raw_os_error(libc::EINVAL))?;
    let result = unsafe {
        libc::syscall(
            libc::SYS_renameat2,
            libc::AT_FDCWD,
            from.as_ptr(),
            libc::AT_FDCWD,
            to.as_ptr(),
            1_u32,
        )
    };
    if result == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

fn claim_pending(config: &Config, job: &str) -> Result<Option<PathBuf>> {
    let pending = pending_path(config, job);
    let inflight = inflight_path(config, job);
    match rename_no_replace(&pending, &inflight) {
        Ok(()) => {
            let pending_dir = pending
                .parent()
                .ok_or_else(|| format!("pending path has no parent: {}", pending.display()))?;
            let inflight_dir = inflight
                .parent()
                .ok_or_else(|| format!("inflight path has no parent: {}", inflight.display()))?;
            sync_directory(pending_dir)?;
            sync_directory(inflight_dir)?;
            Ok(Some(inflight))
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) if error.raw_os_error() == Some(libc::EEXIST) => Err(format!(
            "refusing to overwrite an existing inflight claim for {job}"
        )),
        Err(error) => Err(format!("atomically claim pending job {job}: {error}")),
    }
}

fn remove_inflight(config: &Config, job: &str) -> Result<()> {
    let path = inflight_path(config, job);
    fs::remove_file(&path).map_err(|error| format!("remove inflight request: {error}"))?;
    sync_directory(
        path.parent()
            .ok_or_else(|| format!("inflight path has no parent: {}", path.display()))?,
    )
}

fn job_exists(config: &Config, job: &str) -> bool {
    pending_path(config, job).exists()
        || inflight_path(config, job).exists()
        || config.state_dir.join("results").join(job).exists()
        || config.state_dir.join("jobs").join(job).exists()
}

fn list_pending_entries(config: &Config) -> Result<Vec<fs::DirEntry>> {
    let limit = usize::try_from(config.max_pending_requests)
        .map_err(|_| "max_pending_requests does not fit this platform".to_string())?;
    let mut entries = Vec::new();
    for entry in fs::read_dir(config.state_dir.join("pending"))
        .map_err(|error| format!("read pending queue: {error}"))?
    {
        if entries.len() >= limit {
            return Err(format!(
                "private pending queue exceeds configured maximum of {} requests",
                config.max_pending_requests
            ));
        }
        entries.push(entry.map_err(|error| format!("read pending entry: {error}"))?);
    }
    entries.sort_by_key(|entry| entry.file_name());
    Ok(entries)
}

fn read_pending_usage(config: &Config) -> Result<PendingUsage> {
    let mut usage = PendingUsage {
        requests: 0,
        bytes: 0,
    };
    for entry in list_pending_entries(config)? {
        let path = entry.path();
        let file_type = entry
            .file_type()
            .map_err(|error| format!("inspect pending entry {}: {error}", path.display()))?;
        if path.extension() != Some(OsStr::new("json")) || !file_type.is_file() {
            return Err(format!(
                "private pending queue contains an unsafe entry: {}",
                path.display()
            ));
        }
        let bytes = entry
            .metadata()
            .map_err(|error| format!("stat pending entry {}: {error}", path.display()))?
            .len();
        usage.reserve(bytes)?;
    }
    Ok(usage)
}

fn only_inflight_entry(config: &Config) -> Result<Option<fs::DirEntry>> {
    let mut entries = fs::read_dir(config.state_dir.join("inflight"))
        .map_err(|error| format!("read inflight queue: {error}"))?;
    let Some(first) = entries.next() else {
        return Ok(None);
    };
    let first = first.map_err(|error| format!("read inflight entry: {error}"))?;
    if entries.next().is_some() {
        return Err("worker has more than one inflight claim; refusing automatic recovery".into());
    }
    let path = first.path();
    let file_type = first
        .file_type()
        .map_err(|error| format!("inspect inflight entry {}: {error}", path.display()))?;
    if path.extension() != Some(OsStr::new("json")) || !file_type.is_file() {
        return Err(format!(
            "worker inflight queue contains an unsafe entry: {}",
            path.display()
        ));
    }
    Ok(Some(first))
}

fn published_result_exists(path: &Path) -> Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => Ok(true),
        Ok(_) => Err(format!("result path is unsafe: {}", path.display())),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(format!("stat result {}: {error}", path.display())),
    }
}

fn remove_private_directory_if_present(path: &Path, label: &str) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {
            fs::remove_dir_all(path).map_err(|error| format!("remove stale {label}: {error}"))
        }
        Ok(_) => Err(format!("stale {label} path is unsafe: {}", path.display())),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("stat stale {label} {}: {error}", path.display())),
    }
}

fn recover_inflight(config: &Config) -> Result<()> {
    let Some(entry) = only_inflight_entry(config)? else {
        return Ok(());
    };
    let path = entry.path();
    let request = read_pending_file(config, &path)?;
    let container = format!("tfw-{}-{}", config.agent, request.id);
    cleanup_existing_worker_container(config, &container)?;
    remove_private_directory_if_present(
        &config.state_dir.join("jobs").join(&request.id),
        "job state",
    )?;
    remove_private_directory_if_present(
        &config
            .state_dir
            .join("results")
            .join(format!(".{}.tmp", request.id)),
        "result staging",
    )?;

    let final_result = config.state_dir.join("results").join(&request.id);
    if published_result_exists(&final_result)? {
        remove_inflight(config, &request.id)?;
        return audit(
            config,
            &request.id,
            "inflight-recovered",
            Some(request.action_class),
            "a published result existed after an interrupted claim; it was not replayed",
        );
    }

    finish_without_run(
        config,
        &request,
        "interrupted",
        "worker stopped after atomically claiming this request; its outcome is unknown and it will not be replayed automatically",
    )?;
    remove_inflight(config, &request.id)?;
    audit(
        config,
        &request.id,
        "interrupted",
        Some(request.action_class),
        "inflight claim recovered as outcome-unknown without replay",
    )
}

fn pending_metadata_bytes(config: &Config) -> Result<u64> {
    config
        .max_pending_requests
        .checked_mul(PENDING_FILE_ALLOCATION_BYTES)
        .ok_or_else(|| "worker pending metadata byte reservation overflowed".to_string())
}

fn pending_metadata_inodes(config: &Config) -> Result<u64> {
    config
        .max_pending_requests
        .checked_mul(PENDING_METADATA_INODES)
        .ok_or_else(|| "worker pending metadata inode reservation overflowed".to_string())
}

fn required_execution_state_bytes(config: &Config) -> Result<u64> {
    let max_log_bytes = u64::try_from(config.max_log_bytes)
        .map_err(|_| "worker log reservation does not fit u64".to_string())?;
    let pending_metadata = pending_metadata_bytes(config)?;
    config
        .max_snapshot_bytes
        .checked_mul(2)
        .and_then(|value| value.checked_add(max_log_bytes))
        .and_then(|value| value.checked_add(pending_metadata))
        .and_then(|value| value.checked_add(STATE_CONTROL_RESERVE_BYTES))
        .ok_or_else(|| "worker state byte reservation overflowed".to_string())
}

fn required_execution_state_inodes(config: &Config) -> Result<u64> {
    let pending_metadata = pending_metadata_inodes(config)?;
    config
        .max_snapshot_entries
        .checked_mul(2)
        .and_then(|value| value.checked_add(pending_metadata))
        .and_then(|value| value.checked_add(STATE_CONTROL_RESERVE_INODES))
        .ok_or_else(|| "worker state inode reservation overflowed".to_string())
}

fn ensure_execution_capacity(config: &Config) -> Result<()> {
    let path = CString::new(config.state_dir.as_os_str().as_bytes())
        .map_err(|_| "worker state path contains NUL".to_string())?;
    let mut stats = std::mem::MaybeUninit::<libc::statvfs>::uninit();
    let result = unsafe { libc::statvfs(path.as_ptr(), stats.as_mut_ptr()) };
    if result != 0 {
        return Err(format!(
            "stat worker state capacity: {}",
            io::Error::last_os_error()
        ));
    }
    let stats = unsafe { stats.assume_init() };
    let block_size = if stats.f_frsize == 0 {
        stats.f_bsize as u64
    } else {
        stats.f_frsize as u64
    };
    let available_bytes = (stats.f_bavail as u64)
        .checked_mul(block_size)
        .ok_or_else(|| "worker state free-byte counter overflowed".to_string())?;
    let required_bytes = required_execution_state_bytes(config)?;
    let available_inodes = stats.f_favail as u64;
    let required_inodes = required_execution_state_inodes(config)?;
    if available_bytes < required_bytes || available_inodes < required_inodes {
        return Err(format!(
            "free bytes={available_bytes} required bytes={required_bytes}; free inodes={available_inodes} required inodes={required_inodes}"
        ));
    }
    Ok(())
}

fn execute(config: &Config, workspace_fd: RawFd, request: &JobRequest) -> Result<()> {
    let job_root = config.state_dir.join("jobs").join(&request.id);
    let snapshot = job_root.join("input");
    fs::create_dir(&job_root).map_err(|error| format!("create job state: {error}"))?;
    let mut job_guard = JobDirectoryGuard::new(job_root.clone());
    fs::set_permissions(&job_root, fs::Permissions::from_mode(0o700)).map_err(|e| e.to_string())?;
    fs::create_dir(&snapshot).map_err(|error| format!("create snapshot: {error}"))?;
    set_snapshot_permissions(&snapshot, 0o700)?;
    let mut budget = CopyBudget {
        bytes_left: config.max_snapshot_bytes,
        entries_left: config.max_snapshot_entries,
    };
    if let Err(error) = copy_directory_fd(workspace_fd, &snapshot, &mut budget, true) {
        finish_without_run(
            config,
            request,
            "rejected",
            &format!("snapshot rejected: {error}"),
        )?;
        return Ok(());
    }

    let container = format!("tfw-{}-{}", config.agent, request.id);
    cleanup_existing_worker_container(config, &container)?;
    let mut create = runtime(config);
    create.arg("create");
    append_runtime_policy(&mut create, config, &container, &snapshot);
    create
        .arg(format!("--label=io.tentaflake.worker={}", config.agent))
        .arg("--entrypoint=/bin/bash")
        .arg(&config.image)
        .args(["-ceu", WORKSPACE_INIT_SCRIPT, "tentaflake-worker-job"])
        .args(&request.argv);
    command_ok(&mut create, "create disposable container")?;
    let mut container_guard = ContainerGuard::new(config, container.clone());

    let started = command_ok(
        runtime(config).args(["start", &container]),
        "start disposable container",
    );
    let mut timed_out = false;
    let mut exit_code = None;
    let artifact_export = job_root.join("artifacts-export");
    let mut artifacts_copied = false;
    let mut artifact_diagnostic = None;
    let mut handled_marker: Option<CompletionMarker> = None;
    if started.is_ok() {
        let deadline = Instant::now() + Duration::from_secs(request.timeout_seconds);
        loop {
            let state = runtime_output(
                config,
                ["inspect", "--format={{.State.Running}}", &container],
            )?;
            if String::from_utf8_lossy(&state.stdout).trim() != "true" {
                let status = runtime_output(
                    config,
                    ["inspect", "--format={{.State.ExitCode}}", &container],
                )?;
                exit_code = String::from_utf8_lossy(&status.stdout).trim().parse().ok();
                break;
            }
            if let Some(marker) = read_completion_marker(config, &container)?
                && handled_marker.as_ref() != Some(&marker)
                && let Some(export) =
                    export_artifacts_if_stable(config, &container, &artifact_export, &marker)?
            {
                command_ok(
                    runtime(config).args(["kill", "--signal=USR1", &container]),
                    "release completed disposable container",
                )?;
                artifacts_copied = export.available;
                artifact_diagnostic = export.diagnostic;
                handled_marker = Some(marker);
            }
            if Instant::now() >= deadline {
                timed_out = true;
                let _ = runtime(config).args(["kill", &container]).status();
                break;
            }
            thread::sleep(Duration::from_millis(250));
        }
    }

    let log_output = runtime_output(config, ["logs", &container]).unwrap_or_else(|error| Output {
        status: failure_status(),
        stdout: Vec::new(),
        stderr: error.into_bytes(),
    });
    let (mut log, mut log_truncated) = bounded_combined_log(log_output, config.max_log_bytes);
    if let Some(diagnostic) = artifact_diagnostic {
        append_bounded_log_section(
            &mut log,
            &mut log_truncated,
            config.max_log_bytes,
            "artifact-export",
            diagnostic.as_bytes(),
        );
    }
    let staging = config
        .state_dir
        .join("results")
        .join(format!(".{}.tmp", request.id));
    let final_result = config.state_dir.join("results").join(&request.id);
    fs::create_dir(&staging).map_err(|error| format!("create result staging: {error}"))?;
    harden_shared_directory(&staging)?;
    fs::write(staging.join("job.log"), log).map_err(|error| format!("write job log: {error}"))?;
    let artifacts_dir = staging.join("artifacts");
    let artifacts_available = artifacts_copied
        && !timed_out
        && handled_marker.as_ref().map(|marker| marker.exit_code) == exit_code;
    if artifacts_available {
        harden_shared_directory(&artifact_export)?;
        fs::rename(&artifact_export, &artifacts_dir)
            .map_err(|error| format!("publish artifact staging: {error}"))?;
    } else {
        if artifact_export.exists() {
            fs::remove_dir_all(&artifact_export)
                .map_err(|error| format!("discard incomplete artifacts: {error}"))?;
        }
        fs::create_dir(&artifacts_dir)
            .map_err(|error| format!("create artifact staging: {error}"))?;
    }

    let result = JobResult {
        version: 1,
        id: request.id.clone(),
        action_class: request.action_class,
        status: if timed_out {
            "timed-out".into()
        } else if started.is_err() {
            "runtime-error".into()
        } else if exit_code == Some(0) {
            "succeeded".into()
        } else {
            "failed".into()
        },
        exit_code,
        timed_out,
        artifacts_available,
        log_truncated,
        completed_unix_seconds: unix_seconds(),
        message: "executed in a disposable no-network gVisor capsule; artifacts are read-only to the controller".into(),
    };
    write_json(staging.join("result.json"), &result)?;
    fs::rename(&staging, &final_result).map_err(|error| format!("publish result: {error}"))?;
    sync_directory(
        final_result
            .parent()
            .ok_or_else(|| format!("result path has no parent: {}", final_result.display()))?,
    )?;
    container_guard.cleanup()?;
    job_guard.cleanup()?;
    audit(
        config,
        &request.id,
        "completed",
        Some(request.action_class),
        &result.status,
    )
}

fn read_completion_marker(config: &Config, container: &str) -> Result<Option<CompletionMarker>> {
    let output = runtime(config)
        .args(["exec", container, "head", "-c", "96", COMPLETION_MARKER])
        .output()
        .map_err(|error| format!("read disposable completion marker: {error}"))?;
    if !output.status.success() {
        return Ok(None);
    }
    Ok(parse_completion_marker(&output.stdout))
}

fn parse_completion_marker(bytes: &[u8]) -> Option<CompletionMarker> {
    let raw = std::str::from_utf8(bytes).ok()?.trim_end_matches('\n');
    let mut fields = raw.split(':');
    if fields.next()? != "TFW1" {
        return None;
    }
    let nonce = fields.next()?;
    if nonce.len() != 32 || !nonce.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    let exit_code = fields.next()?.parse::<i32>().ok()?;
    if !(0..=255).contains(&exit_code) || fields.next().is_some() {
        return None;
    }
    Some(CompletionMarker {
        raw: raw.to_owned(),
        exit_code,
    })
}

fn export_artifacts_if_stable(
    config: &Config,
    container: &str,
    destination: &Path,
    marker: &CompletionMarker,
) -> Result<Option<ArtifactExport>> {
    if destination.exists() {
        fs::remove_dir_all(destination)
            .map_err(|error| format!("reset artifact staging: {error}"))?;
    }
    fs::create_dir(destination).map_err(|error| format!("create artifact staging: {error}"))?;
    fs::set_permissions(destination, fs::Permissions::from_mode(0o700))
        .map_err(|error| format!("chmod artifact staging: {error}"))?;

    let mut archive_command = runtime(config);
    archive_command
        .args(["exec", container, "sh", "-c"])
        .arg(
            "test ! -d /workspace/artifacts || ".to_owned()
                + "exec tar -C /workspace/artifacts -cf - .",
        )
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut archive_child = archive_command
        .spawn()
        .map_err(|error| format!("start artifact archive stream: {error}"))?;
    let archive_stdout = archive_child
        .stdout
        .take()
        .ok_or_else(|| "artifact archive stream has no stdout".to_string())?;
    let archive_stderr = archive_child
        .stderr
        .take()
        .ok_or_else(|| "artifact archive stream has no stderr".to_string())?;
    let stderr_thread = thread::spawn(move || drain_bounded(archive_stderr, 8192));
    let imported = import_artifact_tar(
        archive_stdout,
        destination,
        config.max_snapshot_bytes,
        config.max_snapshot_entries,
    );
    let archive_status = archive_child
        .wait()
        .map_err(|error| format!("wait for artifact archive stream: {error}"))?;
    let archive_stderr = stderr_thread
        .join()
        .map_err(|_| "artifact stderr reader panicked".to_string())?
        .map_err(|error| format!("read artifact archive stderr: {error}"))?;

    let (available, diagnostic) = match (archive_status.success(), imported) {
        (true, Ok(entries)) => (entries > 0, None),
        (false, _) => (
            false,
            Some(format!(
                "capsule archive command failed: {}",
                String::from_utf8_lossy(&archive_stderr).trim()
            )),
        ),
        (true, Err(error)) => (false, Some(format!("artifact archive rejected: {error}"))),
    };
    let stable = read_completion_marker(config, container)?.as_ref() == Some(marker);
    if !stable || !available {
        fs::remove_dir_all(destination)
            .map_err(|error| format!("discard unstable artifact staging: {error}"))?;
    }
    Ok(stable.then_some(ArtifactExport {
        available,
        diagnostic,
    }))
}

fn import_artifact_tar<R: Read>(
    reader: R,
    destination: &Path,
    max_bytes: u64,
    max_entries: u64,
) -> Result<u64> {
    let mut archive = tar::Archive::new(reader);
    let entries = archive
        .entries()
        .map_err(|error| format!("read archive entries: {error}"))?;
    let mut bytes_left = max_bytes;
    let mut entries_left = max_entries;
    let mut imported = 0_u64;

    for entry in entries {
        if entries_left == 0 {
            return Err("artifact entry limit exceeded".into());
        }
        entries_left -= 1;
        let mut entry = entry.map_err(|error| format!("read archive entry: {error}"))?;
        let path = normalize_archive_path(
            &entry
                .path()
                .map_err(|error| format!("read artifact path: {error}"))?,
        )?;
        let entry_type = entry.header().entry_type();
        imported += 1;

        if path.as_os_str().is_empty() && entry_type.is_dir() {
            continue;
        }
        if path.as_os_str().is_empty() {
            return Err("artifact archive has an empty non-directory path".into());
        }
        let output_path = destination.join(&path);
        if entry_type.is_dir() {
            fs::create_dir_all(&output_path)
                .map_err(|error| format!("create artifact directory: {error}"))?;
            fs::set_permissions(&output_path, fs::Permissions::from_mode(0o750))
                .map_err(|error| format!("chmod artifact directory: {error}"))?;
            continue;
        }
        if !entry_type.is_file() {
            return Err("artifact archive contains a link or special file".into());
        }
        let size = entry.size();
        if size > bytes_left {
            return Err("artifact byte limit exceeded".into());
        }
        if let Some(parent) = output_path.parent() {
            fs::create_dir_all(parent)
                .map_err(|error| format!("create artifact parent: {error}"))?;
        }
        let mut output = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&output_path)
            .map_err(|error| format!("create artifact file: {error}"))?;
        let copied = io::copy(&mut entry, &mut output)
            .map_err(|error| format!("copy artifact file: {error}"))?;
        if copied != size {
            return Err("artifact size changed during import".into());
        }
        bytes_left -= copied;
        let source_mode = entry.header().mode().unwrap_or(0o600);
        let executable = source_mode & 0o100;
        let mode = 0o640 | executable | (executable >> 3);
        fs::set_permissions(&output_path, fs::Permissions::from_mode(mode))
            .map_err(|error| format!("chmod artifact file: {error}"))?;
    }
    Ok(imported)
}

fn normalize_archive_path(path: &Path) -> Result<PathBuf> {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::Normal(name) => normalized.push(name),
            Component::RootDir | Component::ParentDir | Component::Prefix(_) => {
                return Err("artifact archive contains an unsafe path".into());
            }
        }
    }
    Ok(normalized)
}

fn drain_bounded(mut reader: impl Read, max: usize) -> io::Result<Vec<u8>> {
    let mut kept = Vec::with_capacity(max.min(8192));
    let mut buffer = [0_u8; 8192];
    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        let remaining = max.saturating_sub(kept.len());
        kept.extend_from_slice(&buffer[..read.min(remaining)]);
    }
    Ok(kept)
}

fn append_runtime_policy(command: &mut Command, config: &Config, container: &str, snapshot: &Path) {
    command
        .arg(format!("--name={container}"))
        .args([
            "--runtime=runsc",
            "--network=none",
            "--read-only",
            "--cap-drop=ALL",
        ])
        .arg("--security-opt=no-new-privileges:true")
        .arg(format!(
            "--user={}:{}",
            config.container_uid, config.container_gid
        ))
        .arg(format!("--memory={}", config.memory))
        .arg(format!("--memory-swap={}", config.memory_swap))
        .arg(format!("--cpus={}", config.cpus))
        .arg(format!("--pids-limit={}", config.pids_limit))
        .arg(format!(
            "--tmpfs=/workspace:rw,nosuid,nodev,uid={},gid={},mode=0700,size={}",
            config.container_uid, config.container_gid, config.workspace_tmpfs_size
        ))
        .arg(format!(
            "--tmpfs=/tmp:rw,nosuid,nodev,noexec,uid={},gid={},mode=0700,size={}",
            config.container_uid, config.container_gid, config.tmp_tmpfs_size
        ))
        .arg("--ulimit=nofile=4096:4096")
        .arg(format!("--ulimit=nproc={0}:{0}", config.pids_limit))
        .arg(format!(
            "--mount=type=bind,source={},target=/input,readonly",
            snapshot.display()
        ))
        .arg("--workdir=/workspace");
    append_runtime_log_policy(command, &config.backend);
    if config.backend == "docker" {
        command.arg("--security-opt=apparmor=docker-default");
    }
}

fn append_runtime_log_policy(command: &mut Command, backend: &str) {
    match backend {
        "docker" => {
            command
                .arg("--log-driver=local")
                .arg("--log-opt=max-size=1m")
                .arg("--log-opt=max-file=1")
                .arg("--log-opt=compress=false");
        }
        "podman" => {
            command
                .arg("--log-driver=k8s-file")
                .arg("--log-opt=max-size=1m");
        }
        _ => unreachable!("backend was validated before execution"),
    }
}

fn cleanup_existing_worker_container(config: &Config, container: &str) -> Result<()> {
    let output = runtime(config)
        .args([
            "inspect",
            "--format={{index .Config.Labels \"io.tentaflake.worker\"}}",
            container,
        ])
        .output()
        .map_err(|error| format!("inspect stale worker container: {error}"))?;
    if !output.status.success() {
        return Ok(());
    }
    if String::from_utf8_lossy(&output.stdout).trim() != config.agent {
        return Err(format!("refusing to remove unowned container {container}"));
    }
    remove_worker_container(config, container)
}

fn remove_worker_container(config: &Config, container: &str) -> Result<()> {
    command_ok(
        runtime(config).args(["rm", "--force", container]),
        "remove disposable container",
    )
}

fn runtime(config: &Config) -> Command {
    Command::new(&config.runtime)
}

fn runtime_output<I, S>(config: &Config, args: I) -> Result<Output>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    Command::new(&config.runtime)
        .args(args)
        .output()
        .map_err(|error| format!("run {}: {error}", config.runtime.display()))
}

fn command_ok(command: &mut Command, operation: &str) -> Result<()> {
    let output = command
        .output()
        .map_err(|error| format!("{operation}: {error}"))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(format!(
            "{operation} failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ))
    }
}

#[cfg(unix)]
fn failure_status() -> std::process::ExitStatus {
    use std::os::unix::process::ExitStatusExt;
    std::process::ExitStatus::from_raw(1 << 8)
}

fn bounded_combined_log(output: Output, max: usize) -> (Vec<u8>, bool) {
    let mut combined = output.stdout;
    if !output.stderr.is_empty() {
        combined.extend_from_slice(b"\n[stderr]\n");
        combined.extend_from_slice(&output.stderr);
    }
    let truncated = combined.len() > max;
    combined.truncate(max);
    (combined, truncated)
}

fn append_bounded_log_section(
    log: &mut Vec<u8>,
    truncated: &mut bool,
    max: usize,
    name: &str,
    content: &[u8],
) {
    let section = format!("\n[{name}]\n");
    let wanted = section.len().saturating_add(content.len());
    let remaining = max.saturating_sub(log.len());
    if remaining < wanted {
        *truncated = true;
    }
    let prefix_len = section.len().min(remaining);
    log.extend_from_slice(&section.as_bytes()[..prefix_len]);
    let remaining = max.saturating_sub(log.len());
    log.extend_from_slice(&content[..content.len().min(remaining)]);
}

fn finish_without_run(
    config: &Config,
    request: &JobRequest,
    status: &str,
    message: &str,
) -> Result<()> {
    let staging = config
        .state_dir
        .join("results")
        .join(format!(".{}.tmp", request.id));
    let final_result = config.state_dir.join("results").join(&request.id);
    fs::create_dir(&staging).map_err(|error| format!("create result staging: {error}"))?;
    harden_shared_directory(&staging)?;
    let result = JobResult {
        version: 1,
        id: request.id.clone(),
        action_class: request.action_class,
        status: status.into(),
        exit_code: None,
        timed_out: false,
        artifacts_available: false,
        log_truncated: false,
        completed_unix_seconds: unix_seconds(),
        message: message.into(),
    };
    write_json(staging.join("result.json"), &result)?;
    fs::rename(&staging, &final_result).map_err(|error| format!("publish result: {error}"))?;
    sync_directory(
        final_result
            .parent()
            .ok_or_else(|| format!("result path has no parent: {}", final_result.display()))?,
    )
}

fn write_json(path: PathBuf, value: &impl Serialize) -> Result<()> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o640)
        .open(&path)
        .map_err(|error| format!("create {}: {error}", path.display()))?;
    serde_json::to_writer_pretty(&mut file, value).map_err(|error| error.to_string())?;
    file.write_all(b"\n").map_err(|error| error.to_string())?;
    file.sync_all()
        .map_err(|error| format!("sync {}: {error}", path.display()))
}

fn write_new_private(path: &Path, content: &[u8]) -> Result<()> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .map_err(|error| format!("create {}: {error}", path.display()))?;
    file.write_all(content)
        .map_err(|error| format!("write {}: {error}", path.display()))?;
    file.sync_all()
        .map_err(|error| format!("sync {}: {error}", path.display()))?;
    let parent = path
        .parent()
        .ok_or_else(|| format!("private state path has no parent: {}", path.display()))?;
    sync_directory(parent)
}

fn audit(
    config: &Config,
    job: &str,
    event: &str,
    action_class: Option<ActionClass>,
    detail: &str,
) -> Result<()> {
    let path = config.state_dir.join("audit.jsonl");
    let mut file = OpenOptions::new()
        .append(true)
        .create(true)
        .mode(0o600)
        .open(&path)
        .map_err(|error| format!("open audit: {error}"))?;
    serde_json::to_writer(
        &mut file,
        &AuditEvent {
            timestamp_unix_seconds: unix_seconds(),
            agent: &config.agent,
            job,
            event,
            action_class,
            detail,
        },
    )
    .map_err(|error| error.to_string())?;
    file.write_all(b"\n").map_err(|error| error.to_string())?;
    file.sync_data()
        .map_err(|error| format!("sync audit {}: {error}", path.display()))
}

fn unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn read_bounded(file: &mut File, max: usize) -> io::Result<Vec<u8>> {
    let mut content = Vec::with_capacity(max.min(8192));
    file.take((max as u64) + 1).read_to_end(&mut content)?;
    if content.len() > max {
        Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "file exceeds configured byte limit",
        ))
    } else {
        Ok(content)
    }
}

struct CopyBudget {
    bytes_left: u64,
    entries_left: u64,
}

struct JobDirectoryGuard {
    path: PathBuf,
    active: bool,
}

impl JobDirectoryGuard {
    fn new(path: PathBuf) -> Self {
        Self { path, active: true }
    }

    fn cleanup(&mut self) -> Result<()> {
        if self.active {
            fs::remove_dir_all(&self.path).map_err(|error| format!("clean job state: {error}"))?;
            self.active = false;
        }
        Ok(())
    }
}

impl Drop for JobDirectoryGuard {
    fn drop(&mut self) {
        if self.active {
            let _ = fs::remove_dir_all(&self.path);
        }
    }
}

struct ContainerGuard<'a> {
    config: &'a Config,
    name: String,
    active: bool,
}

impl<'a> ContainerGuard<'a> {
    fn new(config: &'a Config, name: String) -> Self {
        Self {
            config,
            name,
            active: true,
        }
    }

    fn cleanup(&mut self) -> Result<()> {
        if self.active {
            remove_worker_container(self.config, &self.name)?;
            self.active = false;
        }
        Ok(())
    }
}

impl Drop for ContainerGuard<'_> {
    fn drop(&mut self) {
        if self.active {
            let _ = remove_worker_container(self.config, &self.name);
        }
    }
}

fn copy_directory_fd(
    fd: RawFd,
    destination: &Path,
    budget: &mut CopyBudget,
    top: bool,
) -> Result<()> {
    let (mut names, truncated) =
        list_fd_dir(fd, budget.entries_left.saturating_add(u64::from(top)))?;
    if truncated {
        return Err("snapshot entry limit exceeded".into());
    }
    names.sort();
    for name in names {
        if top && name == OsStr::new(".tentaflake-worker") {
            continue;
        }
        if budget.entries_left == 0 {
            return Err("snapshot entry limit exceeded".into());
        }
        budget.entries_left -= 1;
        let stat = fstatat_nofollow(fd, &name)?;
        let destination_path = destination.join(&name);
        let kind = stat.st_mode & libc::S_IFMT;
        match kind {
            libc::S_IFDIR => {
                fs::create_dir(&destination_path)
                    .map_err(|error| format!("create snapshot directory: {error}"))?;
                fs::set_permissions(&destination_path, fs::Permissions::from_mode(0o700))
                    .map_err(|error| error.to_string())?;
                let child = open_dir_at(fd, &name)
                    .map_err(|error| format!("open snapshot directory: {error}"))?;
                copy_directory_fd(child.as_raw_fd(), &destination_path, budget, false)?;
                set_snapshot_permissions(&destination_path, stat.st_mode as u32)?;
            }
            libc::S_IFREG => {
                let source_fd = open_file_at(fd, &name)
                    .map_err(|error| format!("open snapshot file: {error}"))?;
                let mut source = unsafe { File::from_raw_fd(source_fd.into_raw_fd()) };
                let mut destination_file = OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .mode(stat.st_mode as u32 & 0o777)
                    .open(&destination_path)
                    .map_err(|error| format!("create snapshot file: {error}"))?;
                let copied = io::copy(
                    &mut Read::by_ref(&mut source).take(budget.bytes_left + 1),
                    &mut destination_file,
                )
                .map_err(|error| format!("copy snapshot file: {error}"))?;
                if copied > budget.bytes_left {
                    return Err("snapshot byte limit exceeded".into());
                }
                budget.bytes_left -= copied;
                set_snapshot_permissions(&destination_path, stat.st_mode as u32)?;
            }
            libc::S_IFLNK => {
                let target = read_link_at(fd, &name)?;
                symlink(target, &destination_path)
                    .map_err(|error| format!("copy snapshot symlink: {error}"))?;
            }
            _ => {
                return Err(format!(
                    "snapshot contains unsupported special file {:?}",
                    name
                ));
            }
        }
    }
    Ok(())
}

fn set_snapshot_permissions(path: &Path, source_mode: u32) -> Result<()> {
    let owner_read_execute = source_mode & 0o500;
    let capsule_group_access = owner_read_execute >> 3;
    let mode = (source_mode & 0o777) | capsule_group_access;
    fs::set_permissions(path, fs::Permissions::from_mode(mode))
        .map_err(|error| format!("set snapshot permissions for {}: {error}", path.display()))
}

trait IntoRawFdOwned {
    fn into_raw_fd(self) -> RawFd;
}

impl IntoRawFdOwned for OwnedFd {
    fn into_raw_fd(self) -> RawFd {
        use std::os::fd::IntoRawFd;
        IntoRawFd::into_raw_fd(self)
    }
}

fn open_path_no_symlinks(path: &Path) -> Result<OwnedFd> {
    match open_path_with_openat2(path) {
        Ok(fd) => Ok(fd),
        Err(error) if error.raw_os_error() == Some(libc::ENOSYS) => {
            open_path_no_symlinks_fallback(path).map_err(|fallback| {
                format!(
                    "securely open {}: openat2 unavailable; fallback failed: {fallback}",
                    path.display()
                )
            })
        }
        Err(error) => Err(format!("securely open {}: {error}", path.display())),
    }
}

fn open_path_with_openat2(path: &Path) -> io::Result<OwnedFd> {
    let c_path = CString::new(path.as_os_str().as_bytes())
        .map_err(|_| io::Error::from_raw_os_error(libc::EINVAL))?;
    let how = OpenHow {
        flags: (libc::O_PATH | libc::O_DIRECTORY | libc::O_CLOEXEC) as u64,
        mode: 0,
        resolve: RESOLVE_NO_MAGICLINKS | RESOLVE_NO_SYMLINKS,
    };
    let fd = unsafe {
        libc::syscall(
            libc::SYS_openat2,
            libc::AT_FDCWD,
            c_path.as_ptr(),
            &how,
            std::mem::size_of::<OpenHow>(),
        ) as libc::c_int
    };
    if fd < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(unsafe { OwnedFd::from_raw_fd(fd) })
    }
}

fn open_path_no_symlinks_fallback(path: &Path) -> io::Result<OwnedFd> {
    if !path.is_absolute() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "workspace path must be absolute",
        ));
    }

    let flags = libc::O_PATH | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC;
    let mut current = open_at(libc::AT_FDCWD, OsStr::new("/"), flags)?;
    for component in path.components() {
        match component {
            Component::RootDir => {}
            Component::Normal(name) => {
                current = open_at(current.as_raw_fd(), name, flags)?;
            }
            Component::CurDir | Component::ParentDir | Component::Prefix(_) => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "workspace path contains an unsafe component",
                ));
            }
        }
    }
    Ok(current)
}

fn open_dir_at(parent: RawFd, name: &OsStr) -> io::Result<OwnedFd> {
    open_at(
        parent,
        name,
        libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
    )
}

fn open_file_at(parent: RawFd, name: &OsStr) -> io::Result<OwnedFd> {
    open_at(
        parent,
        name,
        libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC,
    )
}

fn open_at(parent: RawFd, name: &OsStr, flags: i32) -> io::Result<OwnedFd> {
    let name =
        CString::new(name.as_bytes()).map_err(|_| io::Error::from_raw_os_error(libc::EINVAL))?;
    let fd = unsafe { libc::openat(parent, name.as_ptr(), flags) };
    if fd < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(unsafe { OwnedFd::from_raw_fd(fd) })
    }
}

fn list_fd_dir(fd: RawFd, max_entries: u64) -> Result<(Vec<OsString>, bool)> {
    let path = PathBuf::from(format!("/proc/self/fd/{fd}"));
    let mut names = Vec::new();
    for entry in fs::read_dir(path).map_err(|error| format!("read directory fd: {error}"))? {
        if names.len() >= usize::try_from(max_entries).unwrap_or(usize::MAX) {
            return Ok((names, true));
        }
        names.push(
            entry
                .map(|value| value.file_name())
                .map_err(|error| error.to_string())?,
        );
    }
    Ok((names, false))
}

struct DirectoryCursor {
    directory: *mut libc::DIR,
}

impl DirectoryCursor {
    fn from_fd(fd: RawFd) -> Result<Self> {
        let duplicated = unsafe { libc::dup(fd) };
        if duplicated < 0 {
            return Err(format!(
                "duplicate inbox directory fd: {}",
                io::Error::last_os_error()
            ));
        }
        let directory = unsafe { libc::fdopendir(duplicated) };
        if directory.is_null() {
            let error = io::Error::last_os_error();
            unsafe {
                libc::close(duplicated);
            }
            return Err(format!("open inbox directory stream: {error}"));
        }
        Ok(Self { directory })
    }

    fn seek(&mut self, cursor: libc::c_long) {
        unsafe {
            libc::seekdir(self.directory, cursor);
        }
    }

    fn cursor(&self) -> Result<libc::c_long> {
        let cursor = unsafe { libc::telldir(self.directory) };
        if cursor < 0 {
            Err(format!(
                "read inbox directory cursor: {}",
                io::Error::last_os_error()
            ))
        } else {
            Ok(cursor)
        }
    }

    fn next_name(&mut self) -> Result<Option<OsString>> {
        loop {
            unsafe {
                *libc::__errno_location() = 0;
            }
            let entry = unsafe { libc::readdir(self.directory) };
            if entry.is_null() {
                let errno = unsafe { *libc::__errno_location() };
                return if errno == 0 {
                    Ok(None)
                } else {
                    Err(format!(
                        "read inbox directory: {}",
                        io::Error::from_raw_os_error(errno)
                    ))
                };
            }
            let bytes = unsafe { CStr::from_ptr((*entry).d_name.as_ptr()).to_bytes() };
            if bytes == b"." || bytes == b".." {
                continue;
            }
            return Ok(Some(OsString::from_vec(bytes.to_vec())));
        }
    }
}

impl Drop for DirectoryCursor {
    fn drop(&mut self) {
        unsafe {
            libc::closedir(self.directory);
        }
    }
}

fn inbox_directory_stamp(inbox_fd: RawFd) -> Result<InboxDirectoryStamp> {
    let mut stat = std::mem::MaybeUninit::<libc::stat>::uninit();
    let result = unsafe { libc::fstat(inbox_fd, stat.as_mut_ptr()) };
    if result != 0 {
        return Err(format!(
            "stat inbox directory: {}",
            io::Error::last_os_error()
        ));
    }
    let stat = unsafe { stat.assume_init() };
    Ok(InboxDirectoryStamp {
        device: stat.st_dev as u64,
        inode: stat.st_ino,
        change_seconds: stat.st_ctime,
        change_nanoseconds: stat.st_ctime_nsec,
    })
}

fn read_inbox_cursor(config: &Config, inbox_fd: RawFd) -> Result<InboxCursorRead> {
    let path = config.state_dir.join(INBOX_CURSOR_FILE);
    let current_stamp = inbox_directory_stamp(inbox_fd)?;
    match fs::read_to_string(&path) {
        Ok(value) => {
            let mut fields = value.split_whitespace();
            let parsed = (|| {
                if fields.next()? != "v1" {
                    return None;
                }
                let stamp = InboxDirectoryStamp {
                    device: fields.next()?.parse::<u64>().ok()?,
                    inode: fields.next()?.parse::<u64>().ok()?,
                    change_seconds: fields.next()?.parse::<i64>().ok()?,
                    change_nanoseconds: fields.next()?.parse::<i64>().ok()?,
                };
                let offset = fields.next()?.parse::<libc::c_long>().ok()?;
                if fields.next().is_some() || offset < 0 {
                    return None;
                }
                Some(InboxCursor { offset, stamp })
            })();
            match parsed {
                Some(cursor) if cursor.stamp == current_stamp => Ok(InboxCursorRead {
                    cursor: Some(cursor),
                    reset: false,
                }),
                _ => Ok(InboxCursorRead {
                    cursor: None,
                    reset: true,
                }),
            }
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(InboxCursorRead {
            cursor: None,
            reset: false,
        }),
        Err(error) => Err(format!("read {}: {error}", path.display())),
    }
}

fn sync_directory(path: &Path) -> Result<()> {
    File::open(path)
        .map_err(|error| format!("open directory {}: {error}", path.display()))?
        .sync_all()
        .map_err(|error| format!("sync directory {}: {error}", path.display()))
}

fn remove_inbox_cursor(path: &Path, state_dir: &Path) -> Result<()> {
    match fs::remove_file(path) {
        Ok(()) => sync_directory(state_dir),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("remove {}: {error}", path.display())),
    }
}

fn write_inbox_cursor(
    config: &Config,
    inbox_fd: RawFd,
    cursor: Option<InboxCursor>,
) -> Result<bool> {
    let path = config.state_dir.join(INBOX_CURSOR_FILE);
    let temporary = config.state_dir.join(format!("{INBOX_CURSOR_FILE}.new"));
    match cursor {
        Some(cursor) => {
            if cursor.offset < 0 {
                return Err("refusing a negative inbox cursor".into());
            }
            if inbox_directory_stamp(inbox_fd)? != cursor.stamp {
                remove_inbox_cursor(&path, &config.state_dir)?;
                return Ok(false);
            }
            let mut file = OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .mode(0o600)
                .open(&temporary)
                .map_err(|error| format!("open {}: {error}", temporary.display()))?;
            writeln!(
                file,
                "v1 {} {} {} {} {}",
                cursor.stamp.device,
                cursor.stamp.inode,
                cursor.stamp.change_seconds,
                cursor.stamp.change_nanoseconds,
                cursor.offset,
            )
            .map_err(|error| format!("write {}: {error}", temporary.display()))?;
            file.sync_all()
                .map_err(|error| format!("sync {}: {error}", temporary.display()))?;
            fs::rename(&temporary, &path)
                .map_err(|error| format!("replace {}: {error}", path.display()))?;
            sync_directory(&config.state_dir)?;
            Ok(true)
        }
        None => {
            remove_inbox_cursor(&path, &config.state_dir)?;
            Ok(true)
        }
    }
}

fn finish_inbox_scan(
    inbox_fd: RawFd,
    directory_stamp: InboxDirectoryStamp,
    mut names: Vec<OsString>,
    deferred: bool,
    ignored_entries: u64,
    scanned_entries: u64,
    next_cursor: Option<libc::c_long>,
    stale_cursor: bool,
) -> Result<InboxScan> {
    names.sort();
    Ok(InboxScan {
        names,
        deferred,
        ignored_entries,
        scanned_entries,
        next_cursor,
        directory_stamp,
        directory_changed: inbox_directory_stamp(inbox_fd)? != directory_stamp,
        stale_cursor,
    })
}

fn collect_inbox_requests(
    inbox_fd: RawFd,
    max_request_bytes: u64,
    cursor: Option<InboxCursor>,
) -> Result<InboxScan> {
    let directory_stamp = inbox_directory_stamp(inbox_fd)?;
    let mut directory = DirectoryCursor::from_fd(inbox_fd)?;
    let mut prefetched_name = None;
    if let Some(cursor) = cursor {
        if cursor.stamp != directory_stamp {
            return finish_inbox_scan(
                inbox_fd,
                directory_stamp,
                Vec::new(),
                false,
                0,
                0,
                None,
                true,
            );
        }
        directory.seek(cursor.offset);
        prefetched_name = directory.next_name()?;
        if prefetched_name.is_none() {
            return finish_inbox_scan(
                inbox_fd,
                directory_stamp,
                Vec::new(),
                false,
                0,
                0,
                None,
                true,
            );
        }
    }
    let mut names = Vec::new();
    let mut ignored_entries = 0_u64;
    let mut scanned_entries = 0_u64;
    loop {
        if scanned_entries >= MAX_INBOX_SCAN_ENTRIES
            || names.len() >= usize::try_from(MAX_INBOX_ENTRIES).unwrap_or(usize::MAX)
        {
            let next_cursor = directory.cursor()?;
            if directory.next_name()?.is_some() {
                return finish_inbox_scan(
                    inbox_fd,
                    directory_stamp,
                    names,
                    true,
                    ignored_entries,
                    scanned_entries,
                    Some(next_cursor),
                    false,
                );
            }
            return finish_inbox_scan(
                inbox_fd,
                directory_stamp,
                names,
                false,
                ignored_entries,
                scanned_entries,
                None,
                false,
            );
        }
        let name = match prefetched_name.take() {
            Some(name) => name,
            None => match directory.next_name()? {
                Some(name) => name,
                None => {
                    return finish_inbox_scan(
                        inbox_fd,
                        directory_stamp,
                        names,
                        false,
                        ignored_entries,
                        scanned_entries,
                        None,
                        false,
                    );
                }
            },
        };
        scanned_entries += 1;
        let bytes = name.as_bytes();
        if !bytes.ends_with(b".json") || bytes.len() > 69 {
            ignored_entries = ignored_entries.saturating_add(1);
            continue;
        }
        let stat = match fstatat_nofollow(inbox_fd, &name) {
            Ok(stat) => stat,
            Err(_) => {
                ignored_entries = ignored_entries.saturating_add(1);
                continue;
            }
        };
        let size = u64::try_from(stat.st_size).ok();
        if stat.st_mode & libc::S_IFMT != libc::S_IFREG
            || !matches!(size, Some(size) if size <= max_request_bytes)
        {
            ignored_entries = ignored_entries.saturating_add(1);
            continue;
        }
        names.push(name);
    }
}

fn fstatat_nofollow(parent: RawFd, name: &OsStr) -> Result<libc::stat> {
    let name = CString::new(name.as_bytes()).map_err(|_| "entry name contains NUL".to_string())?;
    let mut stat = std::mem::MaybeUninit::<libc::stat>::uninit();
    let result = unsafe {
        libc::fstatat(
            parent,
            name.as_ptr(),
            stat.as_mut_ptr(),
            libc::AT_SYMLINK_NOFOLLOW,
        )
    };
    if result == 0 {
        Ok(unsafe { stat.assume_init() })
    } else {
        Err(format!(
            "stat snapshot entry: {}",
            io::Error::last_os_error()
        ))
    }
}

fn read_link_at(parent: RawFd, name: &OsStr) -> Result<OsString> {
    let name = CString::new(name.as_bytes()).map_err(|_| "entry name contains NUL".to_string())?;
    let mut buffer = vec![0_u8; 4097];
    let size = unsafe {
        libc::readlinkat(
            parent,
            name.as_ptr(),
            buffer.as_mut_ptr().cast::<libc::c_char>(),
            buffer.len(),
        )
    };
    if size < 0 {
        return Err(format!(
            "read snapshot symlink: {}",
            io::Error::last_os_error()
        ));
    }
    let size = size as usize;
    if size == buffer.len() {
        return Err("snapshot symlink target is too long".into());
    }
    buffer.truncate(size);
    Ok(OsString::from_vec(buffer))
}

fn unlink_at(parent: RawFd, name: &OsStr) -> Result<()> {
    let name = CString::new(name.as_bytes()).map_err(|_| "entry name contains NUL".to_string())?;
    let result = unsafe { libc::unlinkat(parent, name.as_ptr(), 0) };
    if result == 0 {
        Ok(())
    } else {
        Err(format!(
            "remove inbox entry: {}",
            io::Error::last_os_error()
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::{FileTypeExt, symlink};

    fn temp_dir(name: &str) -> PathBuf {
        let root = std::env::var_os("CARGO_TARGET_TMPDIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                Path::new(env!("CARGO_MANIFEST_DIR"))
                    .parent()
                    .and_then(Path::parent)
                    .expect("crate is in the workspace crates directory")
                    .join("target/test-tmp")
            });
        fs::create_dir_all(&root).unwrap();
        let path = root.join(format!(
            "tentaflake-worker-test-{}-{}-{}",
            name,
            std::process::id(),
            unix_seconds()
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir(&path).unwrap();
        path
    }

    fn test_config(root: &Path) -> Config {
        let config = Config {
            agent: "fixture".into(),
            backend: "docker".into(),
            runtime: "/bin/false".into(),
            image: "example.invalid/fixture@sha256:fixture".into(),
            workspace: root.join("workspace"),
            state_dir: root.join("state"),
            container_uid: 10000,
            container_gid: 10000,
            max_request_bytes: 1024,
            max_pending_requests: 4,
            max_pending_bytes: 2048,
            max_ready_jobs_per_drain: 1,
            max_snapshot_bytes: 1024,
            max_snapshot_entries: 16,
            max_log_bytes: 1024,
            max_timeout_seconds: 10,
            memory: "64m".into(),
            memory_swap: "64m".into(),
            cpus: "0.5".into(),
            pids_limit: 16,
            workspace_tmpfs_size: "8m".into(),
            tmp_tmpfs_size: "4m".into(),
        };
        initialize_test_state_layout(&config);
        config
    }

    fn initialize_test_state_layout(config: &Config) {
        fs::create_dir_all(&config.state_dir).unwrap();
        for name in ["pending", "inflight", "jobs", "results"] {
            fs::create_dir(config.state_dir.join(name)).unwrap();
        }
        fs::write(
            config.state_dir.join(STATE_LAYOUT_MARKER_FILE),
            STATE_LAYOUT_MARKER,
        )
        .unwrap();
        ensure_state_layout(config).unwrap();
    }

    fn request_bytes(id: &str, action_class: &str) -> Vec<u8> {
        format!(
            "{{\"version\":1,\"id\":\"{id}\",\"action_class\":\"{action_class}\",\"argv\":[\"true\"],\"timeout_seconds\":1}}"
        )
        .into_bytes()
    }

    #[test]
    fn worker_state_layout_requires_the_post_mount_marker() {
        let root = temp_dir("state-layout-marker");
        let config = test_config(&root);
        fs::remove_file(config.state_dir.join(STATE_LAYOUT_MARKER_FILE)).unwrap();
        assert!(ensure_state_layout(&config).is_err());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn execution_reservation_includes_pending_metadata() {
        let root = temp_dir("state-capacity-reservation");
        let config = test_config(&root);
        assert_eq!(pending_metadata_bytes(&config).unwrap(), 4 * 4096);
        assert_eq!(pending_metadata_inodes(&config).unwrap(), 4 * 4);
        assert_eq!(
            required_execution_state_bytes(&config).unwrap(),
            2 * 1024 + 1024 + 4 * 4096 + STATE_CONTROL_RESERVE_BYTES
        );
        assert_eq!(
            required_execution_state_inodes(&config).unwrap(),
            2 * 16 + 4 * 4 + STATE_CONTROL_RESERVE_INODES
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn directory_listing_stops_at_the_declared_bound() {
        let root = temp_dir("directory-listing-bound");
        for index in 0..3 {
            fs::write(root.join(format!("entry-{index}")), b"fixture").unwrap();
        }
        let fd = open_path_no_symlinks(&root).unwrap();
        let (names, truncated) = list_fd_dir(fd.as_raw_fd(), 2).unwrap();
        assert_eq!(names.len(), 2);
        assert!(truncated);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn inbox_candidate_batch_is_bounded() {
        let root = temp_dir("inbox-candidate-bound");
        for index in 0..=MAX_INBOX_ENTRIES {
            fs::write(root.join(format!("candidate-{index:04}.json")), b"{}").unwrap();
        }
        let fd = open_path_no_symlinks(&root).unwrap();
        let scan = collect_inbox_requests(fd.as_raw_fd(), 1024, None).unwrap();
        assert_eq!(
            scan.names.len(),
            usize::try_from(MAX_INBOX_ENTRIES).unwrap()
        );
        assert!(scan.deferred);
        assert_eq!(scan.scanned_entries, MAX_INBOX_SCAN_ENTRIES);
        assert_eq!(scan.ignored_entries, 0);
        assert!(scan.next_cursor.is_some());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn inbox_cursor_is_bound_to_the_directory_stamp() {
        let root = temp_dir("inbox-cursor-identity");
        let config = test_config(&root);
        let first = root.join("first");
        let second = root.join("second");
        fs::create_dir(&first).unwrap();
        fs::create_dir(&second).unwrap();
        ensure_state_layout(&config).unwrap();

        let first_fd = open_path_no_symlinks(&first).unwrap();
        let second_fd = open_path_no_symlinks(&second).unwrap();
        let cursor = InboxCursor {
            offset: 17,
            stamp: inbox_directory_stamp(first_fd.as_raw_fd()).unwrap(),
        };
        assert!(write_inbox_cursor(&config, first_fd.as_raw_fd(), Some(cursor)).unwrap());
        assert_eq!(
            read_inbox_cursor(&config, first_fd.as_raw_fd()).unwrap(),
            InboxCursorRead {
                cursor: Some(cursor),
                reset: false,
            }
        );
        assert_eq!(
            read_inbox_cursor(&config, second_fd.as_raw_fd()).unwrap(),
            InboxCursorRead {
                cursor: None,
                reset: true,
            }
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn inbox_cursor_recovers_after_reopen_and_mutation() {
        let root = temp_dir("inbox-cursor-mutation");
        let config = test_config(&root);
        fs::create_dir(&config.workspace).unwrap();
        let control = config.workspace.join(".tentaflake-worker");
        let inbox = control.join("inbox");
        fs::create_dir(&control).unwrap();
        fs::create_dir(&inbox).unwrap();
        for index in 0..=MAX_INBOX_SCAN_ENTRIES {
            fs::create_dir(inbox.join(format!("junk-{index:04}.json"))).unwrap();
        }
        ensure_state_layout(&config).unwrap();

        {
            let workspace = open_path_no_symlinks(&config.workspace).unwrap();
            let control =
                open_dir_at(workspace.as_raw_fd(), OsStr::new(".tentaflake-worker")).unwrap();
            let inbox_fd = open_dir_at(control.as_raw_fd(), OsStr::new("inbox")).unwrap();
            assert!(ingest(&config, inbox_fd.as_raw_fd()).unwrap());
        }
        assert!(config.state_dir.join(INBOX_CURSOR_FILE).is_file());

        fs::write(
            inbox.join("recovered.json"),
            br#"{"version":1,"id":"recovered","action_class":"local-reversible","argv":["true"],"timeout_seconds":1}"#,
        )
        .unwrap();
        {
            let workspace = open_path_no_symlinks(&config.workspace).unwrap();
            let control =
                open_dir_at(workspace.as_raw_fd(), OsStr::new(".tentaflake-worker")).unwrap();
            let inbox_fd = open_dir_at(control.as_raw_fd(), OsStr::new("inbox")).unwrap();
            let cursor = read_inbox_cursor(&config, inbox_fd.as_raw_fd()).unwrap();
            assert!(cursor.cursor.is_none());
            assert!(cursor.reset);
            for _ in 0..4 {
                let deferred = ingest(&config, inbox_fd.as_raw_fd()).unwrap();
                if pending_path(&config, "recovered").is_file() {
                    break;
                }
                assert!(
                    deferred,
                    "a reset scan stopped before reaching the valid request"
                );
            }
        }

        assert!(pending_path(&config, "recovered").is_file());
        let audit = fs::read_to_string(config.state_dir.join("audit.jsonl")).unwrap();
        assert!(
            audit.contains("\"event\":\"inbox-cursor-reset\""),
            "{audit}"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn corrupt_inbox_cursor_is_discarded() {
        let root = temp_dir("inbox-cursor-corrupt");
        let config = test_config(&root);
        let inbox = root.join("inbox");
        fs::create_dir(&inbox).unwrap();
        ensure_state_layout(&config).unwrap();
        fs::write(config.state_dir.join(INBOX_CURSOR_FILE), "not a cursor\n").unwrap();

        let inbox_fd = open_path_no_symlinks(&inbox).unwrap();
        assert_eq!(
            read_inbox_cursor(&config, inbox_fd.as_raw_fd()).unwrap(),
            InboxCursorRead {
                cursor: None,
                reset: true,
            }
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn stale_inbox_cursor_eof_restarts_from_the_beginning() {
        let root = temp_dir("inbox-cursor-eof");
        let config = test_config(&root);
        fs::create_dir(&config.workspace).unwrap();
        let control = config.workspace.join(".tentaflake-worker");
        let inbox = control.join("inbox");
        fs::create_dir(&control).unwrap();
        fs::create_dir(&inbox).unwrap();
        fs::write(
            inbox.join("recovered.json"),
            br#"{"version":1,"id":"recovered","action_class":"local-reversible","argv":["true"],"timeout_seconds":1}"#,
        )
        .unwrap();
        ensure_state_layout(&config).unwrap();

        let workspace = open_path_no_symlinks(&config.workspace).unwrap();
        let control = open_dir_at(workspace.as_raw_fd(), OsStr::new(".tentaflake-worker")).unwrap();
        let inbox_fd = open_dir_at(control.as_raw_fd(), OsStr::new("inbox")).unwrap();
        let mut directory = DirectoryCursor::from_fd(inbox_fd.as_raw_fd()).unwrap();
        while directory.next_name().unwrap().is_some() {}
        let stale_cursor = InboxCursor {
            offset: directory.cursor().unwrap(),
            stamp: inbox_directory_stamp(inbox_fd.as_raw_fd()).unwrap(),
        };
        assert!(write_inbox_cursor(&config, inbox_fd.as_raw_fd(), Some(stale_cursor)).unwrap());

        ingest(&config, inbox_fd.as_raw_fd()).unwrap();
        assert!(pending_path(&config, "recovered").is_file());
        let audit = fs::read_to_string(config.state_dir.join("audit.jsonl")).unwrap();
        assert!(
            audit.contains("\"event\":\"inbox-cursor-reset\""),
            "{audit}"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn inbox_ignores_persistent_junk_without_starving_valid_requests() {
        let root = temp_dir("inbox-non-regular");
        let config = test_config(&root);
        fs::create_dir(&config.workspace).unwrap();
        let control = config.workspace.join(".tentaflake-worker");
        let inbox = control.join("inbox");
        fs::create_dir(&control).unwrap();
        fs::create_dir(&inbox).unwrap();
        for index in 0..MAX_INBOX_ENTRIES {
            fs::create_dir(inbox.join(format!("junk-{index:04}.json"))).unwrap();
        }
        symlink("valid.json", inbox.join("link.json")).unwrap();
        let fifo = inbox.join("pipe.json");
        let fifo = CString::new(fifo.as_os_str().as_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) }, 0);
        fs::write(
            inbox.join("oversized.json"),
            vec![0_u8; config.max_request_bytes as usize + 1],
        )
        .unwrap();
        fs::write(
            inbox.join("valid.json"),
            br#"{"version":1,"id":"valid","action_class":"local-reversible","argv":["true"],"timeout_seconds":1}"#,
        )
        .unwrap();
        ensure_state_layout(&config).unwrap();

        let workspace = open_path_no_symlinks(&config.workspace).unwrap();
        let control = open_dir_at(workspace.as_raw_fd(), OsStr::new(".tentaflake-worker")).unwrap();
        let inbox_fd = open_dir_at(control.as_raw_fd(), OsStr::new("inbox")).unwrap();
        for _ in 0..4 {
            let deferred = ingest(&config, inbox_fd.as_raw_fd()).unwrap();
            if pending_path(&config, "valid").is_file() {
                break;
            }
            assert!(
                deferred,
                "a bounded scan stopped without scheduling a retry"
            );
        }

        assert!(pending_path(&config, "valid").is_file());
        assert!(inbox.join("junk-0000.json").is_dir());
        assert!(inbox.join("link.json").is_symlink());
        assert!(
            fs::symlink_metadata(inbox.join("pipe.json"))
                .unwrap()
                .file_type()
                .is_fifo()
        );
        assert!(inbox.join("oversized.json").is_file());
        let audit = fs::read_to_string(config.state_dir.join("audit.jsonl")).unwrap();
        assert!(audit.contains("\"event\":\"inbox-ignored\""), "{audit}");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn private_pending_queue_rejects_excess_requests() {
        let root = temp_dir("pending-capacity");
        let mut config = test_config(&root);
        config.max_pending_requests = 1;
        config.max_pending_bytes = 1024;
        fs::create_dir(&config.workspace).unwrap();
        let control = config.workspace.join(".tentaflake-worker");
        let inbox = control.join("inbox");
        fs::create_dir(&control).unwrap();
        fs::create_dir(&inbox).unwrap();
        fs::write(
            inbox.join("first.json"),
            request_bytes("first", "external-reversible"),
        )
        .unwrap();
        fs::write(
            inbox.join("second.json"),
            request_bytes("second", "external-reversible"),
        )
        .unwrap();
        ensure_state_layout(&config).unwrap();

        let workspace = open_path_no_symlinks(&config.workspace).unwrap();
        let control = open_dir_at(workspace.as_raw_fd(), OsStr::new(".tentaflake-worker")).unwrap();
        let inbox_fd = open_dir_at(control.as_raw_fd(), OsStr::new("inbox")).unwrap();
        assert!(!ingest(&config, inbox_fd.as_raw_fd()).unwrap());

        assert!(pending_path(&config, "first").is_file());
        assert!(!pending_path(&config, "second").exists());
        assert!(!inbox.join("first.json").exists());
        assert!(!inbox.join("second.json").exists());
        let usage = read_pending_usage(&config).unwrap();
        assert_eq!(usage.requests, 1);
        let audit = fs::read_to_string(config.state_dir.join("audit.jsonl")).unwrap();
        assert!(
            audit.contains("private pending queue capacity reached"),
            "{audit}"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn private_pending_processing_is_limited_per_activation() {
        let root = temp_dir("pending-processing-bound");
        let config = test_config(&root);
        fs::create_dir(&config.workspace).unwrap();
        ensure_state_layout(&config).unwrap();
        write_new_private(
            &pending_path(&config, "first"),
            &request_bytes("first", "forbidden"),
        )
        .unwrap();
        write_new_private(
            &pending_path(&config, "second"),
            &request_bytes("second", "forbidden"),
        )
        .unwrap();

        let workspace = open_path_no_symlinks(&config.workspace).unwrap();
        assert!(process_ready(&config, workspace.as_raw_fd()).unwrap());
        assert!(!pending_path(&config, "first").exists());
        assert!(pending_path(&config, "second").is_file());
        assert!(config.state_dir.join("results").join("first").is_dir());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn pending_claim_is_non_overwriting_and_recovery_never_replays() {
        let root = temp_dir("inflight-recovery");
        let config = test_config(&root);
        ensure_state_layout(&config).unwrap();
        write_new_private(
            &pending_path(&config, "claimed"),
            &request_bytes("claimed", "external-reversible"),
        )
        .unwrap();

        let claimed = claim_pending(&config, "claimed").unwrap().unwrap();
        assert_eq!(claimed, inflight_path(&config, "claimed"));
        assert!(!pending_path(&config, "claimed").exists());
        assert!(claimed.is_file());

        write_new_private(
            &pending_path(&config, "claimed"),
            &request_bytes("claimed", "external-reversible"),
        )
        .unwrap();
        assert!(claim_pending(&config, "claimed").is_err());
        fs::remove_file(pending_path(&config, "claimed")).unwrap();

        recover_inflight(&config).unwrap();
        assert!(!inflight_path(&config, "claimed").exists());
        let result: serde_json::Value = serde_json::from_slice(
            &fs::read(
                config
                    .state_dir
                    .join("results")
                    .join("claimed")
                    .join("result.json"),
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(result["status"], "interrupted");
        let audit = fs::read_to_string(config.state_dir.join("audit.jsonl")).unwrap();
        assert!(audit.contains("outcome-unknown"), "{audit}");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn private_pending_queue_rejects_excess_bytes() {
        let root = temp_dir("pending-byte-capacity");
        let mut config = test_config(&root);
        config.max_request_bytes =
            u64::try_from(request_bytes("second", "external-reversible").len()).unwrap();
        config.max_pending_bytes = config.max_request_bytes;
        fs::create_dir(&config.workspace).unwrap();
        let control = config.workspace.join(".tentaflake-worker");
        let inbox = control.join("inbox");
        fs::create_dir(&control).unwrap();
        fs::create_dir(&inbox).unwrap();
        fs::write(
            inbox.join("first.json"),
            request_bytes("first", "external-reversible"),
        )
        .unwrap();
        fs::write(
            inbox.join("second.json"),
            request_bytes("second", "external-reversible"),
        )
        .unwrap();
        ensure_state_layout(&config).unwrap();

        let workspace = open_path_no_symlinks(&config.workspace).unwrap();
        let control = open_dir_at(workspace.as_raw_fd(), OsStr::new(".tentaflake-worker")).unwrap();
        let inbox_fd = open_dir_at(control.as_raw_fd(), OsStr::new("inbox")).unwrap();
        ingest(&config, inbox_fd.as_raw_fd()).unwrap();

        assert!(pending_path(&config, "first").is_file());
        assert!(!pending_path(&config, "second").exists());
        let audit = fs::read_to_string(config.state_dir.join("audit.jsonl")).unwrap();
        assert!(
            audit.contains("private pending queue capacity reached"),
            "{audit}"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn identifiers_are_strict() {
        assert!(validate_identifier("job", "build_1", 64).is_ok());
        for value in ["", "UPPER", "-leading", "../escape", "has.dot"] {
            assert!(validate_identifier("job", value, 64).is_err(), "{value}");
        }
    }

    #[test]
    fn action_classes_enforce_external_approval() {
        assert!(!ActionClass::LocalReversible.needs_approval());
        assert!(ActionClass::ExternalReversible.needs_approval());
        assert!(ActionClass::Financial.needs_approval());
        assert!(!ActionClass::Forbidden.needs_approval());
    }

    #[test]
    fn direct_operator_commands_adopt_the_declared_worker_group() {
        assert_eq!(
            worker_group_action(0, 0, 10_000).unwrap(),
            WorkerGroupAction::Adopt
        );
        assert_eq!(
            worker_group_action(0, 10_000, 10_000).unwrap(),
            WorkerGroupAction::Keep
        );
        assert!(worker_group_action(1000, 1000, 10_000).is_err());
    }

    #[test]
    fn runtime_log_policy_matches_backend() {
        let mut docker = Command::new("docker");
        append_runtime_log_policy(&mut docker, "docker");
        let docker_args = docker
            .get_args()
            .map(|argument| argument.to_string_lossy())
            .collect::<Vec<_>>();
        assert_eq!(
            docker_args,
            vec![
                "--log-driver=local",
                "--log-opt=max-size=1m",
                "--log-opt=max-file=1",
                "--log-opt=compress=false",
            ]
        );

        let mut podman = Command::new("podman");
        append_runtime_log_policy(&mut podman, "podman");
        let podman_args = podman
            .get_args()
            .map(|argument| argument.to_string_lossy())
            .collect::<Vec<_>>();
        assert_eq!(
            podman_args,
            vec!["--log-driver=k8s-file", "--log-opt=max-size=1m"]
        );
    }

    #[test]
    fn workspace_copy_drops_host_ownership() {
        assert!(WORKSPACE_INIT_SCRIPT.contains("--no-preserve=ownership"));
        assert!(!WORKSPACE_INIT_SCRIPT.contains("cp -a"));
        assert!(WORKSPACE_INIT_SCRIPT.contains(".tentaflake-worker-complete"));
        assert!(WORKSPACE_INIT_SCRIPT.contains("trap 'exit \"$status\"' USR1"));
        assert!(!WORKSPACE_INIT_SCRIPT.contains("exec \"$@\""));
    }

    #[test]
    fn completion_markers_are_strict_and_include_the_exit_code() {
        let marker =
            parse_completion_marker(b"TFW1:0123456789abcdef0123456789abcdef:17\n").unwrap();
        assert_eq!(marker.exit_code, 17);
        assert_eq!(marker.raw, "TFW1:0123456789abcdef0123456789abcdef:17");

        for invalid in [
            b"TFW1:short:0\n".as_slice(),
            b"TFW1:0123456789abcdef0123456789abcdeg:0\n".as_slice(),
            b"TFW1:0123456789abcdef0123456789abcdef:256\n".as_slice(),
            b"TFW2:0123456789abcdef0123456789abcdef:0\n".as_slice(),
        ] {
            assert!(parse_completion_marker(invalid).is_none());
        }
    }

    #[test]
    fn artifact_tar_import_is_bounded_and_normalizes_permissions() {
        let mut builder = tar::Builder::new(Vec::new());
        let mut directory = tar::Header::new_gnu();
        directory.set_entry_type(tar::EntryType::dir());
        directory.set_mode(0o700);
        directory.set_size(0);
        directory.set_cksum();
        builder
            .append_data(&mut directory, "./nested", io::empty())
            .unwrap();
        let mut file = tar::Header::new_gnu();
        file.set_entry_type(tar::EntryType::file());
        file.set_mode(0o700);
        file.set_size(4);
        file.set_cksum();
        builder
            .append_data(&mut file, "./nested/tool", b"test".as_slice())
            .unwrap();
        let archive = builder.into_inner().unwrap();

        let destination = temp_dir("artifact-import");
        assert_eq!(
            import_artifact_tar(archive.as_slice(), &destination, 4, 2).unwrap(),
            2
        );
        assert_eq!(fs::read(destination.join("nested/tool")).unwrap(), b"test");
        assert_eq!(
            fs::metadata(destination.join("nested"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o750
        );
        assert_eq!(
            fs::metadata(destination.join("nested/tool"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o750
        );

        let limited = temp_dir("artifact-import-limited");
        assert!(import_artifact_tar(archive.as_slice(), &limited, 3, 2).is_err());
        fs::remove_dir_all(destination).unwrap();
        fs::remove_dir_all(limited).unwrap();
    }

    #[test]
    fn artifact_tar_import_rejects_links_and_unsafe_paths() {
        assert!(normalize_archive_path(Path::new("../escape")).is_err());
        assert!(normalize_archive_path(Path::new("/absolute")).is_err());

        let mut builder = tar::Builder::new(Vec::new());
        let mut link = tar::Header::new_gnu();
        link.set_entry_type(tar::EntryType::symlink());
        link.set_mode(0o777);
        link.set_size(0);
        link.set_link_name("/etc/passwd").unwrap();
        link.set_cksum();
        builder
            .append_data(&mut link, "escape", io::empty())
            .unwrap();
        let archive = builder.into_inner().unwrap();
        let destination = temp_dir("artifact-import-link");
        let error = import_artifact_tar(archive.as_slice(), &destination, 1024, 10).unwrap_err();
        assert!(error.contains("link or special file"), "{error}");
        assert!(!destination.join("escape").exists());
        fs::remove_dir_all(destination).unwrap();
    }

    #[test]
    fn bounded_reader_rejects_oversize_content() {
        let root = temp_dir("bounded-reader");
        let path = root.join("request");
        fs::write(&path, b"12345").unwrap();
        let mut file = File::open(path).unwrap();
        assert!(read_bounded(&mut file, 4).is_err());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn secure_open_rejects_workspace_symlink() {
        let root = temp_dir("openat2");
        let real = root.join("real");
        let alias = root.join("alias");
        fs::create_dir(&real).unwrap();
        symlink(&real, &alias).unwrap();
        assert!(open_path_no_symlinks(&real).is_ok());
        assert!(open_path_no_symlinks(&alias).is_err());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn secure_open_fallback_rejects_unsafe_components() {
        let root = temp_dir("openat-fallback");
        let real = root.join("real");
        let alias = root.join("alias");
        fs::create_dir(&real).unwrap();
        symlink(&real, &alias).unwrap();

        assert!(open_path_no_symlinks_fallback(&real).is_ok());
        assert!(open_path_no_symlinks_fallback(&alias).is_err());
        assert!(open_path_no_symlinks_fallback(Path::new("relative")).is_err());
        assert!(open_path_no_symlinks_fallback(&real.join("../real")).is_err());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn snapshot_is_bounded_and_skips_control_directory() {
        let source = temp_dir("snapshot-source");
        let destination = temp_dir("snapshot-destination");
        fs::write(source.join("code.rs"), b"fn main() {}\n").unwrap();
        fs::create_dir(source.join("private")).unwrap();
        fs::set_permissions(source.join("private"), fs::Permissions::from_mode(0o700)).unwrap();
        fs::write(source.join("private/secret"), b"private\n").unwrap();
        fs::set_permissions(
            source.join("private/secret"),
            fs::Permissions::from_mode(0o600),
        )
        .unwrap();
        fs::create_dir(source.join(".tentaflake-worker")).unwrap();
        fs::write(source.join(".tentaflake-worker/secret"), b"request").unwrap();
        let source_fd = open_path_no_symlinks(&source).unwrap();
        let mut budget = CopyBudget {
            bytes_left: 1024,
            entries_left: 10,
        };
        copy_directory_fd(source_fd.as_raw_fd(), &destination, &mut budget, true).unwrap();
        assert!(destination.join("code.rs").is_file());
        use std::os::unix::fs::MetadataExt;
        let private_dir = fs::metadata(destination.join("private")).unwrap();
        let private = fs::metadata(destination.join("private/secret")).unwrap();
        assert_eq!(private_dir.mode() & 0o777, 0o750);
        assert_eq!(private.mode() & 0o777, 0o640);
        assert!(!destination.join(".tentaflake-worker").exists());

        let limited = temp_dir("snapshot-limited");
        let mut budget = CopyBudget {
            bytes_left: 2,
            entries_left: 10,
        };
        assert!(copy_directory_fd(source_fd.as_raw_fd(), &limited, &mut budget, true).is_err());
        fs::remove_dir_all(source).unwrap();
        fs::remove_dir_all(destination).unwrap();
        fs::remove_dir_all(limited).unwrap();
    }

    #[test]
    fn result_log_is_capped() {
        let output = Output {
            status: failure_status(),
            stdout: vec![b'a'; 10],
            stderr: vec![b'b'; 10],
        };
        let (log, truncated) = bounded_combined_log(output, 12);
        assert_eq!(log.len(), 12);
        assert!(truncated);
    }

    #[test]
    fn shared_result_permissions_preserve_existing_setgid() {
        assert_eq!(hardened_shared_directory_mode(0o2755), 0o2750);
        assert_eq!(hardened_shared_directory_mode(0o2750), 0o2750);
        assert_eq!(hardened_shared_directory_mode(0o0755), 0o0750);

        let root = temp_dir("shared-result-mode");
        fs::set_permissions(&root, fs::Permissions::from_mode(0o0755)).unwrap();
        harden_shared_directory(&root).unwrap();
        let mode = fs::metadata(&root).unwrap().permissions().mode() & 0o7777;
        assert_eq!(mode, 0o0750);
        harden_shared_directory(&root).unwrap();
        assert_eq!(
            fs::metadata(&root).unwrap().permissions().mode() & 0o7777,
            0o0750
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn job_guard_cleans_abandoned_snapshot() {
        let root = temp_dir("job-guard");
        let job = root.join("job");
        fs::create_dir(&job).unwrap();
        fs::write(job.join("snapshot"), b"fixture").unwrap();
        {
            let _guard = JobDirectoryGuard::new(job.clone());
        }
        assert!(!job.exists());
        fs::remove_dir_all(root).unwrap();
    }
}
