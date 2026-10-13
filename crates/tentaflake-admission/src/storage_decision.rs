//! Persistent allocation decisions. Filesystem observation and a durable permit
//! remain the host's responsibility; a pure allow never authorizes image creation.
use crate::decision::{
    authority_shape, check_expected, check_policy, identifier, normalize, reason,
};
use crate::*;
use std::collections::BTreeSet;

fn subject(kind: StorageKind) -> &'static str {
    match kind {
        StorageKind::Workspace => "workspaceBytes",
        StorageKind::State => "stateBytes",
    }
}

pub(crate) fn validate_requirements(instance: &Instance, startup: bool, reasons: &mut Vec<Reason>) {
    if instance.storage.len() > 2 || (startup && instance.storage.len() != 2) {
        reason(reasons, ReasonCode::InvalidStorage, "requirements");
        return;
    }
    let mut ids = BTreeSet::new();
    let mut kinds = BTreeSet::new();
    for requirement in &instance.storage {
        if !identifier(&requirement.id)
            || !identifier(&requirement.binding_revision)
            || requirement.kind.bytes(instance.resources) < MIN_STORAGE_BYTES
        {
            reason(reasons, ReasonCode::InvalidStorage, "requirement");
        }
        if !ids.insert(&requirement.id) || !kinds.insert(requirement.kind) {
            reason(reasons, ReasonCode::DuplicateStorage, "requirements");
        }
    }
}

/// Shape only: uncertain, orphaned and over-budget inventory must stay loadable.
pub(crate) fn validate_allocations(allocations: &[StorageAllocation], reasons: &mut Vec<Reason>) {
    if allocations.len() > MAX_STORAGE_ALLOCATIONS {
        reason(reasons, ReasonCode::LedgerCapacity, "storage");
        return;
    }
    let mut ids = BTreeSet::new();
    for allocation in allocations {
        if !identifier(&allocation.id)
            || allocation.bytes == 0
            || allocation.binding.as_ref().is_some_and(|binding| {
                allocation.bytes < MIN_STORAGE_BYTES
                    || !identifier(&binding.instance_id)
                    || !identifier(&binding.owner_id)
                    || !identifier(&binding.revision)
            })
        {
            reason(reasons, ReasonCode::InvalidStorage, "allocation");
        }
        if !ids.insert(&allocation.id) {
            reason(reasons, ReasonCode::DuplicateStorage, "allocation");
        }
    }
}

fn allocation_resources(allocation: &StorageAllocation) -> Resources {
    match allocation.kind {
        StorageKind::Workspace => Resources {
            workspace_bytes: allocation.bytes,
            ..Resources::default()
        },
        StorageKind::State => Resources {
            state_bytes: allocation.bytes,
            ..Resources::default()
        },
    }
}

pub(crate) fn allocated_resources(allocations: &[StorageAllocation]) -> Option<Resources> {
    allocations
        .iter()
        .try_fold(Resources::default(), |sum, allocation| {
            sum.checked_add(allocation_resources(allocation))
        })
}

fn matches(
    instance: &Instance,
    requirement: &StorageRequirement,
    allocation: &StorageAllocation,
) -> bool {
    allocation.id == requirement.id
        && allocation.kind == requirement.kind
        && allocation.bytes == requirement.kind.bytes(instance.resources)
        && allocation.binding.as_ref().is_some_and(|binding| {
            binding.instance_id == instance.id
                && binding.owner_id == instance.owner_id
                && binding.revision == requirement.binding_revision
        })
}

pub(crate) fn check_start_storage(
    instance: &Instance,
    allocations: &[StorageAllocation],
    reasons: &mut Vec<Reason>,
) {
    for requirement in &instance.storage {
        match allocations
            .iter()
            .find(|allocation| allocation.id == requirement.id)
        {
            None => reason(
                reasons,
                ReasonCode::MissingStorage,
                subject(requirement.kind),
            ),
            Some(allocation) if !matches(instance, requirement, allocation) => {
                reason(
                    reasons,
                    ReasonCode::StorageConflict,
                    subject(requirement.kind),
                );
            }
            Some(allocation) if allocation.state != StorageState::Present => {
                reason(
                    reasons,
                    ReasonCode::StorageUnknown,
                    subject(requirement.kind),
                );
            }
            Some(_) => {}
        }
    }
}

/// Current owner/declared-slot authorization for receipt visibility. This does
/// not require capability grants, runtime evidence, budget or expected revisions.
pub fn authorize_storage(
    snapshot: &StorageAdmissionSnapshot,
    request: &StorageRequest,
) -> Vec<Reason> {
    let mut reasons = authority_shape(
        snapshot.schema_version,
        &snapshot.policy,
        &snapshot.instance,
        &request.request,
    );
    validate_requirements(&snapshot.instance, false, &mut reasons);
    if !identifier(&request.allocation_id) {
        reason(&mut reasons, ReasonCode::InvalidRequest, "allocationId");
    }
    if !reasons.is_empty() {
        normalize(&mut reasons);
        return reasons;
    }
    if request.request.principal_id != snapshot.instance.owner_id {
        reason(&mut reasons, ReasonCode::WrongPrincipal, "principalId");
    }
    if request.request.instance_id != snapshot.instance.id {
        reason(&mut reasons, ReasonCode::StaleInstance, "instanceId");
    }
    if !snapshot
        .instance
        .storage
        .iter()
        .any(|requirement| requirement.id == request.allocation_id)
    {
        reason(&mut reasons, ReasonCode::InvalidRequest, "allocationId");
    }
    normalize(&mut reasons);
    reasons
}

fn finish(mut reasons: Vec<Reason>, allocation: Option<StorageAllocation>) -> StorageDecision {
    normalize(&mut reasons);
    let allowed = reasons.is_empty();
    StorageDecision {
        allowed,
        reasons,
        allocation: if allowed { allocation } else { None },
    }
}

/// Reserve one declared fixed-size volume. An existing exact Present allocation
/// can only be verified/mounted; it never grants another image-creation permit.
/// Missing images, unknown outcomes, resize and rebinding need explicit recovery.
pub fn decide_storage(
    snapshot: &StorageAdmissionSnapshot,
    request: &StorageRequest,
) -> StorageDecision {
    let mut reasons = authorize_storage(snapshot, request);
    reasons.extend(check_policy(&snapshot.policy));
    reasons.extend(validate_reservations(&snapshot.reservations));
    if !reasons.is_empty() {
        return finish(reasons, None);
    }
    check_expected(
        &snapshot.policy,
        &snapshot.instance,
        &snapshot.reservations,
        &request.request,
        &mut reasons,
    );
    let instance = &snapshot.instance;
    let requirement = instance
        .storage
        .iter()
        .find(|requirement| requirement.id == request.allocation_id)
        .expect("authorization checked declared storage requirement");
    let bytes = requirement.kind.bytes(instance.resources);
    if bytes > requirement.kind.bytes(snapshot.policy.per_instance) {
        reason(
            &mut reasons,
            ReasonCode::InstanceLimit,
            subject(requirement.kind),
        );
    }
    let existing = snapshot
        .reservations
        .storage
        .iter()
        .find(|allocation| allocation.id == requirement.id);
    let allocation = if let Some(existing) = existing {
        if !matches(instance, requirement, existing) {
            reason(
                &mut reasons,
                ReasonCode::StorageConflict,
                subject(requirement.kind),
            );
        } else if existing.state != StorageState::Present {
            reason(
                &mut reasons,
                ReasonCode::StorageUnknown,
                subject(requirement.kind),
            );
        }
        existing.clone()
    } else {
        if snapshot.reservations.storage.len() >= MAX_STORAGE_ALLOCATIONS {
            reason(&mut reasons, ReasonCode::LedgerCapacity, "storage");
        }
        StorageAllocation {
            id: requirement.id.clone(),
            kind: requirement.kind,
            bytes,
            binding: Some(StorageBinding {
                instance_id: instance.id.clone(),
                owner_id: instance.owner_id.clone(),
                revision: requirement.binding_revision.clone(),
            }),
            state: StorageState::Unknown,
        }
    };
    let total = snapshot
        .policy
        .host_reserve
        .retained()
        .checked_add(snapshot.policy.research_reserve.retained())
        .and_then(|sum| sum.checked_add(allocated_resources(&snapshot.reservations.storage)?))
        .and_then(|sum| {
            if existing.is_some() {
                Some(sum)
            } else {
                sum.checked_add(allocation_resources(&allocation))
            }
        });
    match total {
        None => reason(&mut reasons, ReasonCode::ResourceOverflow, "storage"),
        Some(total) => {
            for kind in [StorageKind::Workspace, StorageKind::State] {
                if kind.bytes(total) > kind.bytes(snapshot.policy.capacity) {
                    reason(&mut reasons, ReasonCode::ResourceExhausted, subject(kind));
                }
            }
        }
    }
    finish(reasons, Some(allocation))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> (StorageAdmissionSnapshot, StorageRequest) {
        let recorded: RecordedAdmission =
            serde_json::from_str(include_str!("../tests/recorded-start.json")).unwrap();
        let request = StorageRequest {
            allocation_id: recorded.snapshot.instance.storage[0].id.clone(),
            request: recorded.request,
        };
        (
            StorageAdmissionSnapshot {
                schema_version: recorded.snapshot.schema_version,
                policy: recorded.snapshot.policy,
                instance: recorded.snapshot.instance,
                reservations: recorded.snapshot.reservations,
            },
            request,
        )
    }

    fn denied(snapshot: &StorageAdmissionSnapshot, request: &StorageRequest, code: ReasonCode) {
        let decision = decide_storage(snapshot, request);
        assert!(
            !decision.allowed,
            "unexpected provisioning admission: {decision:?}"
        );
        assert!(decision.allocation.is_none());
        assert!(
            decision.reasons.iter().any(|reason| reason.code == code),
            "missing {code:?}: {decision:?}"
        );
    }

    #[test]
    fn workspace_only_provisioning_needs_no_compute_state_or_external_grants() {
        let (mut snapshot, request) = fixture();
        snapshot.instance.storage.truncate(1);
        snapshot.instance.resources = Resources {
            workspace_bytes: MIN_STORAGE_BYTES,
            ..Resources::default()
        };
        snapshot.instance.requested_capabilities = vec![Capability::Model, Capability::Research];
        snapshot.reservations.storage.clear();
        let decision = decide_storage(&snapshot, &request);
        assert!(decision.allowed, "{decision:?}");
        let allocation = decision.allocation.unwrap();
        assert_eq!(allocation.bytes, MIN_STORAGE_BYTES);
        assert_eq!(allocation.state, StorageState::Unknown);
        assert_eq!(allocation.kind, StorageKind::Workspace);
        assert_eq!(
            allocation.binding.unwrap().owner_id,
            snapshot.instance.owner_id
        );
        assert!(
            snapshot.reservations.storage.is_empty(),
            "pure decision mutated inventory"
        );
    }

    #[test]
    fn storage_permission_never_substitutes_for_startup_authorization() {
        let mut recorded: RecordedAdmission =
            serde_json::from_str(include_str!("../tests/recorded-start.json")).unwrap();
        recorded
            .snapshot
            .instance
            .requested_capabilities
            .push(Capability::Model);
        recorded.snapshot.readiness.evidence.clear();
        let snapshot = StorageAdmissionSnapshot {
            schema_version: recorded.snapshot.schema_version,
            policy: recorded.snapshot.policy.clone(),
            instance: recorded.snapshot.instance.clone(),
            reservations: recorded.snapshot.reservations.clone(),
        };
        let request = StorageRequest {
            request: recorded.request.clone(),
            allocation_id: snapshot.instance.storage[0].id.clone(),
        };
        assert!(decide_storage(&snapshot, &request).allowed);
        let decision = decide(
            &recorded.snapshot,
            &recorded.request,
            recorded.now_unix_seconds,
        );
        assert!(!decision.allowed);
        assert!(
            decision
                .reasons
                .iter()
                .any(|reason| reason.code == ReasonCode::MissingGrant)
        );
        recorded.snapshot.instance.requested_capabilities.clear();
        let decision = decide(
            &recorded.snapshot,
            &recorded.request,
            recorded.now_unix_seconds,
        );
        assert!(
            decision
                .reasons
                .iter()
                .any(|reason| reason.code == ReasonCode::MissingEvidence)
        );
    }

    #[test]
    fn existing_present_allocation_is_verified_without_recharging_or_recreating() {
        let (mut snapshot, request) = fixture();
        let total = snapshot
            .policy
            .host_reserve
            .retained()
            .checked_add(snapshot.policy.research_reserve.retained())
            .unwrap()
            .checked_add(allocated_resources(&snapshot.reservations.storage).unwrap())
            .unwrap();
        snapshot.policy.capacity.workspace_bytes = total.workspace_bytes;
        snapshot.policy.capacity.state_bytes = total.state_bytes;
        let decision = decide_storage(&snapshot, &request);
        assert!(decision.allowed, "{decision:?}");
        assert_eq!(
            decision.allocation.as_ref(),
            Some(&snapshot.reservations.storage[0])
        );
        assert_eq!(decision.allocation.unwrap().state, StorageState::Present);
        snapshot.reservations.storage[0].state = StorageState::Unknown;
        denied(&snapshot, &request, ReasonCode::StorageUnknown);
    }

    #[test]
    fn existing_orphan_binding_or_size_cannot_be_adopted_changed_or_shrunk() {
        for mutation in 0..6 {
            let (mut snapshot, request) = fixture();
            let allocation = &mut snapshot.reservations.storage[0];
            match mutation {
                0 => allocation.binding = None,
                1 => allocation.binding.as_mut().unwrap().owner_id = "former-owner".into(),
                2 => allocation.binding.as_mut().unwrap().instance_id = "other-instance".into(),
                3 => allocation.binding.as_mut().unwrap().revision = "other-binding".into(),
                4 => allocation.bytes += 1,
                5 => allocation.bytes -= 1,
                _ => unreachable!(),
            }
            denied(&snapshot, &request, ReasonCode::StorageConflict);
        }
    }

    #[test]
    fn orphan_and_pending_allocations_count_before_new_provisioning() {
        for state in [StorageState::Unknown, StorageState::Present] {
            let (mut snapshot, request) = fixture();
            snapshot.reservations.storage[0].id = "orphan-slot".into();
            snapshot.reservations.storage[0].binding = None;
            snapshot.reservations.storage[0].state = state;
            let expected = snapshot.policy.host_reserve.workspace_bytes
                + snapshot.policy.research_reserve.workspace_bytes
                + snapshot.reservations.storage[0].bytes
                + snapshot.instance.resources.workspace_bytes;
            snapshot.policy.capacity.workspace_bytes = expected - 1;
            denied(&snapshot, &request, ReasonCode::ResourceExhausted);
            snapshot.policy.capacity.workspace_bytes = expected;
            assert!(decide_storage(&snapshot, &request).allowed);
        }
    }

    #[test]
    fn storage_limits_and_overflow_are_checked_independently_of_compute() {
        let (mut snapshot, request) = fixture();
        snapshot.policy.capacity.pids = 1;
        assert!(
            decide_storage(&snapshot, &request).allowed,
            "storage does not claim compute"
        );
        snapshot.policy.per_instance.workspace_bytes = MIN_STORAGE_BYTES;
        denied(&snapshot, &request, ReasonCode::InstanceLimit);
        for kind in [StorageKind::Workspace, StorageKind::State] {
            let (mut snapshot, request) = fixture();
            snapshot.reservations.storage.push(StorageAllocation {
                id: "overflow-orphan".into(),
                kind,
                bytes: u64::MAX,
                binding: None,
                state: StorageState::Unknown,
            });
            assert!(
                validate_reservations(&snapshot.reservations).is_empty(),
                "over-budget inventory remains loadable"
            );
            denied(&snapshot, &request, ReasonCode::ResourceOverflow);
        }
    }

    #[test]
    fn owner_revisions_epoch_and_complete_inventory_are_mandatory() {
        let (snapshot, request) = fixture();
        for mutation in 0..6 {
            let mut snapshot = snapshot.clone();
            let mut request = request.clone();
            let expected = match mutation {
                0 => {
                    request.request.principal_id = "other-owner".into();
                    ReasonCode::WrongPrincipal
                }
                1 => {
                    request.request.expected.policy = "old-policy".into();
                    ReasonCode::StalePolicy
                }
                2 => {
                    request.request.expected.instance = "old-instance".into();
                    ReasonCode::StaleInstance
                }
                3 => {
                    request.request.expected.ledger += 1;
                    ReasonCode::StaleLedger
                }
                4 => {
                    snapshot.reservations.generation = "other-authority-epoch".into();
                    ReasonCode::GenerationMismatch
                }
                5 => {
                    snapshot.reservations.complete = false;
                    ReasonCode::LedgerIncomplete
                }
                _ => unreachable!(),
            };
            denied(&snapshot, &request, expected);
        }
    }

    #[test]
    fn receipt_authorization_ignores_budget_and_stale_revisions_but_requires_owner_and_slot() {
        let (mut snapshot, mut request) = fixture();
        snapshot.policy.capacity = Resources::default();
        request.request.expected.policy = "old-policy".into();
        request.request.expected.instance = "old-instance".into();
        request.request.expected.ledger += 1;
        assert!(authorize_storage(&snapshot, &request).is_empty());
        assert!(!decide_storage(&snapshot, &request).allowed);
        request.allocation_id = "undeclared-slot".into();
        assert!(
            authorize_storage(&snapshot, &request)
                .iter()
                .any(|reason| reason.code == ReasonCode::InvalidRequest)
        );
        request.allocation_id = snapshot.instance.storage[0].id.clone();
        request.request.principal_id = "other-owner".into();
        assert!(
            authorize_storage(&snapshot, &request)
                .iter()
                .any(|reason| reason.code == ReasonCode::WrongPrincipal)
        );
    }

    #[test]
    fn malformed_or_duplicate_requirements_and_rows_fail_closed() {
        let (mut snapshot, request) = fixture();
        snapshot.instance.storage[1] = snapshot.instance.storage[0].clone();
        denied(&snapshot, &request, ReasonCode::DuplicateStorage);
        let (mut snapshot, request) = fixture();
        snapshot.instance.storage[0].binding_revision = "invalid/path".into();
        denied(&snapshot, &request, ReasonCode::InvalidStorage);
        let (mut snapshot, request) = fixture();
        snapshot
            .reservations
            .storage
            .push(snapshot.reservations.storage[0].clone());
        denied(&snapshot, &request, ReasonCode::DuplicateStorage);
        let (mut snapshot, request) = fixture();
        snapshot.reservations.storage[0].bytes = 0;
        denied(&snapshot, &request, ReasonCode::InvalidStorage);
    }

    #[test]
    fn allocation_count_is_bounded_and_allocation_order_is_not_authority() {
        let (mut snapshot, request) = fixture();
        let first = decide_storage(&snapshot, &request);
        snapshot.reservations.storage.reverse();
        snapshot.instance.storage.reverse();
        assert_eq!(decide_storage(&snapshot, &request), first);
        snapshot.reservations.storage = (0..=MAX_STORAGE_ALLOCATIONS)
            .map(|index| StorageAllocation {
                id: format!("orphan-{index}"),
                kind: StorageKind::Workspace,
                bytes: 1,
                binding: None,
                state: StorageState::Unknown,
            })
            .collect();
        denied(&snapshot, &request, ReasonCode::LedgerCapacity);
    }

    #[test]
    fn small_orphan_files_remain_chargeable_but_cannot_be_owned_quota_volumes() {
        let (mut snapshot, request) = fixture();
        snapshot.reservations.storage[0].bytes = 1;
        denied(&snapshot, &request, ReasonCode::InvalidStorage);
        snapshot.reservations.storage[0].binding = None;
        assert!(validate_reservations(&snapshot.reservations).is_empty());
        denied(&snapshot, &request, ReasonCode::StorageConflict);
    }

    #[test]
    fn storage_snapshot_is_closed_and_old_accounting_version_is_refused() {
        let (mut snapshot, request) = fixture();
        snapshot.schema_version = 1;
        denied(&snapshot, &request, ReasonCode::UnsupportedVersion);
        let (snapshot, _) = fixture();
        let mut unknown = serde_json::to_value(&snapshot).unwrap();
        unknown["ready"] = serde_json::json!(true);
        assert!(serde_json::from_value::<StorageAdmissionSnapshot>(unknown).is_err());
        let mut unknown = serde_json::to_value(&snapshot).unwrap();
        unknown["reservations"]["storage"][0]["state"] = serde_json::json!("absent");
        assert!(serde_json::from_value::<StorageAdmissionSnapshot>(unknown).is_err());
    }
}
