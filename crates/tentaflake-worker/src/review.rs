//! Bounded operator review. Neither command may dispatch or recover a job.
use super::*;
use sha2::{Digest, Sha256};
use std::os::unix::fs::MetadataExt;

const MAX_ENTRIES: usize = 256;
const MAX_QUEUE_BYTES: u64 = 8 * 1024 * 1024;
const MAX_REQUEST_BYTES: usize = 64 * 1024;
const MAX_RESPONSE_BYTES: usize = 256 * 1024;
const MAX_ROWS: usize = 100;
const MAX_DECISIONS: usize = 4096;
const MAX_RECEIPT_BYTES: usize = 1024;

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub(super) enum Decision {
    Approve,
    Deny,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Receipt {
    schema_version: u8,
    agent: String,
    job_id: String,
    request_digest: String,
    decision: Decision,
    state: String,
    drain_required: bool,
}

#[derive(Serialize)]
struct RequestRow {
    id: String,
    request_digest: String,
    action_class: ActionClass,
    argv: Vec<String>,
    cwd: &'static str,
    timeout_seconds: u64,
    state: &'static str,
    approval_status: &'static str,
    can_approve: bool,
    can_deny: bool,
}

#[derive(Serialize)]
struct Requests<'a> {
    schema_version: u8,
    agent: &'a str,
    initialized: bool,
    truncated: bool,
    requests: Vec<RequestRow>,
}

struct Queue {
    root: OwnedFd,
    pending: OwnedFd,
    _lock: File,
}

fn invalid(error: impl std::fmt::Display) -> String {
    format!("invalid: {error}")
}

fn check_owner(file: &File, directory: bool, private: bool) -> Result<()> {
    let metadata = file.metadata().map_err(invalid)?;
    if metadata.uid() != unsafe { libc::geteuid() }
        || (directory && !metadata.is_dir())
        || (!directory && !metadata.is_file())
        || metadata.mode() & if private { 0o077 } else { 0o022 } != 0
    {
        return Err("invalid: review state must be owned by the worker and private".into());
    }
    Ok(())
}

fn checked_directory(parent: RawFd, name: &str, private: bool) -> Result<Option<OwnedFd>> {
    let fd = match open_dir_at(parent, OsStr::new(name)) {
        Ok(fd) => fd,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(invalid(error)),
    };
    check_owner(&File::from(fd.try_clone().map_err(invalid)?), true, private)?;
    Ok(Some(fd))
}

fn read_at(parent: RawFd, name: &str, max: usize) -> Result<Option<Vec<u8>>> {
    let fd = match open_file_at(parent, OsStr::new(name)) {
        Ok(fd) => fd,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(invalid(error)),
    };
    let mut file = File::from(fd);
    check_owner(&file, false, true)?;
    let metadata = file.metadata().map_err(invalid)?;
    if metadata.nlink() != 1 {
        // write_private may have durably published the final hard link before
        // an interruption prevented staging cleanup. Accept exactly that pair,
        // never a link to a different inode or an extra external hard link.
        let staging =
            fstatat_nofollow(parent, OsStr::new(&format!(".{name}.new"))).map_err(invalid)?;
        if metadata.nlink() != 2
            || staging.st_dev != metadata.dev()
            || staging.st_ino != metadata.ino()
            || staging.st_mode & libc::S_IFMT != libc::S_IFREG
        {
            return Err("invalid: review file has an unexpected hard link".into());
        }
    }
    if metadata.len() > max as u64 {
        return Err("limit: review file exceeds byte ceiling".into());
    }
    read_bounded(&mut file, max)
        .map(Some)
        .map_err(|error| format!("limit: {error}"))
}

impl Queue {
    fn open(config: &Config) -> Result<Option<Self>> {
        match fs::symlink_metadata(&config.state_dir) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(invalid(error)),
            Ok(_) => {}
        }
        let root = open_path_no_symlinks(&config.state_dir).map_err(invalid)?;
        check_owner(&File::from(root.try_clone().map_err(invalid)?), true, false)?;
        // Read-only open permits inspection in a read-only service namespace.
        // Never create the lock or repair a partially initialized queue here.
        let lock = match open_file_at(root.as_raw_fd(), OsStr::new(".queue.lock")) {
            Ok(fd) => File::from(fd),
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(invalid(error)),
        };
        check_owner(&lock, false, true)?;
        if lock.metadata().map_err(invalid)?.nlink() != 1 {
            return Err("invalid: queue lock has an unexpected hard link".into());
        }
        if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            return Err("busy: worker queue is already owned or unavailable".into());
        }
        let pending = checked_directory(root.as_raw_fd(), "pending", true)?
            .ok_or("invalid: initialized queue has no private pending directory")?;
        Ok(Some(Self {
            root,
            pending,
            _lock: lock,
        }))
    }

    fn request(&self, config: &Config, job: &str) -> Result<Option<JobRequest>> {
        let Some(bytes) = read_at(
            self.pending.as_raw_fd(),
            &format!("{job}.json"),
            config.max_request_bytes.min(MAX_REQUEST_BYTES as u64) as usize,
        )?
        else {
            return Ok(None);
        };
        let request: JobRequest = serde_json::from_slice(&bytes).map_err(invalid)?;
        validate_request(config, &request).map_err(invalid)?;
        if request.id != job {
            return Err("invalid: private request ID differs from filename".into());
        }
        Ok(Some(request))
    }

    fn receipt(&self, config: &Config, job: &str) -> Result<Option<Receipt>> {
        read_receipt(config, self.root.as_raw_fd(), job)
    }

    fn no_running_claim(&self, job: &str) -> Result<()> {
        let directory = checked_directory(self.root.as_raw_fd(), "running", true)?
            .ok_or("invalid: initialized queue has no private running directory")?;
        match open_file_at(directory.as_raw_fd(), OsStr::new(&format!("{job}.json"))) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(invalid(error)),
            Ok(_) => Err("conflict: job already has a running claim".into()),
        }
    }
}

fn digest(request: &JobRequest) -> Result<String> {
    // All validated request fields, including defaulted timeout, are bound.
    let bytes = serde_json::to_vec(request).map_err(invalid)?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}

fn valid_digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn read_receipt(config: &Config, root: RawFd, job: &str) -> Result<Option<Receipt>> {
    let Some(directory) = checked_directory(root, "decisions", true)? else {
        return Ok(None);
    };
    let Some(bytes) = read_at(directory.as_raw_fd(), job, MAX_RECEIPT_BYTES)? else {
        return Ok(None);
    };
    let receipt: Receipt = serde_json::from_slice(&bytes).map_err(invalid)?;
    if receipt.schema_version != 1
        || receipt.agent != config.agent
        || receipt.job_id != job
        || !valid_digest(&receipt.request_digest)
        || receipt.state != "recorded"
        || !receipt.drain_required
    {
        return Err("invalid: decision receipt does not match its queue identity".into());
    }
    Ok(Some(receipt))
}

pub(super) fn decision_for(config: &Config, request: &JobRequest) -> Result<Option<Decision>> {
    let root = open_path_no_symlinks(&config.state_dir).map_err(invalid)?;
    let Some(receipt) = read_receipt(config, root.as_raw_fd(), &request.id)? else {
        return Ok(None);
    };
    if receipt.request_digest != digest(request)? {
        return Err("conflict: captured request differs from its recorded decision".into());
    }
    if receipt.decision == Decision::Approve && !request.action_class.needs_approval() {
        return Err("invalid: action class cannot carry a recorded approval".into());
    }
    Ok(Some(receipt.decision))
}

fn legacy_approved(queue: &Queue, job: &str) -> Result<bool> {
    let Some(directory) = checked_directory(queue.root.as_raw_fd(), "approvals", true)? else {
        return Err("invalid: initialized queue has no private approvals directory".into());
    };
    match read_at(directory.as_raw_fd(), job, 32)? {
        Some(bytes) if bytes == b"approved\n" => Ok(true),
        Some(_) => Err("invalid: legacy approval record".into()),
        None => Ok(false),
    }
}

pub(super) fn requests(config: &Config) -> Result<()> {
    let mut response = Requests {
        schema_version: 1,
        agent: &config.agent,
        initialized: false,
        truncated: false,
        requests: Vec::new(),
    };
    if let Some(queue) = Queue::open(config)? {
        response.initialized = true;
        let (mut names, _) = queue::scan(
            queue.pending.as_raw_fd(),
            config.max_queue_entries.min(MAX_ENTRIES),
            config.max_queue_bytes.min(MAX_QUEUE_BYTES),
        )
        .map_err(|error| format!("limit: {error}"))?;
        names.sort();
        for name in names {
            // Count every top-level entry, including unpublished staging, and
            // refuse links/special files even when their names would be ignored.
            let stat = fstatat_nofollow(queue.pending.as_raw_fd(), &name).map_err(invalid)?;
            if stat.st_mode & libc::S_IFMT != libc::S_IFREG {
                return Err("invalid: pending directory contains a link or special entry".into());
            }
            let Some(job) = name.to_str().and_then(|name| name.strip_suffix(".json")) else {
                continue;
            };
            validate_identifier("job id", job, 64).map_err(invalid)?;
            let request = queue
                .request(config, job)?
                .ok_or("invalid: captured request disappeared while locked")?;
            queue.no_running_claim(job)?;
            if queue::terminal(config, &request).map_err(invalid)? {
                continue;
            }
            let decision = decision_for(config, &request)?;
            let approved = legacy_approved(&queue, job)?;
            if decision == Some(Decision::Deny) && approved {
                return Err("conflict: recorded denial has a legacy approval".into());
            }
            let status = match decision {
                Some(Decision::Approve) => "approved",
                Some(Decision::Deny) => "denied",
                None if approved => "approved",
                None if request.action_class == ActionClass::Forbidden => "forbidden",
                None if !request.action_class.needs_approval() => "not-required",
                None => "pending",
            };
            if response.requests.len() == MAX_ROWS {
                response.truncated = true;
                break;
            }
            let undecided = decision.is_none() && !approved;
            let row = RequestRow {
                request_digest: digest(&request)?,
                can_approve: undecided && request.action_class.needs_approval(),
                can_deny: undecided,
                id: request.id,
                action_class: request.action_class,
                argv: request.argv,
                cwd: "/workspace",
                timeout_seconds: request.timeout_seconds,
                state: "pending",
                approval_status: status,
            };
            response.requests.push(row);
            if serde_json::to_vec(&response).map_err(invalid)?.len() > MAX_RESPONSE_BYTES {
                response.requests.pop();
                response.truncated = true;
                break;
            }
        }
    }
    print_json(&response)
}

pub(super) fn decide_command(
    config: &Config,
    mut args: impl Iterator<Item = String>,
) -> Result<()> {
    let job = args.next().ok_or("invalid: missing job ID")?;
    validate_identifier("job id", &job, 64).map_err(invalid)?;
    let decision = match args.next().as_deref() {
        Some("approve") => Decision::Approve,
        Some("deny") => Decision::Deny,
        _ => return Err("invalid: decision must be approve or deny".into()),
    };
    if args.next().as_deref() != Some("--expected-digest") {
        return Err("invalid: --expected-digest is required".into());
    }
    let expected = args.next().ok_or("invalid: missing expected digest")?;
    if !valid_digest(&expected) {
        return Err("invalid: expected digest must be 64 lowercase hexadecimal characters".into());
    }
    reject_extra_args(args).map_err(invalid)?;
    let queue = Queue::open(config)?.ok_or("not found: worker queue is not initialized")?;
    let request = queue.request(config, &job)?;
    if let Some(request) = &request
        && digest(request)? != expected
    {
        return Err("conflict: captured request changed; inspect it again".into());
    }
    if let Some(receipt) = queue.receipt(config, &job)? {
        if receipt.request_digest != expected || receipt.decision != decision {
            return Err("conflict: job already has a different recorded decision".into());
        }
        return print_json(&receipt).map_err(|error| format!("uncertain: {error}"));
    }
    let request = request.ok_or("not found: no captured pending request with this ID")?;
    queue.no_running_claim(&job)?;
    if queue::terminal(config, &request).map_err(invalid)? {
        return Err("conflict: job already has a terminal result".into());
    }
    if legacy_approved(&queue, &job)? {
        return Err("conflict: job already has a legacy approval".into());
    }
    if decision == Decision::Approve && !request.action_class.needs_approval() {
        return Err("invalid: this action class cannot be approved".into());
    }
    let receipts = match checked_directory(queue.root.as_raw_fd(), "decisions", true)? {
        Some(directory) => directory,
        None => {
            let name = CString::new("decisions").unwrap();
            if unsafe { libc::mkdirat(queue.root.as_raw_fd(), name.as_ptr(), 0o700) } != 0 {
                return Err(invalid(io::Error::last_os_error()));
            }
            File::from(open_dir_at(queue.root.as_raw_fd(), OsStr::new(".")).map_err(invalid)?)
                .sync_all()
                .map_err(invalid)?;
            checked_directory(queue.root.as_raw_fd(), "decisions", true)?
                .ok_or("invalid: private decision directory disappeared")?
        }
    };
    // Retained receipts prevent job-ID reuse/replay, and have a separate hard
    // budget. Existing receipts remain retryable when new decisions are full.
    let (names, _) = queue::scan(
        receipts.as_raw_fd(),
        MAX_DECISIONS,
        (MAX_DECISIONS * MAX_RECEIPT_BYTES) as u64,
    )
    .map_err(|error| format!("limit: {error}"))?;
    if names.len() >= MAX_DECISIONS {
        return Err("limit: decision receipt capacity reached".into());
    }
    let receipt = Receipt {
        schema_version: 1,
        agent: config.agent.clone(),
        job_id: job.clone(),
        request_digest: expected,
        decision,
        state: "recorded".into(),
        drain_required: true,
    };
    queue::write_private(
        &config.state_dir.join("decisions").join(job),
        &serde_json::to_vec(&receipt).map_err(invalid)?,
    )
    .map_err(|error| format!("uncertain: decision publication did not finish: {error}"))?;
    print_json(&receipt).map_err(|error| format!("uncertain: {error}"))
}

fn print_json(value: &impl Serialize) -> Result<()> {
    let output = serde_json::to_vec(value).map_err(invalid)?;
    if output.len() > MAX_RESPONSE_BYTES {
        return Err("limit: review response exceeds byte ceiling".into());
    }
    let mut stdout = io::stdout().lock();
    stdout.write_all(&output).map_err(invalid)?;
    stdout.write_all(b"\n").map_err(invalid)
}
