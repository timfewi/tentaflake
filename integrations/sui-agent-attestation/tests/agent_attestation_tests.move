#[test_only]
module tentaflake_agent_attestation::agent_attestation_tests;

use std::vector;
use sui::ed25519;
use sui::event;
use sui::object;
use sui::test_scenario::{Self, Scenario};
use sui::tx_context;
use tentaflake_agent_attestation::agent_attestation::{
    Self,
    AdminCap,
    AdminCapTransferred,
    AgentRecord,
    AgentRevoked,
    IssuerAdded,
    IssuerRotated,
    Registry,
    RegistryPauseChanged,
};

const ADMIN: address = @0xA11CE;

const EADMIN_CAP: u64 = 0;
const EPAUSED: u64 = 1;
const EISSUER_NOT_FOUND: u64 = 3;
const ENOT_YET_VALID: u64 = 7;
const EDIGEST_LENGTH: u64 = 11;
const EPUBLIC_KEY_LENGTH: u64 = 12;
const EEMPTY_NETWORK_DOMAIN: u64 = 16;
const EDUPLICATE_ISSUER: u64 = 18;
const ETTL_CONFIGURATION: u64 = 20;
const ESTALE_ATTESTATION: u64 = 22;

const ETEST_VECTOR: u64 = 100;

/// Public deterministic test key from test-vectors/signing-payload-v1.json.
/// The package contains no private seed.
fun test_public_key(): vector<u8> {
    x"03a107bff3ce10be1d70dd18e74bc09967e4d6309ba50d5f1ddc8664125531b8"
}

/// RFC 8032 public test key. No corresponding private seed is stored here.
fun rotated_test_public_key(): vector<u8> {
    x"d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a"
}

fun test_signature(): vector<u8> {
    x"b81aa8efb03356809da51edfb8dfd6051b3e605885175426e8c8a033499ceaa1e1dcbfeb1ff6a8a8ce6a15e0805b344adf85ed437211d7837a934b66b38a1303"
}

fun test_payload(): vector<u8> {
    x"115446412d5355492d4154544553542d76310b7375693a746573746e6574000000000000000000000000000000000000000000000000000000000000001100000000000000000000000000000000000000000000000000000000000000220100000000000000070000000000000003000000000000002a0000000000000064000000000000006900000000000000200101010101010101010101010101010101010101010101010101010101010101200202020202020202020202020202020202020202020202020202020202020202200303030303030303030303030303030303030303030303030303030303030303200404040404040404040404040404040404040404040404040404040404040404"
}

fun derived_test_payload(): vector<u8> {
    agent_attestation::test_signing_bytes(
        b"sui:testnet",
        object::id_from_address(@0x11),
        object::id_from_address(@0x22),
        1,
        7,
        3,
        42,
        100,
        105,
        x"0101010101010101010101010101010101010101010101010101010101010101",
        x"0202020202020202020202020202020202020202020202020202020202020202",
        x"0303030303030303030303030303030303030303030303030303030303030303",
        x"0404040404040404040404040404040404040404040404040404040404040404",
    )
}

fun test_digest(): vector<u8> {
    x"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
}

fun assert_consumer_age(
    not_before_epoch: u64,
    current_epoch: u64,
    max_attestation_age_epochs: u64,
) {
    let registry_id = object::id_from_address(@0x11);
    let record_id = object::id_from_address(@0x22);
    let digest = test_digest();
    let proof = agent_attestation::test_verified_agent(
        copy registry_id,
        copy record_id,
        42,
        not_before_epoch,
        copy digest,
        copy digest,
        copy digest,
        copy digest,
    );
    let ctx = tx_context::new_from_hint(ADMIN, 0, current_epoch, 0, 0);
    agent_attestation::assert_verified_for(
        &proof,
        registry_id,
        record_id,
        42,
        max_attestation_age_epochs,
        &digest,
        &digest,
        &digest,
        &digest,
        &ctx,
    );
}

fun new_valid_registry(scenario: &mut Scenario): (Registry, AdminCap) {
    agent_attestation::new_registry(
        2,
        1,
        b"sui:testnet",
        test_public_key(),
        test_scenario::ctx(scenario),
    )
}

fun finish_registry(registry: Registry, cap: AdminCap, scenario: Scenario) {
    agent_attestation::share_registry(registry);
    agent_attestation::transfer_admin_cap(cap, ADMIN);
    test_scenario::end(scenario);
}

fun finish_two_registries(
    first_registry: Registry,
    first_cap: AdminCap,
    second_registry: Registry,
    second_cap: AdminCap,
    scenario: Scenario,
) {
    agent_attestation::share_registry(first_registry);
    agent_attestation::share_registry(second_registry);
    agent_attestation::transfer_admin_cap(first_cap, ADMIN);
    agent_attestation::transfer_admin_cap(second_cap, ADMIN);
    test_scenario::end(scenario);
}

#[test]
fun production_bcs_signing_vector_verifies() {
    let expected_payload = test_payload();
    let payload = derived_test_payload();
    let public_key = test_public_key();
    let signature = test_signature();

    assert!(payload == expected_payload, ETEST_VECTOR);
    assert!(vector::length(&payload) == 274, ETEST_VECTOR);
    assert!(
        ed25519::ed25519_verify(&signature, &public_key, &payload),
        ETEST_VECTOR,
    );
}

#[test]
fun production_bcs_signing_vector_rejects_one_byte_mutation() {
    let expected_payload = test_payload();
    let mut payload = derived_test_payload();
    assert!(payload == expected_payload, ETEST_VECTOR);
    {
        let last = vector::borrow_mut(&mut payload, 273);
        *last = *last ^ 1;
    };

    let public_key = test_public_key();
    let signature = test_signature();
    assert!(
        !ed25519::ed25519_verify(&signature, &public_key, &payload),
        ETEST_VECTOR,
    );
}

#[test]
fun consumer_accepts_attestation_at_inclusive_age_boundary() {
    assert_consumer_age(100, 105, 5);
}

#[test]
#[expected_failure(abort_code = ESTALE_ATTESTATION)]
fun consumer_rejects_attestation_older_than_its_policy() {
    assert_consumer_age(100, 106, 5);
}

#[test]
#[expected_failure(abort_code = ENOT_YET_VALID)]
fun consumer_rejects_future_not_before_epoch() {
    assert_consumer_age(100, 99, 5);
}

#[test]
fun accepts_bounded_registry_configuration() {
    let mut scenario = test_scenario::begin(ADMIN);
    let (registry, cap) = new_valid_registry(&mut scenario);
    finish_registry(registry, cap, scenario);
}

#[test]
fun privileged_registry_changes_emit_exact_payloads() {
    let mut scenario = test_scenario::begin(ADMIN);
    let (mut registry, cap) = new_valid_registry(&mut scenario);
    let registry_id = object::id(&registry);
    let admin_cap_id = object::id(&cap);
    let staged_public_key = test_public_key();
    let rotated_public_key = rotated_test_public_key();

    agent_attestation::add_issuer(
        &cap,
        &mut registry,
        7,
        copy staged_public_key,
    );
    agent_attestation::rotate_issuer(
        &cap,
        &mut registry,
        7,
        copy rotated_public_key,
    );
    agent_attestation::set_paused(&cap, &mut registry, true);
    agent_attestation::share_registry(registry);
    agent_attestation::transfer_admin_cap(cap, ADMIN);

    let added = event::events_by_type<IssuerAdded>();
    let rotated = event::events_by_type<IssuerRotated>();
    let paused = event::events_by_type<RegistryPauseChanged>();
    let transferred = event::events_by_type<AdminCapTransferred>();
    agent_attestation::assert_single_issuer_added_event(
        &added,
        copy registry_id,
        7,
        &staged_public_key,
    );
    agent_attestation::assert_single_issuer_rotated_event(
        &rotated,
        copy registry_id,
        7,
        2,
        &rotated_public_key,
    );
    agent_attestation::assert_single_registry_pause_changed_event(
        &paused,
        copy registry_id,
        true,
    );
    agent_attestation::assert_single_admin_cap_transferred_event(
        &transferred,
        registry_id,
        admin_cap_id,
        ADMIN,
    );

    test_scenario::end(scenario);
}

#[test]
fun revocation_emits_typed_event() {
    let mut scenario = test_scenario::begin(ADMIN);
    let (mut registry, cap) = new_valid_registry(&mut scenario);
    agent_attestation::register_agent(
        &cap,
        &mut registry,
        test_digest(),
        test_scenario::ctx(&mut scenario),
    );
    agent_attestation::share_registry(registry);
    agent_attestation::transfer_admin_cap(cap, ADMIN);

    let initial_effects = test_scenario::next_tx(&mut scenario, ADMIN);
    assert!(test_scenario::num_user_events(&initial_effects) == 2, ETEST_VECTOR);
    let registry = test_scenario::take_shared<Registry>(&scenario);
    let mut record = test_scenario::take_shared<AgentRecord>(&scenario);
    let cap = test_scenario::take_from_sender<AdminCap>(&scenario);
    let registry_id = object::id(&registry);
    let record_id = object::id(&record);

    agent_attestation::revoke_agent(&cap, &registry, &mut record);
    let revoked = event::events_by_type<AgentRevoked>();
    agent_attestation::assert_single_agent_revoked_event(
        &revoked,
        registry_id,
        record_id,
    );

    test_scenario::return_shared(registry);
    test_scenario::return_shared(record);
    test_scenario::return_to_sender(&scenario, cap);
    let revoke_effects = test_scenario::next_tx(&mut scenario, ADMIN);
    assert!(test_scenario::num_user_events(&revoke_effects) == 1, ETEST_VECTOR);
    test_scenario::end(scenario);
}

#[test]
#[expected_failure(abort_code = EEMPTY_NETWORK_DOMAIN)]
fun rejects_empty_network_domain() {
    let mut scenario = test_scenario::begin(ADMIN);
    let (registry, cap) = agent_attestation::new_registry(
        2,
        1,
        vector::empty(),
        test_public_key(),
        test_scenario::ctx(&mut scenario),
    );
    finish_registry(registry, cap, scenario);
}

#[test]
#[expected_failure(abort_code = ETTL_CONFIGURATION)]
fun rejects_zero_ttl() {
    let mut scenario = test_scenario::begin(ADMIN);
    let (registry, cap) = agent_attestation::new_registry(
        0,
        1,
        b"sui:testnet",
        test_public_key(),
        test_scenario::ctx(&mut scenario),
    );
    finish_registry(registry, cap, scenario);
}

#[test]
#[expected_failure(abort_code = ETTL_CONFIGURATION)]
fun rejects_ttl_above_protocol_bound() {
    let mut scenario = test_scenario::begin(ADMIN);
    let (registry, cap) = agent_attestation::new_registry(
        366,
        1,
        b"sui:testnet",
        test_public_key(),
        test_scenario::ctx(&mut scenario),
    );
    finish_registry(registry, cap, scenario);
}

#[test]
#[expected_failure(abort_code = ETTL_CONFIGURATION)]
fun rejects_backdate_above_protocol_bound() {
    let mut scenario = test_scenario::begin(ADMIN);
    let (registry, cap) = agent_attestation::new_registry(
        2,
        8,
        b"sui:testnet",
        test_public_key(),
        test_scenario::ctx(&mut scenario),
    );
    finish_registry(registry, cap, scenario);
}

#[test]
#[expected_failure(abort_code = EPUBLIC_KEY_LENGTH)]
fun rejects_short_initial_public_key() {
    let mut scenario = test_scenario::begin(ADMIN);
    let mut public_key = test_public_key();
    let _removed = vector::pop_back(&mut public_key);
    let (registry, cap) = agent_attestation::new_registry(
        2,
        1,
        b"sui:testnet",
        public_key,
        test_scenario::ctx(&mut scenario),
    );
    finish_registry(registry, cap, scenario);
}

#[test]
#[expected_failure(abort_code = EDUPLICATE_ISSUER)]
fun rejects_duplicate_issuer_id() {
    let mut scenario = test_scenario::begin(ADMIN);
    let (mut registry, cap) = new_valid_registry(&mut scenario);

    agent_attestation::add_issuer(&cap, &mut registry, 7, test_public_key());
    agent_attestation::add_issuer(&cap, &mut registry, 7, test_public_key());

    finish_registry(registry, cap, scenario);
}

#[test]
#[expected_failure(abort_code = EISSUER_NOT_FOUND)]
fun rotation_requires_a_staged_issuer() {
    let mut scenario = test_scenario::begin(ADMIN);
    let (mut registry, cap) = new_valid_registry(&mut scenario);

    agent_attestation::rotate_issuer(&cap, &mut registry, 7, test_public_key());

    finish_registry(registry, cap, scenario);
}

#[test]
#[expected_failure(abort_code = EPAUSED)]
fun pause_blocks_agent_registration() {
    let mut scenario = test_scenario::begin(ADMIN);
    let (mut registry, cap) = new_valid_registry(&mut scenario);

    agent_attestation::set_paused(&cap, &mut registry, true);
    agent_attestation::register_agent(
        &cap,
        &mut registry,
        test_digest(),
        test_scenario::ctx(&mut scenario),
    );

    finish_registry(registry, cap, scenario);
}

#[test]
#[expected_failure(abort_code = EDIGEST_LENGTH)]
fun rejects_short_handle_commitment() {
    let mut scenario = test_scenario::begin(ADMIN);
    let (mut registry, cap) = new_valid_registry(&mut scenario);
    let mut handle_commitment = test_digest();
    let _removed = vector::pop_back(&mut handle_commitment);

    agent_attestation::register_agent(
        &cap,
        &mut registry,
        handle_commitment,
        test_scenario::ctx(&mut scenario),
    );

    finish_registry(registry, cap, scenario);
}

#[test]
#[expected_failure(abort_code = EADMIN_CAP)]
fun rejects_admin_cap_from_another_registry() {
    let mut scenario = test_scenario::begin(ADMIN);
    let (mut first_registry, first_cap) = new_valid_registry(&mut scenario);
    let (second_registry, second_cap) = new_valid_registry(&mut scenario);

    agent_attestation::register_agent(
        &second_cap,
        &mut first_registry,
        test_digest(),
        test_scenario::ctx(&mut scenario),
    );

    finish_two_registries(
        first_registry,
        first_cap,
        second_registry,
        second_cap,
        scenario,
    );
}
