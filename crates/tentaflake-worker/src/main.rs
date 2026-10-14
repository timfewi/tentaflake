use serde::{Deserialize, Serialize};
use std::ffi::{CString, OsStr, OsString};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt, symlink};
use std::os::unix::process::CommandExt;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Output};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tentaflake_admission::host_fence::HostFence;

type Result<T> = std::result::Result<T, String>;

mod probe;
mod queue;
mod runtime;

const RESOLVE_NO_MAGICLINKS: u64 = 0x02;
const RESOLVE_NO_SYMLINKS: u64 = 0x04;
const COMPLETION_MARKER: &str = "/workspace/.tentaflake-worker-complete";
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
    #[serde(default)]
    admission_cli: Option<PathBuf>,
    #[serde(skip)]
    maintenance_fence: Option<HostFence>,
    image: String,
    workspace: PathBuf,
    state_dir: PathBuf,
    container_uid: u32,
    container_gid: u32,
    max_request_bytes: u64,
    #[serde(default = "default_queue_entries")]
    max_inbox_entries: usize,
    #[serde(default = "default_queue_bytes")]
    max_inbox_bytes: u64,
    #[serde(default = "default_queue_entries")]
    max_queue_entries: usize,
    #[serde(default = "default_queue_bytes")]
    max_queue_bytes: u64,
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

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
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

fn default_queue_entries() -> usize {
    256
}
fn default_queue_bytes() -> u64 {
    8 * 1024 * 1024
}

fn main() {
    if let Err(error) = run() {
        eprintln!("tentaflake-worker: {error}");
        std::process::exit(1);
    }
}

#[derive(Debug, PartialEq, Eq)]
enum WorkerCommand {
    Probe,
    Maintenance,
    Drain,
    Approve(String),
    Deny(String),
}

fn parse_args(
    mut args: impl Iterator<Item = OsString>,
) -> Result<(PathBuf, WorkerCommand, Option<RawFd>)> {
    const USAGE: &str =
        "usage: tentaflake-worker --config FILE probe|maintenance|drain|approve|deny [JOB]";
    if args.next().as_deref() != Some(OsStr::new("--config")) {
        return Err(USAGE.into());
    }
    let path = args.next().filter(|value| !value.is_empty()).ok_or(USAGE)?;
    let mut command = args.next().ok_or(USAGE)?;
    let fence_fd = if command == "--maintenance-fence-fd" {
        let value = args.next().ok_or("missing maintenance fence descriptor")?;
        let value = value
            .to_str()
            .ok_or("invalid maintenance fence descriptor")?;
        let fd = value
            .parse::<RawFd>()
            .ok()
            .filter(|fd| *fd >= 3 && fd.to_string() == value)
            .ok_or("invalid maintenance fence descriptor")?;
        command = args.next().ok_or(USAGE)?;
        Some(fd)
    } else {
        None
    };
    let command = match command.to_str() {
        Some("probe") => WorkerCommand::Probe,
        Some("maintenance") => WorkerCommand::Maintenance,
        Some("drain") => WorkerCommand::Drain,
        Some(kind @ ("approve" | "deny")) => {
            let job = args.next().ok_or("missing job id")?;
            let job = job.to_str().ok_or("job id must be UTF-8")?;
            validate_identifier("job id", job, 64)?;
            if kind == "approve" {
                WorkerCommand::Approve(job.to_owned())
            } else {
                WorkerCommand::Deny(job.to_owned())
            }
        }
        _ => return Err(USAGE.into()),
    };
    if args.next().is_some() {
        return Err("unexpected argument".into());
    }
    if fence_fd.is_some() && matches!(command, WorkerCommand::Probe | WorkerCommand::Maintenance) {
        return Err("read-only worker commands do not adopt a maintenance fence".into());
    }
    Ok((PathBuf::from(path), command, fence_fd))
}

fn run() -> Result<()> {
    // Reject the entire invocation before even reading configuration. In
    // particular, malformed operator commands must not initialize queue state.
    let (config_path, command, fence_fd) = parse_args(std::env::args_os().skip(1))?;
    let mut config: Config = serde_json::from_reader(
        File::open(&config_path)
            .map_err(|error| format!("open {}: {error}", config_path.display()))?,
    )
    .map_err(|error| format!("parse {}: {error}", config_path.display()))?;
    validate_config(&config)?;
    if command == WorkerCommand::Probe {
        let snapshot = probe::collect(&config)?;
        println!(
            "{}",
            serde_json::to_string(&snapshot).map_err(|_| "encode worker probe")?
        );
        return Ok(());
    }
    if command == WorkerCommand::Maintenance {
        let snapshot = probe::maintenance(&config)?;
        println!(
            "{}",
            serde_json::to_string(&snapshot).map_err(|_| "encode worker maintenance proof")?
        );
        return Ok(());
    }
    if let Some(cli) = &config.admission_cli {
        if let Some(fd) = fence_fd {
            // SAFETY: F_GETFD only inspects the supplied descriptor and reports invalid descriptors.
            if tentaflake_admission::effective_uid() != 0
                || unsafe { libc::fcntl(fd, libc::F_GETFD) } < 0
            {
                return Err("guarded worker requires a valid root-owned maintenance fence".into());
            }
            // The private argument transfers this inherited descriptor. The
            // shared owner authenticates the fixed protected path and lock.
            // SAFETY: The private launcher transfers this validated inherited descriptor, not a borrowed File.
            config.maintenance_fence = Some(HostFence::adopt(unsafe { File::from_raw_fd(fd) })?);
        } else {
            let error = delegation_command(cli, &config_path, &command)?.exec();
            return Err(format!("cannot delegate worker to host admission: {error}"));
        }
    } else {
        if fence_fd.is_some() {
            return Err("maintenance fence requires configured host admission".into());
        }
        require_legacy_without_ledger(Path::new("/var/lib/tentaflake-admission"))?;
    }
    enter_worker_group(&config)?;
    ensure_state_layout(&config)?;
    let _queue_lock = queue::lock(&config)?;

    match command {
        WorkerCommand::Drain => drain(&config),
        WorkerCommand::Approve(job) => approve(&config, &job),
        WorkerCommand::Deny(job) => deny(&config, &job),
        WorkerCommand::Probe | WorkerCommand::Maintenance => {
            unreachable!("read-only commands return before queue mutation")
        }
    }
}

fn delegation_command(cli: &Path, config: &Path, action: &WorkerCommand) -> Result<Command> {
    let mut command = Command::new(cli);
    command
        .env_clear()
        .args(["admission", "worker-run"])
        .arg(config);
    match action {
        WorkerCommand::Drain => {
            command.arg("drain");
        }
        WorkerCommand::Approve(job) => {
            command.args(["approve", job]);
        }
        WorkerCommand::Deny(job) => {
            command.args(["deny", job]);
        }
        WorkerCommand::Probe | WorkerCommand::Maintenance => {
            return Err("read-only worker command cannot delegate".into());
        }
    }
    Ok(command)
}

fn require_legacy_without_ledger(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        _ => Err("worker requires configured host admission while a global ledger exists or is unavailable".into()),
    }
}

fn enter_worker_group(config: &Config) -> Result<()> {
    let effective_uid = tentaflake_admission::effective_uid();
    let effective_gid = tentaflake_admission::effective_gid();
    match worker_group_action(effective_uid, effective_gid, config.container_gid)? {
        WorkerGroupAction::Keep => Ok(()),
        WorkerGroupAction::Adopt => {
            // SAFETY: setegid takes an integer GID; the kernel checks permission and the result is verified.
            let result = unsafe { libc::setegid(config.container_gid) };
            if result == 0 && tentaflake_admission::effective_gid() == config.container_gid {
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

fn validate_config(config: &Config) -> Result<()> {
    validate_identifier("agent", &config.agent, 48)?;
    if !matches!(config.backend.as_str(), "docker" | "podman") {
        return Err("backend must be docker or podman".into());
    }
    if let Some(path) = &config.admission_cli {
        let root = path
            .to_str()
            .and_then(|s| s.strip_prefix("/nix/store/"))
            .and_then(|s| s.strip_suffix("/bin/tentaflake"))
            .filter(|s| s.len() > 33 && s.len() <= 255);
        if !root.is_some_and(|s| {
            s.as_bytes()[32] == b'-'
                && s.as_bytes()[..32]
                    .iter()
                    .all(|byte| b"0123456789abcdfghijklmnpqrsvwxyz".contains(byte))
                && s.as_bytes()[33..]
                    .iter()
                    .all(|byte| byte.is_ascii_alphanumeric() || b"+-._?=".contains(byte))
        }) {
            return Err("admission_cli must be an immutable store bin/tentaflake path".into());
        }
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
        || config.max_inbox_entries == 0
        || config.max_inbox_bytes == 0
        || config.max_queue_entries == 0
        || config.max_queue_bytes == 0
        || config.max_snapshot_bytes == 0
        || config.max_snapshot_entries == 0
        || config.max_log_bytes == 0
        || config.max_timeout_seconds == 0
        || config.pids_limit == 0
    {
        return Err("worker limits must all be positive".into());
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

fn ensure_state_layout(config: &Config) -> Result<()> {
    for name in ["pending", "running", "approvals", "jobs"] {
        let path = config.state_dir.join(name);
        fs::create_dir_all(&path).map_err(|error| format!("create {}: {error}", path.display()))?;
        open_path_no_symlinks(&path)?;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700))
            .map_err(|error| format!("chmod {}: {error}", path.display()))?;
    }
    let results = config.state_dir.join("results");
    fs::create_dir_all(&results)
        .map_err(|error| format!("create {}: {error}", results.display()))?;
    harden_shared_directory(&results)?;
    Ok(())
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
    queue::recover(config)?;
    let workspace = open_path_no_symlinks(&config.workspace)?;
    process_ready(config, workspace.as_raw_fd())?;
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

    ingest(config, inbox.as_raw_fd())?;
    process_ready(config, workspace.as_raw_fd())
}

fn ingest(config: &Config, inbox_fd: RawFd) -> Result<()> {
    let (mut names, _) = queue::scan(inbox_fd, config.max_inbox_entries, config.max_inbox_bytes)
        .inspect_err(|_| {
            let _ = audit(
                config,
                "unknown",
                "queue-overload",
                None,
                "inbox exceeds entry or byte ceiling",
            );
        })?;
    let mut usage = queue::usage(config)?;
    names.sort();
    for name in names {
        let bytes = name.as_bytes();
        if !bytes.ends_with(b".json") || bytes.len() > 69 {
            continue;
        }
        let file = match open_file_at(inbox_fd, &name) {
            Ok(file) => file,
            Err(error) if error.raw_os_error() == Some(libc::ELOOP) => {
                audit(
                    config,
                    "unknown",
                    "request-rejected",
                    None,
                    "inbox entry is a symlink",
                )?;
                continue;
            }
            Err(error) => return Err(format!("open inbox entry {:?}: {error}", name)),
        };
        let mut file = File::from(file);
        if !file.metadata().map_err(|e| e.to_string())?.is_file() {
            return Err("inbox request must be a regular file".into());
        }
        let content = read_bounded(&mut file, config.max_request_bytes as usize)
            .map_err(|error| format!("read inbox entry {:?}: {error}", name))?;
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
                continue;
            }
        };

        if job_exists(config, &request.id) {
            // A damaged legacy private copy requires operator recovery. Keep
            // the inbox copy instead of discarding the only complete request.
            if pending_path(config, &request.id)
                .try_exists()
                .map_err(|e| e.to_string())?
            {
                read_pending(config, &request.id)?;
            }
            audit(
                config,
                &request.id,
                "request-rejected",
                Some(request.action_class),
                "job id already exists",
            )?;
            unlink_at(inbox_fd, &name)?;
            continue;
        }

        let pending = pending_path(config, &request.id);
        if usage.0 >= config.max_queue_entries
            || (content.len() as u64) > config.max_queue_bytes.saturating_sub(usage.1)
        {
            audit(
                config,
                &request.id,
                "queue-overload",
                Some(request.action_class),
                "private queue exceeds entry or byte ceiling",
            )?;
            return Err("private worker queue is full; request remains in the inbox".into());
        }
        queue::write_private(&pending, &content)?;
        usage.0 += 1;
        usage.1 += content.len() as u64;
        unlink_at(inbox_fd, &name)?;
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
    Ok(())
}

fn process_ready(config: &Config, workspace_fd: RawFd) -> Result<()> {
    let parent = config.state_dir.join("pending");
    let fd = open_path_no_symlinks(&parent)?;
    let (mut pending, _) = queue::scan(
        fd.as_raw_fd(),
        config.max_queue_entries,
        config.max_queue_bytes,
    )?;
    pending.sort();
    for name in pending {
        let path = parent.join(name);
        if path.extension() != Some(OsStr::new("json")) {
            continue;
        }
        let job = path
            .file_stem()
            .and_then(OsStr::to_str)
            .ok_or_else(|| "invalid pending job filename".to_string())?;
        let request = read_pending(config, job)?;
        if queue::terminal(config, &request)? {
            queue::retire(config, "pending", &request)?;
            continue;
        }
        if request.action_class == ActionClass::Forbidden {
            finish_without_run(
                config,
                &request,
                "rejected",
                "action class is always forbidden",
            )?;
            queue::retire(config, "pending", &request)?;
            continue;
        }
        let approved = queue::approved(config, &request.id)?;
        if request.action_class.needs_approval() && !approved {
            continue;
        }
        queue::claim(config, &request)?;
        if config
            .state_dir
            .join("jobs")
            .join(&request.id)
            .try_exists()
            .map_err(|e| e.to_string())?
        {
            // Legacy workers used the snapshot directory as their only claim.
            // Its presence is ambiguous; never retry that execution silently.
            queue::recover(config)?;
        } else {
            execute(config, workspace_fd, &request)?;
            queue::retire(config, "running", &request)?;
        }
    }
    Ok(())
}

fn approve(config: &Config, job: &str) -> Result<()> {
    // Approval can dispatch just like drain. An unrelated approval must never
    // bypass the unresolved-dispatch barrier or consume another reservation.
    queue::recover(config)?;
    let request = read_pending(config, job)?;
    if queue::terminal(config, &request)? {
        return Err("job is already terminal; drain the queue to retire its stale request".into());
    }
    if !request.action_class.needs_approval() || request.action_class == ActionClass::Forbidden {
        return Err("this action class cannot be approved".into());
    }
    let approval = config.state_dir.join("approvals").join(job);
    queue::write_private(&approval, b"approved\n")?;
    audit(
        config,
        job,
        "approved",
        Some(request.action_class),
        "host operator approval recorded",
    )?;
    let workspace = open_path_no_symlinks(&config.workspace)?;
    process_ready(config, workspace.as_raw_fd())
}

fn deny(config: &Config, job: &str) -> Result<()> {
    let request = read_pending(config, job)?;
    if queue::terminal(config, &request)? {
        return Err("job is already terminal; drain the queue to retire its stale request".into());
    }
    finish_without_run(
        config,
        &request,
        "denied",
        "host operator denied the request",
    )?;
    queue::retire(config, "pending", &request)?;
    audit(
        config,
        job,
        "denied",
        Some(request.action_class),
        "host operator denied request",
    )
}

fn read_pending(config: &Config, job: &str) -> Result<JobRequest> {
    queue::read_request(config, "pending", job)
}

fn pending_path(config: &Config, job: &str) -> PathBuf {
    config.state_dir.join("pending").join(format!("{job}.json"))
}

fn job_exists(config: &Config, job: &str) -> bool {
    pending_path(config, job).exists()
        || config
            .state_dir
            .join("running")
            .join(format!("{job}.json"))
            .exists()
        || config.state_dir.join("approvals").join(job).exists()
        || config.state_dir.join("results").join(job).exists()
        || config.state_dir.join("jobs").join(job).exists()
}

fn execute(config: &Config, workspace_fd: RawFd, request: &JobRequest) -> Result<()> {
    // Resolve once, then create by content identity. A tag change or missing
    // image must never redirect this dispatch or trigger registry acquisition.
    let image = runtime::local_image(config)?;
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
    // Publish intent before the daemon can observe create. A lost response or
    // killed client cannot prove non-execution; keep the snapshot and claim.
    queue::write_private(
        &job_root.join("dispatch.json"),
        &serde_json::to_vec(&serde_json::json!({
            "schemaVersion": 1, "agent": config.agent, "job": request.id,
            "image": image, "containerName": container,
        }))
        .map_err(|_| "cannot serialize worker dispatch intent")?,
    )?;
    queue::sync_directory(&config.state_dir.join("jobs"))?;
    job_guard.preserve_on_error = true;
    let mut create = runtime(config)?;
    create.arg("create");
    append_runtime_policy(&mut create, config, &container, &snapshot);
    create
        .arg(format!("--label=io.tentaflake.worker={}", config.agent))
        .arg("--pull=never")
        .arg("--entrypoint=/bin/bash")
        .arg(&image)
        .args(["-ceu", WORKSPACE_INIT_SCRIPT, "tentaflake-worker-job"])
        .args(&request.argv);
    let created = runtime::output(&mut create)?;
    if !created.status.success() {
        return Err("worker create did not confirm success; dispatch intent retained".into());
    }
    let container = runtime::container_id(&created.stdout)?;
    let mut container_guard = ContainerGuard::new(config, container.clone());
    queue::write_private(
        &job_root.join("container.json"),
        &serde_json::to_vec(&serde_json::json!({
            "schemaVersion": 1, "containerId": container,
        }))
        .map_err(|_| "cannot serialize worker container identity")?,
    )?;

    let started = command_ok(
        runtime(config)?.args(["start", &container]),
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
                && let Some(export) = export_artifacts_if_stable(
                    config,
                    &container,
                    &artifact_export,
                    &marker,
                    deadline,
                )?
            {
                command_ok(
                    runtime(config)?.args(["kill", "--signal=USR1", &container]),
                    "release completed disposable container",
                )?;
                artifacts_copied = export.available;
                artifact_diagnostic = export.diagnostic;
                handled_marker = Some(marker);
            }
            if Instant::now() >= deadline {
                timed_out = true;
                let _ = runtime_output(config, ["kill", &container]);
                break;
            }
            thread::sleep(Duration::from_millis(250));
        }
    }

    let (log_output, capture_truncated) =
        runtime::logs(config, &container).unwrap_or_else(|error| {
            (
                Output {
                    status: failure_status(),
                    stdout: Vec::new(),
                    stderr: error.into_bytes(),
                },
                true,
            )
        });
    let (mut log, mut log_truncated) = bounded_combined_log(log_output, config.max_log_bytes);
    log_truncated |= capture_truncated;
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
    queue::publish_result(&staging, &final_result)?;
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
    let output = runtime_output(
        config,
        ["exec", container, "head", "-c", "96", COMPLETION_MARKER],
    )?;
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
    deadline: Instant,
) -> Result<Option<ArtifactExport>> {
    if destination.exists() {
        fs::remove_dir_all(destination)
            .map_err(|error| format!("reset artifact staging: {error}"))?;
    }
    fs::create_dir(destination).map_err(|error| format!("create artifact staging: {error}"))?;
    fs::set_permissions(destination, fs::Permissions::from_mode(0o700))
        .map_err(|error| format!("chmod artifact staging: {error}"))?;

    let mut archive_command = runtime(config)?;
    archive_command.args(["exec", container, "sh", "-c"]).arg(
        "test ! -d /workspace/artifacts || ".to_owned()
            + "exec tar -C /workspace/artifacts -cf - .",
    );
    let mut stream = runtime::ArtifactStream::start(
        &mut archive_command,
        artifact_wire_limit(config.max_snapshot_bytes, config.max_snapshot_entries)?,
        deadline,
    )?;
    let imported = import_artifact_tar(
        &mut stream,
        destination,
        config.max_snapshot_bytes,
        config.max_snapshot_entries,
    );
    // A rejected archive can still have a bounded, fully observed producer.
    // Transport uncertainty instead propagates and retains the dispatch intent.
    let (archive_status, archive_stderr) = stream.finish()?;

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
        .map_err(|error| format!("read archive entries: {error}"))?
        .raw(true);
    let mut bytes_left = max_bytes;
    let mut entries_left = max_entries;
    let mut imported = 0_u64;
    let mut long_name: Option<PathBuf> = None;

    for entry in entries {
        if entries_left == 0 {
            return Err("artifact entry limit exceeded".into());
        }
        let mut entry = entry.map_err(|error| format!("read archive entry: {error}"))?;
        let entry_type = entry.header().entry_type();
        if entry_type.is_gnu_longname() {
            if long_name.is_some() || entry.size() == 0 || entry.size() > ARTIFACT_PATH_BYTES {
                return Err("artifact archive has an invalid long name".into());
            }
            let mut bytes = Vec::with_capacity(entry.size() as usize);
            entry
                .read_to_end(&mut bytes)
                .map_err(|_| "read artifact long name")?;
            if bytes.len() as u64 != entry.size() || bytes.pop() != Some(0) || bytes.contains(&0) {
                return Err("artifact archive has an invalid long name".into());
            }
            long_name = Some(PathBuf::from(OsString::from_vec(bytes)));
            continue;
        }
        entries_left -= 1;
        // Raw iteration rejects unsupported PAX/sparse/link records before the
        // library can allocate their metadata or expand sparse data.
        if !entry_type.is_file() && !entry_type.is_dir() {
            return Err("artifact archive contains a link or special file".into());
        }
        if entry_type.is_dir() && entry.size() != 0 {
            return Err("artifact directory has a nonempty payload".into());
        }
        let raw_path = match long_name.take() {
            Some(path) => path,
            None => entry
                .path()
                .map_err(|error| format!("read artifact path: {error}"))?
                .into_owned(),
        };
        let path = normalize_archive_path(&raw_path)?;
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
    if long_name.is_some() {
        return Err("artifact archive has an orphan long name".into());
    }
    Ok(imported)
}

// GNU tar uses at most one bounded long-name record per accepted entry and
// pads its final output to a 10 KiB record. Count headers and padding even when
// the importer rejects an entry or stops at the TAR end marker.
const ARTIFACT_PATH_BYTES: u64 = 4096;

fn artifact_wire_limit(max_bytes: u64, max_entries: u64) -> Result<u64> {
    max_entries
        .checked_mul(512 + 512 + 512 + ARTIFACT_PATH_BYTES)
        .and_then(|overhead| max_bytes.checked_add(overhead))
        .and_then(|total| total.checked_add(10 * 1024))
        .ok_or_else(|| "artifact wire limit overflow".into())
}

fn normalize_archive_path(path: &Path) -> Result<PathBuf> {
    if path.as_os_str().len() >= ARTIFACT_PATH_BYTES as usize {
        return Err("artifact path limit exceeded".into());
    }
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
    let listing = runtime_output(
        config,
        [
            "ps",
            "--all",
            &format!("--filter=name={container}"),
            "--format={{.Names}}",
        ],
    )?;
    if !listing.status.success() {
        return Err("cannot determine whether the owned worker container still exists".into());
    }
    if !String::from_utf8_lossy(&listing.stdout)
        .lines()
        .any(|name| name.trim() == container)
    {
        return Ok(());
    }
    let output = runtime_output(
        config,
        [
            "inspect",
            "--format={{index .Config.Labels \"io.tentaflake.worker\"}}",
            container,
        ],
    )?;
    if !output.status.success() {
        return Err(format!(
            "cannot inspect ownership of worker container {container}"
        ));
    }
    if String::from_utf8_lossy(&output.stdout).trim() != config.agent {
        return Err(format!("refusing to remove unowned container {container}"));
    }
    remove_worker_container(config, container)
}

fn remove_worker_container(config: &Config, container: &str) -> Result<()> {
    command_ok(
        runtime(config)?.args(["rm", "--force", container]),
        "remove disposable container",
    )
}

fn runtime(config: &Config) -> Result<Command> {
    runtime::command(config)
}

fn runtime_output<I, S>(config: &Config, args: I) -> Result<Output>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    runtime::output(runtime(config)?.args(args))
}

fn command_ok(command: &mut Command, operation: &str) -> Result<()> {
    let output = runtime::output(command).map_err(|error| format!("{operation}: {error}"))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(format!(
            "{operation} did not confirm success; outcome remains unknown"
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
    queue::publish_result(&staging, &final_result)
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
    file.write_all(b"\n").map_err(|error| error.to_string())
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
    preserve_on_error: bool,
}

impl JobDirectoryGuard {
    fn new(path: PathBuf) -> Self {
        Self {
            path,
            active: true,
            preserve_on_error: false,
        }
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
        if self.active && !self.preserve_on_error {
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
    let mut names = list_fd_dir(fd)?;
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
                let mut source = File::from(source_fd);
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
    // SAFETY: c_path and the repr(C) OpenHow value remain live; their exact size is supplied.
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
        // SAFETY: openat2 returned a new descriptor; OwnedFd takes sole ownership.
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
        libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK,
    )
}

fn open_at(parent: RawFd, name: &OsStr, flags: i32) -> io::Result<OwnedFd> {
    let name =
        CString::new(name.as_bytes()).map_err(|_| io::Error::from_raw_os_error(libc::EINVAL))?;
    // SAFETY: name is NUL-terminated; all callers use non-creating flags, so no mode is needed.
    let fd = unsafe { libc::openat(parent, name.as_ptr(), flags) };
    if fd < 0 {
        Err(io::Error::last_os_error())
    } else {
        // SAFETY: openat returned a new descriptor; OwnedFd takes sole ownership.
        Ok(unsafe { OwnedFd::from_raw_fd(fd) })
    }
}

fn list_fd_dir(fd: RawFd) -> Result<Vec<OsString>> {
    let path = PathBuf::from(format!("/proc/self/fd/{fd}"));
    fs::read_dir(path)
        .map_err(|error| format!("read directory fd: {error}"))?
        .map(|entry| {
            entry
                .map(|value| value.file_name())
                .map_err(|error| error.to_string())
        })
        .collect()
}

fn fstatat_nofollow(parent: RawFd, name: &OsStr) -> Result<libc::stat> {
    let name = CString::new(name.as_bytes()).map_err(|_| "entry name contains NUL".to_string())?;
    let mut stat = std::mem::MaybeUninit::<libc::stat>::uninit();
    // SAFETY: name is NUL-terminated and stat points to writable storage of the correct type.
    let result = unsafe {
        libc::fstatat(
            parent,
            name.as_ptr(),
            stat.as_mut_ptr(),
            libc::AT_SYMLINK_NOFOLLOW,
        )
    };
    if result == 0 {
        // SAFETY: A successful fstatat initialized stat before it is read.
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
    // SAFETY: name is NUL-terminated; buffer is writable for the supplied length.
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
    // SAFETY: name is a live NUL-terminated string; unlinkat takes no other pointers.
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
    use std::os::unix::fs::symlink;

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

    #[test]
    fn private_fence_arguments_are_exact_and_readonly_commands_cannot_adopt() {
        let parse = |args: &[&str]| parse_args(args.iter().map(|value| OsString::from(*value)));
        assert_eq!(
            parse(&[
                "--config",
                "/config",
                "--maintenance-fence-fd",
                "3",
                "approve",
                "job_1"
            ])
            .unwrap(),
            (
                PathBuf::from("/config"),
                WorkerCommand::Approve("job_1".into()),
                Some(3)
            )
        );
        for fd in ["", "0", "2", "-1", "03", "+3", "2147483648", "env"] {
            assert!(
                parse(&["--config", "/config", "--maintenance-fence-fd", fd, "drain"]).is_err()
            );
        }
        for kind in ["probe", "maintenance"] {
            assert!(parse(&["--config", "/config", "--maintenance-fence-fd", "3", kind]).is_err());
        }
        for action in [
            WorkerCommand::Drain,
            WorkerCommand::Approve("job_1".into()),
            WorkerCommand::Deny("job_1".into()),
        ] {
            let command = delegation_command(
                Path::new("/immutable-cli"),
                Path::new("/exact-config"),
                &action,
            )
            .unwrap();
            let mut expected = vec!["admission", "worker-run", "/exact-config"];
            match action {
                WorkerCommand::Drain => expected.push("drain"),
                WorkerCommand::Approve(_) => expected.extend(["approve", "job_1"]),
                WorkerCommand::Deny(_) => expected.extend(["deny", "job_1"]),
                _ => unreachable!(),
            }
            assert_eq!(
                command.get_args().collect::<Vec<_>>(),
                expected.iter().map(OsStr::new).collect::<Vec<_>>()
            );
        }
    }

    #[test]
    fn legacy_mutation_refuses_any_prior_or_uncertain_ledger_path() {
        let root = temp_dir("legacy-ledger");
        let path = root.join("ledger");
        assert!(require_legacy_without_ledger(&path).is_ok());
        fs::write(&path, b"malformed ledger").unwrap();
        assert!(require_legacy_without_ledger(&path).is_err());
        assert!(require_legacy_without_ledger(&path.join("not-a-directory")).is_err());
        fs::remove_file(&path).unwrap();
        symlink(root.join("missing"), &path).unwrap();
        assert!(require_legacy_without_ledger(&path).is_err());
        fs::remove_file(&path).unwrap();
        fs::create_dir(&path).unwrap();
        assert!(require_legacy_without_ledger(&path).is_err());
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

    fn append_raw_tar_record(archive: &mut Vec<u8>, kind: u8, size: u64, data: &[u8]) {
        let mut header = tar::Header::new_gnu();
        header.set_path("record").unwrap();
        header.set_entry_type(tar::EntryType::new(kind));
        header.set_mode(0o600);
        header.set_size(size);
        header.set_cksum();
        archive.extend_from_slice(header.as_bytes());
        archive.extend_from_slice(data);
        archive.resize(archive.len().next_multiple_of(512), 0);
    }

    #[test]
    fn artifact_import_accepts_bounded_gnu_long_names() {
        let mut archive = Vec::new();
        let path = format!("{}/tool", "a".repeat(200));
        let name = format!("{path}\0");
        append_raw_tar_record(&mut archive, b'L', name.len() as u64, name.as_bytes());
        append_raw_tar_record(&mut archive, b'0', 4, b"test");
        archive.extend_from_slice(&[0; 1024]);
        let destination = temp_dir("artifact-long-name");
        assert_eq!(
            import_artifact_tar(archive.as_slice(), &destination, 4, 1).unwrap(),
            1
        );
        assert_eq!(fs::read(destination.join(path)).unwrap(), b"test");
        fs::remove_dir_all(destination).unwrap();
    }

    #[test]
    fn artifact_import_rejects_hidden_metadata_before_reading_its_payload() {
        for (kind, size) in [
            (b'L', 4097),
            (b'x', 1 << 40),
            (b'g', 1 << 40),
            (b'K', 1 << 40),
            (b'S', 1 << 40),
            (b'5', 1),
        ] {
            let mut archive = Vec::new();
            append_raw_tar_record(&mut archive, kind, size, &[]);
            let destination = temp_dir("artifact-metadata");
            let error = import_artifact_tar(archive.as_slice(), &destination, 4, 1).unwrap_err();
            assert!(
                error.contains("long name")
                    || error.contains("special file")
                    || error.contains("nonempty payload"),
                "{error}"
            );
            assert_eq!(fs::read_dir(&destination).unwrap().count(), 0);
            fs::remove_dir_all(destination).unwrap();
        }
    }

    #[test]
    fn artifact_import_rejects_double_orphan_and_malformed_long_names() {
        for names in [
            vec![b"path\0".as_slice(), b"other\0"],
            vec![b"path\0"],
            vec![b"unterminated"],
            vec![b"bad\0path\0"],
            vec![b"../escape\0"],
        ] {
            let mut archive = Vec::new();
            for name in &names {
                append_raw_tar_record(&mut archive, b'L', name.len() as u64, name);
            }
            // A single valid name without the next record is an orphan. All
            // other cases also have a following regular file to exercise use.
            if names != [b"path\0".as_slice()] {
                append_raw_tar_record(&mut archive, b'0', 0, &[]);
            }
            archive.extend_from_slice(&[0; 1024]);
            let destination = temp_dir("artifact-invalid-long-name");
            assert!(import_artifact_tar(archive.as_slice(), &destination, 4, 2).is_err());
            assert_eq!(fs::read_dir(&destination).unwrap().count(), 0);
            fs::remove_dir_all(destination).unwrap();
        }
        assert!(artifact_wire_limit(u64::MAX, 1).is_err());
        assert!(artifact_wire_limit(1, u64::MAX).is_err());
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
