#[test_only]
module tentaflake_agent_attestation::consumer_capability_example;

use sui::object::{Self, ID, UID};
use sui::tx_context::{Self, TxContext};
use tentaflake_agent_attestation::agent_attestation::{Self, VerifiedAgent};

const EACTOR_CAPABILITY: u64 = 0;

/// Consumer-owned trusted configuration. In production this object must be
/// created and updated only through the consuming package's own governance.
public struct ConsumerPolicy has key {
    id: UID,
    expected_registry_id: ID,
    expected_agent_record_id: ID,
    minimum_sequence: u64,
    max_attestation_age_epochs: u64,
    expected_policy_digest: vector<u8>,
    expected_image_digest: vector<u8>,
    expected_golden_eval_report_digest: vector<u8>,
    expected_evidence_digest: vector<u8>,
}

/// Independent authorization for the protected action. Evidence that an issuer
/// made a statement about an agent never creates this authority.
public struct ProtectedActionCap has key {
    id: UID,
    policy_id: ID,
}

#[test_only]
fun new_policy_and_action_cap(
    expected_registry_id: ID,
    expected_agent_record_id: ID,
    minimum_sequence: u64,
    max_attestation_age_epochs: u64,
    expected_policy_digest: vector<u8>,
    expected_image_digest: vector<u8>,
    expected_golden_eval_report_digest: vector<u8>,
    expected_evidence_digest: vector<u8>,
    ctx: &mut TxContext,
): (ConsumerPolicy, ProtectedActionCap) {
    let policy_uid = object::new(ctx);
    let policy_id = object::uid_to_inner(&policy_uid);
    (
        ConsumerPolicy {
            id: policy_uid,
            expected_registry_id,
            expected_agent_record_id,
            minimum_sequence,
            max_attestation_age_epochs,
            expected_policy_digest,
            expected_image_digest,
            expected_golden_eval_report_digest,
            expected_evidence_digest,
        },
        ProtectedActionCap {
            id: object::new(ctx),
            policy_id,
        },
    )
}

/// A consuming package should perform its protected state change only after
/// both checks return. The policy supplies authenticated IDs, commitments, and
/// maximum age; the current TxContext supplies the epoch. The action capability
/// authorizes the actor independently of agent evidence.
public fun authorize_action(
    policy: &ConsumerPolicy,
    action_cap: &ProtectedActionCap,
    proof: &VerifiedAgent,
    ctx: &TxContext,
) {
    assert_actor_authorized(policy, action_cap);
    agent_attestation::assert_verified_for(
        proof,
        copy policy.expected_registry_id,
        copy policy.expected_agent_record_id,
        copy policy.minimum_sequence,
        copy policy.max_attestation_age_epochs,
        &policy.expected_policy_digest,
        &policy.expected_image_digest,
        &policy.expected_golden_eval_report_digest,
        &policy.expected_evidence_digest,
        ctx,
    );
}

fun assert_actor_authorized(
    policy: &ConsumerPolicy,
    action_cap: &ProtectedActionCap,
) {
    assert!(
        action_cap.policy_id == object::uid_to_inner(&policy.id),
        EACTOR_CAPABILITY,
    );
}

#[test_only]
fun test_digest(): vector<u8> {
    x"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
}

#[test_only]
fun destroy_policy_and_cap(policy: ConsumerPolicy, action_cap: ProtectedActionCap) {
    let ConsumerPolicy {
        id: policy_uid,
        expected_registry_id: _,
        expected_agent_record_id: _,
        minimum_sequence: _,
        max_attestation_age_epochs: _,
        expected_policy_digest: _,
        expected_image_digest: _,
        expected_golden_eval_report_digest: _,
        expected_evidence_digest: _,
    } = policy;
    let ProtectedActionCap {
        id: cap_uid,
        policy_id: _,
    } = action_cap;
    object::delete(policy_uid);
    object::delete(cap_uid);
}

#[test]
fun matching_action_capability_is_accepted() {
    let mut ctx = tx_context::dummy();
    let (policy, action_cap) = new_policy_and_action_cap(
        object::id_from_address(@0x11),
        object::id_from_address(@0x22),
        42,
        1,
        test_digest(),
        test_digest(),
        test_digest(),
        test_digest(),
        &mut ctx,
    );

    assert_actor_authorized(&policy, &action_cap);
    destroy_policy_and_cap(policy, action_cap);
}

#[test]
#[expected_failure(abort_code = EACTOR_CAPABILITY)]
fun action_capability_for_another_policy_is_rejected() {
    let mut ctx = tx_context::dummy();
    let (first_policy, first_cap) = new_policy_and_action_cap(
        object::id_from_address(@0x11),
        object::id_from_address(@0x22),
        42,
        1,
        test_digest(),
        test_digest(),
        test_digest(),
        test_digest(),
        &mut ctx,
    );
    let (second_policy, second_cap) = new_policy_and_action_cap(
        object::id_from_address(@0x33),
        object::id_from_address(@0x44),
        7,
        1,
        test_digest(),
        test_digest(),
        test_digest(),
        test_digest(),
        &mut ctx,
    );

    assert_actor_authorized(&first_policy, &second_cap);

    destroy_policy_and_cap(first_policy, first_cap);
    destroy_policy_and_cap(second_policy, second_cap);
}
