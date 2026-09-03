module tentaflake_agent_attestation::agent_attestation;

use std::bcs;
use std::vector;
use sui::ed25519;
use sui::event;
use sui::object::{Self, ID, UID};
use sui::transfer;
use sui::tx_context::{Self, TxContext};

/// Errors are intentionally specific so callers can fail closed and distinguish
/// a bad attestation from an unavailable relay.
const EADMIN_CAP: u64 = 0;
const EPAUSED: u64 = 1;
const EAGENT_REVOKED: u64 = 2;
const EISSUER_NOT_FOUND: u64 = 3;
const EISSUER_INACTIVE: u64 = 4;
const EISSUER_SET_EPOCH: u64 = 5;
const ESEQUENCE: u64 = 6;
const ENOT_YET_VALID: u64 = 7;
const EEXPIRED: u64 = 8;
const ETTL: u64 = 9;
const ESIGNATURE: u64 = 10;
const EDIGEST_LENGTH: u64 = 11;
const EPUBLIC_KEY_LENGTH: u64 = 12;
const ESIGNATURE_LENGTH: u64 = 13;
const EREGISTRY_BINDING: u64 = 14;
const EPROTOCOL_VERSION: u64 = 15;
const EEMPTY_NETWORK_DOMAIN: u64 = 16;
const EMANY_ISSUERS: u64 = 17;
const EDUPLICATE_ISSUER: u64 = 18;
const ENO_ATTESTATION: u64 = 19;
const ETTL_CONFIGURATION: u64 = 20;
const ETRUSTED_BINDING: u64 = 21;
const ESTALE_ATTESTATION: u64 = 22;

#[test_only]
const ETEST_EVENT: u64 = 100;

const STATUS_ACTIVE: u8 = 0;
const STATUS_REVOKED: u8 = 1;
const DIGEST_LENGTH: u64 = 32;
const PUBLIC_KEY_LENGTH: u64 = 32;
const SIGNATURE_LENGTH: u64 = 64;
const MAX_NETWORK_DOMAIN_LENGTH: u64 = 128;
const MAX_ISSUERS: u64 = 16;
const MAX_TTL_EPOCHS: u64 = 365;
const MAX_BACKDATE_EPOCHS: u64 = 7;
const CURRENT_PROTOCOL_VERSION: u64 = 1;
const DOMAIN: vector<u8> = b"TFA-SUI-ATTEST-v1";

/// Address-owned capability. It deliberately has no store ability, so only
/// this module can transfer it and consumers cannot wrap it in an arbitrary
/// transferable object.
public struct AdminCap has key {
    id: UID,
    registry_id: ID,
}

/// One shared registry defines the issuer set and protocol semantics for many
/// agent records. Sharing makes it addressable, not authorized.
public struct Registry has key {
    id: UID,
    protocol_version: u64,
    issuer_set_epoch: u64,
    max_ttl_epochs: u64,
    max_backdate_epochs: u64,
    network_domain: vector<u8>,
    paused: bool,
    issuers: vector<Issuer>,
}

struct Issuer has copy, drop, store {
    key_id: u64,
    public_key: vector<u8>,
    active: bool,
}

/// Each logical agent has one shared, latest-state object. It stores
/// commitments only; no prompt, response, customer data, wallet, or host name
/// belongs on chain.
public struct AgentRecord has key {
    id: UID,
    registry_id: ID,
    handle_commitment: vector<u8>,
    status: u8,
    sequence: u64,
    issuer_key_id: u64,
    issuer_set_epoch: u64,
    not_before_epoch: u64,
    expires_epoch: u64,
    policy_digest: vector<u8>,
    image_digest: vector<u8>,
    golden_eval_report_digest: vector<u8>,
    evidence_digest: vector<u8>,
}

struct Attestation has copy, drop {
    registry_id: ID,
    agent_record_id: ID,
    protocol_version: u64,
    issuer_key_id: u64,
    issuer_set_epoch: u64,
    sequence: u64,
    not_before_epoch: u64,
    expires_epoch: u64,
    policy_digest: vector<u8>,
    image_digest: vector<u8>,
    golden_eval_report_digest: vector<u8>,
    evidence_digest: vector<u8>,
}

struct SigningPayload has copy, drop {
    domain: vector<u8>,
    network_domain: vector<u8>,
    registry_id: ID,
    agent_record_id: ID,
    protocol_version: u64,
    issuer_key_id: u64,
    issuer_set_epoch: u64,
    sequence: u64,
    not_before_epoch: u64,
    expires_epoch: u64,
    policy_digest: vector<u8>,
    image_digest: vector<u8>,
    golden_eval_report_digest: vector<u8>,
    evidence_digest: vector<u8>,
}

/// A short-lived, non-storable proof for a same-transaction handoff.
/// This type is not a standalone authorization token: a receiving module must
/// call assert_verified_for with its own trusted IDs, commitments, and age
/// policy.
public struct VerifiedAgent has drop {
    registry_id: ID,
    agent_record_id: ID,
    sequence: u64,
    not_before_epoch: u64,
    policy_digest: vector<u8>,
    image_digest: vector<u8>,
    golden_eval_report_digest: vector<u8>,
    evidence_digest: vector<u8>,
}

public struct AgentRegistered has copy, drop {
    registry_id: ID,
    agent_record_id: ID,
}

public struct AttestationSubmitted has copy, drop {
    registry_id: ID,
    agent_record_id: ID,
    issuer_key_id: u64,
    issuer_set_epoch: u64,
    sequence: u64,
    not_before_epoch: u64,
    expires_epoch: u64,
}

public struct AdminCapTransferred has copy, drop {
    registry_id: ID,
    admin_cap_id: ID,
    recipient: address,
}

public struct IssuerAdded has copy, drop {
    registry_id: ID,
    issuer_key_id: u64,
    public_key: vector<u8>,
}

public struct IssuerRotated has copy, drop {
    registry_id: ID,
    issuer_key_id: u64,
    issuer_set_epoch: u64,
    public_key: vector<u8>,
}

public struct RegistryPauseChanged has copy, drop {
    registry_id: ID,
    paused: bool,
}

public struct AgentRevoked has copy, drop {
    registry_id: ID,
    agent_record_id: ID,
}

/// Build a registry without sharing it. Deployment code should immediately use
/// share_registry, and transfer the returned capability to an operator
/// multisig or timelock rather than to an agent host.
public fun new_registry(
    max_ttl_epochs: u64,
    max_backdate_epochs: u64,
    network_domain: vector<u8>,
    initial_issuer_key: vector<u8>,
    ctx: &mut TxContext,
): (Registry, AdminCap) {
    assert!(
        vector::length(&network_domain) > 0
            && vector::length(&network_domain) <= MAX_NETWORK_DOMAIN_LENGTH,
        EEMPTY_NETWORK_DOMAIN,
    );
    assert!(
        max_ttl_epochs > 0 && max_ttl_epochs <= MAX_TTL_EPOCHS,
        ETTL_CONFIGURATION,
    );
    assert!(
        max_backdate_epochs <= MAX_BACKDATE_EPOCHS,
        ETTL_CONFIGURATION,
    );
    assert_public_key(&initial_issuer_key);

    let registry_uid = object::new(ctx);
    let registry_id = object::uid_to_inner(&registry_uid);
    let registry = Registry {
        id: registry_uid,
        protocol_version: CURRENT_PROTOCOL_VERSION,
        issuer_set_epoch: 1,
        max_ttl_epochs,
        max_backdate_epochs,
        network_domain,
        paused: false,
        issuers: vector[Issuer {
            key_id: 0,
            public_key: initial_issuer_key,
            active: true,
        }],
    };
    let cap = AdminCap {
        id: object::new(ctx),
        registry_id,
    };
    (registry, cap)
}

public fun share_registry(registry: Registry) {
    transfer::share_object(registry);
}

public fun transfer_admin_cap(cap: AdminCap, recipient: address) {
    event::emit(AdminCapTransferred {
        registry_id: copy cap.registry_id,
        admin_cap_id: object::uid_to_inner(&cap.id),
        recipient,
    });
    transfer::transfer(cap, recipient);
}

public fun register_agent(
    cap: &AdminCap,
    registry: &mut Registry,
    handle_commitment: vector<u8>,
    ctx: &mut TxContext,
) {
    assert_admin(cap, registry);
    assert!(!registry.paused, EPAUSED);
    assert_digest(&handle_commitment);

    let registry_id = object::uid_to_inner(&registry.id);
    let record_uid = object::new(ctx);
    let record_id = object::uid_to_inner(&record_uid);
    let record = AgentRecord {
        id: record_uid,
        registry_id,
        handle_commitment,
        status: STATUS_ACTIVE,
        sequence: 0,
        issuer_key_id: 0,
        issuer_set_epoch: 0,
        not_before_epoch: 0,
        expires_epoch: 0,
        policy_digest: vector::empty(),
        image_digest: vector::empty(),
        golden_eval_report_digest: vector::empty(),
        evidence_digest: vector::empty(),
    };
    transfer::share_object(record);
    event::emit(AgentRegistered {
        registry_id,
        agent_record_id: record_id,
    });
}

/// Stage one bounded issuer key. A registry deliberately has exactly one active
/// issuer at a time; adding a key alone never broadens signing authority.
public fun add_issuer(
    cap: &AdminCap,
    registry: &mut Registry,
    key_id: u64,
    public_key: vector<u8>,
) {
    assert_admin(cap, registry);
    assert_public_key(&public_key);
    assert!(!issuer_exists(registry, key_id), EDUPLICATE_ISSUER);
    assert!(vector::length(&registry.issuers) < MAX_ISSUERS, EMANY_ISSUERS);
    let event_public_key = copy public_key;
    vector::push_back(
        &mut registry.issuers,
        Issuer {
            key_id,
            public_key,
            active: false,
        },
    );
    event::emit(IssuerAdded {
        registry_id: object::uid_to_inner(&registry.id),
        issuer_key_id: key_id,
        public_key: event_public_key,
    });
}

/// Atomically replace the single active issuer. The target may be a staged key
/// or the current key; every rotation invalidates previous verification proofs.
public fun rotate_issuer(
    cap: &AdminCap,
    registry: &mut Registry,
    key_id: u64,
    public_key: vector<u8>,
) {
    assert_admin(cap, registry);
    assert_public_key(&public_key);
    let event_public_key = copy public_key;
    let index = 0;
    let length = vector::length(&registry.issuers);
    while (index < length) {
        vector::borrow_mut(&mut registry.issuers, index).active = false;
        index = index + 1;
    };
    let issuer = issuer_mut(registry, key_id);
    issuer.public_key = public_key;
    issuer.active = true;
    registry.issuer_set_epoch = registry.issuer_set_epoch + 1;
    event::emit(IssuerRotated {
        registry_id: object::uid_to_inner(&registry.id),
        issuer_key_id: key_id,
        issuer_set_epoch: registry.issuer_set_epoch,
        public_key: event_public_key,
    });
}

public fun set_paused(cap: &AdminCap, registry: &mut Registry, paused: bool) {
    assert_admin(cap, registry);
    registry.paused = paused;
    event::emit(RegistryPauseChanged {
        registry_id: object::uid_to_inner(&registry.id),
        paused,
    });
}

/// Revoke rather than delete: consumers can distinguish a revoked record from
/// one that was never registered.
public fun revoke_agent(cap: &AdminCap, registry: &Registry, record: &mut AgentRecord) {
    assert_admin(cap, registry);
    assert_record_belongs_to_registry(registry, record);
    record.status = STATUS_REVOKED;
    event::emit(AgentRevoked {
        registry_id: object::uid_to_inner(&registry.id),
        agent_record_id: object::uid_to_inner(&record.id),
    });
}

/// Anyone may relay an attestation, but only the single active issuer's fresh,
/// domain-separated Ed25519 signature can update the shared record.
public entry fun submit_attestation(
    registry: &Registry,
    record: &mut AgentRecord,
    issuer_key_id: u64,
    issuer_set_epoch: u64,
    sequence: u64,
    not_before_epoch: u64,
    expires_epoch: u64,
    policy_digest: vector<u8>,
    image_digest: vector<u8>,
    golden_eval_report_digest: vector<u8>,
    evidence_digest: vector<u8>,
    signature: vector<u8>,
    ctx: &TxContext,
) {
    let attestation = Attestation {
        registry_id: object::uid_to_inner(&registry.id),
        agent_record_id: object::uid_to_inner(&record.id),
        protocol_version: registry.protocol_version,
        issuer_key_id,
        issuer_set_epoch,
        sequence,
        not_before_epoch,
        expires_epoch,
        policy_digest,
        image_digest,
        golden_eval_report_digest,
        evidence_digest,
    };
    submit_attestation_inner(registry, record, attestation, signature, ctx);
}

/// Verify a record against a consumer's pinned registry/record IDs,
/// commitments, minimum sequence, and maximum age in epochs. The consumer must
/// own or otherwise authenticate these expected values; accepting only
/// VerifiedAgent would be an unbound universal proof.
public fun verify_bound(
    registry: &Registry,
    record: &AgentRecord,
    expected_registry_id: ID,
    expected_agent_record_id: ID,
    minimum_sequence: u64,
    max_attestation_age_epochs: u64,
    expected_policy_digest: &vector<u8>,
    expected_image_digest: &vector<u8>,
    expected_golden_eval_report_digest: &vector<u8>,
    expected_evidence_digest: &vector<u8>,
    ctx: &TxContext,
): VerifiedAgent {
    let proof = verify_current(registry, record, minimum_sequence, ctx);
    assert_verified_for(
        &proof,
        expected_registry_id,
        expected_agent_record_id,
        minimum_sequence,
        max_attestation_age_epochs,
        expected_policy_digest,
        expected_image_digest,
        expected_golden_eval_report_digest,
        expected_evidence_digest,
        ctx,
    );
    proof
}

/// Re-check a proof handed across a programmable transaction block. Receiving
/// modules must call this with their own pinned IDs, expected commitments, and
/// maximum acceptable age, using the current transaction context.
public fun assert_verified_for(
    proof: &VerifiedAgent,
    expected_registry_id: ID,
    expected_agent_record_id: ID,
    minimum_sequence: u64,
    max_attestation_age_epochs: u64,
    expected_policy_digest: &vector<u8>,
    expected_image_digest: &vector<u8>,
    expected_golden_eval_report_digest: &vector<u8>,
    expected_evidence_digest: &vector<u8>,
    ctx: &TxContext,
) {
    assert!(proof.registry_id == expected_registry_id, ETRUSTED_BINDING);
    assert!(
        proof.agent_record_id == expected_agent_record_id,
        ETRUSTED_BINDING,
    );
    assert!(proof.sequence >= minimum_sequence, ESEQUENCE);
    let current_epoch = tx_context::epoch(ctx);
    assert!(proof.not_before_epoch <= current_epoch, ENOT_YET_VALID);
    assert!(
        current_epoch - proof.not_before_epoch <= max_attestation_age_epochs,
        ESTALE_ATTESTATION,
    );
    assert_digest_matches(&proof.policy_digest, expected_policy_digest);
    assert_digest_matches(&proof.image_digest, expected_image_digest);
    assert_digest_matches(
        &proof.golden_eval_report_digest,
        expected_golden_eval_report_digest,
    );
    assert_digest_matches(&proof.evidence_digest, expected_evidence_digest);
}

fun verify_current(
    registry: &Registry,
    record: &AgentRecord,
    minimum_sequence: u64,
    ctx: &TxContext,
): VerifiedAgent {
    assert_current_protocol(registry);
    assert!(!registry.paused, EPAUSED);
    assert_record_belongs_to_registry(registry, record);
    assert!(record.status == STATUS_ACTIVE, EAGENT_REVOKED);
    assert!(record.sequence > 0, ENO_ATTESTATION);
    assert!(record.sequence >= minimum_sequence, ESEQUENCE);
    assert!(
        record.issuer_set_epoch == registry.issuer_set_epoch,
        EISSUER_SET_EPOCH,
    );
    let current_epoch = tx_context::epoch(ctx);
    assert!(record.not_before_epoch <= current_epoch, ENOT_YET_VALID);
    assert!(current_epoch < record.expires_epoch, EEXPIRED);
    VerifiedAgent {
        registry_id: object::uid_to_inner(&registry.id),
        agent_record_id: object::uid_to_inner(&record.id),
        sequence: record.sequence,
        not_before_epoch: record.not_before_epoch,
        policy_digest: copy record.policy_digest,
        image_digest: copy record.image_digest,
        golden_eval_report_digest: copy record.golden_eval_report_digest,
        evidence_digest: copy record.evidence_digest,
    }
}

fun submit_attestation_inner(
    registry: &Registry,
    record: &mut AgentRecord,
    attestation: Attestation,
    signature: vector<u8>,
    ctx: &TxContext,
) {
    assert_current_protocol(registry);
    assert!(!registry.paused, EPAUSED);
    assert_record_belongs_to_registry(registry, record);
    assert!(record.status == STATUS_ACTIVE, EAGENT_REVOKED);
    assert!(
        attestation.protocol_version == registry.protocol_version,
        EPROTOCOL_VERSION,
    );
    assert!(
        attestation.issuer_set_epoch == registry.issuer_set_epoch,
        EISSUER_SET_EPOCH,
    );
    assert!(attestation.sequence == record.sequence + 1, ESEQUENCE);
    assert!(attestation.expires_epoch > attestation.not_before_epoch, ETTL);
    assert!(
        attestation.expires_epoch - attestation.not_before_epoch <= registry.max_ttl_epochs,
        ETTL,
    );
    let current_epoch = tx_context::epoch(ctx);
    assert!(attestation.not_before_epoch <= current_epoch, ENOT_YET_VALID);
    assert!(
        current_epoch - attestation.not_before_epoch <= registry.max_backdate_epochs,
        ESTALE_ATTESTATION,
    );
    assert!(current_epoch < attestation.expires_epoch, EEXPIRED);
    assert_digest(&attestation.policy_digest);
    assert_digest(&attestation.image_digest);
    assert_digest(&attestation.golden_eval_report_digest);
    assert_digest(&attestation.evidence_digest);
    assert!(vector::length(&signature) == SIGNATURE_LENGTH, ESIGNATURE_LENGTH);

    let issuer = issuer(registry, attestation.issuer_key_id);
    assert!(issuer.active, EISSUER_INACTIVE);
    let bytes = signing_bytes(registry, &attestation);
    assert!(
        ed25519::ed25519_verify(&signature, &issuer.public_key, &bytes),
        ESIGNATURE,
    );

    let sequence = attestation.sequence;
    let issuer_key_id = attestation.issuer_key_id;
    let issuer_set_epoch = attestation.issuer_set_epoch;
    let not_before_epoch = attestation.not_before_epoch;
    let expires_epoch = attestation.expires_epoch;
    record.sequence = sequence;
    record.issuer_key_id = issuer_key_id;
    record.issuer_set_epoch = issuer_set_epoch;
    record.not_before_epoch = not_before_epoch;
    record.expires_epoch = expires_epoch;
    record.policy_digest = attestation.policy_digest;
    record.image_digest = attestation.image_digest;
    record.golden_eval_report_digest = attestation.golden_eval_report_digest;
    record.evidence_digest = attestation.evidence_digest;

    event::emit(AttestationSubmitted {
        registry_id: object::uid_to_inner(&registry.id),
        agent_record_id: object::uid_to_inner(&record.id),
        issuer_key_id,
        issuer_set_epoch,
        sequence,
        not_before_epoch,
        expires_epoch,
    });
}

#[test_only]
/// Require exactly one issuer-add event with the expected indexed payload.
public fun assert_single_issuer_added_event(
    events: &vector<IssuerAdded>,
    expected_registry_id: ID,
    expected_issuer_key_id: u64,
    expected_public_key: &vector<u8>,
) {
    assert!(vector::length(events) == 1, ETEST_EVENT);
    let emitted = vector::borrow(events, 0);
    assert!(emitted.registry_id == expected_registry_id, ETEST_EVENT);
    assert!(
        emitted.issuer_key_id == expected_issuer_key_id,
        ETEST_EVENT,
    );
    assert!(&emitted.public_key == expected_public_key, ETEST_EVENT);
}

#[test_only]
/// Require exactly one issuer-rotation event with the expected indexed payload.
public fun assert_single_issuer_rotated_event(
    events: &vector<IssuerRotated>,
    expected_registry_id: ID,
    expected_issuer_key_id: u64,
    expected_issuer_set_epoch: u64,
    expected_public_key: &vector<u8>,
) {
    assert!(vector::length(events) == 1, ETEST_EVENT);
    let emitted = vector::borrow(events, 0);
    assert!(emitted.registry_id == expected_registry_id, ETEST_EVENT);
    assert!(
        emitted.issuer_key_id == expected_issuer_key_id,
        ETEST_EVENT,
    );
    assert!(
        emitted.issuer_set_epoch == expected_issuer_set_epoch,
        ETEST_EVENT,
    );
    assert!(&emitted.public_key == expected_public_key, ETEST_EVENT);
}

#[test_only]
/// Require exactly one pause event with the expected indexed payload.
public fun assert_single_registry_pause_changed_event(
    events: &vector<RegistryPauseChanged>,
    expected_registry_id: ID,
    expected_paused: bool,
) {
    assert!(vector::length(events) == 1, ETEST_EVENT);
    let emitted = vector::borrow(events, 0);
    assert!(emitted.registry_id == expected_registry_id, ETEST_EVENT);
    assert!(emitted.paused == expected_paused, ETEST_EVENT);
}

#[test_only]
/// Require exactly one AdminCap-transfer event with the expected indexed payload.
public fun assert_single_admin_cap_transferred_event(
    events: &vector<AdminCapTransferred>,
    expected_registry_id: ID,
    expected_admin_cap_id: ID,
    expected_recipient: address,
) {
    assert!(vector::length(events) == 1, ETEST_EVENT);
    let emitted = vector::borrow(events, 0);
    assert!(emitted.registry_id == expected_registry_id, ETEST_EVENT);
    assert!(emitted.admin_cap_id == expected_admin_cap_id, ETEST_EVENT);
    assert!(emitted.recipient == expected_recipient, ETEST_EVENT);
}

#[test_only]
/// Require exactly one revocation event with the expected indexed payload.
public fun assert_single_agent_revoked_event(
    events: &vector<AgentRevoked>,
    expected_registry_id: ID,
    expected_agent_record_id: ID,
) {
    assert!(vector::length(events) == 1, ETEST_EVENT);
    let emitted = vector::borrow(events, 0);
    assert!(emitted.registry_id == expected_registry_id, ETEST_EVENT);
    assert!(
        emitted.agent_record_id == expected_agent_record_id,
        ETEST_EVENT,
    );
}

#[test_only]
/// Construct a non-storable proof for focused consumer-policy unit tests.
public fun test_verified_agent(
    registry_id: ID,
    agent_record_id: ID,
    sequence: u64,
    not_before_epoch: u64,
    policy_digest: vector<u8>,
    image_digest: vector<u8>,
    golden_eval_report_digest: vector<u8>,
    evidence_digest: vector<u8>,
): VerifiedAgent {
    assert_digest(&policy_digest);
    assert_digest(&image_digest);
    assert_digest(&golden_eval_report_digest);
    assert_digest(&evidence_digest);
    VerifiedAgent {
        registry_id,
        agent_record_id,
        sequence,
        not_before_epoch,
        policy_digest,
        image_digest,
        golden_eval_report_digest,
        evidence_digest,
    }
}

#[test_only]
/// Serialize fixture fields through the same BCS path used for verification.
public fun test_signing_bytes(
    network_domain: vector<u8>,
    registry_id: ID,
    agent_record_id: ID,
    protocol_version: u64,
    issuer_key_id: u64,
    issuer_set_epoch: u64,
    sequence: u64,
    not_before_epoch: u64,
    expires_epoch: u64,
    policy_digest: vector<u8>,
    image_digest: vector<u8>,
    golden_eval_report_digest: vector<u8>,
    evidence_digest: vector<u8>,
): vector<u8> {
    let attestation = Attestation {
        registry_id,
        agent_record_id,
        protocol_version,
        issuer_key_id,
        issuer_set_epoch,
        sequence,
        not_before_epoch,
        expires_epoch,
        policy_digest,
        image_digest,
        golden_eval_report_digest,
        evidence_digest,
    };
    signing_payload_bytes(network_domain, &attestation)
}

fun signing_bytes(registry: &Registry, attestation: &Attestation): vector<u8> {
    signing_payload_bytes(copy registry.network_domain, attestation)
}

fun signing_payload_bytes(
    network_domain: vector<u8>,
    attestation: &Attestation,
): vector<u8> {
    bcs::to_bytes(&SigningPayload {
        domain: DOMAIN,
        network_domain,
        registry_id: copy attestation.registry_id,
        agent_record_id: copy attestation.agent_record_id,
        protocol_version: attestation.protocol_version,
        issuer_key_id: attestation.issuer_key_id,
        issuer_set_epoch: attestation.issuer_set_epoch,
        sequence: attestation.sequence,
        not_before_epoch: attestation.not_before_epoch,
        expires_epoch: attestation.expires_epoch,
        policy_digest: copy attestation.policy_digest,
        image_digest: copy attestation.image_digest,
        golden_eval_report_digest: copy attestation.golden_eval_report_digest,
        evidence_digest: copy attestation.evidence_digest,
    })
}

fun assert_current_protocol(registry: &Registry) {
    assert!(
        registry.protocol_version == CURRENT_PROTOCOL_VERSION,
        EPROTOCOL_VERSION,
    );
}

fun assert_admin(cap: &AdminCap, registry: &Registry) {
    assert_current_protocol(registry);
    assert!(
        cap.registry_id == object::uid_to_inner(&registry.id),
        EADMIN_CAP,
    );
}

fun assert_record_belongs_to_registry(registry: &Registry, record: &AgentRecord) {
    assert!(
        record.registry_id == object::uid_to_inner(&registry.id),
        EREGISTRY_BINDING,
    );
}

fun assert_public_key(public_key: &vector<u8>) {
    assert!(
        vector::length(public_key) == PUBLIC_KEY_LENGTH,
        EPUBLIC_KEY_LENGTH,
    );
}

fun assert_digest(digest: &vector<u8>) {
    assert!(vector::length(digest) == DIGEST_LENGTH, EDIGEST_LENGTH);
}

fun assert_digest_matches(actual: &vector<u8>, expected: &vector<u8>) {
    assert_digest(expected);
    assert!(actual == expected, ETRUSTED_BINDING);
}

fun issuer_exists(registry: &Registry, key_id: u64): bool {
    let index = 0;
    let length = vector::length(&registry.issuers);
    while (index < length) {
        if (vector::borrow(&registry.issuers, index).key_id == key_id) {
            return true;
        };
        index = index + 1;
    };
    false
}

fun issuer(registry: &Registry, key_id: u64): &Issuer {
    let index = 0;
    let length = vector::length(&registry.issuers);
    while (index < length) {
        let candidate = vector::borrow(&registry.issuers, index);
        if (candidate.key_id == key_id) {
            return candidate;
        };
        index = index + 1;
    };
    abort EISSUER_NOT_FOUND
}

fun issuer_mut(registry: &mut Registry, key_id: u64): &mut Issuer {
    let index = 0;
    let length = vector::length(&registry.issuers);
    while (index < length) {
        let candidate = vector::borrow_mut(&mut registry.issuers, index);
        if (candidate.key_id == key_id) {
            return candidate;
        };
        index = index + 1;
    };
    abort EISSUER_NOT_FOUND
}
