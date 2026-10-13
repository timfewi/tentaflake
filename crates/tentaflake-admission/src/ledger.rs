//! Private, durable startup claims. Only a newly committed claim grants dispatch.
//!
//! The caller owns authentication, current host evidence, bounded dispatch, and
//! proof of container ownership/absence. An unknown result never frees capacity.
//! Keep the permit until the bounded start handoff and outcome commit finish.
use crate::decision::identifier;
use crate::{
    AdmissionRequest, AdmissionSnapshot, Decision, MAX_RESERVATIONS, Reservation,
    ReservationSnapshot, ReservationState, SCHEMA_VERSION, authorize, decide,
    validate_reservations,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::ffi::{CString, OsStr};
use std::fmt;
use std::fs::File;
use std::io::{self, Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, RawFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::path::{Component, Path};

pub const MAX_RECEIPTS: usize = MAX_RESERVATIONS;
pub const MAX_LEDGER_BYTES: usize = 8 * 1024 * 1024;
const STATE: &str = "ledger.json";
const STAGING: &str = ".ledger.pending";
const LOCK: &str = ".lock";

#[derive(Debug)]
pub enum LedgerError {
    Io(io::Error),
    Invalid(&'static str),
    Busy,
    Conflict,
    StaleRevision,
    Context,
    Full,
}

impl fmt::Display for LedgerError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "admission ledger I/O: {error}"),
            Self::Invalid(reason) => write!(formatter, "invalid admission ledger: {reason}"),
            Self::Busy => formatter.write_str("admission ledger is busy"),
            Self::Conflict => {
                formatter.write_str("admission operation conflicts with durable state")
            }
            Self::StaleRevision => formatter.write_str("admission ledger revision changed"),
            Self::Context => formatter.write_str("current admission evidence is unavailable"),
            Self::Full => formatter.write_str("admission ledger capacity exhausted"),
        }
    }
}

impl std::error::Error for LedgerError {}

impl From<io::Error> for LedgerError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

/// Historical decision plus current outcome of this exact operation. No full
/// aggregate snapshot is persisted; offline replay can export that separately.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct Receipt {
    pub request: AdmissionRequest,
    pub decision: Decision,
    pub reservation: Reservation,
    pub claimed_revision: u64,
    pub updated_revision: u64,
}

/// Host-owned observations, never assertions accepted from an untrusted client.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Observation {
    Running,
    /// Authoritative backend absence AND the old dispatcher can no longer run.
    /// A timeout, daemon outage, name-only match, or failed preStart is not proof.
    Stopped,
    Unknown,
}

pub enum ClaimOutcome<'a> {
    New(DispatchPermit<'a>),
    Existing(Box<Receipt>),
    Denied(Decision),
}

pub struct Ledger {
    directory: File,
}

struct LedgerLock {
    file: File,
}

impl Drop for LedgerLock {
    fn drop(&mut self) {
        // fork temporarily shares the open file description even with CLOEXEC.
        // Release our completed transaction explicitly; closing only this copy
        // can otherwise keep the lock until an unrelated child reaches exec.
        // File's drop still closes the descriptor if unlocking fails.
        unsafe {
            libc::flock(self.file.as_raw_fd(), libc::LOCK_UN);
        }
    }
}

/// Not Clone or serializable: this is the only dispatch authority returned by
/// the ledger. Dropping it leaves the already durable Unknown reservation.
#[must_use = "a new permit owns one bounded dispatch; dropping retains an uncertain claim"]
pub struct DispatchPermit<'a> {
    ledger: &'a Ledger,
    _lock: LedgerLock,
    state: State,
    index: usize,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct State {
    schema_version: u32,
    revision: u64,
    generation: String,
    receipts: Vec<Receipt>,
}

impl Ledger {
    /// Explicit bootstrap only. Creates a new private directory and refuses an
    /// existing path, including an empty directory left by an interrupted init.
    /// `generation` is the stable host admission authority epoch, not a boot ID.
    pub fn initialize(path: &Path, generation: &str) -> Result<Self, LedgerError> {
        if !identifier(generation) {
            return Err(LedgerError::Invalid("generation"));
        }
        let name = path
            .file_name()
            .ok_or(LedgerError::Invalid("directory path"))?;
        let parent = open_directory(
            path.parent()
                .ok_or(LedgerError::Invalid("directory parent"))?,
        )?;
        let encoded_name = cstring(name)?;
        if unsafe { libc::mkdirat(parent.as_raw_fd(), encoded_name.as_ptr(), 0o700) } != 0 {
            return Err(io::Error::last_os_error().into());
        }
        parent.sync_all()?;
        let ledger = Self {
            directory: open_at(parent.as_raw_fd(), name, libc::O_RDONLY | libc::O_DIRECTORY)?,
        };
        ledger.validate_directory()?;
        let _lock = ledger.lock(true)?;
        ledger.write(&State {
            schema_version: SCHEMA_VERSION,
            revision: 1,
            generation: generation.to_owned(),
            receipts: Vec::new(),
        })?;
        Ok(ledger)
    }

    /// Missing, malformed or incomplete state is an error, never an empty ledger.
    pub fn open(path: &Path) -> Result<Self, LedgerError> {
        let ledger = Self {
            directory: open_directory(path)?,
        };
        ledger.validate_directory()?;
        let _lock = ledger.lock(false)?;
        ledger.read()?;
        Ok(ledger)
    }

    pub fn snapshot(&self) -> Result<ReservationSnapshot, LedgerError> {
        let _lock = self.lock(false)?;
        Ok(self.read()?.snapshot())
    }

    /// Collect trusted policy, identity/grants and readiness while holding the
    /// exclusive lock. The supplied reservation list is always replaced by disk
    /// state. Even an identical retry repeats current authorization before its
    /// receipt is disclosed; it can never acquire another dispatch permit.
    pub fn claim(
        &self,
        request: &AdmissionRequest,
        fresh_context: impl FnOnce() -> Result<(AdmissionSnapshot, u64), String>,
    ) -> Result<ClaimOutcome<'_>, LedgerError> {
        let lock = self.lock(false)?;
        let mut state = self.read()?;
        let (mut snapshot, now) = fresh_context().map_err(|_| LedgerError::Context)?;
        snapshot.reservations = state.snapshot();
        let reasons = authorize(&snapshot, request, now);
        if !reasons.is_empty() {
            return Ok(ClaimOutcome::Denied(Decision {
                allowed: false,
                reasons,
                reservation: None,
            }));
        }
        if let Some(receipt) = state
            .receipts
            .iter()
            .find(|receipt| receipt.request.operation_id == request.operation_id)
        {
            if receipt.request != *request {
                return Err(LedgerError::Conflict);
            }
            return Ok(ClaimOutcome::Existing(Box::new(receipt.clone())));
        }
        let decision = decide(&snapshot, request, now);
        if !decision.allowed {
            return Ok(ClaimOutcome::Denied(decision));
        }
        if state.receipts.len() >= MAX_RECEIPTS {
            return Err(LedgerError::Full);
        }
        let mut reservation = decision
            .reservation
            .clone()
            .ok_or(LedgerError::Invalid("allowed decision without reservation"))?;
        // A crash anywhere after this commit may have dispatched. Recovery may
        // inspect the exact operation, but must never repeat its dispatch.
        reservation.state = ReservationState::Unknown;
        state.advance()?;
        state.receipts.push(Receipt {
            request: request.clone(),
            decision,
            reservation,
            claimed_revision: state.revision,
            updated_revision: state.revision,
        });
        self.write(&state)?;
        let index = state.receipts.len() - 1;
        Ok(ClaimOutcome::New(DispatchPermit {
            ledger: self,
            _lock: lock,
            state,
            index,
        }))
    }

    /// Reconcile one exact operation under the dispatch lock. The host callback
    /// must prove ownership/absence without relying on the instance name alone.
    /// No records or persistent storage allocations are deleted here.
    pub fn reconcile(
        &self,
        operation_id: &str,
        expected_ledger_revision: u64,
        observe: impl FnOnce(&Receipt) -> Result<Observation, String>,
    ) -> Result<Receipt, LedgerError> {
        let _lock = self.lock(false)?;
        let mut state = self.read()?;
        if state.revision != expected_ledger_revision {
            return Err(LedgerError::StaleRevision);
        }
        let index = state
            .receipts
            .iter()
            .position(|receipt| receipt.request.operation_id == operation_id)
            .ok_or(LedgerError::Conflict)?;
        if !state.current().values().any(|current| *current == index) {
            return Err(LedgerError::Conflict);
        }
        let observation = observe(&state.receipts[index]).map_err(|_| LedgerError::Context)?;
        self.record(&mut state, index, observation)
    }

    fn record(
        &self,
        state: &mut State,
        index: usize,
        observation: Observation,
    ) -> Result<Receipt, LedgerError> {
        let next = match observation {
            Observation::Running => ReservationState::Active,
            Observation::Stopped => ReservationState::Retained,
            Observation::Unknown => ReservationState::Unknown,
        };
        let previous = state.receipts[index].reservation.state;
        if previous == ReservationState::Retained && next != ReservationState::Retained {
            return Err(LedgerError::Conflict);
        }
        if previous != next {
            state.advance()?;
            let receipt = &mut state.receipts[index];
            receipt.reservation.state = next;
            if next == ReservationState::Retained {
                receipt.reservation.resources = receipt.reservation.resources.retained();
            }
            receipt.updated_revision = state.revision;
            self.write(state)?;
        }
        Ok(state.receipts[index].clone())
    }

    fn validate_directory(&self) -> Result<(), LedgerError> {
        let metadata = self.directory.metadata()?;
        if !metadata.is_dir()
            || metadata.uid() != unsafe { libc::geteuid() }
            || metadata.mode() & 0o7777 != 0o700
        {
            return Err(LedgerError::Invalid(
                "directory must be owned and mode 0700",
            ));
        }
        Ok(())
    }

    fn lock(&self, create: bool) -> Result<LedgerLock, LedgerError> {
        self.validate_directory()?;
        let flags = libc::O_RDWR
            | if create {
                libc::O_CREAT | libc::O_EXCL
            } else {
                0
            };
        let file = self.open_file(LOCK, flags)?;
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            let error = io::Error::last_os_error();
            return Err(if error.kind() == io::ErrorKind::WouldBlock {
                LedgerError::Busy
            } else {
                error.into()
            });
        }
        Ok(LedgerLock { file })
    }

    fn open_file(&self, name: &str, flags: i32) -> Result<File, LedgerError> {
        let file = open_at(self.directory.as_raw_fd(), OsStr::new(name), flags)?;
        let metadata = file.metadata()?;
        if !metadata.is_file()
            || metadata.uid() != unsafe { libc::geteuid() }
            || metadata.mode() & 0o7777 != 0o600
            || metadata.nlink() != 1
        {
            return Err(LedgerError::Invalid(
                "files must be private regular unlinked copies",
            ));
        }
        Ok(file)
    }

    fn read(&self) -> Result<State, LedgerError> {
        let file = self.open_file(STATE, libc::O_RDONLY)?;
        if file.metadata()?.len() > MAX_LEDGER_BYTES as u64 {
            return Err(LedgerError::Full);
        }
        let mut bytes = Vec::new();
        file.take(MAX_LEDGER_BYTES as u64 + 1)
            .read_to_end(&mut bytes)?;
        if bytes.len() > MAX_LEDGER_BYTES {
            return Err(LedgerError::Full);
        }
        let state: State =
            serde_json::from_slice(&bytes).map_err(|_| LedgerError::Invalid("record encoding"))?;
        state.validate()?;
        Ok(state)
    }

    fn write(&self, state: &State) -> Result<(), LedgerError> {
        state.validate()?;
        let bytes =
            serde_json::to_vec(state).map_err(|_| LedgerError::Invalid("record encoding"))?;
        if bytes.len() > MAX_LEDGER_BYTES {
            return Err(LedgerError::Full);
        }
        // Preserve interrupted staging for explicit recovery. Never silently
        // discard evidence of a failed durable transaction or reset the ledger.
        let mut pending = self.open_file(STAGING, libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL)?;
        pending.write_all(&bytes)?;
        pending.sync_all()?;
        let source = cstring(OsStr::new(STAGING))?;
        let target = cstring(OsStr::new(STATE))?;
        if unsafe {
            libc::renameat(
                self.directory.as_raw_fd(),
                source.as_ptr(),
                self.directory.as_raw_fd(),
                target.as_ptr(),
            )
        } != 0
        {
            return Err(io::Error::last_os_error().into());
        }
        self.directory.sync_all()?;
        Ok(())
    }
}

impl DispatchPermit<'_> {
    pub fn receipt(&self) -> &Receipt {
        &self.state.receipts[self.index]
    }

    /// Commit the outcome before releasing the global dispatch lock. A failed
    /// write cannot return a new permit; uncertain claims remain charged.
    pub fn complete(mut self, observation: Observation) -> Result<Receipt, LedgerError> {
        self.ledger.record(&mut self.state, self.index, observation)
    }
}

impl State {
    fn advance(&mut self) -> Result<(), LedgerError> {
        self.revision = self.revision.checked_add(1).ok_or(LedgerError::Full)?;
        Ok(())
    }

    fn current(&self) -> BTreeMap<&str, usize> {
        let mut current = BTreeMap::new();
        for (index, receipt) in self.receipts.iter().enumerate() {
            let instance = receipt.request.instance_id.as_str();
            if current.get(instance).is_none_or(|previous: &usize| {
                self.receipts[*previous].claimed_revision < receipt.claimed_revision
            }) {
                current.insert(instance, index);
            }
        }
        current
    }

    fn snapshot(&self) -> ReservationSnapshot {
        ReservationSnapshot {
            revision: self.revision,
            generation: self.generation.clone(),
            complete: true,
            entries: self
                .current()
                .values()
                .map(|index| self.receipts[*index].reservation.clone())
                .collect(),
        }
    }

    fn validate(&self) -> Result<(), LedgerError> {
        if self.schema_version != SCHEMA_VERSION
            || self.revision == 0
            || !identifier(&self.generation)
            || self.receipts.len() > MAX_RECEIPTS
        {
            return Err(LedgerError::Invalid("header or receipt limit"));
        }
        if !validate_reservations(&self.snapshot()).is_empty() {
            return Err(LedgerError::Invalid("reservations"));
        }
        let current = self.current();
        let mut operations = BTreeSet::new();
        let mut claims = BTreeSet::new();
        let latest_update = self
            .receipts
            .iter()
            .map(|receipt| receipt.updated_revision)
            .max()
            .unwrap_or(1);
        if latest_update != self.revision {
            return Err(LedgerError::Invalid("unaccounted revision"));
        }
        for (index, receipt) in self.receipts.iter().enumerate() {
            let request = &receipt.request;
            let original = receipt
                .decision
                .reservation
                .as_ref()
                .ok_or(LedgerError::Invalid("missing original reservation"))?;
            let mut expected = original.clone();
            expected.state = receipt.reservation.state;
            if expected.state == ReservationState::Retained {
                expected.resources = expected.resources.retained();
            }
            if request.schema_version != SCHEMA_VERSION
                || !identifier(&request.operation_id)
                || !identifier(&request.instance_id)
                || !identifier(&request.principal_id)
                || !identifier(&request.expected.policy)
                || !identifier(&request.expected.instance)
                || request.expected.ledger == 0
                || request.expected.ledger.checked_add(1) != Some(receipt.claimed_revision)
                || !operations.insert(&request.operation_id)
                || !claims.insert(receipt.claimed_revision)
                || receipt.claimed_revision > receipt.updated_revision
                || receipt.updated_revision > self.revision
                || !receipt.decision.allowed
                || !receipt.decision.reasons.is_empty()
                || original.state != ReservationState::Reserved
                || original.instance_id != request.instance_id
                || original.owner_id != request.principal_id
                || original.instance_revision != request.expected.instance
                || expected != receipt.reservation
                || receipt.reservation.state == ReservationState::Reserved
                || (current.get(request.instance_id.as_str()) != Some(&index)
                    && receipt.reservation.state != ReservationState::Retained)
            {
                return Err(LedgerError::Invalid(
                    "receipt identity, revision or outcome",
                ));
            }
            let latest = &self.receipts[current[request.instance_id.as_str()]].reservation;
            if latest.owner_id != receipt.reservation.owner_id
                || latest.resources.workspace_bytes < receipt.reservation.resources.workspace_bytes
                || latest.resources.state_bytes < receipt.reservation.resources.state_bytes
            {
                return Err(LedgerError::Invalid(
                    "retained storage ownership or capacity",
                ));
            }
            let original_snapshot = ReservationSnapshot {
                revision: self.revision,
                generation: self.generation.clone(),
                complete: true,
                entries: vec![original.clone()],
            };
            if !validate_reservations(&original_snapshot).is_empty() {
                return Err(LedgerError::Invalid("original reservation"));
            }
        }
        Ok(())
    }
}

fn cstring(value: &OsStr) -> Result<CString, LedgerError> {
    CString::new(value.as_bytes()).map_err(|_| LedgerError::Invalid("path contains NUL"))
}

fn open_at(directory: RawFd, name: &OsStr, flags: i32) -> Result<File, LedgerError> {
    let name = cstring(name)?;
    let fd = unsafe {
        libc::openat(
            directory,
            name.as_ptr(),
            flags | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC,
            0o600,
        )
    };
    if fd < 0 {
        return Err(io::Error::last_os_error().into());
    }
    Ok(unsafe { File::from_raw_fd(fd) })
}

fn open_directory(path: &Path) -> Result<File, LedgerError> {
    if !path.is_absolute() {
        return Err(LedgerError::Invalid("directory path must be absolute"));
    }
    let mut directory = open_at(
        libc::AT_FDCWD,
        OsStr::new("/"),
        libc::O_RDONLY | libc::O_DIRECTORY,
    )?;
    trusted_directory(&directory)?;
    for component in path.components() {
        match component {
            Component::RootDir => {}
            Component::Normal(name) => {
                directory = open_at(
                    directory.as_raw_fd(),
                    name,
                    libc::O_RDONLY | libc::O_DIRECTORY,
                )?;
                trusted_directory(&directory)?;
            }
            _ => return Err(LedgerError::Invalid("directory path components")),
        }
    }
    Ok(directory)
}

fn trusted_directory(directory: &File) -> Result<(), LedgerError> {
    let metadata = directory.metadata()?;
    // A root-owned sticky directory such as /tmp protects each root/euid-owned
    // child from rename by other users. Every following component is checked,
    // so an attacker-owned child cannot smuggle in a second lock namespace.
    let root_sticky = metadata.uid() == 0 && metadata.mode() & 0o1000 != 0;
    if !metadata.is_dir()
        || (metadata.uid() != 0 && metadata.uid() != unsafe { libc::geteuid() })
        || (metadata.mode() & 0o022 != 0 && !root_sticky)
    {
        return Err(LedgerError::Invalid("untrusted directory ancestry"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        Capability, EvidenceKind, EvidenceSource, EvidenceStatus, ExpectedRevisions, Grant,
        HostPolicy, Instance, Isolation, MIN_STORAGE_BYTES, ReadinessEvidence, ReadinessGeneration,
        ReadinessSnapshot, ReasonCode, Resources,
    };
    use std::fs::{self, DirBuilder, OpenOptions};
    use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt, symlink};
    use std::path::PathBuf;
    use std::process::Command;
    use std::time::{SystemTime, UNIX_EPOCH};

    struct Fixture(PathBuf);

    impl Fixture {
        fn new() -> Self {
            let unique = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let root = std::env::temp_dir().canonicalize().unwrap().join(format!(
                "tentaflake-admission-{}-{unique}",
                std::process::id()
            ));
            DirBuilder::new().mode(0o700).create(&root).unwrap();
            Self(root)
        }

        fn path(&self) -> PathBuf {
            self.0.join("ledger")
        }

        fn ledger(&self) -> Ledger {
            Ledger::initialize(&self.path(), "authority-1").unwrap()
        }

        fn child(&self, mode: &str) {
            let output = Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "ledger::tests::subprocess_entry",
                    "--ignored",
                    "--nocapture",
                ])
                .env("TENTAFLAKE_LEDGER_TEST_PATH", self.path())
                .env("TENTAFLAKE_LEDGER_TEST_MODE", mode)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "subprocess {mode}: {}{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).unwrap();
        }
    }

    fn resources(count: u64) -> Resources {
        Resources {
            memory_bytes: count * 1024 * 1024 * 1024,
            memory_swap_bytes: count * 1024 * 1024 * 1024,
            nano_cpus: count * 1_000_000_000,
            pids: count * 128,
            workspace_bytes: count * MIN_STORAGE_BYTES,
            state_bytes: count * MIN_STORAGE_BYTES,
        }
    }

    fn context(instance: &str, revision: u64) -> (AdmissionSnapshot, AdmissionRequest) {
        let sources = [
            EvidenceKind::Runtime,
            EvidenceKind::WorkspaceQuota,
            EvidenceKind::StateQuota,
        ];
        let snapshot = AdmissionSnapshot {
            schema_version: SCHEMA_VERSION,
            policy: HostPolicy {
                revision: "policy-1".into(),
                generation: "authority-1".into(),
                capacity: resources(2),
                host_reserve: Resources::default(),
                research_reserve: Resources::default(),
                per_instance: resources(1),
                max_active: 2,
                evidence_max_age_seconds: 30,
                grants: Vec::new(),
            },
            instance: Instance {
                id: instance.into(),
                owner_id: "owner-1".into(),
                revision: "instance-1".into(),
                isolation: Isolation::Container,
                resources: resources(1),
                requested_capabilities: Vec::new(),
            },
            readiness: ReadinessSnapshot {
                generation: "authority-1".into(),
                sources: sources
                    .iter()
                    .map(|kind| ReadinessGeneration {
                        kind: *kind,
                        generation: "source-1".into(),
                    })
                    .collect(),
                evidence: sources
                    .iter()
                    .map(|kind| ReadinessEvidence {
                        instance_id: instance.into(),
                        kind: *kind,
                        status: EvidenceStatus::Ready,
                        source: EvidenceSource::Live,
                        policy_revision: "policy-1".into(),
                        instance_revision: "instance-1".into(),
                        generation: "source-1".into(),
                        observed_at: 100,
                        expires_at: 130,
                    })
                    .collect(),
            },
            // Intentionally untrusted/incomplete input: claim must replace it.
            reservations: ReservationSnapshot {
                revision: 0,
                generation: "ignored".into(),
                complete: false,
                entries: Vec::new(),
            },
        };
        let request = AdmissionRequest {
            schema_version: SCHEMA_VERSION,
            operation_id: format!("start-{instance}"),
            instance_id: instance.into(),
            principal_id: "owner-1".into(),
            expected: ExpectedRevisions {
                policy: "policy-1".into(),
                instance: "instance-1".into(),
                ledger: revision,
            },
        };
        (snapshot, request)
    }

    fn claim<'a>(ledger: &'a Ledger, instance: &str) -> DispatchPermit<'a> {
        let revision = ledger.snapshot().unwrap().revision;
        let (snapshot, request) = context(instance, revision);
        match ledger.claim(&request, || Ok((snapshot, 110))).unwrap() {
            ClaimOutcome::New(permit) => permit,
            _ => panic!("expected a new durable claim"),
        }
    }

    #[test]
    #[ignore = "subprocess fixture; invoked by filesystem tests"]
    fn subprocess_entry() {
        let path = PathBuf::from(std::env::var_os("TENTAFLAKE_LEDGER_TEST_PATH").unwrap());
        match std::env::var("TENTAFLAKE_LEDGER_TEST_MODE")
            .unwrap()
            .as_str()
        {
            "busy" => assert!(matches!(Ledger::open(&path), Err(LedgerError::Busy))),
            "crash" => {
                let ledger = Ledger::open(&path).unwrap();
                let _permit = claim(&ledger, "a");
                // No Rust destructor runs. The OS releases flock; the durable
                // uncertain reservation must survive and prohibit redispatch.
                std::process::exit(0);
            }
            "write-failure" => {
                let ledger = Ledger::open(&path).unwrap();
                let (snapshot, request) = context("a", 1);
                let limit = libc::rlimit {
                    rlim_cur: 64,
                    rlim_max: 64,
                };
                unsafe {
                    assert_ne!(libc::signal(libc::SIGXFSZ, libc::SIG_IGN), libc::SIG_ERR);
                    assert_eq!(libc::setrlimit(libc::RLIMIT_FSIZE, &limit), 0);
                }
                assert!(matches!(
                    ledger.claim(&request, || Ok((snapshot, 110))),
                    Err(LedgerError::Io(_))
                ));
            }
            _ => panic!("unknown subprocess fixture"),
        }
    }

    #[test]
    fn exclusive_process_lock_covers_evidence_dispatch_and_commit() {
        let fixture = Fixture::new();
        let ledger = fixture.ledger();
        let (snapshot, request) = context("a", 1);
        let permit = match ledger
            .claim(&request, || {
                assert!(matches!(ledger.snapshot(), Err(LedgerError::Busy)));
                Ok((snapshot, 110))
            })
            .unwrap()
        {
            ClaimOutcome::New(permit) => permit,
            _ => panic!("expected permit"),
        };
        assert_eq!(
            permit.receipt().reservation.state,
            ReservationState::Unknown
        );
        fixture.child("busy");
        let receipt = permit.complete(Observation::Running).unwrap();
        assert_eq!(receipt.claimed_revision, 2);
        assert_eq!(receipt.updated_revision, 3);
        let reopened = Ledger::open(&fixture.path()).unwrap();
        assert_eq!(
            reopened.snapshot().unwrap().entries[0].state,
            ReservationState::Active
        );
    }

    #[test]
    fn releasing_lock_does_not_wait_for_an_inherited_descriptor_to_close() {
        let fixture = Fixture::new();
        let ledger = fixture.ledger();
        let lock = ledger.lock(false).unwrap();
        // A forked child briefly shares this open file description before exec
        // closes O_CLOEXEC descriptors. A duplicate reproduces that ownership.
        let inherited = lock.file.try_clone().unwrap();
        assert!(matches!(ledger.snapshot(), Err(LedgerError::Busy)));
        drop(lock);
        assert_eq!(ledger.snapshot().unwrap().revision, 1);
        drop(inherited);
    }

    #[test]
    fn process_exit_retains_claim_and_retries_never_grant_dispatch() {
        let fixture = Fixture::new();
        let ledger = fixture.ledger();
        fixture.child("crash");
        let durable = ledger.snapshot().unwrap();
        assert_eq!(durable.revision, 2);
        assert_eq!(durable.entries[0].state, ReservationState::Unknown);
        assert_eq!(durable.entries[0].resources, resources(1));

        let (mut snapshot, request) = context("a", 1);
        snapshot.readiness.evidence.clear();
        snapshot.policy.revision = "policy-new".into();
        snapshot.instance.revision = "instance-new".into();
        let mut refreshed = false;
        let result = ledger
            .claim(&request, || {
                refreshed = true;
                Ok((snapshot.clone(), 1000))
            })
            .unwrap();
        assert!(refreshed);
        assert!(matches!(result, ClaimOutcome::Existing(_)));
        snapshot.instance.owner_id = "new-owner".into();
        assert!(matches!(
            ledger.claim(&request, || Ok((snapshot, 110))).unwrap(),
            ClaimOutcome::Denied(Decision { allowed: false, .. })
        ));
        let (snapshot, mut changed) = context("a", 2);
        changed.operation_id = request.operation_id;
        assert!(matches!(
            ledger.claim(&changed, || Ok((snapshot, 110))),
            Err(LedgerError::Conflict)
        ));
        let (snapshot, other) = context("b", 2);
        let ClaimOutcome::Denied(decision) = ledger.claim(&other, || Ok((snapshot, 110))).unwrap()
        else {
            panic!("uncertain claim must block new admission");
        };
        assert!(
            decision
                .reasons
                .iter()
                .any(|reason| reason.code == ReasonCode::UnknownReservation)
        );
    }

    #[test]
    fn receipt_replay_rechecks_current_capability_grants() {
        let fixture = Fixture::new();
        let ledger = fixture.ledger();
        let (mut snapshot, request) = context("a", 1);
        snapshot
            .instance
            .requested_capabilities
            .push(Capability::Model);
        snapshot.policy.grants.push(Grant {
            instance_id: "a".into(),
            owner_id: "owner-1".into(),
            capability: Capability::Model,
            expires_at: None,
        });
        snapshot.readiness.sources.push(ReadinessGeneration {
            kind: EvidenceKind::Model,
            generation: "model-1".into(),
        });
        snapshot.readiness.evidence.push(ReadinessEvidence {
            kind: EvidenceKind::Model,
            generation: "model-1".into(),
            ..snapshot.readiness.evidence[0].clone()
        });
        drop(
            ledger
                .claim(&request, || Ok((snapshot.clone(), 110)))
                .unwrap(),
        );
        snapshot.policy.grants.clear();
        let ClaimOutcome::Denied(decision) =
            ledger.claim(&request, || Ok((snapshot, 110))).unwrap()
        else {
            panic!("revoked grant must deny receipt visibility");
        };
        assert!(
            decision
                .reasons
                .iter()
                .any(|reason| reason.code == ReasonCode::MissingGrant)
        );
        assert_eq!(
            ledger.snapshot().unwrap().entries[0].resources,
            resources(1)
        );
    }

    #[test]
    fn exact_reconciliation_retains_storage_and_cannot_release_a_newer_operation() {
        let fixture = Fixture::new();
        let ledger = fixture.ledger();
        let active = claim(&ledger, "a").complete(Observation::Running).unwrap();
        assert!(matches!(
            ledger.reconcile("start-a", active.claimed_revision, |_| Ok(
                Observation::Stopped
            )),
            Err(LedgerError::StaleRevision)
        ));
        let stopped = ledger
            .reconcile("start-a", active.updated_revision, |_| {
                Ok(Observation::Stopped)
            })
            .unwrap();
        assert_eq!(stopped.reservation.resources, resources(1).retained());
        assert_eq!(ledger.snapshot().unwrap().entries[0], stopped.reservation);
        let (snapshot, mut request) = context("a", stopped.updated_revision);
        request.operation_id = "restart-a".into();
        let ClaimOutcome::New(permit) = ledger.claim(&request, || Ok((snapshot, 110))).unwrap()
        else {
            panic!("stopped instance should reuse its retained storage reservation");
        };
        let next = permit.receipt().clone();
        drop(permit);
        let mut observed = false;
        assert!(matches!(
            ledger.reconcile("start-a", next.updated_revision, |_| {
                observed = true;
                Ok(Observation::Stopped)
            }),
            Err(LedgerError::Conflict)
        ));
        assert!(!observed);
        let unknown = ledger
            .reconcile("restart-a", next.updated_revision, |_| {
                Ok(Observation::Unknown)
            })
            .unwrap();
        assert_eq!(unknown, next);
        assert_eq!(
            ledger.snapshot().unwrap().entries[0].resources,
            resources(1)
        );
    }

    #[test]
    fn stopped_storage_stays_in_aggregate_and_new_claims_use_disk_reservations() {
        let fixture = Fixture::new();
        let ledger = fixture.ledger();
        claim(&ledger, "a").complete(Observation::Stopped).unwrap();
        claim(&ledger, "b").complete(Observation::Running).unwrap();
        let revision = ledger.snapshot().unwrap().revision;
        let (snapshot, request) = context("c", revision);
        let ClaimOutcome::Denied(decision) =
            ledger.claim(&request, || Ok((snapshot, 110))).unwrap()
        else {
            panic!("retained storage must count against host capacity");
        };
        assert!(decision.reasons.iter().any(|reason| {
            reason.code == ReasonCode::ResourceExhausted && reason.subject == "workspaceBytes"
        }));
        assert!(decision.reasons.iter().any(|reason| {
            reason.code == ReasonCode::ResourceExhausted && reason.subject == "stateBytes"
        }));
        assert_eq!(ledger.snapshot().unwrap().revision, revision);
    }

    #[test]
    fn real_write_failure_never_publishes_a_claim_or_discards_staging() {
        let fixture = Fixture::new();
        let ledger = fixture.ledger();
        let before = fs::read(fixture.path().join(STATE)).unwrap();
        fixture.child("write-failure");
        assert_eq!(fs::read(fixture.path().join(STATE)).unwrap(), before);
        assert!(fixture.path().join(STAGING).is_file());
        assert!(ledger.snapshot().unwrap().entries.is_empty());
        let (snapshot, request) = context("a", 1);
        assert!(matches!(
            ledger.claim(&request, || Ok((snapshot, 110))),
            Err(LedgerError::Io(_))
        ));
        assert!(fixture.path().join(STAGING).is_file());
    }

    #[test]
    fn missing_corrupt_or_oversized_state_never_reinitializes() {
        let fixture = Fixture::new();
        let ledger = fixture.ledger();
        let path = fixture.path().join(STATE);
        let original = fs::read(&path).unwrap();
        fs::write(&path, b"{\"schemaVersion\":1}").unwrap();
        assert!(matches!(ledger.snapshot(), Err(LedgerError::Invalid(_))));
        fs::write(&path, &original).unwrap();
        OpenOptions::new()
            .write(true)
            .open(&path)
            .unwrap()
            .set_len(MAX_LEDGER_BYTES as u64 + 1)
            .unwrap();
        assert!(matches!(ledger.snapshot(), Err(LedgerError::Full)));
        fs::remove_file(path).unwrap();
        assert!(Ledger::open(&fixture.path()).is_err());
        assert!(Ledger::initialize(&fixture.path(), "authority-1").is_err());
    }

    #[test]
    fn private_paths_reject_symlinks_hardlinks_and_public_modes() {
        let fixture = Fixture::new();
        let ledger = fixture.ledger();
        let path = fixture.path().join(STATE);
        let original = fixture.0.join("original");
        fs::rename(&path, &original).unwrap();
        symlink(&original, &path).unwrap();
        assert!(ledger.snapshot().is_err());
        fs::remove_file(&path).unwrap();
        fs::hard_link(&original, &path).unwrap();
        assert!(matches!(ledger.snapshot(), Err(LedgerError::Invalid(_))));
        fs::remove_file(&path).unwrap();
        fs::rename(&original, &path).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(matches!(ledger.snapshot(), Err(LedgerError::Invalid(_))));
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        let alias = fixture.0.join("alias");
        symlink(fixture.path(), &alias).unwrap();
        assert!(Ledger::open(&alias).is_err());
        fs::set_permissions(fixture.path(), fs::Permissions::from_mode(0o755)).unwrap();
        assert!(matches!(
            Ledger::open(&fixture.path()),
            Err(LedgerError::Invalid(_))
        ));
    }

    #[test]
    fn writable_or_foreign_ancestors_cannot_create_a_second_lock_namespace() {
        let fixture = Fixture::new();
        let _ledger = fixture.ledger();
        fs::set_permissions(&fixture.0, fs::Permissions::from_mode(0o777)).unwrap();
        assert!(matches!(
            Ledger::open(&fixture.path()),
            Err(LedgerError::Invalid(_))
        ));
        assert!(matches!(
            Ledger::initialize(&fixture.0.join("second"), "authority-1"),
            Err(LedgerError::Invalid(_))
        ));
        assert!(!fixture.0.join("second").exists());
        fs::set_permissions(&fixture.0, fs::Permissions::from_mode(0o700)).unwrap();
        if unsafe { libc::geteuid() } == 0 {
            let directory = File::open(&fixture.0).unwrap();
            assert_eq!(unsafe { libc::fchown(directory.as_raw_fd(), 65534, !0) }, 0);
            assert!(matches!(
                Ledger::open(&fixture.path()),
                Err(LedgerError::Invalid(_))
            ));
            assert_eq!(unsafe { libc::fchown(directory.as_raw_fd(), 0, !0) }, 0);
        }
        assert!(Ledger::open(&fixture.path()).is_ok());
    }

    #[test]
    fn durable_record_limits_and_operation_identity_are_validated() {
        let fixture = Fixture::new();
        let ledger = fixture.ledger();
        drop(claim(&ledger, "a"));
        let path = fixture.path().join(STATE);
        let original = fs::read(&path).unwrap();
        let mut state: State = serde_json::from_slice(&original).unwrap();
        state.receipts[0].request.operation_id = "different-operation".into();
        state.receipts.push(state.receipts[0].clone());
        let mut file = OpenOptions::new()
            .write(true)
            .truncate(true)
            .mode(0o600)
            .open(&path)
            .unwrap();
        file.write_all(&serde_json::to_vec(&state).unwrap())
            .unwrap();
        file.sync_all().unwrap();
        assert!(matches!(ledger.snapshot(), Err(LedgerError::Invalid(_))));
        fs::write(&path, original).unwrap();
        assert_eq!(ledger.snapshot().unwrap().entries.len(), 1);
    }
}
