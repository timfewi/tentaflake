//! Shared startup admission over an explicit, host-owned snapshot.
//!
//! A pure decision neither authenticates its input nor enforces containment.
//! Callers must collect trusted current policy and launch-prerequisite evidence,
//! reserve under the authoritative ledger lock, and fence dispatch against changes.
//! A new runtime claim persists its exact host/daemon/supervisor/image intent
//! before returning the only dispatch permit; that permit must durably bind the
//! full container ID before startup can be recorded Running. Legacy v3 runtime
//! receipts retain their charges without becoming live identity evidence.
//! Source declarations, fixtures and a successful decision are not runtime or
//! vendor acceptance. The HTTP contract remains owned by `api/v1/openapi.json`.

mod decision;
pub mod host_fence;
pub mod ledger;
pub mod protected_path;
mod runtime;
mod storage_decision;

pub use decision::{
    authorize, check_policy as validate_policy, decide, identifier, validate_authority,
    validate_reservations,
};
use serde::{Deserialize, Serialize};
pub use storage_decision::{authorize_storage, decide_storage};

/// Effective identity of the calling thread for host-owned permission checks.
pub fn effective_uid() -> libc::uid_t {
    // SAFETY: geteuid has no pointer arguments or caller-side preconditions.
    unsafe { libc::geteuid() }
}

/// Effective group used by the existing host and Worker permission checks.
pub fn effective_gid() -> libc::gid_t {
    // SAFETY: getegid has no pointer arguments or caller-side preconditions.
    unsafe { libc::getegid() }
}

/// Version two accounts persistent volumes independently of startup receipts.
/// Version-one ledgers require explicit migration; never reinterpret them empty.
pub const SCHEMA_VERSION: u32 = 2;
pub const MAX_RESERVATIONS: usize = 4096;
pub const MAX_STORAGE_ALLOCATIONS: usize = 2 * MAX_RESERVATIONS;
pub const MAX_GRANTS: usize = 4096;
pub const MIN_STORAGE_BYTES: u64 = 128 * 1024 * 1024;

/// CPU uses integer nanocores. Memory swap is combined memory plus swap.
/// Storage remains charged after compute stops, including private agent state.
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct Resources {
    pub memory_bytes: u64,
    pub memory_swap_bytes: u64,
    pub nano_cpus: u64,
    pub pids: u64,
    pub workspace_bytes: u64,
    pub state_bytes: u64,
}

impl Resources {
    pub fn checked_add(self, other: Self) -> Option<Self> {
        Some(Self {
            memory_bytes: self.memory_bytes.checked_add(other.memory_bytes)?,
            memory_swap_bytes: self
                .memory_swap_bytes
                .checked_add(other.memory_swap_bytes)?,
            nano_cpus: self.nano_cpus.checked_add(other.nano_cpus)?,
            pids: self.pids.checked_add(other.pids)?,
            workspace_bytes: self.workspace_bytes.checked_add(other.workspace_bytes)?,
            state_bytes: self.state_bytes.checked_add(other.state_bytes)?,
        })
    }

    pub fn retained(self) -> Self {
        Self {
            workspace_bytes: self.workspace_bytes,
            state_bytes: self.state_bytes,
            ..Self::default()
        }
    }

    /// Compute charged by a runtime receipt; allocation rows alone charge disk.
    pub fn compute(self) -> Self {
        Self {
            workspace_bytes: 0,
            state_bytes: 0,
            ..self
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "kebab-case")]
pub enum Capability {
    Model,
    Research,
    Worker,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum Isolation {
    Container,
    GuestKernel,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct Grant {
    pub instance_id: String,
    pub owner_id: String,
    pub capability: Capability,
    pub expires_at: Option<u64>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct HostPolicy {
    /// Opaque revision derived from authoritative policy content, never a counter.
    pub revision: String,
    /// Durable authority epoch. Changing it requires explicit ledger migration;
    /// it is not the boot ID or a collector-generated lease identifier.
    pub generation: String,
    pub capacity: Resources,
    pub host_reserve: Resources,
    pub research_reserve: Resources,
    pub per_instance: Resources,
    pub max_active: u32,
    pub evidence_max_age_seconds: u64,
    pub grants: Vec<Grant>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct Instance {
    pub id: String,
    pub owner_id: String,
    pub revision: String,
    pub isolation: Isolation,
    pub resources: Resources,
    pub requested_capabilities: Vec<Capability>,
    /// Host-owned fixed backing slots. Startup requires both kinds; storage-only
    /// provisioning also supports legacy workspace-only definitions.
    pub storage: Vec<StorageRequirement>,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "kebab-case")]
pub enum StorageKind {
    Workspace,
    State,
}

impl StorageKind {
    pub fn bytes(self, resources: Resources) -> u64 {
        match self {
            Self::Workspace => resources.workspace_bytes,
            Self::State => resources.state_bytes,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct StorageRequirement {
    /// Stable host-generated identity of the canonical image backing slot.
    pub id: String,
    pub kind: StorageKind,
    /// Hash of image path, mount path and UID/GID, independent of command revision.
    pub binding_revision: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct StorageBinding {
    pub instance_id: String,
    pub owner_id: String,
    pub revision: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum StorageState {
    /// Provisioning may have happened. The entire allocation remains charged.
    Unknown,
    /// A trusted collector confirmed the image and any recorded owner binding.
    Present,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct StorageAllocation {
    pub id: String,
    pub kind: StorageKind,
    pub bytes: u64,
    /// None identifies retained orphan storage, never implicitly adoptable.
    pub binding: Option<StorageBinding>,
    pub state: StorageState,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ExpectedRevisions {
    pub policy: String,
    pub instance: String,
    pub ledger: u64,
}

/// The host authenticates principal_id before invoking this library.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct AdmissionRequest {
    pub schema_version: u32,
    pub operation_id: String,
    pub instance_id: String,
    pub principal_id: String,
    pub expected: ExpectedRevisions,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "kebab-case")]
pub enum EvidenceKind {
    Runtime,
    WorkspaceQuota,
    StateQuota,
    Model,
    Research,
    Worker,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum EvidenceStatus {
    Ready,
    Unready,
    Unknown,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum EvidenceSource {
    Live,
    Declaration,
}

/// Independently collected current service generation, not an observer-made ID.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ReadinessGeneration {
    pub kind: EvidenceKind,
    pub generation: String,
}

/// Startup prerequisites only, not post-start containment or vendor acceptance.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ReadinessEvidence {
    pub instance_id: String,
    pub kind: EvidenceKind,
    pub status: EvidenceStatus,
    pub source: EvidenceSource,
    pub policy_revision: String,
    pub instance_revision: String,
    pub generation: String,
    pub observed_at: u64,
    pub expires_at: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ReadinessSnapshot {
    /// Same durable authority epoch as policy and ledger. Per-source generations
    /// separately fence host boot, runtime daemon and Research controller changes.
    pub generation: String,
    pub sources: Vec<ReadinessGeneration>,
    pub evidence: Vec<ReadinessEvidence>,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum ReservationState {
    Reserved,
    Active,
    Retained,
    Unknown,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct Reservation {
    pub instance_id: String,
    pub owner_id: String,
    pub instance_revision: String,
    pub state: ReservationState,
    /// Original requested resources; storage fields are historical descriptions.
    /// Only compute() contributes to capacity; allocation rows own disk charges.
    pub resources: Resources,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ReservationSnapshot {
    pub revision: u64,
    /// Established explicitly for a genuine new ledger; never reset on restart.
    pub generation: String,
    /// Trusted inventory completeness, including all managed volume creators;
    /// merely reading a ledger or missing directory cannot establish this.
    pub complete: bool,
    pub entries: Vec<Reservation>,
    /// Every retained or possibly provisioned volume, including never-started
    /// instances and orphans. These rows alone own persistent-storage accounting.
    pub storage: Vec<StorageAllocation>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct AdmissionSnapshot {
    pub schema_version: u32,
    pub policy: HostPolicy,
    pub instance: Instance,
    pub readiness: ReadinessSnapshot,
    pub reservations: ReservationSnapshot,
}

/// Storage provisioning has no runtime/capability readiness prerequisite. The
/// host still authenticates this closed policy/instance/inventory snapshot.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct StorageAdmissionSnapshot {
    pub schema_version: u32,
    pub policy: HostPolicy,
    pub instance: Instance,
    pub reservations: ReservationSnapshot,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct StorageRequest {
    pub request: AdmissionRequest,
    pub allocation_id: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct StorageDecision {
    pub allowed: bool,
    pub reasons: Vec<Reason>,
    pub allocation: Option<StorageAllocation>,
}

/// Offline replay envelope, not a trusted live probe or an HTTP request.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct RecordedAdmission {
    pub now_unix_seconds: u64,
    pub snapshot: AdmissionSnapshot,
    pub request: AdmissionRequest,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "kebab-case")]
pub enum ReasonCode {
    UnsupportedVersion,
    InvalidPolicy,
    InvalidInstance,
    InvalidRequest,
    StalePolicy,
    StaleInstance,
    StaleLedger,
    WrongPrincipal,
    UnsupportedIsolation,
    MissingGrant,
    UnrequestedGrant,
    ExpiredGrant,
    DuplicateGrant,
    InvalidReservation,
    DuplicateReservation,
    LedgerIncomplete,
    UnknownReservation,
    ReservationConflict,
    InvalidStorage,
    DuplicateStorage,
    MissingStorage,
    StorageConflict,
    StorageUnknown,
    InvalidEvidence,
    DuplicateEvidence,
    MissingEvidence,
    MissingGeneration,
    StaleEvidence,
    GenerationMismatch,
    DeclarationOnly,
    ReadinessUnknown,
    NotReady,
    ResourceOverflow,
    ResourceExhausted,
    InstanceLimit,
    ActiveLimit,
    LedgerCapacity,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct Reason {
    pub code: ReasonCode,
    /// A bounded field/capability label, never raw diagnostics or credentials.
    pub subject: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct Decision {
    pub allowed: bool,
    pub reasons: Vec<Reason>,
    pub reservation: Option<Reservation>,
}
