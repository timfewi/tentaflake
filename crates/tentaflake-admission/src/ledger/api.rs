//! Dynamic API ownership in the existing admission ledger. These records grant
//! no filesystem or runtime authority until a newly claimed permit is returned.
//!
//! This first stopped-create foundation supports only nonexpiring installed
//! creation/issued grants. Operator abort releases only allocations whose exact
//! absence has been proven under the host fence. Production dynamic creation
//! still requires actual helper and recovery collector integration.
use super::*;
use crate::{HostPolicy, Instance, Isolation, StorageKind};

pub const MAX_OPERATIONS: usize = 4096;
const MAX_ENVIRONMENTS: usize = 1024;
const MAX_ACTION_GRANTS: usize = 4096;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "UPPERCASE")]
pub enum ApiMethod {
    Post,
    Put,
    Patch,
    Delete,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ApiKey {
    pub host: String,
    pub principal: String,
    pub epoch: String,
    pub method: ApiMethod,
    pub canonical_target: String,
    pub key: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ApiMutation {
    pub operation_id: String,
    pub key: ApiKey,
    /// Digest covers the exact accepted request, content type and preconditions.
    pub request_digest: String,
}

/// Supplied only after authentication and current installed-plan observation.
/// This is not deserializable client input. Reads still check published authority.
pub struct ApiContext {
    pub authority: PublishedAuthority,
    pub host: String,
    pub principal: String,
    pub epoch: String,
    pub now: u64,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
pub enum ApiAction {
    #[serde(rename = "environments.create")]
    EnvironmentsCreate,
    #[serde(rename = "environments.read")]
    EnvironmentsRead,
    #[serde(rename = "environments.start")]
    EnvironmentsStart,
    #[serde(rename = "environments.stop")]
    EnvironmentsStop,
    #[serde(rename = "workspaces.read")]
    WorkspacesRead,
    #[serde(rename = "workspaces.retain")]
    WorkspacesRetain,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields, tag = "kind", rename_all = "kebab-case")]
pub enum ApiTarget {
    Host { id: String },
    Environment { id: String },
    Workspace { id: String },
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ActionGrant {
    pub id: String,
    pub action: ApiAction,
    pub target: ApiTarget,
    pub expires_at: Option<u64>,
}

/// Reconstructed by the host from an installed creation template. No command,
/// path, credential or client-supplied runtime configuration is stored here.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct EnvironmentSpec {
    pub template_id: String,
    pub template_revision: String,
    pub name: String,
    pub image_id: String,
    pub agent_runtime: Option<String>,
    pub tools: Vec<String>,
    pub skills: Vec<String>,
    pub instance: Instance,
    pub workspace_id: String,
    pub grants: Vec<ActionGrant>,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum EnvironmentState {
    Provisioning,
    Stopped,
    Unknown,
    Aborted,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct EnvironmentRecord {
    pub host: String,
    pub spec: EnvironmentSpec,
    pub state: EnvironmentState,
    pub operation_id: String,
    pub created_at: u64,
    /// Changes only with this resource, never with an unrelated token login.
    pub revision: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct WorkspaceRecord {
    pub id: String,
    pub environment_id: String,
    pub owner_id: String,
    pub allocation_id: String,
    pub revision: u64,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum ApiOperationState {
    Queued,
    Unknown,
    Succeeded,
    Aborted,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ApiExecution {
    pub boot_id: String,
    pub supervisor: SupervisorIdentity,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ApiOperation {
    pub mutation: ApiMutation,
    pub authority: PublishedAuthority,
    pub environment_id: String,
    pub state: ApiOperationState,
    pub created_at: u64,
    pub updated_at: u64,
    pub queued_revision: u64,
    pub claimed_revision: Option<u64>,
    pub updated_revision: u64,
    pub execution: Option<ApiExecution>,
    /// Native allocation operation identities, not another accounting ledger.
    pub storage_operations: Vec<String>,
    /// Success identifies exact newly created resources after a lost response.
    pub result: Option<CreateResult>,
    #[serde(default)]
    pub abort: Option<CreateAbortReceipt>,
    #[serde(default)]
    pub recovery: Option<CreateRecoveryReceipt>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct CreateAbortReceipt {
    /// Exact original CAS for historical retries; no new release permission.
    pub expected_ledger_revision: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct CreateRecoveryReceipt {
    /// Exact original CAS for historical retries; never another observation.
    pub expected_ledger_revision: u64,
}

/// Trusted read-only observation, never deserialized API request data. Absent
/// means both final AND staging paths are absent, with no mount, loop mapping
/// or writer retaining the backing image. PresentStopped means the exact final
/// image/binding is verified, unmounted, and has no staging data or live writer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CreateAbortStorageState {
    Absent,
    PresentStopped,
}

pub struct CreateAbortStorageObservation {
    pub allocation_id: String,
    pub state: CreateAbortStorageState,
}

/// The observer holds the outer host fence and performs no mutating helpers.
/// Bounded read-only verification of the exact existing images is permitted.
/// Unknown requires proof that the exact stored supervisor and all descendants
/// cannot still dispatch. Queued has never had a permit and requires None.
pub struct CreateAbortObservation {
    pub fenced_execution: Option<ApiExecution>,
    pub allocations: [CreateAbortStorageObservation; 2],
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct CreateResult {
    pub environment_id: String,
    pub environment_revision: u64,
    pub workspace_id: String,
    pub workspace_revision: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ApiLookup {
    Found(Box<ApiOperation>),
    Absent,
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ApiQueued {
    New(Box<ApiOperation>),
    Existing(Box<ApiOperation>),
}

pub struct ApiSnapshot {
    pub grants_revision: u64,
    pub environments: Vec<EnvironmentRecord>,
    pub workspaces: Vec<WorkspaceRecord>,
    pub grants: Vec<ActionGrant>,
}

/// Current creation policy and exact proposed grants, authenticated by the host
/// while the ledger is locked. The callback must also prove space for BOTH new
/// allocations and reject any source instance/path/slot collision. It checks
/// the create grant belongs to the authenticated context's principal; resource
/// ownership and the exact issued grant targets are checked again by the ledger.
pub struct CreatePolicy {
    pub policy: HostPolicy,
    pub create_grant: ActionGrant,
    pub issued_grants: Vec<ActionGrant>,
}

#[derive(Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(super) struct ApiState {
    pub(super) operations: Vec<ApiOperation>,
    pub(super) environments: Vec<EnvironmentRecord>,
    pub(super) workspaces: Vec<WorkspaceRecord>,
    pub(super) grants_revision: u64,
}

#[must_use = "only a new claim permits helpers; dropping retains both charges and Unknown"]
pub struct CreationPermit<'a> {
    ledger: &'a Ledger,
    _lock: LedgerLock,
    state: Box<State>,
    operation: usize,
    environment: usize,
    failed: bool,
}

pub enum CreateClaim<'a> {
    New(CreationPermit<'a>),
    Existing(Box<ApiOperation>),
}

fn id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 96
        && value.as_bytes()[0].is_ascii_alphanumeric()
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b))
}

fn key_uuid(value: &str) -> bool {
    value.len() == 36
        && value.bytes().enumerate().all(|(i, b)| {
            if [8, 13, 18, 23].contains(&i) {
                b == b'-'
            } else {
                b.is_ascii_hexdigit()
            }
        })
}

fn boot_uuid(value: &str) -> bool {
    key_uuid(value)
        && !value.bytes().any(|b| b.is_ascii_uppercase())
        && value.bytes().any(|b| b.is_ascii_hexdigit() && b != b'0')
}

impl ApiKey {
    fn canonicalized(&self) -> Result<Self, LedgerError> {
        self.validate()?;
        let mut key = self.clone();
        key.key.make_ascii_lowercase();
        Ok(key)
    }

    fn validate(&self) -> Result<(), LedgerError> {
        if !id(&self.host)
            || !id(&self.principal)
            || !id(&self.epoch)
            || !key_uuid(&self.key)
            || self.canonical_target.len() > 4096
            || !self.canonical_target.starts_with("/api/v1/")
            || !self
                .canonical_target
                .bytes()
                .all(|b| b.is_ascii_graphic() && b != b'#')
        {
            return Err(LedgerError::Invalid("API operation identity"));
        }
        Ok(())
    }
}

impl ApiMutation {
    fn validate(&self) -> Result<(), LedgerError> {
        self.key.validate()?;
        if !id(&self.operation_id)
            || !self
                .request_digest
                .strip_prefix("sha256:")
                .is_some_and(operator_fingerprint)
        {
            return Err(LedgerError::Invalid("API operation digest"));
        }
        Ok(())
    }
}

impl ActionGrant {
    fn validate(&self) -> Result<(), LedgerError> {
        let target = match &self.target {
            ApiTarget::Host { id }
            | ApiTarget::Environment { id }
            | ApiTarget::Workspace { id } => id,
        };
        if !id(&self.id) || !id(target) || self.expires_at == Some(0) {
            return Err(LedgerError::Invalid("API action grant"));
        }
        Ok(())
    }
    fn active(&self, now: u64) -> bool {
        self.expires_at.is_none_or(|expiry| now < expiry)
    }
}

impl EnvironmentSpec {
    fn validate(&self) -> Result<(), LedgerError> {
        let mut ids = BTreeSet::new();
        if !id(&self.template_id)
            || !identifier(&self.template_revision)
            || !id(&self.image_id)
            || self.agent_runtime.as_ref().is_some_and(|v| !id(v))
            || !id(&self.instance.id)
            || !id(&self.instance.owner_id)
            || !id(&self.workspace_id)
            || self.instance.id == self.workspace_id
            || !identifier(&self.instance.revision)
            || self.name.is_empty()
            || self.name.chars().count() > 160
            || self.name.chars().any(char::is_control)
            || self.tools.len() > 64
            || self.skills.len() > 64
            || self.tools.iter().any(|v| !id(v) || !ids.insert((0, v)))
            || self.skills.iter().any(|v| !id(v) || !ids.insert((1, v)))
            || !self.instance.requested_capabilities.is_empty()
            || self.instance.isolation != Isolation::Container
            || self.grants.is_empty()
            || self.grants.len() > 128
        {
            return Err(LedgerError::Invalid("dynamic environment specification"));
        }
        let mut reasons = validate_reservations(&ReservationSnapshot {
            revision: 1,
            generation: "shape".into(),
            complete: true,
            entries: vec![Reservation {
                instance_id: self.instance.id.clone(),
                owner_id: self.instance.owner_id.clone(),
                instance_revision: self.instance.revision.clone(),
                state: ReservationState::Unknown,
                resources: self.instance.resources,
            }],
            storage: Vec::new(),
        });
        crate::storage_decision::validate_requirements(&self.instance, true, &mut reasons);
        if !reasons.is_empty() {
            return Err(LedgerError::Invalid("dynamic storage requirements"));
        }
        let mut grant_ids = BTreeSet::new();
        let mut actions = BTreeSet::new();
        for grant in &self.grants {
            grant.validate()?;
            let target = match grant.action {
                ApiAction::EnvironmentsRead
                | ApiAction::EnvironmentsStart
                | ApiAction::EnvironmentsStop => ApiTarget::Environment {
                    id: self.instance.id.clone(),
                },
                ApiAction::WorkspacesRead | ApiAction::WorkspacesRetain => ApiTarget::Workspace {
                    id: self.workspace_id.clone(),
                },
                ApiAction::EnvironmentsCreate => {
                    return Err(LedgerError::Invalid("inherited host creation grant"));
                }
            };
            if grant.expires_at.is_some()
                || grant.target != target
                || !grant_ids.insert(&grant.id)
                || !actions.insert(grant.action)
            {
                return Err(LedgerError::Invalid("dynamic grant target or duplicate"));
            }
        }
        Ok(())
    }
}

impl ApiExecution {
    fn validate(&self) -> Result<(), LedgerError> {
        let p = &self.supervisor;
        if !boot_uuid(&self.boot_id)
            || p.invocation_id.len() != 32
            || !p
                .invocation_id
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            || p.invocation_id.bytes().all(|b| b == b'0')
            || p.pid == 0
            || p.pid > i32::MAX as u32
            || p.start_time_ticks == 0
            || p.unit.len() <= 8
            || p.unit.len() > 255
            || !p.unit.ends_with(".service")
            || !p
                .unit
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"_.@:-".contains(&b))
        {
            return Err(LedgerError::Invalid("API supervisor identity"));
        }
        Ok(())
    }
}

impl State {
    fn api_context(&self, context: &ApiContext) -> Result<(), LedgerError> {
        context.authority.validate()?;
        if !id(&context.host) || !id(&context.principal) || !id(&context.epoch) || context.now == 0
        {
            return Err(LedgerError::Invalid("authenticated API context"));
        }
        if context.authority != self.published {
            return Err(LedgerError::StaleAuthority);
        }
        Ok(())
    }

    pub(super) fn dynamic_instance(&self, id: &str) -> bool {
        self.api
            .environments
            .iter()
            .any(|env| env.spec.instance.id == id)
    }

    pub(super) fn require_quiescent_api(&self) -> Result<(), LedgerError> {
        if self.api.operations.iter().any(|op| {
            matches!(
                op.state,
                ApiOperationState::Queued | ApiOperationState::Unknown
            )
        }) {
            return Err(LedgerError::NotQuiescent);
        }
        Ok(())
    }

    fn api_key_owner(&self, context: &ApiContext, key: &ApiKey) -> Result<(), LedgerError> {
        self.api_context(context)?;
        key.validate()?;
        if key.host != context.host || key.principal != context.principal {
            return Err(LedgerError::Conflict);
        }
        Ok(())
    }
}

impl Ledger {
    pub fn api_lookup(&self, context: &ApiContext, key: &ApiKey) -> Result<ApiLookup, LedgerError> {
        let _lock = self.lock(false)?;
        let state = self.read()?;
        let key = key.canonicalized()?;
        state.api_key_owner(context, &key)?;
        if let Some(op) = state
            .api
            .operations
            .iter()
            .find(|op| op.mutation.key == key)
        {
            return Ok(ApiLookup::Found(Box::new(op.clone())));
        }
        Ok(if key.epoch == context.epoch {
            ApiLookup::Absent
        } else {
            ApiLookup::Unknown
        })
    }

    pub fn api_operation(
        &self,
        context: &ApiContext,
        operation_id: &str,
    ) -> Result<Option<ApiOperation>, LedgerError> {
        let _lock = self.lock(false)?;
        let state = self.read()?;
        state.api_context(context)?;
        if !id(operation_id) {
            return Err(LedgerError::Invalid("API operation ID"));
        }
        Ok(state
            .api
            .operations
            .iter()
            .find(|op| {
                op.mutation.operation_id == operation_id
                    && op.mutation.key.host == context.host
                    && op.mutation.key.principal == context.principal
            })
            .cloned())
    }

    pub fn api_snapshot(&self, context: &ApiContext) -> Result<ApiSnapshot, LedgerError> {
        let _lock = self.lock(false)?;
        let state = self.read()?;
        state.api_context(context)?;
        let owned: Vec<_> = state
            .api
            .environments
            .iter()
            .filter(|env| {
                env.host == context.host && env.spec.instance.owner_id == context.principal
            })
            .collect();
        let grants: Vec<_> = owned
            .iter()
            .filter(|env| env.state != EnvironmentState::Aborted)
            .flat_map(|env| env.spec.grants.iter())
            .filter(|grant| grant.active(context.now))
            .cloned()
            .collect();
        let environments = owned
            .iter()
            .filter(|env| {
                grants.iter().any(|grant| {
                    grant.action == ApiAction::EnvironmentsRead
                        && grant.target
                            == (ApiTarget::Environment {
                                id: env.spec.instance.id.clone(),
                            })
                })
            })
            .map(|env| (*env).clone())
            .collect();
        let workspaces = state
            .api
            .workspaces
            .iter()
            .filter(|workspace| {
                workspace.owner_id == context.principal
                    && owned
                        .iter()
                        .any(|env| env.spec.workspace_id == workspace.id)
                    && grants.iter().any(|grant| {
                        grant.action == ApiAction::WorkspacesRead
                            && grant.target
                                == (ApiTarget::Workspace {
                                    id: workspace.id.clone(),
                                })
                    })
            })
            .cloned()
            .collect();
        Ok(ApiSnapshot {
            grants_revision: state.api.grants_revision,
            environments,
            workspaces,
            grants,
        })
    }

    /// Root maintenance view under the shared host fence. Not an HTTP list;
    /// removed/revoked owners must still participate in complete unit inventory.
    pub fn api_environments(
        &self,
        authority: &PublishedAuthority,
    ) -> Result<Vec<(EnvironmentRecord, ApiOperation)>, LedgerError> {
        let _lock = self.lock(false)?;
        let state = self.read()?;
        if *authority != state.published {
            return Err(LedgerError::StaleAuthority);
        }
        // This immutable creation-operation link remains separate from future
        // current runtime operations, so removing a template cannot hide its
        // original authority, supervisor, paths or owner from maintenance.
        Ok(state
            .api
            .environments
            .iter()
            .map(|environment| {
                let operation = state
                    .api
                    .operations
                    .iter()
                    .find(|op| op.mutation.operation_id == environment.operation_id)
                    .expect("validated creation operation");
                (environment.clone(), operation.clone())
            })
            .collect())
    }

    pub fn api_queue_create(
        &self,
        context: &ApiContext,
        mutation: &ApiMutation,
        spec: &EnvironmentSpec,
        expected_grants_revision: u64,
        fresh: impl FnOnce(
            &PublishedAuthority,
            &ReservationSnapshot,
            &EnvironmentSpec,
        ) -> Result<CreatePolicy, String>,
    ) -> Result<ApiQueued, LedgerError> {
        let _lock = self.lock(false)?;
        let mut state = self.read()?;
        mutation.validate()?;
        let mut mutation = mutation.clone();
        mutation.key = mutation.key.canonicalized()?;
        state.api_key_owner(context, &mutation.key)?;
        if let Some(op) = state
            .api
            .operations
            .iter()
            .find(|op| op.mutation.key == mutation.key)
        {
            if op.mutation.request_digest != mutation.request_digest {
                return Err(LedgerError::Conflict);
            }
            return Ok(ApiQueued::Existing(Box::new(op.clone())));
        }
        state.require_stable()?;
        spec.validate()?;
        if mutation.key.epoch != context.epoch
            || expected_grants_revision != state.api.grants_revision
        {
            return Err(LedgerError::StaleRevision);
        }
        if mutation.key.method != ApiMethod::Post
            || mutation.key.canonical_target != "/api/v1/environments"
            || spec.instance.owner_id != context.principal
        {
            return Err(LedgerError::Invalid("create request binding"));
        }
        if state.api.operations.len() >= MAX_OPERATIONS
            || state.api.environments.len() >= MAX_ENVIRONMENTS
            || state.storage.len() + 2 > MAX_STORAGE_ALLOCATIONS
            || state
                .api
                .environments
                .iter()
                .map(|env| env.spec.grants.len())
                .sum::<usize>()
                + spec.grants.len()
                > MAX_ACTION_GRANTS
        {
            return Err(LedgerError::Full);
        }
        if state
            .api
            .operations
            .iter()
            .any(|op| op.mutation.operation_id == mutation.operation_id)
            || state.dynamic_instance(&spec.instance.id)
            || state.dynamic_instance(&spec.workspace_id)
            || state
                .api
                .workspaces
                .iter()
                .any(|w| w.id == spec.workspace_id || w.id == spec.instance.id)
            || state.storage.iter().any(|row| {
                row.allocation
                    .binding
                    .as_ref()
                    .is_some_and(|binding| binding.instance_id == spec.instance.id)
            })
            || state
                .receipts
                .iter()
                .any(|r| r.request.instance_id == spec.instance.id)
            || spec
                .instance
                .storage
                .iter()
                .any(|wanted| state.storage.iter().any(|r| r.allocation.id == wanted.id))
            || state
                .api
                .environments
                .iter()
                .flat_map(|env| &env.spec.grants)
                .any(|old| spec.grants.iter().any(|new| new.id == old.id))
        {
            return Err(LedgerError::Conflict);
        }
        let policy =
            fresh(&state.published, &state.snapshot(), spec).map_err(|_| LedgerError::Context)?;
        state.check_policy(&policy.policy)?;
        let mut limits = Vec::new();
        crate::decision::check_instance_limits(&policy.policy, &spec.instance, &mut limits);
        if !limits.is_empty() {
            return Err(LedgerError::Invalid(
                "dynamic instance exceeds host ceiling",
            ));
        }
        policy.create_grant.validate()?;
        if policy.create_grant.action != ApiAction::EnvironmentsCreate
            || policy.create_grant.target
                != (ApiTarget::Host {
                    id: context.host.clone(),
                })
            || policy.create_grant.expires_at.is_some()
            || !policy.create_grant.active(context.now)
            || policy.issued_grants != spec.grants
            || spec
                .grants
                .iter()
                .any(|g| g.id == policy.create_grant.id || !g.active(context.now))
        {
            return Err(LedgerError::Conflict);
        }
        let mut storage_operations = Vec::new();
        for requirement in &spec.instance.storage {
            let operation_id = format!(
                "{}:{}",
                mutation.operation_id,
                match requirement.kind {
                    StorageKind::Workspace => "workspace",
                    StorageKind::State => "state",
                }
            );
            if state.native_operation_exists(&operation_id)
                || state.api_operation_exists(&operation_id)
                || state.native_operation_exists(&mutation.operation_id)
            {
                return Err(LedgerError::Conflict);
            }
            let request = StorageRequest {
                request: AdmissionRequest {
                    schema_version: SCHEMA_VERSION,
                    operation_id: operation_id.clone(),
                    instance_id: spec.instance.id.clone(),
                    principal_id: context.principal.clone(),
                    expected: crate::ExpectedRevisions {
                        policy: policy.policy.revision.clone(),
                        instance: spec.instance.revision.clone(),
                        ledger: state.revision,
                    },
                },
                allocation_id: requirement.id.clone(),
            };
            let snapshot = StorageAdmissionSnapshot {
                schema_version: SCHEMA_VERSION,
                policy: policy.policy.clone(),
                instance: spec.instance.clone(),
                reservations: state.snapshot(),
            };
            let decision = decide_storage(&snapshot, &request);
            if !decision.allowed {
                return Err(LedgerError::StorageDenied(Box::new(decision)));
            }
            let allocation = decision
                .allocation
                .ok_or(LedgerError::Invalid("create allocation"))?;
            state.advance()?;
            state.storage.push(StorageReceipt {
                request: Some(request),
                release: None,
                allocation,
                claimed_revision: state.revision,
                updated_revision: state.revision,
            });
            storage_operations.push(operation_id);
        }
        state.advance()?;
        let op = ApiOperation {
            mutation: mutation.clone(),
            authority: state.published.clone(),
            environment_id: spec.instance.id.clone(),
            state: ApiOperationState::Queued,
            created_at: context.now,
            updated_at: context.now,
            queued_revision: state.revision,
            claimed_revision: None,
            updated_revision: state.revision,
            execution: None,
            storage_operations,
            result: None,
            abort: None,
            recovery: None,
        };
        state.api.workspaces.push(WorkspaceRecord {
            id: spec.workspace_id.clone(),
            environment_id: spec.instance.id.clone(),
            owner_id: context.principal.clone(),
            allocation_id: spec
                .instance
                .storage
                .iter()
                .find(|r| r.kind == StorageKind::Workspace)
                .expect("validated pair")
                .id
                .clone(),
            revision: state.revision,
        });
        state.api.environments.push(EnvironmentRecord {
            host: context.host.clone(),
            spec: spec.clone(),
            state: EnvironmentState::Provisioning,
            operation_id: mutation.operation_id.clone(),
            created_at: context.now,
            revision: state.revision,
        });
        state.api.grants_revision = state.revision;
        state.api.operations.push(op.clone());
        self.write(&state)?;
        Ok(ApiQueued::New(Box::new(op)))
    }

    /// Authenticate the current installed template, exact systemd invocation,
    /// creation authorization and physical pool again under this lock. The
    /// callback must fence other helpers and prove BOTH final/staging slots
    /// absent. An old Queued row alone is never evidence of absence or readiness.
    pub fn api_claim_create(
        &self,
        context: &ApiContext,
        operation_id: &str,
        execution: &ApiExecution,
        fresh: impl FnOnce(
            &PublishedAuthority,
            &ReservationSnapshot,
            &EnvironmentSpec,
            &ApiExecution,
        ) -> Result<(), String>,
    ) -> Result<CreateClaim<'_>, LedgerError> {
        let lock = self.lock(false)?;
        let mut state = self.read()?;
        state.api_context(context)?;
        let operation = state
            .api
            .operations
            .iter()
            .position(|op| {
                op.mutation.operation_id == operation_id
                    && op.mutation.key.host == context.host
                    && op.mutation.key.principal == context.principal
            })
            .ok_or(LedgerError::Conflict)?;
        if state.api.operations[operation].state != ApiOperationState::Queued {
            return Ok(CreateClaim::Existing(Box::new(
                state.api.operations[operation].clone(),
            )));
        }
        state.require_stable()?;
        execution.validate()?;
        // This is a root-owned supervisor handoff, not another HTTP mutation.
        // A restarted HTTP epoch does not invalidate already accepted work.
        if context.now < state.api.operations[operation].updated_at {
            return Err(LedgerError::Invalid("API observation time"));
        }
        let environment = state
            .api
            .environments
            .iter()
            .position(|env| env.operation_id == operation_id)
            .ok_or(LedgerError::Invalid("create environment"))?;
        if state.api.environments[environment]
            .spec
            .grants
            .iter()
            .any(|grant| !grant.active(context.now))
        {
            return Err(LedgerError::Conflict);
        }
        fresh(
            &state.published,
            &state.snapshot(),
            &state.api.environments[environment].spec,
            execution,
        )
        .map_err(|_| LedgerError::Context)?;
        state.advance()?;
        let op = &mut state.api.operations[operation];
        op.state = ApiOperationState::Unknown;
        op.execution = Some(execution.clone());
        op.claimed_revision = Some(state.revision);
        op.updated_at = context.now;
        op.updated_revision = state.revision;
        state.api.environments[environment].state = EnvironmentState::Unknown;
        state.api.environments[environment].revision = state.revision;
        self.write(&state)?;
        Ok(CreateClaim::New(CreationPermit {
            ledger: self,
            _lock: lock,
            state: Box::new(state),
            operation,
            environment,
            failed: false,
        }))
    }
}

impl CreationPermit<'_> {
    pub fn operation(&self) -> &ApiOperation {
        &self.state.api.operations[self.operation]
    }
    pub fn environment(&self) -> &EnvironmentSpec {
        &self.state.api.environments[self.environment].spec
    }

    /// Calls the bounded helper exactly once for a previously uncreated slot.
    /// The host proves final/staging absence and observes the exact result while
    /// retaining its inherited outer fence. Failure poisons this permit.
    pub fn provision(
        &mut self,
        kind: StorageKind,
        create: impl FnOnce(&StorageAllocation) -> Result<(), String>,
    ) -> Result<StorageReceipt, LedgerError> {
        if self.failed {
            return Err(LedgerError::Context);
        }
        let wanted = self
            .environment()
            .instance
            .storage
            .iter()
            .find(|r| r.kind == kind)
            .ok_or(LedgerError::Invalid("create storage kind"))?;
        let index = self
            .state
            .storage
            .iter()
            .position(|r| r.allocation.id == wanted.id)
            .ok_or(LedgerError::Invalid("create allocation"))?;
        if self.state.storage[index].allocation.state != StorageState::Unknown {
            return Err(LedgerError::Conflict);
        }
        self.failed = true;
        create(&self.state.storage[index].allocation).map_err(|_| LedgerError::Context)?;
        let receipt = self
            .ledger
            .record_storage(&mut self.state, index, StorageState::Present)?;
        self.failed = false;
        Ok(receipt)
    }

    /// The host has stopped/unmounted both prepared volumes and fenced every
    /// helper before success. Present is image identity, not mounted readiness.
    pub fn complete(mut self, now: u64) -> Result<ApiOperation, LedgerError> {
        if self.failed || now < self.operation().updated_at {
            return Err(LedgerError::Context);
        }
        if self.environment().instance.storage.iter().any(|wanted| {
            !self.state.storage.iter().any(|r| {
                r.allocation.id == wanted.id && r.allocation.state == StorageState::Present
            })
        }) {
            return Err(LedgerError::NotQuiescent);
        }
        self.state
            .finish_create(self.operation, self.environment, now)?;
        self.ledger.write(&self.state)?;
        Ok(self.state.api.operations[self.operation].clone())
    }
}

impl Ledger {
    /// Explicit operator recovery after the old supervisor and descendants are
    /// fenced. BOTH native rows must already be Present: filesystem validity
    /// alone cannot prove ownership initialization completed. The observer must
    /// verify both exact existing images are unmounted, with no staging data or
    /// remaining creator/writer. Bounded read-only verification is permitted;
    /// mutating helpers, creation, deletion and repair are forbidden. Failure
    /// keeps both charges and Unknown. Exact terminal retries return history
    /// without observing again; no recovery path returns a permit.
    pub fn api_recover_create(
        &self,
        authority: &PublishedAuthority,
        operation_id: &str,
        expected_ledger_revision: u64,
        now: u64,
        observe: impl FnOnce(
            &ApiOperation,
            &EnvironmentSpec,
            &[StorageAllocation],
        ) -> Result<(), String>,
    ) -> Result<ApiOperation, LedgerError> {
        let _lock = self.lock(false)?;
        let mut state = self.read()?;
        authority.validate()?;
        if *authority != state.published {
            return Err(LedgerError::StaleAuthority);
        }
        if !id(operation_id) || now == 0 {
            return Err(LedgerError::Invalid("API recovery identity or time"));
        }
        let operation = state
            .api
            .operations
            .iter()
            .position(|op| op.mutation.operation_id == operation_id)
            .ok_or(LedgerError::Conflict)?;
        let op = &state.api.operations[operation];
        if let Some(recovery) = &op.recovery {
            if recovery.expected_ledger_revision != expected_ledger_revision {
                return Err(LedgerError::StaleRevision);
            }
            return Ok(op.clone());
        }
        state.require_stable()?;
        if state.revision != expected_ledger_revision {
            return Err(LedgerError::StaleRevision);
        }
        if op.state != ApiOperationState::Unknown || now < op.updated_at {
            return Err(LedgerError::Conflict);
        }
        let environment = state
            .api
            .environments
            .iter()
            .position(|env| env.operation_id == operation_id)
            .ok_or(LedgerError::Invalid("create environment"))?;
        let spec = &state.api.environments[environment].spec;
        let allocations: Vec<_> = spec
            .instance
            .storage
            .iter()
            .map(|wanted| {
                state
                    .storage
                    .iter()
                    .find(|row| row.allocation.id == wanted.id)
                    .expect("validated allocation")
                    .allocation
                    .clone()
            })
            .collect();
        if allocations
            .iter()
            .any(|row| row.state != StorageState::Present)
        {
            return Err(LedgerError::NotQuiescent);
        }
        observe(op, spec, &allocations).map_err(|_| LedgerError::Context)?;
        state.finish_create(operation, environment, now)?;
        state.api.operations[operation].recovery = Some(CreateRecoveryReceipt {
            expected_ledger_revision,
        });
        self.write(&state)?;
        Ok(state.api.operations[operation].clone())
    }

    /// Terminal operator cleanup, never dispatch authority. The caller holds the
    /// outer host fence and authenticates the current installed maintenance plan.
    /// The observer must fence the exact recorded supervisor and all descendants,
    /// then inspect BOTH original slots (including staging, mounts, loops and
    /// open writers). It must not create, delete, unmount or run mutating helpers;
    /// bounded read-only verification is permitted. Failure
    /// leaves all charges unchanged. A queued operation has never had a permit,
    /// but still requires exact absence; Unknown may retain stopped images.
    pub fn api_abort_create(
        &self,
        authority: &PublishedAuthority,
        operation_id: &str,
        expected_ledger_revision: u64,
        now: u64,
        observe: impl FnOnce(
            &ApiOperation,
            &EnvironmentSpec,
            &[StorageReceipt],
        ) -> Result<CreateAbortObservation, String>,
    ) -> Result<ApiOperation, LedgerError> {
        let _lock = self.lock(false)?;
        let mut state = self.read()?;
        authority.validate()?;
        if *authority != state.published {
            return Err(LedgerError::StaleAuthority);
        }
        if !id(operation_id) || now == 0 {
            return Err(LedgerError::Invalid("API abort identity or time"));
        }
        let operation = state
            .api
            .operations
            .iter()
            .position(|op| op.mutation.operation_id == operation_id)
            .ok_or(LedgerError::Conflict)?;
        let op = &state.api.operations[operation];
        if let Some(abort) = &op.abort {
            if abort.expected_ledger_revision != expected_ledger_revision {
                return Err(LedgerError::StaleRevision);
            }
            return Ok(op.clone());
        }
        state.require_stable()?;
        if state.revision != expected_ledger_revision {
            return Err(LedgerError::StaleRevision);
        }
        if !matches!(
            op.state,
            ApiOperationState::Queued | ApiOperationState::Unknown
        ) || now < op.updated_at
        {
            return Err(LedgerError::Conflict);
        }
        let environment = state
            .api
            .environments
            .iter()
            .position(|env| env.operation_id == operation_id)
            .expect("validated environment");
        let spec = &state.api.environments[environment].spec;
        let indexes: Vec<_> = spec
            .instance
            .storage
            .iter()
            .map(|wanted| {
                state
                    .storage
                    .iter()
                    .position(|row| row.allocation.id == wanted.id)
                    .expect("validated allocation")
            })
            .collect();
        let rows: Vec<_> = indexes
            .iter()
            .map(|index| state.storage[*index].clone())
            .collect();
        let observation = observe(op, spec, &rows).map_err(|_| LedgerError::Context)?;
        if observation.fenced_execution != op.execution {
            return Err(LedgerError::Context);
        }
        let mut seen = BTreeSet::new();
        for observed in &observation.allocations {
            if !seen.insert(&observed.allocation_id)
                || !rows
                    .iter()
                    .any(|row| row.allocation.id == observed.allocation_id)
                || (op.state == ApiOperationState::Queued
                    && observed.state != CreateAbortStorageState::Absent)
            {
                return Err(LedgerError::Context);
            }
        }
        // Validate the whole pair before changing even the in-memory state.
        // A single durable write commits releases, retained images and grants.
        for index in indexes {
            let observed = observation
                .allocations
                .iter()
                .find(|observed| observed.allocation_id == state.storage[index].allocation.id)
                .expect("validated complete pair");
            match observed.state {
                CreateAbortStorageState::Absent => {
                    state.advance()?;
                    state.storage[index].updated_revision = state.revision;
                    state.storage[index].release = Some(StorageRelease {
                        operation_id: operation_id.into(),
                        released_revision: state.revision,
                    });
                }
                CreateAbortStorageState::PresentStopped => {
                    if state.storage[index].allocation.state != StorageState::Present {
                        state.advance()?;
                        state.storage[index].allocation.state = StorageState::Present;
                        state.storage[index].updated_revision = state.revision;
                    }
                }
            }
        }
        state.advance()?;
        let env = &mut state.api.environments[environment];
        env.state = EnvironmentState::Aborted;
        env.revision = state.revision;
        let workspace_id = env.spec.workspace_id.clone();
        state
            .api
            .workspaces
            .iter_mut()
            .find(|row| row.id == workspace_id)
            .expect("validated workspace")
            .revision = state.revision;
        state.api.grants_revision = state.revision;
        let op = &mut state.api.operations[operation];
        op.state = ApiOperationState::Aborted;
        op.updated_at = now;
        op.updated_revision = state.revision;
        op.abort = Some(CreateAbortReceipt {
            expected_ledger_revision,
        });
        self.write(&state)?;
        Ok(state.api.operations[operation].clone())
    }
}

impl State {
    fn finish_create(
        &mut self,
        operation: usize,
        environment: usize,
        now: u64,
    ) -> Result<(), LedgerError> {
        self.advance()?;
        let env = &mut self.api.environments[environment];
        env.state = EnvironmentState::Stopped;
        env.revision = self.revision;
        let workspace = self
            .api
            .workspaces
            .iter_mut()
            .find(|w| w.id == env.spec.workspace_id)
            .expect("validated workspace");
        workspace.revision = self.revision;
        let op = &mut self.api.operations[operation];
        op.state = ApiOperationState::Succeeded;
        op.updated_at = now;
        op.updated_revision = self.revision;
        op.result = Some(CreateResult {
            environment_id: env.spec.instance.id.clone(),
            environment_revision: env.revision,
            workspace_id: workspace.id.clone(),
            workspace_revision: workspace.revision,
        });
        Ok(())
    }

    pub(super) fn native_operation_exists(&self, operation: &str) -> bool {
        self.receipts
            .iter()
            .any(|r| r.request.operation_id == operation)
            || self.storage.iter().any(|r| {
                r.request
                    .as_ref()
                    .is_some_and(|r| r.request.operation_id == operation)
            })
            || self
                .publications
                .iter()
                .any(|r| r.request.operation_id == operation)
    }

    pub(super) fn api_operation_exists(&self, operation: &str) -> bool {
        self.api
            .operations
            .iter()
            .any(|r| r.mutation.operation_id == operation)
    }

    pub(super) fn dynamic_allocation(&self, allocation: &str) -> bool {
        self.api.environments.iter().any(|env| {
            env.spec
                .instance
                .storage
                .iter()
                .any(|slot| slot.id == allocation)
        })
    }

    pub(super) fn validate_api(&self, revisions: &mut BTreeSet<u64>) -> Result<(), LedgerError> {
        if self.api.operations.len() > MAX_OPERATIONS
            || self.api.environments.len() > MAX_ENVIRONMENTS
            || self.api.operations.len() != self.api.environments.len()
            || self.api.workspaces.len() != self.api.environments.len()
            || self
                .api
                .environments
                .iter()
                .map(|env| env.spec.grants.len())
                .sum::<usize>()
                > MAX_ACTION_GRANTS
            || self.api.grants_revision
                != self
                    .api
                    .operations
                    .iter()
                    .map(|op| {
                        if op.state == ApiOperationState::Aborted {
                            op.updated_revision
                        } else {
                            op.queued_revision
                        }
                    })
                    .max()
                    .unwrap_or(0)
        {
            return Err(LedgerError::Invalid("API inventory or grant revision"));
        }
        let mut keys = BTreeSet::new();
        let mut operations = BTreeSet::new();
        let mut resources = BTreeSet::new();
        let mut grants = BTreeSet::new();
        let mut slots = BTreeSet::new();
        let mut previous_queue = 1;
        for op in &self.api.operations {
            op.mutation.validate()?;
            op.authority.validate()?;
            // The exact full key scope is retained; a UUID is not globally
            // authoritative across principals, hosts, methods or epochs.
            let key = serde_json::to_string(&op.mutation.key)
                .map_err(|_| LedgerError::Invalid("API key"))?;
            if op.mutation.key.key.bytes().any(|b| b.is_ascii_uppercase())
                || !keys.insert(key)
                || !operations.insert(&op.mutation.operation_id)
                || self.native_operation_exists(&op.mutation.operation_id)
                || op.mutation.key.method != ApiMethod::Post
                || op.mutation.key.canonical_target != "/api/v1/environments"
                || op.created_at == 0
                || op.updated_at < op.created_at
                || op.queued_revision < 4
                || op.queued_revision <= previous_queue
                || op.queued_revision > op.updated_revision
                || op.updated_revision > self.revision
                || op.authority.generation != self.published.generation
                || (op.authority != self.published
                    && !self
                        .publications
                        .iter()
                        .any(|p| p.request.expected_authority == op.authority))
            {
                return Err(LedgerError::Invalid("API operation identity or history"));
            }
            previous_queue = op.queued_revision;
            let own_revisions: BTreeSet<_> = [
                Some(op.queued_revision),
                op.claimed_revision,
                Some(op.updated_revision),
            ]
            .into_iter()
            .flatten()
            .collect();
            if own_revisions.iter().any(|rev| !revisions.insert(*rev)) {
                return Err(LedgerError::Invalid("API revision collision"));
            }
            let env = self
                .api
                .environments
                .iter()
                .find(|env| env.operation_id == op.mutation.operation_id)
                .ok_or(LedgerError::Invalid("API environment missing"))?;
            env.spec.validate()?;
            let spec = &env.spec;
            if env.host != op.mutation.key.host
                || spec.instance.owner_id != op.mutation.key.principal
                || spec.instance.id != op.environment_id
                || env.created_at != op.created_at
                || env.revision != op.updated_revision
                || !resources.insert(&spec.instance.id)
                || !resources.insert(&spec.workspace_id)
                || self
                    .receipts
                    .iter()
                    .any(|r| r.request.instance_id == spec.instance.id)
                || spec
                    .grants
                    .iter()
                    .any(|grant| !grants.insert(&grant.id) || !grant.active(op.created_at))
            {
                return Err(LedgerError::Invalid("API resource ownership or revision"));
            }
            let workspace = self
                .api
                .workspaces
                .iter()
                .find(|row| row.id == spec.workspace_id)
                .ok_or(LedgerError::Invalid("API workspace missing"))?;
            if workspace.environment_id != spec.instance.id
                || workspace.owner_id != spec.instance.owner_id
                || workspace.allocation_id
                    != spec
                        .instance
                        .storage
                        .iter()
                        .find(|slot| slot.kind == StorageKind::Workspace)
                        .expect("validated requirements")
                        .id
                || workspace.revision
                    != if matches!(
                        op.state,
                        ApiOperationState::Succeeded | ApiOperationState::Aborted
                    ) {
                        op.updated_revision
                    } else {
                        op.queued_revision
                    }
            {
                return Err(LedgerError::Invalid("API workspace binding"));
            }
            if op.storage_operations.len() != 2 {
                return Err(LedgerError::Invalid("API allocation references"));
            }
            if (op.state == ApiOperationState::Aborted) != op.abort.is_some() {
                return Err(LedgerError::Invalid("API abort outcome"));
            }
            if let Some(recovery) = &op.recovery
                && (op.state != ApiOperationState::Succeeded
                    || op
                        .claimed_revision
                        .is_none_or(|claim| recovery.expected_ledger_revision <= claim)
                    || recovery.expected_ledger_revision.checked_add(1)
                        != Some(op.updated_revision))
            {
                return Err(LedgerError::Invalid("API recovery outcome or revision"));
            }
            let mut all_present = true;
            let mut all_released = true;
            let mut abort_updates = BTreeSet::new();
            for (offset, (wanted, operation)) in spec
                .instance
                .storage
                .iter()
                .zip(&op.storage_operations)
                .enumerate()
            {
                let row = self
                    .storage
                    .iter()
                    .find(|row| row.allocation.id == wanted.id)
                    .ok_or(LedgerError::Invalid("API allocation missing"))?;
                let request = row
                    .request
                    .as_ref()
                    .ok_or(LedgerError::Invalid("API imported allocation"))?;
                let expected_operation = format!(
                    "{}:{}",
                    op.mutation.operation_id,
                    match wanted.kind {
                        StorageKind::Workspace => "workspace",
                        StorageKind::State => "state",
                    }
                );
                if !slots.insert(&wanted.id)
                    || *operation != expected_operation
                    || request.request.operation_id != *operation
                    || request.request.instance_id != spec.instance.id
                    || request.request.principal_id != spec.instance.owner_id
                    || request.request.expected.instance != spec.instance.revision
                    || request.request.expected.policy != op.authority.revision
                    || row.claimed_revision != op.queued_revision - 2 + offset as u64
                    || row.allocation.kind != wanted.kind
                    || row.allocation.bytes != wanted.kind.bytes(spec.instance.resources)
                    || !row.allocation.binding.as_ref().is_some_and(|binding| {
                        binding.instance_id == spec.instance.id
                            && binding.owner_id == spec.instance.owner_id
                            && binding.revision == wanted.binding_revision
                    })
                    || (row.release.is_none()
                        && row.allocation.state == StorageState::Unknown
                        && row.updated_revision != row.claimed_revision)
                    || (row.allocation.state == StorageState::Present
                        && op
                            .claimed_revision
                            .is_none_or(|claim| row.updated_revision <= claim))
                    || (matches!(
                        op.state,
                        ApiOperationState::Succeeded | ApiOperationState::Aborted
                    ) && row.updated_revision >= op.updated_revision)
                    || (op.state == ApiOperationState::Queued
                        && (row.allocation.state != StorageState::Unknown
                            || row.updated_revision != row.claimed_revision))
                {
                    return Err(LedgerError::Invalid("API native allocation binding"));
                }
                if let Some(release) = &row.release {
                    let abort = op
                        .abort
                        .as_ref()
                        .ok_or(LedgerError::Invalid("release without API abort"))?;
                    if release.operation_id != op.mutation.operation_id
                        || release.released_revision <= abort.expected_ledger_revision
                    {
                        return Err(LedgerError::Invalid("API release ownership"));
                    }
                } else if op.state == ApiOperationState::Aborted
                    && row.allocation.state != StorageState::Present
                {
                    return Err(LedgerError::Invalid("unresolved aborted allocation"));
                }
                if let Some(abort) = &op.abort
                    && row.updated_revision > abort.expected_ledger_revision
                    && !abort_updates.insert(row.updated_revision)
                {
                    return Err(LedgerError::Invalid(
                        "API abort allocation revision collision",
                    ));
                }
                all_present &=
                    row.release.is_none() && row.allocation.state == StorageState::Present;
                all_released &= row.release.is_some();
            }
            if matches!(
                op.state,
                ApiOperationState::Queued | ApiOperationState::Unknown
            ) && (self.pending().is_some() || op.authority != self.published)
            {
                return Err(LedgerError::Invalid("unresolved API publication"));
            }
            match op.state {
                ApiOperationState::Aborted => {
                    let abort = op.abort.as_ref().expect("validated abort outcome");
                    if abort.expected_ledger_revision
                        < op.claimed_revision.unwrap_or(op.queued_revision)
                        || abort.expected_ledger_revision >= op.updated_revision
                        || op.result.is_some()
                        || env.state != EnvironmentState::Aborted
                    {
                        return Err(LedgerError::Invalid("API abort revision or result"));
                    }
                    let mut expected = abort.expected_ledger_revision;
                    for update in &abort_updates {
                        expected = expected
                            .checked_add(1)
                            .ok_or(LedgerError::Invalid("API abort revision overflow"))?;
                        if *update != expected {
                            return Err(LedgerError::Invalid("API abort transaction revisions"));
                        }
                    }
                    if expected.checked_add(1) != Some(op.updated_revision) {
                        return Err(LedgerError::Invalid("API abort terminal revision"));
                    }
                    match (&op.execution, op.claimed_revision) {
                        (None, None) if all_released => {}
                        (Some(execution), Some(claim))
                            if claim > op.queued_revision
                                && claim <= abort.expected_ledger_revision =>
                        {
                            execution.validate()?
                        }
                        _ => return Err(LedgerError::Invalid("API abort dispatch identity")),
                    }
                }
                ApiOperationState::Queued => {
                    if op.execution.is_some()
                        || op.claimed_revision.is_some()
                        || op.result.is_some()
                        || op.updated_revision != op.queued_revision
                        || env.state != EnvironmentState::Provisioning
                    {
                        return Err(LedgerError::Invalid("queued API operation"));
                    }
                }
                ApiOperationState::Unknown | ApiOperationState::Succeeded => {
                    op.execution
                        .as_ref()
                        .ok_or(LedgerError::Invalid("API dispatch identity missing"))?
                        .validate()?;
                    let claim = op
                        .claimed_revision
                        .ok_or(LedgerError::Invalid("API dispatch revision missing"))?;
                    if claim <= op.queued_revision || claim > op.updated_revision {
                        return Err(LedgerError::Invalid("API dispatch revision"));
                    }
                    if op.state == ApiOperationState::Unknown {
                        if op.updated_revision != claim
                            || env.state != EnvironmentState::Unknown
                            || op.result.is_some()
                        {
                            return Err(LedgerError::Invalid("unknown API result"));
                        }
                    } else if claim >= op.updated_revision
                        || env.state != EnvironmentState::Stopped
                        || !all_present
                        || op.result
                            != Some(CreateResult {
                                environment_id: spec.instance.id.clone(),
                                environment_revision: env.revision,
                                workspace_id: workspace.id.clone(),
                                workspace_revision: workspace.revision,
                            })
                    {
                        return Err(LedgerError::Invalid("successful API result"));
                    }
                }
            }
        }
        if self
            .storage
            .iter()
            .any(|row| row.release.is_some() && !slots.contains(&row.allocation.id))
        {
            return Err(LedgerError::Invalid("unlinked storage release"));
        }
        if self.storage.iter().any(|row| {
            row.allocation
                .binding
                .as_ref()
                .is_some_and(|binding| self.dynamic_instance(&binding.instance_id))
                && !slots.contains(&row.allocation.id)
        }) {
            return Err(LedgerError::Invalid("unlinked dynamic allocation"));
        }
        Ok(())
    }
}
