//! Private, durable startup and storage claims. Only a new claim grants dispatch.
//!
//! The caller owns authentication, current host evidence, bounded dispatch, and
//! proof of container ownership/absence. An unknown result never frees capacity.
//! Keep the permit until the bounded start handoff and outcome commit finish.
use crate::decision::identifier;
use crate::{
    AdmissionRequest, AdmissionSnapshot, Decision, HostPolicy, MAX_RESERVATIONS,
    MAX_STORAGE_ALLOCATIONS, Reservation, ReservationSnapshot, ReservationState, SCHEMA_VERSION,
    StorageAdmissionSnapshot, StorageAllocation, StorageDecision, StorageRequest, StorageState,
    authorize, authorize_storage, decide, decide_storage, validate_reservations,
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
const LEDGER_SCHEMA_VERSION: u32 = 3;
const MAX_PUBLICATIONS: usize = MAX_RECEIPTS;
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
    StaleAuthority,
    TransitionPending,
    NotQuiescent,
    StorageDenied(Box<StorageDecision>),
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
            Self::StaleAuthority => formatter.write_str("published admission authority changed"),
            Self::TransitionPending => formatter.write_str("admission publication is pending"),
            Self::NotQuiescent => formatter.write_str("admission work is not proven quiescent"),
            Self::StorageDenied(_) => formatter.write_str("present storage mutation was denied"),
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

/// A complete, host-collected inventory of all managed backing slots, including
/// removed definitions and orphan files. Completeness means enumeration, never
/// filesystem readiness. Unknown rows remain charged. The host must fence all
/// quota creators while collecting and bootstrapping this inventory.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct StorageInventory {
    pub complete: bool,
    pub allocations: Vec<StorageAllocation>,
}

/// Exactly one immutable provision intent per allocation. Imported inventory
/// has no create request and can never yield permission to recreate its image.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct StorageReceipt {
    pub request: Option<StorageRequest>,
    pub allocation: StorageAllocation,
    pub claimed_revision: u64,
    pub updated_revision: u64,
}

/// Reference to one immutable, non-secret host manifest. The trusted collector
/// verifies the manifest bytes against `revision`, which is also HostPolicy's
/// revision. The ledger validates the reference's shape, not its filesystem.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct PublishedAuthority {
    pub generation: String,
    pub revision: String,
    pub manifest_path: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct PublicationRequest {
    pub operation_id: String,
    pub expected_ledger_revision: u64,
    pub expected_authority: PublishedAuthority,
    pub candidate: PublishedAuthority,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum PublicationOutcome {
    Pending,
    Published,
    Cancelled,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct PublicationReceipt {
    pub request: PublicationRequest,
    pub outcome: PublicationOutcome,
    pub begun_revision: u64,
    pub updated_revision: u64,
}

pub enum PublicationBegin {
    New(PublicationReceipt),
    Existing(PublicationReceipt),
}

/// Trusted live collector output, never a declaration or client assertion.
/// `mutators_fenced` covers old AND candidate start/reload/prepare/mount/owner
/// jobs, helpers and other managed creators, including queued native jobs that
/// may outlive a completed prepare unit. Helpers may not escape their callbacks.
pub struct PublicationObservation {
    pub installed: PublishedAuthority,
    pub mutators_fenced: bool,
}

/// Recovery diagnostics only. This snapshot grants no mutation authority.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PublicationStatus {
    pub ledger_revision: u64,
    pub published: PublishedAuthority,
    pub pending: Option<PublicationReceipt>,
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

pub enum StorageClaimOutcome<'a> {
    New(StoragePermit<'a>),
    Existing(Box<StorageReceipt>),
    Denied(StorageDecision),
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

/// A charge is not proof that the backing slot is empty. Before dispatching the
/// fixed prepare helper, the trusted host must verify final and staging absence
/// under this permit. Existing data must never be overwritten or reformatted.
#[must_use = "a new storage permit owns one bounded provision; dropping retains its charge"]
pub struct StoragePermit<'a> {
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
    published: PublishedAuthority,
    receipts: Vec<Receipt>,
    storage: Vec<StorageReceipt>,
    publications: Vec<PublicationReceipt>,
}

impl Ledger {
    /// Explicit bootstrap only. Creates a new private directory and refuses an
    /// existing path, including an empty directory left by an interrupted init.
    /// The host must also fence/prove absence of unrepresented running compute.
    /// Authority generation is a durable epoch, not a boot ID; no implicit reset.
    pub fn initialize(
        path: &Path,
        authority: PublishedAuthority,
        inventory: StorageInventory,
    ) -> Result<Self, LedgerError> {
        let name = path
            .file_name()
            .ok_or(LedgerError::Invalid("directory path"))?;
        let parent = open_directory(
            path.parent()
                .ok_or(LedgerError::Invalid("directory parent"))?,
        )?;
        Self::initialize_in(parent, name, authority, inventory)
    }

    fn initialize_in(
        parent: File,
        name: &OsStr,
        authority: PublishedAuthority,
        inventory: StorageInventory,
    ) -> Result<Self, LedgerError> {
        if !inventory.complete {
            return Err(LedgerError::Invalid("incomplete storage inventory"));
        }
        let state = State {
            schema_version: LEDGER_SCHEMA_VERSION,
            revision: 1,
            published: authority,
            receipts: Vec::new(),
            publications: Vec::new(),
            storage: inventory
                .allocations
                .into_iter()
                .map(|allocation| StorageReceipt {
                    request: None,
                    allocation,
                    claimed_revision: 1,
                    updated_revision: 1,
                })
                .collect(),
        };
        // Validate before creating any ledger path. Existing over-budget rows
        // remain truthful inventory; budget enforcement belongs to admission.
        state.validate()?;
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
        ledger.write(&state)?;
        Ok(ledger)
    }

    /// Missing, malformed or incomplete state is an error, never an empty ledger.
    pub fn open(path: &Path) -> Result<Self, LedgerError> {
        Self::open_in(open_directory(path)?)
    }

    fn open_in(directory: File) -> Result<Self, LedgerError> {
        let ledger = Self { directory };
        ledger.validate_directory()?;
        let _lock = ledger.lock(false)?;
        ledger.read()?;
        Ok(ledger)
    }

    pub fn snapshot(&self) -> Result<ReservationSnapshot, LedgerError> {
        let _lock = self.lock(false)?;
        Ok(self.read()?.snapshot())
    }

    pub fn publication_status(&self) -> Result<PublicationStatus, LedgerError> {
        let _lock = self.lock(false)?;
        let state = self.read()?;
        Ok(PublicationStatus {
            ledger_revision: state.revision,
            published: state.published.clone(),
            pending: state.pending().cloned(),
        })
    }

    /// Begin an authenticated host publication, not an agent-owned operation.
    /// Persist Pending before stopping/draining/installing outside the lock.
    /// Reopening or retrying never cancels a transition or repeats that work.
    pub fn begin_publication(
        &self,
        request: &PublicationRequest,
    ) -> Result<PublicationBegin, LedgerError> {
        let _lock = self.lock(false)?;
        let mut state = self.read()?;
        request.validate()?;
        if let Some(receipt) = state
            .publications
            .iter()
            .find(|receipt| receipt.request.operation_id == request.operation_id)
        {
            if receipt.request != *request {
                return Err(LedgerError::Conflict);
            }
            return Ok(PublicationBegin::Existing(receipt.clone()));
        }
        if state
            .receipts
            .iter()
            .any(|receipt| receipt.request.operation_id == request.operation_id)
            || state.storage.iter().any(|receipt| {
                receipt
                    .request
                    .as_ref()
                    .is_some_and(|storage| storage.request.operation_id == request.operation_id)
            })
        {
            return Err(LedgerError::Conflict);
        }
        state.require_stable()?;
        if state.published != request.expected_authority {
            return Err(LedgerError::StaleAuthority);
        }
        if state.revision != request.expected_ledger_revision {
            return Err(LedgerError::StaleRevision);
        }
        if state.publications.len() >= MAX_PUBLICATIONS {
            return Err(LedgerError::Full);
        }
        state.advance()?;
        let receipt = PublicationReceipt {
            request: request.clone(),
            outcome: PublicationOutcome::Pending,
            begun_revision: state.revision,
            updated_revision: state.revision,
        };
        state.publications.push(receipt.clone());
        self.write(&state)?;
        Ok(PublicationBegin::New(receipt))
    }

    /// Commit the candidate only after exact live installation and drain proof.
    /// This method never installs a manifest, drains jobs or releases resources.
    pub fn finish_publication(
        &self,
        operation_id: &str,
        expected_ledger_revision: u64,
        observe: impl FnOnce(
            &PublicationReceipt,
            &ReservationSnapshot,
        ) -> Result<PublicationObservation, String>,
    ) -> Result<PublicationReceipt, LedgerError> {
        self.finalize_publication(
            operation_id,
            expected_ledger_revision,
            PublicationOutcome::Published,
            observe,
        )
    }

    /// Explicit recovery only: the host must first restore the original plan
    /// and fence both plans' mutators. No rollback is inferred from time or exit.
    pub fn cancel_publication(
        &self,
        operation_id: &str,
        expected_ledger_revision: u64,
        observe: impl FnOnce(
            &PublicationReceipt,
            &ReservationSnapshot,
        ) -> Result<PublicationObservation, String>,
    ) -> Result<PublicationReceipt, LedgerError> {
        self.finalize_publication(
            operation_id,
            expected_ledger_revision,
            PublicationOutcome::Cancelled,
            observe,
        )
    }

    fn finalize_publication(
        &self,
        operation_id: &str,
        expected_ledger_revision: u64,
        outcome: PublicationOutcome,
        observe: impl FnOnce(
            &PublicationReceipt,
            &ReservationSnapshot,
        ) -> Result<PublicationObservation, String>,
    ) -> Result<PublicationReceipt, LedgerError> {
        let _lock = self.lock(false)?;
        let mut state = self.read()?;
        let index = state
            .publications
            .iter()
            .position(|receipt| receipt.request.operation_id == operation_id)
            .ok_or(LedgerError::Conflict)?;
        let receipt = &state.publications[index];
        if receipt.outcome != PublicationOutcome::Pending {
            if receipt.outcome != outcome {
                return Err(LedgerError::Conflict);
            }
            if expected_ledger_revision.checked_add(1) != Some(receipt.updated_revision) {
                return Err(LedgerError::StaleRevision);
            }
            // Historical visibility only; no observer or mutation is replayed.
            return Ok(receipt.clone());
        }
        if state.revision != expected_ledger_revision {
            return Err(LedgerError::StaleRevision);
        }
        let snapshot = state.snapshot();
        if snapshot
            .entries
            .iter()
            .any(|entry| entry.state != ReservationState::Retained)
            || snapshot
                .storage
                .iter()
                .any(|allocation| allocation.state != StorageState::Present)
        {
            return Err(LedgerError::NotQuiescent);
        }
        let desired = if outcome == PublicationOutcome::Published {
            &receipt.request.candidate
        } else {
            &receipt.request.expected_authority
        };
        let observed = observe(receipt, &snapshot).map_err(|_| LedgerError::Context)?;
        if !observed.mutators_fenced {
            return Err(LedgerError::NotQuiescent);
        }
        if observed.installed != *desired {
            return Err(LedgerError::StaleAuthority);
        }
        state.published = desired.clone();
        state.advance()?;
        state.publications[index].outcome = outcome;
        state.publications[index].updated_revision = state.revision;
        self.write(&state)?;
        Ok(state.publications[index].clone())
    }

    /// Collect trusted policy, identity/grants and readiness while holding the
    /// exclusive lock. The supplied reservation list is always replaced by disk
    /// state. Even an identical retry repeats current authorization before its
    /// receipt is disclosed; it can never acquire another dispatch permit.
    pub fn claim(
        &self,
        request: &AdmissionRequest,
        fresh_context: impl FnOnce(&PublishedAuthority) -> Result<(AdmissionSnapshot, u64), String>,
    ) -> Result<ClaimOutcome<'_>, LedgerError> {
        let lock = self.lock(false)?;
        let mut state = self.read()?;
        let (mut snapshot, now) =
            fresh_context(&state.published).map_err(|_| LedgerError::Context)?;
        state.check_policy(&snapshot.policy)?;
        snapshot.reservations = state.snapshot();
        let reasons = authorize(&snapshot, request, now);
        if !reasons.is_empty() {
            return Ok(ClaimOutcome::Denied(Decision {
                allowed: false,
                reasons,
                reservation: None,
            }));
        }
        if state
            .publications
            .iter()
            .any(|receipt| receipt.request.operation_id == request.operation_id)
            || state.storage.iter().any(|receipt| {
                receipt
                    .request
                    .as_ref()
                    .is_some_and(|storage| storage.request.operation_id == request.operation_id)
            })
        {
            return Err(LedgerError::Conflict);
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
        state.require_stable()?;
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

    /// Reserve one declared backing slot before any allocation or formatting.
    /// Owner, desired binding and revisions are freshly checked under the same
    /// lock used by startup. Model/Research readiness does not gate storage for
    /// deliberately stopped instances. All managed quota creators must use this
    /// path once the host enables this ledger's aggregate storage accounting.
    pub fn reserve_storage(
        &self,
        request: &StorageRequest,
        fresh_context: impl FnOnce(&PublishedAuthority) -> Result<StorageAdmissionSnapshot, String>,
    ) -> Result<StorageClaimOutcome<'_>, LedgerError> {
        let lock = self.lock(false)?;
        let mut state = self.read()?;
        let mut snapshot = fresh_context(&state.published).map_err(|_| LedgerError::Context)?;
        state.check_policy(&snapshot.policy)?;
        snapshot.reservations = state.snapshot();
        let reasons = authorize_storage(&snapshot, request);
        if !reasons.is_empty() {
            return Ok(StorageClaimOutcome::Denied(StorageDecision {
                allowed: false,
                reasons,
                allocation: None,
            }));
        }
        if state
            .publications
            .iter()
            .any(|receipt| receipt.request.operation_id == request.request.operation_id)
            || state
                .receipts
                .iter()
                .any(|receipt| receipt.request.operation_id == request.request.operation_id)
        {
            return Err(LedgerError::Conflict);
        }
        if let Some(receipt) = state.storage.iter().find(|receipt| {
            receipt.request.as_ref().is_some_and(|existing| {
                existing.request.operation_id == request.request.operation_id
            })
        }) {
            if receipt.request.as_ref() != Some(request) {
                return Err(LedgerError::Conflict);
            }
            return Ok(StorageClaimOutcome::Existing(Box::new(receipt.clone())));
        }
        state.require_stable()?;
        let decision = decide_storage(&snapshot, request);
        if !decision.allowed {
            return Ok(StorageClaimOutcome::Denied(decision));
        }
        if let Some(receipt) = state
            .storage
            .iter()
            .find(|receipt| receipt.allocation.id == request.allocation_id)
        {
            return Ok(StorageClaimOutcome::Existing(Box::new(receipt.clone())));
        }
        if state.storage.len() >= MAX_STORAGE_ALLOCATIONS {
            return Err(LedgerError::Full);
        }
        let mut allocation = decision.allocation.ok_or(LedgerError::Invalid(
            "allowed storage decision without allocation",
        ))?;
        allocation.state = StorageState::Unknown;
        state.advance()?;
        state.storage.push(StorageReceipt {
            request: Some(request.clone()),
            allocation,
            claimed_revision: state.revision,
            updated_revision: state.revision,
        });
        self.write(&state)?;
        let index = state.storage.len() - 1;
        Ok(StorageClaimOutcome::New(StoragePermit {
            ledger: self,
            _lock: lock,
            state,
            index,
        }))
    }

    /// Run one bounded ownership/verification mutation under the current plan.
    /// This never grants creation authority, even when an image is missing.
    /// The host must keep helper processes inside the callback lifetime and
    /// verify exact files/mounts before chown, chmod or directory initialization.
    pub fn with_present_storage<T>(
        &self,
        authority: &PublishedAuthority,
        request: &StorageRequest,
        fresh_context: impl FnOnce(&PublishedAuthority) -> Result<StorageAdmissionSnapshot, String>,
        mutate: impl FnOnce(&StorageAllocation) -> Result<T, String>,
    ) -> Result<T, LedgerError> {
        let _lock = self.lock(false)?;
        let state = self.read()?;
        state.require_stable()?;
        if state.published != *authority {
            return Err(LedgerError::StaleAuthority);
        }
        let mut snapshot = fresh_context(&state.published).map_err(|_| LedgerError::Context)?;
        state.check_policy(&snapshot.policy)?;
        snapshot.reservations = state.snapshot();
        let decision = decide_storage(&snapshot, request);
        if !decision.allowed {
            return Err(LedgerError::StorageDenied(Box::new(decision)));
        }
        let allocation = state
            .storage
            .iter()
            .find(|receipt| receipt.allocation.id == request.allocation_id)
            .map(|receipt| &receipt.allocation)
            .filter(|allocation| allocation.state == StorageState::Present)
            .ok_or(LedgerError::Conflict)?;
        mutate(allocation).map_err(|_| LedgerError::Context)
    }

    /// Reconcile an exact charged slot after a fenced provisioner has stopped.
    /// Present requires trusted inspection of the exact final image and binding.
    /// Missing/partial images stay Unknown: neither absence nor timeout grants
    /// permission to recreate, release, resize or rebind the allocation.
    pub fn reconcile_storage(
        &self,
        allocation_id: &str,
        expected_ledger_revision: u64,
        observe: impl FnOnce(&StorageReceipt) -> Result<StorageState, String>,
    ) -> Result<StorageReceipt, LedgerError> {
        let _lock = self.lock(false)?;
        let mut state = self.read()?;
        if state.revision != expected_ledger_revision {
            return Err(LedgerError::StaleRevision);
        }
        let index = state
            .storage
            .iter()
            .position(|receipt| receipt.allocation.id == allocation_id)
            .ok_or(LedgerError::Conflict)?;
        let observed = observe(&state.storage[index]).map_err(|_| LedgerError::Context)?;
        self.record_storage(&mut state, index, observed)
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

    fn record_storage(
        &self,
        state: &mut State,
        index: usize,
        observed: StorageState,
    ) -> Result<StorageReceipt, LedgerError> {
        if state.storage[index].allocation.state != observed {
            state.advance()?;
            state.storage[index].allocation.state = observed;
            state.storage[index].updated_revision = state.revision;
            self.write(state)?;
        }
        Ok(state.storage[index].clone())
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

impl StoragePermit<'_> {
    pub fn receipt(&self) -> &StorageReceipt {
        &self.state.storage[self.index]
    }

    /// Commit trusted inspection before releasing the provision lock. Present
    /// does not prove mount readiness; startup still requires fresh live evidence.
    pub fn complete(mut self, observed: StorageState) -> Result<StorageReceipt, LedgerError> {
        self.ledger
            .record_storage(&mut self.state, self.index, observed)
    }
}

impl PublishedAuthority {
    fn validate(&self) -> Result<(), LedgerError> {
        if !identifier(&self.generation)
            || !identifier(&self.revision)
            || self.manifest_path.len() > 4096
            || !self.manifest_path.starts_with('/')
            || !self
                .manifest_path
                .bytes()
                .all(|byte| byte.is_ascii() && !byte.is_ascii_control())
            || self
                .manifest_path
                .split('/')
                .skip(1)
                .any(|part| part.is_empty() || part == "." || part == "..")
        {
            return Err(LedgerError::Invalid("published authority"));
        }
        Ok(())
    }
}

impl PublicationRequest {
    fn validate(&self) -> Result<(), LedgerError> {
        self.expected_authority.validate()?;
        self.candidate.validate()?;
        if !identifier(&self.operation_id)
            || self.expected_ledger_revision == 0
            || self.candidate == self.expected_authority
            || self.candidate.generation != self.expected_authority.generation
        {
            return Err(LedgerError::Invalid(
                "publication request or authority epoch",
            ));
        }
        Ok(())
    }
}

impl State {
    fn advance(&mut self) -> Result<(), LedgerError> {
        self.revision = self.revision.checked_add(1).ok_or(LedgerError::Full)?;
        Ok(())
    }

    fn pending(&self) -> Option<&PublicationReceipt> {
        self.publications
            .last()
            .filter(|receipt| receipt.outcome == PublicationOutcome::Pending)
    }

    fn require_stable(&self) -> Result<(), LedgerError> {
        if self.pending().is_some() {
            return Err(LedgerError::TransitionPending);
        }
        Ok(())
    }

    fn check_policy(&self, policy: &HostPolicy) -> Result<(), LedgerError> {
        if policy.generation != self.published.generation
            || policy.revision != self.published.revision
        {
            return Err(LedgerError::StaleAuthority);
        }
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
            generation: self.published.generation.clone(),
            complete: true,
            entries: self
                .current()
                .values()
                .map(|index| self.receipts[*index].reservation.clone())
                .collect(),
            storage: self
                .storage
                .iter()
                .map(|receipt| receipt.allocation.clone())
                .collect(),
        }
    }

    fn validate(&self) -> Result<(), LedgerError> {
        if self.schema_version != LEDGER_SCHEMA_VERSION
            || self.revision == 0
            || self.receipts.len() > MAX_RECEIPTS
            || self.storage.len() > MAX_STORAGE_ALLOCATIONS
            || self.publications.len() > MAX_PUBLICATIONS
        {
            return Err(LedgerError::Invalid("header or receipt limit"));
        }
        self.published.validate()?;
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
            .chain(self.storage.iter().map(|receipt| receipt.updated_revision))
            .chain(
                self.publications
                    .iter()
                    .map(|receipt| receipt.updated_revision),
            )
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
            if latest.owner_id != receipt.reservation.owner_id {
                return Err(LedgerError::Invalid("historical reservation ownership"));
            }
            let original_snapshot = ReservationSnapshot {
                revision: self.revision,
                generation: self.published.generation.clone(),
                complete: true,
                entries: vec![original.clone()],
                storage: Vec::new(),
            };
            if !validate_reservations(&original_snapshot).is_empty() {
                return Err(LedgerError::Invalid("original reservation"));
            }
        }
        for receipt in &self.storage {
            if receipt.claimed_revision == 0
                || receipt.claimed_revision > receipt.updated_revision
                || receipt.updated_revision > self.revision
            {
                return Err(LedgerError::Invalid("storage receipt revision"));
            }
            match &receipt.request {
                None if receipt.claimed_revision == 1 => {}
                Some(request) => {
                    let admission = &request.request;
                    let binding = receipt
                        .allocation
                        .binding
                        .as_ref()
                        .ok_or(LedgerError::Invalid("provisioned storage without binding"))?;
                    if admission.schema_version != SCHEMA_VERSION
                        || !identifier(&admission.operation_id)
                        || !identifier(&admission.instance_id)
                        || !identifier(&admission.principal_id)
                        || !identifier(&admission.expected.policy)
                        || !identifier(&admission.expected.instance)
                        || admission.expected.ledger == 0
                        || admission.expected.ledger.checked_add(1)
                            != Some(receipt.claimed_revision)
                        || !operations.insert(&admission.operation_id)
                        || !claims.insert(receipt.claimed_revision)
                        || request.allocation_id != receipt.allocation.id
                        || binding.instance_id != admission.instance_id
                        || binding.owner_id != admission.principal_id
                    {
                        return Err(LedgerError::Invalid("storage receipt identity or revision"));
                    }
                }
                None => return Err(LedgerError::Invalid("inventory import revision")),
            }
        }
        let mut authority = self
            .publications
            .first()
            .map(|receipt| &receipt.request.expected_authority)
            .unwrap_or(&self.published);
        let mut previous_end = 1;
        for (index, receipt) in self.publications.iter().enumerate() {
            let request = &receipt.request;
            request.validate()?;
            if request.expected_authority != *authority
                || request.candidate.generation != self.published.generation
                || request.expected_ledger_revision.checked_add(1) != Some(receipt.begun_revision)
                || receipt.begun_revision <= previous_end
                || receipt.updated_revision > self.revision
                || !operations.insert(&request.operation_id)
                || !claims.insert(receipt.begun_revision)
            {
                return Err(LedgerError::Invalid(
                    "publication identity, order or revision",
                ));
            }
            if receipt.outcome == PublicationOutcome::Pending {
                if index + 1 != self.publications.len()
                    || receipt.updated_revision != receipt.begun_revision
                {
                    return Err(LedgerError::Invalid("pending publication"));
                }
            } else if receipt.updated_revision <= receipt.begun_revision {
                return Err(LedgerError::Invalid("terminal publication revision"));
            }
            if receipt.outcome == PublicationOutcome::Published {
                authority = &request.candidate;
            }
            previous_end = receipt.updated_revision;
        }
        if authority != &self.published {
            return Err(LedgerError::Invalid("published authority history"));
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
    let directory = open_at(
        libc::AT_FDCWD,
        OsStr::new("/"),
        libc::O_RDONLY | libc::O_DIRECTORY,
    )?;
    walk_directory(directory, path.strip_prefix("/").unwrap())
}

fn walk_directory(mut directory: File, relative: &Path) -> Result<File, LedgerError> {
    trusted_directory(&directory)?;
    for component in relative.components() {
        match component {
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
        ReadinessSnapshot, ReasonCode, Resources, StorageBinding, StorageKind, StorageRequirement,
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
            self.ledger_with(inventory(&["a", "b"]))
        }

        fn ledger_with(&self, inventory: StorageInventory) -> Ledger {
            self.initialize("ledger", inventory).unwrap()
        }

        fn initialize(
            &self,
            name: &str,
            inventory: StorageInventory,
        ) -> Result<Ledger, LedgerError> {
            Ledger::initialize_in(
                fixture_directory(&self.0, "")?,
                OsStr::new(name),
                authority("policy-1"),
                inventory,
            )
        }

        fn open(&self, name: &str) -> Result<Ledger, LedgerError> {
            Ledger::open_in(fixture_directory(&self.0, name)?)
        }

        fn child(&self, mode: &str) {
            let output = Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "ledger::tests::subprocess_entry",
                    "--ignored",
                    "--nocapture",
                ])
                .env("TENTAFLAKE_LEDGER_TEST_ROOT", &self.0)
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

    fn fixture_directory(root: &Path, relative: &str) -> Result<File, LedgerError> {
        // The Nix build namespace can map / to an unowned overflow UID. Anchor
        // filesystem tests at their explicit private directory, then exercise
        // exactly the production traversal and ownership checks beneath it.
        let directory = open_at(
            libc::AT_FDCWD,
            root.as_os_str(),
            libc::O_RDONLY | libc::O_DIRECTORY,
        )?;
        walk_directory(directory, Path::new(relative))
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).unwrap();
        }
    }

    fn authority(revision: &str) -> PublishedAuthority {
        PublishedAuthority {
            generation: "authority-1".into(),
            revision: revision.into(),
            manifest_path: format!("/nix/store/fixture-{revision}/admission.json"),
        }
    }

    fn publication(ledger: &Ledger, operation_id: &str, revision: &str) -> PublicationRequest {
        let status = ledger.publication_status().unwrap();
        PublicationRequest {
            operation_id: operation_id.into(),
            expected_ledger_revision: status.ledger_revision,
            expected_authority: status.published,
            candidate: authority(revision),
        }
    }

    fn installed(authority: &PublishedAuthority) -> Result<PublicationObservation, String> {
        Ok(PublicationObservation {
            installed: authority.clone(),
            mutators_fenced: true,
        })
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

    fn requirements(instance: &str) -> Vec<StorageRequirement> {
        [
            ("workspace", StorageKind::Workspace),
            ("state", StorageKind::State),
        ]
        .into_iter()
        .map(|(name, kind)| StorageRequirement {
            id: format!("{name}-{instance}"),
            kind,
            binding_revision: format!("binding-{name}-{instance}"),
        })
        .collect()
    }

    fn inventory(instances: &[&str]) -> StorageInventory {
        StorageInventory {
            complete: true,
            allocations: instances
                .iter()
                .flat_map(|instance| {
                    requirements(instance)
                        .into_iter()
                        .map(move |requirement| StorageAllocation {
                            id: requirement.id,
                            kind: requirement.kind,
                            bytes: MIN_STORAGE_BYTES,
                            binding: Some(StorageBinding {
                                instance_id: (*instance).into(),
                                owner_id: "owner-1".into(),
                                revision: requirement.binding_revision,
                            }),
                            state: StorageState::Present,
                        })
                })
                .collect(),
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
                storage: requirements(instance),
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
                storage: Vec::new(),
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
        match ledger.claim(&request, |_| Ok((snapshot, 110))).unwrap() {
            ClaimOutcome::New(permit) => permit,
            _ => panic!("expected a new durable claim"),
        }
    }

    fn storage_context(
        instance: &str,
        revision: u64,
    ) -> (StorageAdmissionSnapshot, StorageRequest) {
        let (snapshot, mut request) = context(instance, revision);
        request.operation_id = format!("provision-workspace-{instance}");
        (
            StorageAdmissionSnapshot {
                schema_version: snapshot.schema_version,
                policy: snapshot.policy,
                instance: snapshot.instance,
                reservations: snapshot.reservations,
            },
            StorageRequest {
                request,
                allocation_id: format!("workspace-{instance}"),
            },
        )
    }

    fn reserve<'a>(ledger: &'a Ledger, instance: &str) -> StoragePermit<'a> {
        let (snapshot, request) = storage_context(instance, ledger.snapshot().unwrap().revision);
        match ledger.reserve_storage(&request, |_| Ok(snapshot)).unwrap() {
            StorageClaimOutcome::New(permit) => permit,
            _ => panic!("expected a new storage claim"),
        }
    }

    #[test]
    #[ignore = "subprocess fixture; invoked by filesystem tests"]
    fn subprocess_entry() {
        let root = PathBuf::from(std::env::var_os("TENTAFLAKE_LEDGER_TEST_ROOT").unwrap());
        let open = || Ledger::open_in(fixture_directory(&root, "ledger")?);
        match std::env::var("TENTAFLAKE_LEDGER_TEST_MODE")
            .unwrap()
            .as_str()
        {
            "busy" => assert!(matches!(open(), Err(LedgerError::Busy))),
            "crash" => {
                let ledger = open().unwrap();
                let _permit = claim(&ledger, "a");
                // No Rust destructor runs. The OS releases flock; the durable
                // uncertain reservation must survive and prohibit redispatch.
                std::process::exit(0);
            }
            "storage-crash" => {
                let ledger = open().unwrap();
                let _permit = reserve(&ledger, "a");
                std::process::exit(0);
            }
            "publication-crash" => {
                let ledger = open().unwrap();
                let request = publication(&ledger, "publish-2", "policy-2");
                assert!(matches!(
                    ledger.begin_publication(&request).unwrap(),
                    PublicationBegin::New(_)
                ));
                std::process::exit(0);
            }
            "storage-exhausted" => {
                let ledger = open().unwrap();
                let (snapshot, request) = storage_context("b", ledger.snapshot().unwrap().revision);
                let StorageClaimOutcome::Denied(decision) =
                    ledger.reserve_storage(&request, |_| Ok(snapshot)).unwrap()
                else {
                    panic!("other process must see the committed storage charge");
                };
                assert!(
                    decision
                        .reasons
                        .iter()
                        .any(|reason| reason.code == ReasonCode::ResourceExhausted)
                );
            }
            mode @ ("write-failure" | "storage-write-failure" | "publication-write-failure") => {
                let ledger = open().unwrap();
                let limit = libc::rlimit {
                    rlim_cur: 64,
                    rlim_max: 64,
                };
                unsafe {
                    assert_ne!(libc::signal(libc::SIGXFSZ, libc::SIG_IGN), libc::SIG_ERR);
                    assert_eq!(libc::setrlimit(libc::RLIMIT_FSIZE, &limit), 0);
                }
                if mode == "write-failure" {
                    let (snapshot, request) = context("a", 1);
                    assert!(matches!(
                        ledger.claim(&request, |_| Ok((snapshot, 110))),
                        Err(LedgerError::Io(_))
                    ));
                } else if mode == "storage-write-failure" {
                    let (snapshot, request) = storage_context("a", 1);
                    assert!(matches!(
                        ledger.reserve_storage(&request, |_| Ok(snapshot)),
                        Err(LedgerError::Io(_))
                    ));
                } else {
                    let request = publication(&ledger, "publish-2", "policy-2");
                    assert!(matches!(
                        ledger.begin_publication(&request),
                        Err(LedgerError::Io(_))
                    ));
                }
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
            .claim(&request, |_| {
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
        let reopened = fixture.open("ledger").unwrap();
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
        snapshot.instance.revision = "instance-new".into();
        let mut refreshed = false;
        let result = ledger
            .claim(&request, |_| {
                refreshed = true;
                Ok((snapshot.clone(), 1000))
            })
            .unwrap();
        assert!(refreshed);
        assert!(matches!(result, ClaimOutcome::Existing(_)));
        snapshot.instance.owner_id = "new-owner".into();
        assert!(matches!(
            ledger.claim(&request, |_| Ok((snapshot, 110))).unwrap(),
            ClaimOutcome::Denied(Decision { allowed: false, .. })
        ));
        let (snapshot, mut changed) = context("a", 2);
        changed.operation_id = request.operation_id;
        assert!(matches!(
            ledger.claim(&changed, |_| Ok((snapshot, 110))),
            Err(LedgerError::Conflict)
        ));
        let (snapshot, other) = context("b", 2);
        let ClaimOutcome::Denied(decision) = ledger.claim(&other, |_| Ok((snapshot, 110))).unwrap()
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
                .claim(&request, |_| Ok((snapshot.clone(), 110)))
                .unwrap(),
        );
        snapshot.policy.grants.clear();
        let ClaimOutcome::Denied(decision) =
            ledger.claim(&request, |_| Ok((snapshot, 110))).unwrap()
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
        let ClaimOutcome::New(permit) = ledger.claim(&request, |_| Ok((snapshot, 110))).unwrap()
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
        let (snapshot, request) = storage_context("c", revision);
        let StorageClaimOutcome::Denied(decision) =
            ledger.reserve_storage(&request, |_| Ok(snapshot)).unwrap()
        else {
            panic!("retained storage must count against host capacity");
        };
        assert!(decision.reasons.iter().any(|reason| {
            reason.code == ReasonCode::ResourceExhausted && reason.subject == "workspaceBytes"
        }));
        assert_eq!(ledger.snapshot().unwrap().storage.len(), 4);
        assert_eq!(ledger.snapshot().unwrap().revision, revision);
    }

    #[test]
    fn restart_can_use_smaller_new_slots_while_old_allocations_remain_charged() {
        let fixture = Fixture::new();
        let mut initial = inventory(&["a"]);
        for allocation in &mut initial.allocations {
            allocation.bytes = 2 * MIN_STORAGE_BYTES;
        }
        let mut smaller = inventory(&["a"]).allocations;
        for allocation in &mut smaller {
            allocation.id.push_str("-new");
            allocation
                .binding
                .as_mut()
                .unwrap()
                .revision
                .push_str("-new");
        }
        initial.allocations.extend(smaller);
        let expected_storage = initial.allocations.clone();
        let ledger = fixture.ledger_with(initial);
        let (mut snapshot, request) = context("a", 1);
        snapshot.policy.capacity = resources(3);
        snapshot.policy.per_instance = resources(2);
        snapshot.instance.resources = resources(2);
        let ClaimOutcome::New(permit) = ledger.claim(&request, |_| Ok((snapshot, 110))).unwrap()
        else {
            panic!("original larger slots should be admitted");
        };
        let stopped = permit.complete(Observation::Stopped).unwrap();

        let (mut snapshot, mut request) = context("a", stopped.updated_revision);
        snapshot.policy.capacity = resources(3);
        snapshot.policy.per_instance = resources(2);
        snapshot.instance.revision = "instance-2".into();
        for requirement in &mut snapshot.instance.storage {
            requirement.id.push_str("-new");
            requirement.binding_revision.push_str("-new");
        }
        for evidence in &mut snapshot.readiness.evidence {
            evidence.instance_revision = "instance-2".into();
        }
        request.operation_id = "restart-on-smaller-slots".into();
        request.expected.instance = "instance-2".into();
        let ClaimOutcome::New(permit) = ledger.claim(&request, |_| Ok((snapshot, 110))).unwrap()
        else {
            panic!("new exact slots should not resize or release historical allocations");
        };
        permit.complete(Observation::Running).unwrap();
        let current = fixture.open("ledger").unwrap().snapshot().unwrap();
        assert_eq!(current.entries[0].resources, resources(1));
        assert_eq!(current.storage, expected_storage);
        assert_eq!(
            current
                .storage
                .iter()
                .map(|allocation| allocation.bytes)
                .sum::<u64>(),
            6 * MIN_STORAGE_BYTES
        );
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
            ledger.claim(&request, |_| Ok((snapshot, 110))),
            Err(LedgerError::Io(_))
        ));
        assert!(fixture.path().join(STAGING).is_file());
    }

    #[test]
    fn storage_reservation_serializes_the_last_slot_across_processes() {
        let fixture = Fixture::new();
        let mut initial = inventory(&[]);
        initial.allocations.push(StorageAllocation {
            id: "orphan-workspace".into(),
            kind: StorageKind::Workspace,
            bytes: MIN_STORAGE_BYTES,
            binding: None,
            state: StorageState::Present,
        });
        let ledger = fixture.ledger_with(initial);
        let (snapshot, request) = storage_context("a", 1);
        let StorageClaimOutcome::New(permit) = ledger
            .reserve_storage(&request, |_| {
                assert!(matches!(ledger.snapshot(), Err(LedgerError::Busy)));
                Ok(snapshot)
            })
            .unwrap()
        else {
            panic!("last available slot must be claimable");
        };
        assert_eq!(permit.receipt().allocation.state, StorageState::Unknown);
        fixture.child("busy");
        permit.complete(StorageState::Present).unwrap();
        fixture.child("storage-exhausted");
        assert_eq!(ledger.snapshot().unwrap().storage.len(), 2);
    }

    #[test]
    fn storage_process_exit_preserves_charge_and_never_replays_provision() {
        let fixture = Fixture::new();
        let ledger = fixture.ledger_with(inventory(&[]));
        fixture.child("storage-crash");
        let durable = ledger.snapshot().unwrap();
        assert_eq!(durable.revision, 2);
        assert_eq!(durable.storage.len(), 1);
        assert_eq!(durable.storage[0].state, StorageState::Unknown);
        assert_eq!(durable.storage[0].bytes, MIN_STORAGE_BYTES);

        let (snapshot, request) = storage_context("a", 1);
        let StorageClaimOutcome::Existing(receipt) =
            ledger.reserve_storage(&request, |_| Ok(snapshot)).unwrap()
        else {
            panic!("interrupted operation cannot acquire another create permit");
        };
        assert_eq!(receipt.allocation, durable.storage[0]);
        let (snapshot, mut new_operation) = storage_context("a", 2);
        new_operation.request.operation_id = "retry-provision-a".into();
        let StorageClaimOutcome::Denied(decision) = ledger
            .reserve_storage(&new_operation, |_| Ok(snapshot))
            .unwrap()
        else {
            panic!("changing operation identity cannot recreate unknown storage");
        };
        assert!(
            decision
                .reasons
                .iter()
                .any(|reason| reason.code == ReasonCode::StorageUnknown)
        );
        let unchanged = ledger
            .reconcile_storage("workspace-a", 2, |_| Ok(StorageState::Unknown))
            .unwrap();
        assert_eq!(unchanged.updated_revision, 2);
        let present = ledger
            .reconcile_storage("workspace-a", 2, |_| Ok(StorageState::Present))
            .unwrap();
        assert_eq!(present.updated_revision, 3);
        assert_eq!(present.allocation.bytes, MIN_STORAGE_BYTES);
        assert!(ledger.snapshot().unwrap().entries.is_empty());
    }

    #[test]
    fn storage_write_failure_returns_no_permission_and_preserves_existing_inventory() {
        let fixture = Fixture::new();
        let ledger = fixture.ledger_with(inventory(&[]));
        let before = fs::read(fixture.path().join(STATE)).unwrap();
        fixture.child("storage-write-failure");
        assert_eq!(fs::read(fixture.path().join(STATE)).unwrap(), before);
        assert!(fixture.path().join(STAGING).is_file());
        assert!(ledger.snapshot().unwrap().storage.is_empty());
        let (snapshot, request) = storage_context("a", 1);
        assert!(matches!(
            ledger.reserve_storage(&request, |_| Ok(snapshot)),
            Err(LedgerError::Io(_))
        ));
    }

    #[test]
    fn bootstrap_retains_orphan_unknown_and_over_budget_inventory() {
        let fixture = Fixture::new();
        let mut initial = inventory(&["a"]);
        initial.allocations.push(StorageAllocation {
            id: "removed-instance-workspace".into(),
            kind: StorageKind::Workspace,
            bytes: 3 * MIN_STORAGE_BYTES,
            binding: None,
            state: StorageState::Unknown,
        });
        let expected = initial.allocations.clone();
        let ledger = fixture.ledger_with(initial);
        assert_eq!(ledger.snapshot().unwrap().storage, expected);
        let (snapshot, request) = context("a", 1);
        let ClaimOutcome::Denied(decision) =
            ledger.claim(&request, |_| Ok((snapshot, 110))).unwrap()
        else {
            panic!("preexisting over-budget inventory must block new startup");
        };
        assert!(
            decision
                .reasons
                .iter()
                .any(|reason| reason.code == ReasonCode::ResourceExhausted)
        );
        let (snapshot, request) = storage_context("b", 1);
        assert!(matches!(
            ledger.reserve_storage(&request, |_| Ok(snapshot)).unwrap(),
            StorageClaimOutcome::Denied(_)
        ));
        assert_eq!(
            fixture.open("ledger").unwrap().snapshot().unwrap().storage,
            expected
        );
    }

    #[test]
    fn incomplete_or_duplicate_bootstrap_does_not_create_an_empty_ledger() {
        let fixture = Fixture::new();
        let mut incomplete = inventory(&[]);
        incomplete.complete = false;
        assert!(matches!(
            fixture.initialize("ledger", incomplete),
            Err(LedgerError::Invalid(_))
        ));
        assert!(!fixture.path().exists());
        let mut duplicate = inventory(&["a"]);
        duplicate.allocations.push(duplicate.allocations[0].clone());
        assert!(matches!(
            fixture.initialize("ledger", duplicate),
            Err(LedgerError::Invalid(_))
        ));
        assert!(!fixture.path().exists());
    }

    #[test]
    fn imported_present_storage_cannot_recreate_a_missing_image_or_rebind() {
        let fixture = Fixture::new();
        let ledger = fixture.ledger();
        let (snapshot, request) = storage_context("a", 1);
        let missing_image = fixture.0.join("must-not-be-created.img");
        match ledger.reserve_storage(&request, |_| Ok(snapshot)).unwrap() {
            StorageClaimOutcome::New(_) => {
                fs::write(&missing_image, b"unexpected dispatch").unwrap();
                panic!("imported allocation cannot authorize creation");
            }
            StorageClaimOutcome::Existing(receipt) => assert!(receipt.request.is_none()),
            StorageClaimOutcome::Denied(_) => panic!("matching allocation should be visible"),
        }
        assert!(!missing_image.exists());
        let (mut snapshot, request) = storage_context("a", 1);
        snapshot.instance.storage[0].binding_revision = "changed-mount-or-uid".into();
        assert!(matches!(
            ledger.reserve_storage(&request, |_| Ok(snapshot)).unwrap(),
            StorageClaimOutcome::Denied(_)
        ));
        assert_eq!(ledger.snapshot().unwrap().revision, 1);
    }

    #[test]
    fn storage_replay_reauthorizes_and_operation_keys_cannot_change_kind() {
        let fixture = Fixture::new();
        let ledger = fixture.ledger_with(inventory(&[]));
        let original = reserve(&ledger, "a")
            .complete(StorageState::Present)
            .unwrap();
        let (mut snapshot, request) = storage_context("a", 1);
        snapshot.instance.owner_id = "other-owner".into();
        assert!(matches!(
            ledger.reserve_storage(&request, |_| Ok(snapshot)).unwrap(),
            StorageClaimOutcome::Denied(_)
        ));
        let (snapshot, mut changed) = storage_context("a", 1);
        changed.allocation_id = "state-a".into();
        assert!(matches!(
            ledger.reserve_storage(&changed, |_| Ok(snapshot)),
            Err(LedgerError::Conflict)
        ));
        let (snapshot, mut start) = context("a", original.updated_revision);
        start.operation_id = request.request.operation_id;
        assert!(matches!(
            ledger.claim(&start, |_| Ok((snapshot, 110))),
            Err(LedgerError::Conflict)
        ));
    }

    #[test]
    fn storage_only_legacy_workspace_does_not_need_compute_state_or_model() {
        let fixture = Fixture::new();
        let ledger = fixture.ledger_with(inventory(&[]));
        let (mut snapshot, request) = storage_context("legacy", 1);
        snapshot.instance.resources = Resources {
            workspace_bytes: MIN_STORAGE_BYTES,
            ..Resources::default()
        };
        snapshot
            .instance
            .storage
            .retain(|requirement| requirement.kind == StorageKind::Workspace);
        snapshot.instance.requested_capabilities = vec![Capability::Model];
        let StorageClaimOutcome::New(permit) =
            ledger.reserve_storage(&request, |_| Ok(snapshot)).unwrap()
        else {
            panic!("storage-only provisioning must support a stopped workspace-only definition");
        };
        permit.complete(StorageState::Present).unwrap();
        assert_eq!(ledger.snapshot().unwrap().storage.len(), 1);
    }

    #[test]
    fn interrupted_publication_blocks_work_but_preserves_receipts_and_reconciliation() {
        let fixture = Fixture::new();
        let ledger = fixture.ledger_with(inventory(&["a"]));
        let provision = reserve(&ledger, "b").receipt().clone();
        let start = claim(&ledger, "a").receipt().clone();
        fixture.child("publication-crash");
        let ledger = fixture.open("ledger").unwrap();
        let status = ledger.publication_status().unwrap();
        let pending = status.pending.unwrap();
        assert_eq!(status.published, authority("policy-1"));
        assert_eq!(pending.outcome, PublicationOutcome::Pending);
        assert_eq!(ledger.snapshot().unwrap().revision, 4);

        let (snapshot, request) = context("b", 4);
        assert!(matches!(
            ledger.claim(&request, |_| Ok((snapshot, 110))),
            Err(LedgerError::TransitionPending)
        ));
        let (snapshot, request) = storage_context("a", 4);
        assert!(matches!(
            ledger.reserve_storage(&request, |_| Ok(snapshot.clone())),
            Err(LedgerError::TransitionPending)
        ));
        assert!(matches!(
            ledger.with_present_storage::<()>(
                &status.published,
                &request,
                |_| Ok(snapshot),
                |_| panic!("pending publication must fence owner mutation")
            ),
            Err(LedgerError::TransitionPending)
        ));

        let (snapshot, _) = context("a", 4);
        assert!(matches!(
            ledger
                .claim(&start.request, |_| Ok((snapshot, 110)))
                .unwrap(),
            ClaimOutcome::Existing(_)
        ));
        let (snapshot, _) = storage_context("b", 4);
        assert!(matches!(
            ledger
                .reserve_storage(provision.request.as_ref().unwrap(), |_| Ok(snapshot))
                .unwrap(),
            StorageClaimOutcome::Existing(_)
        ));
        assert!(matches!(
            ledger.finish_publication("publish-2", 4, |_, _| panic!(
                "unknown runtime must block collection"
            )),
            Err(LedgerError::NotQuiescent)
        ));
        ledger
            .reconcile(&start.request.operation_id, 4, |_| Ok(Observation::Stopped))
            .unwrap();
        assert!(matches!(
            ledger.finish_publication("publish-2", 5, |_, _| panic!(
                "unknown storage must block collection"
            )),
            Err(LedgerError::NotQuiescent)
        ));
        ledger
            .reconcile_storage("workspace-b", 5, |_| Ok(StorageState::Present))
            .unwrap();
        let charges = ledger.snapshot().unwrap().storage;
        let receipt = ledger
            .finish_publication("publish-2", 6, |receipt, snapshot| {
                assert_eq!(snapshot.entries[0].state, ReservationState::Retained);
                assert_eq!(receipt, &pending);
                assert!(matches!(ledger.snapshot(), Err(LedgerError::Busy)));
                fixture.child("busy");
                installed(&receipt.request.candidate)
            })
            .unwrap();
        assert_eq!(receipt.updated_revision, 7);
        assert_eq!(ledger.snapshot().unwrap().storage, charges);
        assert_eq!(
            ledger.publication_status().unwrap().published,
            authority("policy-2")
        );
        assert!(ledger.publication_status().unwrap().pending.is_none());
    }

    #[test]
    fn publication_retries_are_exact_and_operation_ids_are_global() {
        let fixture = Fixture::new();
        let ledger = fixture.ledger_with(inventory(&["a"]));
        let storage = reserve(&ledger, "b")
            .complete(StorageState::Present)
            .unwrap();
        let runtime = claim(&ledger, "a").complete(Observation::Stopped).unwrap();
        for operation_id in [
            &runtime.request.operation_id,
            &storage.request.as_ref().unwrap().request.operation_id,
        ] {
            let request = publication(&ledger, operation_id, "policy-2");
            assert!(matches!(
                ledger.begin_publication(&request),
                Err(LedgerError::Conflict)
            ));
        }
        let request = publication(&ledger, "publish-2", "policy-2");
        let mut changed = request.clone();
        changed.candidate.generation = "new-epoch".into();
        assert!(matches!(
            ledger.begin_publication(&changed),
            Err(LedgerError::Invalid(_))
        ));
        let PublicationBegin::New(begun) = ledger.begin_publication(&request).unwrap() else {
            panic!("new publication")
        };
        let PublicationBegin::Existing(repeated) = ledger.begin_publication(&request).unwrap()
        else {
            panic!("exact retry")
        };
        assert_eq!(repeated, begun);
        changed = request.clone();
        changed.candidate = authority("policy-3");
        assert!(matches!(
            ledger.begin_publication(&changed),
            Err(LedgerError::Conflict)
        ));
        let another = publication(&ledger, "publish-other", "policy-3");
        assert!(matches!(
            ledger.begin_publication(&another),
            Err(LedgerError::TransitionPending)
        ));
        let (snapshot, mut start) = context("a", begun.updated_revision);
        start.operation_id = request.operation_id.clone();
        assert!(matches!(
            ledger.claim(&start, |_| Ok((snapshot, 110))),
            Err(LedgerError::Conflict)
        ));
        let (snapshot, mut provision) = storage_context("a", begun.updated_revision);
        provision.request.operation_id = request.operation_id.clone();
        assert!(matches!(
            ledger.reserve_storage(&provision, |_| Ok(snapshot)),
            Err(LedgerError::Conflict)
        ));

        let finished = ledger
            .finish_publication("publish-2", begun.updated_revision, |receipt, _| {
                installed(&receipt.request.candidate)
            })
            .unwrap();
        let second = publication(&ledger, "publish-3", "policy-3");
        let mut stale = second.clone();
        stale.expected_authority = authority("policy-1");
        assert!(matches!(
            ledger.begin_publication(&stale),
            Err(LedgerError::StaleAuthority)
        ));
        stale = second.clone();
        stale.expected_ledger_revision -= 1;
        assert!(matches!(
            ledger.begin_publication(&stale),
            Err(LedgerError::StaleRevision)
        ));
        ledger.begin_publication(&second).unwrap();
        assert_eq!(
            ledger
                .finish_publication("publish-2", begun.updated_revision, |_, _| panic!(
                    "terminal retry cannot replay observer"
                ))
                .unwrap(),
            finished
        );
        let PublicationBegin::Existing(historical) = ledger.begin_publication(&request).unwrap()
        else {
            panic!("historical retry")
        };
        assert_eq!(historical, finished);
        assert!(matches!(
            ledger.finish_publication("publish-2", begun.updated_revision + 1, |_, _| panic!(
                "changed retry"
            )),
            Err(LedgerError::StaleRevision)
        ));
        assert!(matches!(
            ledger.cancel_publication("publish-2", begun.updated_revision, |_, _| panic!(
                "opposite outcome"
            )),
            Err(LedgerError::Conflict)
        ));
    }

    #[test]
    fn failed_publication_observation_preserves_pending_until_explicit_cancel() {
        let fixture = Fixture::new();
        let ledger = fixture.ledger();
        let request = publication(&ledger, "publish-2", "policy-2");
        ledger.begin_publication(&request).unwrap();
        let before = fs::read(fixture.path().join(STATE)).unwrap();
        assert!(matches!(
            ledger.finish_publication("publish-2", 1, |_, _| panic!("stale revision")),
            Err(LedgerError::StaleRevision)
        ));
        assert!(matches!(
            ledger.finish_publication("publish-2", 2, |_, _| Err("observer unavailable".into())),
            Err(LedgerError::Context)
        ));
        assert!(matches!(
            ledger.finish_publication("publish-2", 2, |_, _| Ok(PublicationObservation {
                installed: request.candidate.clone(),
                mutators_fenced: false
            })),
            Err(LedgerError::NotQuiescent)
        ));
        assert!(matches!(
            ledger.finish_publication("publish-2", 2, |_, _| installed(
                &request.expected_authority
            )),
            Err(LedgerError::StaleAuthority)
        ));
        assert!(matches!(
            ledger.cancel_publication("publish-2", 2, |_, _| installed(&request.candidate)),
            Err(LedgerError::StaleAuthority)
        ));
        assert_eq!(fs::read(fixture.path().join(STATE)).unwrap(), before);
        let ledger = fixture.open("ledger").unwrap();
        assert!(ledger.publication_status().unwrap().pending.is_some());
        let cancelled = ledger
            .cancel_publication("publish-2", 2, |receipt, _| {
                installed(&receipt.request.expected_authority)
            })
            .unwrap();
        assert_eq!(cancelled.outcome, PublicationOutcome::Cancelled);
        assert_eq!(
            ledger.publication_status().unwrap().published,
            authority("policy-1")
        );
        assert!(ledger.publication_status().unwrap().pending.is_none());
        assert_eq!(
            ledger
                .cancel_publication("publish-2", 2, |_, _| panic!("cancel retry is historical"))
                .unwrap(),
            cancelled
        );
        assert!(matches!(
            ledger.finish_publication("publish-2", 2, |_, _| panic!("cancel cannot finish")),
            Err(LedgerError::Conflict)
        ));
    }

    #[test]
    fn published_authority_fences_stale_collectors_but_allows_current_receipt_visibility() {
        let fixture = Fixture::new();
        let ledger = fixture.ledger();
        let original = claim(&ledger, "a").complete(Observation::Stopped).unwrap();
        let request = publication(&ledger, "publish-2", "policy-2");
        ledger.begin_publication(&request).unwrap();
        ledger
            .finish_publication("publish-2", 4, |receipt, _| {
                installed(&receipt.request.candidate)
            })
            .unwrap();
        let (snapshot, _) = context("a", 5);
        assert!(matches!(
            ledger.claim(&original.request, |_| Ok((snapshot.clone(), 110))),
            Err(LedgerError::StaleAuthority)
        ));
        let result = ledger
            .claim(&original.request, |published| {
                assert_eq!(published, &authority("policy-2"));
                let mut current = snapshot.clone();
                current.policy.revision = published.revision.clone();
                current.readiness.evidence.clear();
                Ok((current, 110))
            })
            .unwrap();
        assert!(matches!(result, ClaimOutcome::Existing(_)));
        let (mut snapshot, mut start) = context("b", 5);
        start.expected.policy = "policy-2".into();
        snapshot.policy.revision = "policy-2".into();
        snapshot.policy.generation = "different-epoch".into();
        assert!(matches!(
            ledger.claim(&start, |_| Ok((snapshot.clone(), 110))),
            Err(LedgerError::StaleAuthority)
        ));
        snapshot.policy.generation = "authority-1".into();
        for evidence in &mut snapshot.readiness.evidence {
            evidence.policy_revision = "policy-2".into();
        }
        assert!(matches!(
            ledger.claim(&start, |_| Ok((snapshot, 110))).unwrap(),
            ClaimOutcome::New(_)
        ));
    }

    #[test]
    fn present_storage_mutation_holds_publication_lock_and_never_grants_creation() {
        let fixture = Fixture::new();
        let ledger = fixture.ledger_with(inventory(&["a"]));
        let (snapshot, request) = storage_context("a", 1);
        let publication = publication(&ledger, "publish-2", "policy-2");
        let output = ledger
            .with_present_storage(
                &authority("policy-1"),
                &request,
                |published| {
                    assert_eq!(published, &authority("policy-1"));
                    Ok(snapshot.clone())
                },
                |allocation| {
                    assert_eq!(allocation.id, "workspace-a");
                    assert_eq!(allocation.state, StorageState::Present);
                    fixture.child("busy");
                    assert!(matches!(
                        ledger.begin_publication(&publication),
                        Err(LedgerError::Busy)
                    ));
                    Ok(42)
                },
            )
            .unwrap();
        assert_eq!(output, 42);
        assert_eq!(ledger.snapshot().unwrap().revision, 1);
        assert!(matches!(
            ledger.with_present_storage::<()>(
                &authority("policy-2"),
                &request,
                |_| panic!("stale authority"),
                |_| Ok(())
            ),
            Err(LedgerError::StaleAuthority)
        ));
        let mut stale = request.clone();
        stale.request.expected.ledger = 2;
        assert!(matches!(
            ledger.with_present_storage::<()>(
                &authority("policy-1"),
                &stale,
                |_| Ok(snapshot.clone()),
                |_| panic!("stale request")
            ),
            Err(LedgerError::StorageDenied(_))
        ));
        let mut unowned = snapshot.clone();
        unowned.instance.owner_id = "other-owner".into();
        assert!(matches!(
            ledger.with_present_storage::<()>(
                &authority("policy-1"),
                &request,
                |_| Ok(unowned),
                |_| panic!("wrong owner")
            ),
            Err(LedgerError::StorageDenied(_))
        ));
        let (absent, absent_request) = storage_context("b", 1);
        assert!(matches!(
            ledger.with_present_storage::<()>(
                &authority("policy-1"),
                &absent_request,
                |_| Ok(absent),
                |_| panic!("pure allow is not creation authority")
            ),
            Err(LedgerError::Conflict)
        ));
        ledger
            .reconcile_storage("workspace-a", 1, |_| Ok(StorageState::Unknown))
            .unwrap();
        let (snapshot, request) = storage_context("a", 2);
        assert!(matches!(
            ledger.with_present_storage::<()>(
                &authority("policy-1"),
                &request,
                |_| Ok(snapshot),
                |_| panic!("unknown allocation")
            ),
            Err(LedgerError::StorageDenied(_))
        ));
    }

    #[test]
    fn publication_write_failure_does_not_publish_or_discard_staging() {
        let fixture = Fixture::new();
        let ledger = fixture.ledger();
        let before = fs::read(fixture.path().join(STATE)).unwrap();
        fixture.child("publication-write-failure");
        assert_eq!(fs::read(fixture.path().join(STATE)).unwrap(), before);
        assert!(fixture.path().join(STAGING).is_file());
        assert!(ledger.publication_status().unwrap().pending.is_none());
        let request = publication(&ledger, "publish-2", "policy-2");
        assert!(matches!(
            ledger.begin_publication(&request),
            Err(LedgerError::Io(_))
        ));
        assert!(fixture.path().join(STAGING).is_file());
    }

    #[test]
    fn inconsistent_or_unbounded_publication_history_is_not_loadable() {
        let fixture = Fixture::new();
        let ledger = fixture.ledger();
        let request = publication(&ledger, "publish-2", "policy-2");
        ledger.begin_publication(&request).unwrap();
        ledger
            .finish_publication("publish-2", 2, |receipt, _| {
                installed(&receipt.request.candidate)
            })
            .unwrap();
        let path = fixture.path().join(STATE);
        let bytes = fs::read(&path).unwrap();
        let mut state: State = serde_json::from_slice(&bytes).unwrap();
        state.published = authority("policy-3");
        fs::write(&path, serde_json::to_vec(&state).unwrap()).unwrap();
        assert!(matches!(
            fixture.open("ledger"),
            Err(LedgerError::Invalid(_))
        ));
        let mut state: State = serde_json::from_slice(&bytes).unwrap();
        state.publications[0].request.candidate.generation = "other-epoch".into();
        fs::write(&path, serde_json::to_vec(&state).unwrap()).unwrap();
        assert!(matches!(
            fixture.open("ledger"),
            Err(LedgerError::Invalid(_))
        ));
        let mut state: State = serde_json::from_slice(&bytes).unwrap();
        state
            .publications
            .resize(MAX_PUBLICATIONS + 1, state.publications[0].clone());
        fs::write(&path, serde_json::to_vec(&state).unwrap()).unwrap();
        assert!(matches!(
            fixture.open("ledger"),
            Err(LedgerError::Invalid(_))
        ));
        fs::write(&path, bytes).unwrap();
        assert_eq!(
            ledger.publication_status().unwrap().published,
            authority("policy-2")
        );
    }

    #[test]
    fn missing_corrupt_or_oversized_state_never_reinitializes() {
        let fixture = Fixture::new();
        let ledger = fixture.ledger();
        let path = fixture.path().join(STATE);
        let original = fs::read(&path).unwrap();
        let mut old: serde_json::Value = serde_json::from_slice(&original).unwrap();
        old["schemaVersion"] = serde_json::json!(2);
        old.as_object_mut().unwrap().remove("published");
        old.as_object_mut().unwrap().remove("publications");
        old["generation"] = serde_json::json!("authority-1");
        let old = serde_json::to_vec(&old).unwrap();
        fs::write(&path, &old).unwrap();
        assert!(fixture.open("ledger").is_err());
        assert_eq!(fs::read(&path).unwrap(), old);
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
        assert!(fixture.open("ledger").is_err());
        assert!(fixture.initialize("ledger", inventory(&[])).is_err());
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
        assert!(fixture.open("alias").is_err());
        fs::set_permissions(fixture.path(), fs::Permissions::from_mode(0o755)).unwrap();
        assert!(matches!(
            fixture.open("ledger"),
            Err(LedgerError::Invalid(_))
        ));
    }

    #[test]
    fn public_api_still_checks_absolute_ancestry() {
        let fixture = Fixture::new();
        let _ledger = fixture.ledger();
        assert!(Ledger::open(Path::new("ledger")).is_err());
        let public_path = fixture.0.join("public-ledger");
        let root_uid = fs::metadata("/").unwrap().uid();
        if root_uid != 0 && root_uid != unsafe { libc::geteuid() } {
            // In the actual Nix sandbox / belongs to an unmapped UID.
            // A fixture anchor must not make the public API accept it.
            assert!(matches!(
                Ledger::open(&fixture.path()),
                Err(LedgerError::Invalid("untrusted directory ancestry"))
            ));
            assert!(matches!(
                Ledger::initialize(&public_path, authority("policy-1"), inventory(&[])),
                Err(LedgerError::Invalid("untrusted directory ancestry"))
            ));
            assert!(!public_path.exists());
        } else {
            Ledger::initialize(&public_path, authority("policy-1"), inventory(&[])).unwrap();
            assert!(Ledger::open(&public_path).is_ok());
        }
    }

    #[test]
    fn anchored_traversal_checks_descendants_and_rejects_escape() {
        let fixture = Fixture::new();
        let child = fixture.0.join("child");
        DirBuilder::new().mode(0o700).create(&child).unwrap();
        assert!(fixture_directory(&fixture.0, "child").is_ok());
        fs::set_permissions(&child, fs::Permissions::from_mode(0o777)).unwrap();
        assert!(matches!(
            fixture_directory(&fixture.0, "child"),
            Err(LedgerError::Invalid("untrusted directory ancestry"))
        ));
        fs::set_permissions(&child, fs::Permissions::from_mode(0o700)).unwrap();
        for escape in ["..", "child/../ledger", "/"] {
            assert!(fixture_directory(&fixture.0, escape).is_err());
        }
    }

    #[test]
    fn writable_or_foreign_ancestors_cannot_create_a_second_lock_namespace() {
        let fixture = Fixture::new();
        let _ledger = fixture.ledger();
        fs::set_permissions(&fixture.0, fs::Permissions::from_mode(0o777)).unwrap();
        assert!(matches!(
            fixture.open("ledger"),
            Err(LedgerError::Invalid(_))
        ));
        assert!(matches!(
            fixture.initialize("second", inventory(&[])),
            Err(LedgerError::Invalid(_))
        ));
        assert!(!fixture.0.join("second").exists());
        fs::set_permissions(&fixture.0, fs::Permissions::from_mode(0o700)).unwrap();
        if unsafe { libc::geteuid() } == 0 {
            let directory = File::open(&fixture.0).unwrap();
            assert_eq!(unsafe { libc::fchown(directory.as_raw_fd(), 65534, !0) }, 0);
            assert!(matches!(
                fixture.open("ledger"),
                Err(LedgerError::Invalid(_))
            ));
            assert_eq!(unsafe { libc::fchown(directory.as_raw_fd(), 0, !0) }, 0);
        }
        assert!(fixture.open("ledger").is_ok());
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
