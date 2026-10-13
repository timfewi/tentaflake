use crate::*;
use std::collections::{BTreeMap, BTreeSet};

pub(crate) fn identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value.as_bytes()[0].is_ascii_alphanumeric()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"_.:-".contains(&byte))
}

fn fields(resources: Resources) -> [(&'static str, u64); 6] {
    [
        ("memoryBytes", resources.memory_bytes),
        ("memorySwapBytes", resources.memory_swap_bytes),
        ("nanoCpus", resources.nano_cpus),
        ("pids", resources.pids),
        ("workspaceBytes", resources.workspace_bytes),
        ("stateBytes", resources.state_bytes),
    ]
}

fn active_resources(resources: Resources) -> bool {
    fields(resources).iter().all(|(_, value)| *value > 0)
        && resources.memory_swap_bytes >= resources.memory_bytes
        && resources.workspace_bytes >= MIN_STORAGE_BYTES
        && resources.state_bytes >= MIN_STORAGE_BYTES
}

pub(crate) fn reason(reasons: &mut Vec<Reason>, code: ReasonCode, subject: &str) {
    reasons.push(Reason {
        code,
        subject: subject.to_owned(),
    });
}

pub(crate) fn normalize(reasons: &mut Vec<Reason>) {
    reasons.sort();
    reasons.dedup();
}

fn finish(mut reasons: Vec<Reason>, instance: &Instance) -> Decision {
    normalize(&mut reasons);
    let allowed = reasons.is_empty();
    Decision {
        allowed,
        reasons,
        reservation: allowed.then(|| Reservation {
            instance_id: instance.id.clone(),
            owner_id: instance.owner_id.clone(),
            instance_revision: instance.revision.clone(),
            state: ReservationState::Reserved,
            resources: instance.resources,
        }),
    }
}

/// Structural validation only: unknown reservations remain loadable for recovery.
/// They block new admission, but must not be discarded as malformed ledger data.
pub fn validate_reservations(snapshot: &ReservationSnapshot) -> Vec<Reason> {
    let mut reasons = Vec::new();
    if snapshot.revision == 0 || !identifier(&snapshot.generation) {
        reason(&mut reasons, ReasonCode::InvalidReservation, "ledger");
    }
    if snapshot.entries.len() > MAX_RESERVATIONS {
        reason(&mut reasons, ReasonCode::LedgerCapacity, "entries");
        return reasons;
    }
    let mut instances = BTreeSet::new();
    for entry in &snapshot.entries {
        if !identifier(&entry.instance_id)
            || !identifier(&entry.owner_id)
            || !identifier(&entry.instance_revision)
        {
            reason(&mut reasons, ReasonCode::InvalidReservation, "identity");
        }
        if !instances.insert(&entry.instance_id) {
            reason(&mut reasons, ReasonCode::DuplicateReservation, "instance");
        }
        let resources_valid = if entry.state == ReservationState::Retained {
            entry.resources == entry.resources.retained()
                && entry.resources.workspace_bytes >= MIN_STORAGE_BYTES
                && entry.resources.state_bytes >= MIN_STORAGE_BYTES
        } else {
            active_resources(entry.resources)
        };
        if !resources_valid {
            reason(&mut reasons, ReasonCode::InvalidReservation, "resources");
        }
    }
    storage_decision::validate_allocations(&snapshot.storage, &mut reasons);
    normalize(&mut reasons);
    reasons
}

fn evidence_name(kind: EvidenceKind) -> &'static str {
    match kind {
        EvidenceKind::Runtime => "runtime",
        EvidenceKind::WorkspaceQuota => "workspace-quota",
        EvidenceKind::StateQuota => "state-quota",
        EvidenceKind::Model => "model",
        EvidenceKind::Research => "research",
        EvidenceKind::Worker => "worker",
    }
}

fn capability_kind(capability: Capability) -> EvidenceKind {
    match capability {
        Capability::Model => EvidenceKind::Model,
        Capability::Research => EvidenceKind::Research,
        Capability::Worker => EvidenceKind::Worker,
    }
}

pub(crate) fn authority_shape(
    schema_version: u32,
    policy: &HostPolicy,
    instance: &Instance,
    request: &AdmissionRequest,
) -> Vec<Reason> {
    let mut reasons = Vec::new();
    if schema_version != SCHEMA_VERSION || request.schema_version != SCHEMA_VERSION {
        reason(
            &mut reasons,
            ReasonCode::UnsupportedVersion,
            "schemaVersion",
        );
    }
    if !identifier(&policy.revision)
        || !identifier(&policy.generation)
        || policy.grants.len() > MAX_GRANTS
    {
        reason(&mut reasons, ReasonCode::InvalidPolicy, "policy");
    }
    if !identifier(&instance.id)
        || !identifier(&instance.owner_id)
        || !identifier(&instance.revision)
        || instance.requested_capabilities.len() > 3
        || instance
            .requested_capabilities
            .iter()
            .collect::<BTreeSet<_>>()
            .len()
            != instance.requested_capabilities.len()
    {
        reason(&mut reasons, ReasonCode::InvalidInstance, "instance");
    }
    if !identifier(&request.operation_id)
        || !identifier(&request.instance_id)
        || !identifier(&request.principal_id)
        || !identifier(&request.expected.policy)
        || !identifier(&request.expected.instance)
        || request.expected.ledger == 0
    {
        reason(&mut reasons, ReasonCode::InvalidRequest, "request");
    }
    if !reasons.is_empty() {
        return reasons;
    }
    let mut grants = BTreeSet::new();
    for grant in &policy.grants {
        if !identifier(&grant.instance_id) || !identifier(&grant.owner_id) {
            reason(&mut reasons, ReasonCode::InvalidPolicy, "grant-identity");
        }
        if grant.instance_id == instance.id && grant.owner_id != instance.owner_id {
            reason(&mut reasons, ReasonCode::InvalidPolicy, "grant-owner");
        }
        if !grants.insert((&grant.instance_id, &grant.owner_id, grant.capability)) {
            reason(&mut reasons, ReasonCode::DuplicateGrant, "grants");
        }
    }
    reasons
}

pub(crate) fn check_policy(policy: &HostPolicy) -> Vec<Reason> {
    let mut reasons = Vec::new();
    if !active_resources(policy.capacity)
        || !active_resources(policy.per_instance)
        || policy.host_reserve.memory_swap_bytes < policy.host_reserve.memory_bytes
        || policy.research_reserve.memory_swap_bytes < policy.research_reserve.memory_bytes
        || policy.max_active == 0
        || policy.max_active as usize > MAX_RESERVATIONS
        || policy.evidence_max_age_seconds == 0
        || policy.evidence_max_age_seconds > 300
    {
        reason(&mut reasons, ReasonCode::InvalidPolicy, "resources");
    }
    reasons
}

fn check_shape(snapshot: &AdmissionSnapshot) -> Vec<Reason> {
    let mut reasons = validate_reservations(&snapshot.reservations);
    reasons.extend(check_policy(&snapshot.policy));
    storage_decision::validate_requirements(&snapshot.instance, true, &mut reasons);
    if !active_resources(snapshot.instance.resources) {
        reason(&mut reasons, ReasonCode::InvalidInstance, "resources");
    }
    if !identifier(&snapshot.readiness.generation)
        || snapshot.readiness.evidence.len() > 6
        || snapshot.readiness.sources.len() > 6
    {
        reason(&mut reasons, ReasonCode::InvalidEvidence, "readiness");
    }
    if !reasons.is_empty() {
        return reasons;
    }
    let mut source_kinds = BTreeSet::new();
    for source in &snapshot.readiness.sources {
        if !identifier(&source.generation) {
            reason(
                &mut reasons,
                ReasonCode::InvalidEvidence,
                "source-generation",
            );
        }
        if !source_kinds.insert(source.kind) {
            reason(
                &mut reasons,
                ReasonCode::DuplicateEvidence,
                evidence_name(source.kind),
            );
        }
    }
    let mut evidence_kinds = BTreeSet::new();
    for evidence in &snapshot.readiness.evidence {
        if evidence.instance_id != snapshot.instance.id
            || !identifier(&evidence.policy_revision)
            || !identifier(&evidence.instance_revision)
            || !identifier(&evidence.generation)
            || evidence.expires_at <= evidence.observed_at
        {
            reason(
                &mut reasons,
                ReasonCode::InvalidEvidence,
                evidence_name(evidence.kind),
            );
        }
        if !evidence_kinds.insert(evidence.kind) {
            reason(
                &mut reasons,
                ReasonCode::DuplicateEvidence,
                evidence_name(evidence.kind),
            );
        }
    }
    reasons
}

/// Current owner/grant checks for receipt visibility, without treating stale
/// expected revisions or unavailable runtime evidence as proof of no dispatch.
/// This still depends on the caller authenticating the supplied host snapshot.
pub fn authorize(
    snapshot: &AdmissionSnapshot,
    request: &AdmissionRequest,
    now_unix_seconds: u64,
) -> Vec<Reason> {
    let mut reasons = authority_shape(
        snapshot.schema_version,
        &snapshot.policy,
        &snapshot.instance,
        request,
    );
    if !reasons.is_empty() {
        normalize(&mut reasons);
        return reasons;
    }
    let instance = &snapshot.instance;
    if request.principal_id != instance.owner_id {
        reason(&mut reasons, ReasonCode::WrongPrincipal, "principalId");
    }
    if request.instance_id != instance.id {
        reason(&mut reasons, ReasonCode::StaleInstance, "instanceId");
    }
    for capability in &instance.requested_capabilities {
        let subject = evidence_name(capability_kind(*capability));
        match snapshot.policy.grants.iter().find(|grant| {
            grant.instance_id == instance.id
                && grant.owner_id == instance.owner_id
                && grant.capability == *capability
        }) {
            None => reason(&mut reasons, ReasonCode::MissingGrant, subject),
            Some(grant)
                if grant
                    .expires_at
                    .is_some_and(|expiry| expiry <= now_unix_seconds) =>
            {
                reason(&mut reasons, ReasonCode::ExpiredGrant, subject);
            }
            Some(_) => {}
        }
    }
    for grant in &snapshot.policy.grants {
        if grant.instance_id == instance.id
            && grant.owner_id == instance.owner_id
            && !instance.requested_capabilities.contains(&grant.capability)
        {
            reason(
                &mut reasons,
                ReasonCode::UnrequestedGrant,
                evidence_name(capability_kind(grant.capability)),
            );
        }
    }
    normalize(&mut reasons);
    reasons
}

fn check_readiness(snapshot: &AdmissionSnapshot, now: u64, reasons: &mut Vec<Reason>) {
    let sources: BTreeMap<_, _> = snapshot
        .readiness
        .sources
        .iter()
        .map(|source| (source.kind, &source.generation))
        .collect();
    let evidence: BTreeMap<_, _> = snapshot
        .readiness
        .evidence
        .iter()
        .map(|evidence| (evidence.kind, evidence))
        .collect();
    let required = [
        EvidenceKind::Runtime,
        EvidenceKind::WorkspaceQuota,
        EvidenceKind::StateQuota,
    ]
    .into_iter()
    .chain(
        snapshot
            .instance
            .requested_capabilities
            .iter()
            .copied()
            .map(capability_kind),
    );
    for kind in required {
        let subject = evidence_name(kind);
        let Some(record) = evidence.get(&kind) else {
            reason(reasons, ReasonCode::MissingEvidence, subject);
            continue;
        };
        match sources.get(&kind) {
            None => reason(reasons, ReasonCode::MissingGeneration, subject),
            Some(generation) if **generation != record.generation => {
                reason(reasons, ReasonCode::GenerationMismatch, subject);
            }
            Some(_) => {}
        }
        if record.policy_revision != snapshot.policy.revision
            || record.instance_revision != snapshot.instance.revision
            || record.observed_at > now
            || record.expires_at <= now
            || now.saturating_sub(record.observed_at) > snapshot.policy.evidence_max_age_seconds
        {
            reason(reasons, ReasonCode::StaleEvidence, subject);
        }
        if record.source != EvidenceSource::Live {
            reason(reasons, ReasonCode::DeclarationOnly, subject);
        }
        match record.status {
            EvidenceStatus::Unknown => reason(reasons, ReasonCode::ReadinessUnknown, subject),
            EvidenceStatus::Unready => reason(reasons, ReasonCode::NotReady, subject),
            EvidenceStatus::Ready => {}
        }
    }
}

fn check_resources(snapshot: &AdmissionSnapshot, reasons: &mut Vec<Reason>) {
    let policy = &snapshot.policy;
    let instance = &snapshot.instance;
    for ((name, requested), (_, maximum)) in fields(instance.resources)
        .into_iter()
        .zip(fields(policy.per_instance))
    {
        if requested > maximum {
            reason(reasons, ReasonCode::InstanceLimit, name);
        }
    }
    let mut total = policy
        .host_reserve
        .checked_add(policy.research_reserve)
        .and_then(|sum| {
            sum.checked_add(storage_decision::allocated_resources(
                &snapshot.reservations.storage,
            )?)
        });
    let mut active = 1_u64;
    let mut replacing = false;
    for reservation in &snapshot.reservations.entries {
        if reservation.state == ReservationState::Unknown {
            reason(reasons, ReasonCode::UnknownReservation, "ledger");
        }
        if reservation.instance_id == instance.id {
            if reservation.state == ReservationState::Retained
                && reservation.owner_id == instance.owner_id
            {
                replacing = true;
                continue;
            }
            reason(reasons, ReasonCode::ReservationConflict, "instance");
        }
        if reservation.state != ReservationState::Retained {
            active += 1;
        }
        total = total.and_then(|sum| sum.checked_add(reservation.resources.compute()));
    }
    if snapshot.reservations.entries.len() >= MAX_RESERVATIONS && !replacing {
        reason(reasons, ReasonCode::LedgerCapacity, "entries");
    }
    if active > u64::from(policy.max_active) {
        reason(reasons, ReasonCode::ActiveLimit, "maxActive");
    }
    match total.and_then(|sum| sum.checked_add(instance.resources.compute())) {
        None => reason(reasons, ReasonCode::ResourceOverflow, "aggregate"),
        Some(total) => {
            for ((name, used), (_, capacity)) in
                fields(total).into_iter().zip(fields(policy.capacity))
            {
                if used > capacity {
                    reason(reasons, ReasonCode::ResourceExhausted, name);
                }
            }
        }
    }
    storage_decision::check_start_storage(instance, &snapshot.reservations.storage, reasons);
}

pub(crate) fn check_expected(
    policy: &HostPolicy,
    instance: &Instance,
    reservations: &ReservationSnapshot,
    request: &AdmissionRequest,
    reasons: &mut Vec<Reason>,
) {
    if request.expected.policy != policy.revision {
        reason(reasons, ReasonCode::StalePolicy, "expected.policy");
    }
    if request.expected.instance != instance.revision || request.instance_id != instance.id {
        reason(reasons, ReasonCode::StaleInstance, "expected.instance");
    }
    if request.expected.ledger != reservations.revision {
        reason(reasons, ReasonCode::StaleLedger, "expected.ledger");
    }
    if reservations.generation != policy.generation {
        reason(reasons, ReasonCode::GenerationMismatch, "host");
    }
    if !reservations.complete {
        reason(reasons, ReasonCode::LedgerIncomplete, "ledger");
    }
}

/// Decide a new start intent. Existing operation IDs must be reconciled by the
/// locked ledger, not replayed through this pure function as new dispatches.
pub fn decide(
    snapshot: &AdmissionSnapshot,
    request: &AdmissionRequest,
    now_unix_seconds: u64,
) -> Decision {
    let mut reasons = authorize(snapshot, request, now_unix_seconds);
    reasons.extend(check_shape(snapshot));
    if !reasons.is_empty() {
        return finish(reasons, &snapshot.instance);
    }
    let policy = &snapshot.policy;
    let instance = &snapshot.instance;
    check_expected(
        policy,
        instance,
        &snapshot.reservations,
        request,
        &mut reasons,
    );
    if snapshot.readiness.generation != policy.generation {
        reason(&mut reasons, ReasonCode::GenerationMismatch, "host");
    }
    if instance.isolation != Isolation::Container {
        reason(
            &mut reasons,
            ReasonCode::UnsupportedIsolation,
            "guest-kernel",
        );
    }
    check_readiness(snapshot, now_unix_seconds, &mut reasons);
    check_resources(snapshot, &mut reasons);
    finish(reasons, instance)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> RecordedAdmission {
        serde_json::from_str(include_str!("../tests/recorded-start.json")).unwrap()
    }

    fn decision(record: &RecordedAdmission) -> Decision {
        decide(&record.snapshot, &record.request, record.now_unix_seconds)
    }

    fn denied(record: &RecordedAdmission, code: ReasonCode) {
        let result = decision(record);
        assert!(!result.allowed, "unexpected admission: {result:?}");
        assert!(result.reservation.is_none());
        assert!(
            result.reasons.iter().any(|reason| reason.code == code),
            "missing {code:?}: {result:?}"
        );
    }

    fn enable(record: &mut RecordedAdmission, capability: Capability) {
        let snapshot = &mut record.snapshot;
        let kind = capability_kind(capability);
        snapshot.instance.requested_capabilities.push(capability);
        snapshot.policy.grants.push(Grant {
            instance_id: snapshot.instance.id.clone(),
            owner_id: snapshot.instance.owner_id.clone(),
            capability,
            expires_at: Some(1030),
        });
        let generation = format!("fixture-{}-source", evidence_name(kind));
        snapshot.readiness.sources.push(ReadinessGeneration {
            kind,
            generation: generation.clone(),
        });
        snapshot.readiness.evidence.push(ReadinessEvidence {
            instance_id: snapshot.instance.id.clone(),
            kind,
            status: EvidenceStatus::Ready,
            source: EvidenceSource::Live,
            policy_revision: snapshot.policy.revision.clone(),
            instance_revision: snapshot.instance.revision.clone(),
            generation,
            observed_at: 1000,
            expires_at: 1030,
        });
    }

    fn peer(record: &RecordedAdmission, id: &str, state: ReservationState) -> Reservation {
        Reservation {
            instance_id: id.into(),
            owner_id: "peer-owner".into(),
            instance_revision: "peer-revision".into(),
            state,
            resources: if state == ReservationState::Retained {
                record.snapshot.instance.resources.retained()
            } else {
                record.snapshot.instance.resources
            },
        }
    }

    fn set(resources: &mut Resources, dimension: usize, value: u64) {
        match dimension {
            0 => resources.memory_bytes = value,
            1 => resources.memory_swap_bytes = value,
            2 => resources.nano_cpus = value,
            3 => resources.pids = value,
            4 => resources.workspace_bytes = value,
            5 => resources.state_bytes = value,
            _ => unreachable!(),
        }
    }

    #[test]
    fn recorded_offline_start_requires_no_external_grants() {
        let record = fixture();
        let result = decision(&record);
        assert!(result.allowed, "{result:?}");
        assert!(result.reasons.is_empty());
        let reservation = result.reservation.unwrap();
        assert_eq!(reservation.resources, record.snapshot.instance.resources);
        assert_eq!(reservation.state, ReservationState::Reserved);
        assert!(
            record.snapshot.reservations.entries.is_empty(),
            "pure decision mutated ledger"
        );
    }

    #[test]
    fn every_external_capability_requires_exact_current_grant_and_evidence() {
        for capability in [Capability::Model, Capability::Research, Capability::Worker] {
            let mut record = fixture();
            enable(&mut record, capability);
            assert!(decision(&record).allowed);
            let mut missing = record.clone();
            missing.snapshot.policy.grants.clear();
            denied(&missing, ReasonCode::MissingGrant);
            let mut foreign = record.clone();
            foreign.snapshot.policy.grants[0].instance_id = "different-instance".into();
            denied(&foreign, ReasonCode::MissingGrant);
            foreign.snapshot.policy.grants[0].instance_id = record.snapshot.instance.id.clone();
            foreign.snapshot.policy.grants[0].owner_id = "different-owner".into();
            denied(&foreign, ReasonCode::InvalidPolicy);
            let mut expired = record.clone();
            expired.snapshot.policy.grants[0].expires_at = Some(expired.now_unix_seconds);
            denied(&expired, ReasonCode::ExpiredGrant);
            record.snapshot.readiness.evidence.pop();
            denied(&record, ReasonCode::MissingEvidence);
        }
    }

    #[test]
    fn undeclared_and_duplicate_grants_do_not_expand_authority() {
        let mut record = fixture();
        enable(&mut record, Capability::Model);
        record.snapshot.instance.requested_capabilities.clear();
        denied(&record, ReasonCode::UnrequestedGrant);
        record
            .snapshot
            .instance
            .requested_capabilities
            .push(Capability::Model);
        record
            .snapshot
            .policy
            .grants
            .push(record.snapshot.policy.grants[0].clone());
        denied(&record, ReasonCode::DuplicateGrant);
    }

    #[test]
    fn stale_owner_transport_grant_cannot_turn_an_offline_instance_into_a_connected_one() {
        let mut record = fixture();
        record.snapshot.policy.grants.push(Grant {
            instance_id: record.snapshot.instance.id.clone(),
            owner_id: "previous-owner".into(),
            capability: Capability::Model,
            expires_at: None,
        });
        denied(&record, ReasonCode::InvalidPolicy);
        assert!(!authorize(&record.snapshot, &record.request, record.now_unix_seconds).is_empty());
    }

    #[test]
    fn exact_owner_and_all_expected_revisions_are_required() {
        let mut record = fixture();
        record.request.principal_id = "another-owner".into();
        denied(&record, ReasonCode::WrongPrincipal);
        let mut record = fixture();
        record.request.expected.policy = "old-policy-content".into();
        record.request.expected.instance = "old-instance-content".into();
        record.request.expected.ledger += 1;
        for code in [
            ReasonCode::StalePolicy,
            ReasonCode::StaleInstance,
            ReasonCode::StaleLedger,
        ] {
            denied(&record, code);
        }
    }

    #[test]
    fn strict_never_falls_back_to_container() {
        let mut record = fixture();
        record.snapshot.instance.isolation = Isolation::GuestKernel;
        denied(&record, ReasonCode::UnsupportedIsolation);
    }

    #[test]
    fn unknown_unready_and_source_only_records_never_prove_readiness() {
        let mut record = fixture();
        record.snapshot.readiness.evidence[0].status = EvidenceStatus::Unknown;
        denied(&record, ReasonCode::ReadinessUnknown);
        record.snapshot.readiness.evidence[0].status = EvidenceStatus::Unready;
        denied(&record, ReasonCode::NotReady);
        record.snapshot.readiness.evidence[0].status = EvidenceStatus::Ready;
        record.snapshot.readiness.evidence[0].source = EvidenceSource::Declaration;
        denied(&record, ReasonCode::DeclarationOnly);
    }

    #[test]
    fn expiration_future_observation_age_and_content_revisions_are_checked() {
        for scenario in 0..5 {
            let mut record = fixture();
            let evidence = &mut record.snapshot.readiness.evidence[0];
            match scenario {
                0 => evidence.expires_at = record.now_unix_seconds,
                1 => evidence.observed_at = record.now_unix_seconds + 1,
                2 => evidence.observed_at = 1,
                3 => evidence.policy_revision = "old-policy".into(),
                4 => evidence.instance_revision = "old-instance".into(),
                _ => unreachable!(),
            }
            denied(&record, ReasonCode::StaleEvidence);
        }
    }

    #[test]
    fn runtime_and_research_generations_are_independent_of_durable_host_epoch() {
        let mut record = fixture();
        enable(&mut record, Capability::Research);
        record
            .snapshot
            .readiness
            .sources
            .last_mut()
            .unwrap()
            .generation = "new-vpn-controller-lease".into();
        denied(&record, ReasonCode::GenerationMismatch);
        let mut record = fixture();
        record.snapshot.readiness.sources[0].generation = "new-runtime-process".into();
        denied(&record, ReasonCode::GenerationMismatch);
        let mut record = fixture();
        record.snapshot.policy.generation = "new-authority-epoch".into();
        denied(&record, ReasonCode::GenerationMismatch);
        assert_eq!(
            record.snapshot.reservations.generation,
            "fixture-authority-epoch"
        );
    }

    #[test]
    fn missing_duplicate_and_foreign_evidence_fail_closed() {
        let mut record = fixture();
        record.snapshot.readiness.sources.clear();
        denied(&record, ReasonCode::MissingGeneration);
        let mut record = fixture();
        record
            .snapshot
            .readiness
            .evidence
            .push(record.snapshot.readiness.evidence[0].clone());
        denied(&record, ReasonCode::DuplicateEvidence);
        let mut record = fixture();
        record
            .snapshot
            .readiness
            .sources
            .push(record.snapshot.readiness.sources[0].clone());
        denied(&record, ReasonCode::DuplicateEvidence);
        let mut record = fixture();
        record.snapshot.readiness.evidence[0].instance_id = "another-instance".into();
        denied(&record, ReasonCode::InvalidEvidence);
        let mut record = fixture();
        record.snapshot.readiness.evidence[0].expires_at =
            record.snapshot.readiness.evidence[0].observed_at;
        denied(&record, ReasonCode::InvalidEvidence);
        for missing in [EvidenceKind::WorkspaceQuota, EvidenceKind::StateQuota] {
            let mut record = fixture();
            record
                .snapshot
                .readiness
                .evidence
                .retain(|evidence| evidence.kind != missing);
            denied(&record, ReasonCode::MissingEvidence);
        }
    }

    #[test]
    fn every_aggregate_dimension_includes_both_host_and_research_reserves() {
        let record = fixture();
        let total = record
            .snapshot
            .policy
            .host_reserve
            .checked_add(record.snapshot.policy.research_reserve)
            .unwrap()
            .checked_add(record.snapshot.instance.resources)
            .unwrap();
        for (dimension, (name, value)) in fields(total).into_iter().enumerate() {
            let mut constrained = record.clone();
            constrained.snapshot.policy.capacity = total;
            set(
                &mut constrained.snapshot.policy.capacity,
                dimension,
                value - 1,
            );
            let result = decision(&constrained);
            assert!(
                result.reasons.contains(&Reason {
                    code: ReasonCode::ResourceExhausted,
                    subject: name.into()
                }),
                "{name}: {result:?}"
            );
            assert!(!result.allowed);
            constrained.snapshot.policy.capacity = total;
            assert!(
                decision(&constrained).allowed,
                "exact capacity should fit for {name}"
            );
        }
    }

    #[test]
    fn every_aggregate_addition_is_checked_for_overflow() {
        for dimension in 0..6 {
            let mut record = fixture();
            set(
                &mut record.snapshot.policy.host_reserve,
                dimension,
                u64::MAX,
            );
            if dimension == 0 {
                record.snapshot.policy.host_reserve.memory_swap_bytes = u64::MAX;
            }
            denied(&record, ReasonCode::ResourceOverflow);
        }
    }

    #[test]
    fn instance_ceiling_and_combined_swap_are_independent_checks() {
        let mut record = fixture();
        record.snapshot.instance.resources.nano_cpus =
            record.snapshot.policy.per_instance.nano_cpus + 1;
        denied(&record, ReasonCode::InstanceLimit);
        let mut record = fixture();
        record.snapshot.instance.resources.memory_swap_bytes =
            record.snapshot.instance.resources.memory_bytes - 1;
        denied(&record, ReasonCode::InvalidInstance);
        let mut record = fixture();
        record.snapshot.instance.resources.state_bytes = MIN_STORAGE_BYTES - 1;
        denied(&record, ReasonCode::InvalidInstance);
    }

    #[test]
    fn active_limit_counts_reserved_and_running_instances() {
        let mut record = fixture();
        record.snapshot.reservations.entries =
            vec![peer(&record, "running-peer", ReservationState::Active)];
        assert!(decision(&record).allowed);
        record.snapshot.reservations.entries.push(peer(
            &record,
            "reserved-peer",
            ReservationState::Reserved,
        ));
        denied(&record, ReasonCode::ActiveLimit);
    }

    #[test]
    fn retained_workspace_and_private_state_remain_charged() {
        for dimension in [4, 5] {
            let mut record = fixture();
            let retained = peer(&record, "retained-peer", ReservationState::Retained);
            for mut allocation in record.snapshot.reservations.storage.clone() {
                allocation.id = format!("peer-{}", allocation.id);
                allocation.binding.as_mut().unwrap().instance_id = retained.instance_id.clone();
                allocation.binding.as_mut().unwrap().owner_id = retained.owner_id.clone();
                record.snapshot.reservations.storage.push(allocation);
            }
            let total = record
                .snapshot
                .policy
                .host_reserve
                .checked_add(record.snapshot.policy.research_reserve)
                .unwrap()
                .checked_add(record.snapshot.instance.resources)
                .unwrap()
                .checked_add(retained.resources)
                .unwrap();
            record.snapshot.reservations.entries.push(retained);
            record.snapshot.policy.capacity = total;
            assert!(decision(&record).allowed);
            set(
                &mut record.snapshot.policy.capacity,
                dimension,
                fields(total)[dimension].1 - 1,
            );
            denied(&record, ReasonCode::ResourceExhausted);
        }
    }

    #[test]
    fn restart_charges_allocations_once_and_keeps_historical_resources_descriptive() {
        let mut record = fixture();
        let mut retained = decision(&record).reservation.unwrap();
        retained.state = ReservationState::Retained;
        retained.resources = retained.resources.retained();
        record.snapshot.policy.capacity = record
            .snapshot
            .policy
            .host_reserve
            .checked_add(record.snapshot.policy.research_reserve)
            .unwrap()
            .checked_add(record.snapshot.instance.resources)
            .unwrap();
        record.snapshot.reservations.entries.push(retained);
        assert!(
            decision(&record).allowed,
            "restart must not double-charge retained storage"
        );
        record.snapshot.reservations.entries[0]
            .resources
            .workspace_bytes += 1;
        assert!(
            decision(&record).allowed,
            "historical storage is not another allocation"
        );
        record.snapshot.reservations.entries[0].owner_id = "another-owner".into();
        denied(&record, ReasonCode::ReservationConflict);
    }

    #[test]
    fn orphan_and_never_started_allocations_still_consume_capacity() {
        for state in [StorageState::Present, StorageState::Unknown] {
            for orphan in [false, true] {
                let mut record = fixture();
                let mut allocation = record.snapshot.reservations.storage[0].clone();
                allocation.id = "never-started-volume".into();
                allocation.state = state;
                allocation.binding = if orphan {
                    None
                } else {
                    Some(StorageBinding {
                        instance_id: "never-started".into(),
                        owner_id: "previous-owner".into(),
                        revision: "previous-binding".into(),
                    })
                };
                let base = record
                    .snapshot
                    .policy
                    .host_reserve
                    .checked_add(record.snapshot.policy.research_reserve)
                    .unwrap()
                    .checked_add(record.snapshot.instance.resources)
                    .unwrap();
                record.snapshot.policy.capacity = base;
                record.snapshot.reservations.storage.push(allocation);
                assert!(record.snapshot.reservations.entries.is_empty());
                denied(&record, ReasonCode::ResourceExhausted);
                record.snapshot.policy.capacity.workspace_bytes +=
                    record.snapshot.instance.resources.workspace_bytes;
                assert!(
                    decision(&record).allowed,
                    "unrelated uncertain storage stays charged"
                );
            }
        }
    }

    #[test]
    fn startup_requires_exact_present_owned_allocations() {
        let mut record = fixture();
        record.snapshot.reservations.storage.pop();
        denied(&record, ReasonCode::MissingStorage);
        let mut record = fixture();
        record.snapshot.reservations.storage[0].state = StorageState::Unknown;
        denied(&record, ReasonCode::StorageUnknown);
        for mutation in 0..5 {
            let mut record = fixture();
            let allocation = &mut record.snapshot.reservations.storage[0];
            match mutation {
                0 => allocation.binding = None,
                1 => allocation.binding.as_mut().unwrap().owner_id = "previous-owner".into(),
                2 => allocation.binding.as_mut().unwrap().revision = "previous-path-binding".into(),
                3 => allocation.bytes += 1,
                4 => allocation.kind = StorageKind::State,
                _ => unreachable!(),
            }
            denied(&record, ReasonCode::StorageConflict);
        }
        let mut record = fixture();
        record.snapshot.instance.resources.workspace_bytes -= 1;
        denied(&record, ReasonCode::StorageConflict);
        let mut record = fixture();
        record.snapshot.instance.storage.pop();
        denied(&record, ReasonCode::InvalidStorage);
    }

    #[test]
    fn command_revision_change_preserves_independently_bound_storage() {
        let mut record = fixture();
        let revision = "updated-command-revision";
        record.snapshot.instance.revision = revision.into();
        record.request.expected.instance = revision.into();
        for evidence in &mut record.snapshot.readiness.evidence {
            evidence.instance_revision = revision.into();
        }
        assert!(decision(&record).allowed);
    }

    #[test]
    fn malformed_duplicate_storage_never_becomes_complete_inventory() {
        let mut record = fixture();
        record
            .snapshot
            .reservations
            .storage
            .push(record.snapshot.reservations.storage[0].clone());
        denied(&record, ReasonCode::DuplicateStorage);
        let mut record = fixture();
        record.snapshot.reservations.storage[0].bytes = 0;
        denied(&record, ReasonCode::InvalidStorage);
        let mut record = fixture();
        record.snapshot.instance.storage[1] = record.snapshot.instance.storage[0].clone();
        denied(&record, ReasonCode::DuplicateStorage);
        let mut record = fixture();
        record.snapshot.reservations.storage[0]
            .binding
            .as_mut()
            .unwrap()
            .revision = "x".repeat(129);
        denied(&record, ReasonCode::InvalidStorage);
    }

    #[test]
    fn unknown_and_incomplete_ledger_never_release_capacity() {
        let mut record = fixture();
        record.snapshot.reservations.entries.push(peer(
            &record,
            "uncertain-peer",
            ReservationState::Unknown,
        ));
        assert!(
            validate_reservations(&record.snapshot.reservations).is_empty(),
            "unknown receipts must remain recoverable"
        );
        denied(&record, ReasonCode::UnknownReservation);
        record.snapshot.reservations.entries.clear();
        record.snapshot.reservations.complete = false;
        denied(&record, ReasonCode::LedgerIncomplete);
    }

    #[test]
    fn duplicate_or_malformed_reservations_deny() {
        let mut record = fixture();
        let reservation = peer(&record, "peer", ReservationState::Active);
        record.snapshot.reservations.entries = vec![reservation.clone(), reservation];
        denied(&record, ReasonCode::DuplicateReservation);
        record.snapshot.reservations.entries.pop();
        record.snapshot.reservations.entries[0].state = ReservationState::Retained;
        denied(&record, ReasonCode::InvalidReservation);
    }

    #[test]
    fn receipt_authorization_does_not_infer_absence_from_stale_readiness_or_budget() {
        let mut record = fixture();
        enable(&mut record, Capability::Model);
        record.snapshot.readiness.evidence.clear();
        record.snapshot.policy.capacity = Resources::default();
        record.request.expected.ledger += 1;
        record.request.expected.policy = "old-policy".into();
        assert!(authorize(&record.snapshot, &record.request, record.now_unix_seconds).is_empty());
        assert!(!decision(&record).allowed);
        record.snapshot.policy.grants.clear();
        assert!(
            authorize(&record.snapshot, &record.request, record.now_unix_seconds)
                .iter()
                .any(|reason| reason.code == ReasonCode::MissingGrant)
        );
    }

    #[test]
    fn decisions_are_stable_under_evidence_order_and_replay() {
        let mut record = fixture();
        enable(&mut record, Capability::Research);
        record.snapshot.readiness.evidence[0].status = EvidenceStatus::Unknown;
        record.snapshot.readiness.evidence[1].source = EvidenceSource::Declaration;
        let first = decision(&record);
        record.snapshot.readiness.evidence.reverse();
        record.snapshot.readiness.sources.reverse();
        assert_eq!(decision(&record), first);
        let replay: RecordedAdmission =
            serde_json::from_str(&serde_json::to_string(&record).unwrap()).unwrap();
        assert_eq!(decision(&replay), first);
    }

    #[test]
    fn recorded_format_rejects_unknown_fields_enums_and_unsigned_overflow() {
        let base = serde_json::to_value(fixture()).unwrap();
        let mut forged = base.clone();
        forged["snapshot"]["instance"]["allowPublicNetwork"] = serde_json::json!(true);
        assert!(serde_json::from_value::<RecordedAdmission>(forged).is_err());
        let mut unknown = base.clone();
        unknown["snapshot"]["instance"]["requestedCapabilities"] = serde_json::json!(["internet"]);
        assert!(serde_json::from_value::<RecordedAdmission>(unknown).is_err());
        let mut negative = base;
        negative["snapshot"]["instance"]["resources"]["pids"] = serde_json::json!(-1);
        assert!(serde_json::from_value::<RecordedAdmission>(negative).is_err());
        let overflow = include_str!("../tests/recorded-start.json")
            .replace("\"pids\": 64", "\"pids\": 18446744073709551616");
        assert!(serde_json::from_str::<RecordedAdmission>(&overflow).is_err());
        let mut record = fixture();
        record.snapshot.schema_version += 1;
        denied(&record, ReasonCode::UnsupportedVersion);
    }
}
