use super::*;
use crate::ledger::api::*;

fn api_context() -> ApiContext {
    ApiContext {
        authority: authority("policy-1"),
        host: "host-1".into(),
        principal: "owner-1".into(),
        epoch: "mutation-1".into(),
        now: 100,
    }
}

fn spec(name: &str) -> EnvironmentSpec {
    EnvironmentSpec {
        template_id: "offline".into(),
        template_revision: "template-1".into(),
        name: "Public test environment".into(),
        image_id: "image-1".into(),
        agent_runtime: None,
        tools: vec!["shell".into()],
        skills: Vec::new(),
        instance: context(name, 1).0.instance,
        workspace_id: format!("workspace-id-{name}"),
        grants: vec![
            ActionGrant {
                id: format!("read-{name}"),
                action: ApiAction::EnvironmentsRead,
                target: ApiTarget::Environment { id: name.into() },
                expires_at: None,
            },
            ActionGrant {
                id: format!("workspace-read-{name}"),
                action: ApiAction::WorkspacesRead,
                target: ApiTarget::Workspace {
                    id: format!("workspace-id-{name}"),
                },
                expires_at: None,
            },
        ],
    }
}

fn mutation() -> ApiMutation {
    ApiMutation {
        operation_id: "api-op-1".into(),
        key: ApiKey {
            host: "host-1".into(),
            principal: "owner-1".into(),
            epoch: "mutation-1".into(),
            method: ApiMethod::Post,
            canonical_target: "/api/v1/environments".into(),
            key: "00000000-0000-4000-8000-000000000001".into(),
        },
        request_digest: format!("sha256:{}", "a".repeat(64)),
    }
}

fn policy(spec: &EnvironmentSpec) -> CreatePolicy {
    CreatePolicy {
        policy: context("env-a", 1).0.policy,
        create_grant: ActionGrant {
            id: "create-1".into(),
            action: ApiAction::EnvironmentsCreate,
            target: ApiTarget::Host {
                id: "host-1".into(),
            },
            expires_at: None,
        },
        issued_grants: spec.grants.clone(),
    }
}

fn execution() -> ApiExecution {
    ApiExecution {
        boot_id: "00000000-0000-4000-8000-000000000001".into(),
        supervisor: SupervisorIdentity {
            unit: "tentaflake-create@api-op-1.service".into(),
            invocation_id: "a".repeat(32),
            pid: 42,
            start_time_ticks: 123,
        },
    }
}

fn queue(ledger: &Ledger) -> ApiOperation {
    match ledger
        .api_queue_create(
            &api_context(),
            &mutation(),
            &spec("env-a"),
            0,
            |_, snapshot, spec| {
                assert!(snapshot.storage.is_empty());
                Ok(policy(spec))
            },
        )
        .unwrap()
    {
        ApiQueued::New(op) => *op,
        _ => panic!("expected new queue record"),
    }
}

fn claim(ledger: &Ledger) -> CreationPermit<'_> {
    match ledger
        .api_claim_create(
            &api_context(),
            "api-op-1",
            &execution(),
            |_, snapshot, _, _| {
                assert_eq!(snapshot.storage.len(), 2);
                Ok(())
            },
        )
        .unwrap()
    {
        CreateClaim::New(permit) => permit,
        _ => panic!("expected new create permit"),
    }
}

#[test]
fn stopped_creation_uses_native_storage_once_and_stable_resource_revisions() {
    let fixture = Fixture::new();
    let ledger = fixture.ledger_with(inventory(&[]));
    let queued = queue(&ledger);
    assert_eq!(queued.state, ApiOperationState::Queued);
    let queued_snapshot = ledger.snapshot().unwrap();
    assert_eq!(queued_snapshot.storage.len(), 2);
    assert!(queued_snapshot.entries.is_empty());
    assert!(
        queued_snapshot
            .storage
            .iter()
            .all(|row| row.state == StorageState::Unknown)
    );
    let before_login = ledger.api_snapshot(&api_context()).unwrap();
    ledger
        .consume_operator_login(&authority("policy-1"), &"a".repeat(64))
        .unwrap();
    let after_login = ledger.api_snapshot(&api_context()).unwrap();
    assert_eq!(before_login.grants_revision, after_login.grants_revision);
    assert_eq!(before_login.environments, after_login.environments);
    assert_eq!(before_login.workspaces, after_login.workspaces);
    let mut permit = claim(&ledger);
    assert!(matches!(ledger.snapshot(), Err(LedgerError::Busy)));
    for kind in [StorageKind::Workspace, StorageKind::State] {
        permit.provision(kind, |_| Ok(())).unwrap();
    }
    assert!(matches!(
        permit.provision(StorageKind::Workspace, |_| panic!("duplicate create")),
        Err(LedgerError::Conflict)
    ));
    let completed = permit.complete(101).unwrap();
    assert_eq!(completed.state, ApiOperationState::Succeeded);
    assert!(completed.result.is_some());
    let reopened = fixture.open("ledger").unwrap();
    assert!(matches!(
        reopened.api_claim_create(
            &api_context(),
            "api-op-1",
            &execution(),
            |_, _, _, _| panic!("replayed helper")
        ),
        Ok(CreateClaim::Existing(_))
    ));
    let snapshot = reopened.snapshot().unwrap();
    assert_eq!(snapshot.storage.len(), 2);
    assert!(
        snapshot
            .storage
            .iter()
            .all(|row| row.state == StorageState::Present)
    );
    assert!(snapshot.entries.is_empty());
    assert_eq!(
        reopened
            .api_snapshot(&api_context())
            .unwrap()
            .grants_revision,
        before_login.grants_revision
    );
    let request = publication(&reopened, "publish-1", "policy-2");
    reopened.begin_publication(&request).unwrap();
    reopened
        .finish_publication(
            "publish-1",
            reopened.snapshot().unwrap().revision,
            |_, _| installed(&request.candidate),
        )
        .unwrap();
    let mut changed = api_context();
    changed.authority = request.candidate;
    assert!(matches!(
        reopened.api_lookup(&changed, &mutation().key),
        Ok(ApiLookup::Found(_))
    ));
    let retained = reopened.api_environments(&changed.authority).unwrap();
    assert_eq!(retained[0].1, authority("policy-1"));
    assert_eq!(retained[0].0.spec.template_revision, "template-1");
}

#[test]
fn partial_creation_retains_both_charges_and_recovery_never_recreates() {
    let fixture = Fixture::new();
    let ledger = fixture.ledger_with(inventory(&[]));
    queue(&ledger);
    let mut permit = claim(&ledger);
    permit
        .provision(StorageKind::Workspace, |_| Ok(()))
        .unwrap();
    assert!(matches!(
        permit.provision(StorageKind::State, |_| Err(
            "helper failed after dispatch".into()
        )),
        Err(LedgerError::Context)
    ));
    assert!(matches!(
        permit.provision(StorageKind::State, |_| panic!("poisoned permit")),
        Err(LedgerError::Context)
    ));
    drop(permit);
    let reopened = fixture.open("ledger").unwrap();
    let snapshot = reopened.snapshot().unwrap();
    assert_eq!(snapshot.storage.len(), 2);
    assert_eq!(snapshot.storage[0].state, StorageState::Present);
    assert_eq!(snapshot.storage[1].state, StorageState::Unknown);
    assert!(matches!(
        reopened.api_claim_create(
            &api_context(),
            "api-op-1",
            &execution(),
            |_, _, _, _| panic!("duplicate dispatch")
        ),
        Ok(CreateClaim::Existing(_))
    ));
    assert!(matches!(
        reopened.begin_publication(&publication(&reopened, "publish-1", "policy-2")),
        Err(LedgerError::NotQuiescent)
    ));
    assert!(matches!(
        reopened.reconcile_storage("state-env-a", snapshot.revision, |_| panic!(
            "unlinked recovery"
        )),
        Err(LedgerError::Conflict)
    ));
    assert!(matches!(
        reopened.api_recover_create(
            &api_context(),
            "api-op-1",
            snapshot.revision - 1,
            |_, _, _| panic!("stale observation")
        ),
        Err(LedgerError::StaleRevision)
    ));
    assert!(matches!(
        reopened.api_recover_create(
            &api_context(),
            "api-op-1",
            snapshot.revision,
            |_, _, _| Err("missing final image".into())
        ),
        Err(LedgerError::Context)
    ));
    assert_eq!(reopened.snapshot().unwrap(), snapshot);
    let recovered = reopened
        .api_recover_create(
            &api_context(),
            "api-op-1",
            snapshot.revision,
            |op, spec, rows| {
                assert_eq!(op.execution, Some(execution()));
                assert_eq!(spec.instance.id, "env-a");
                assert_eq!(rows.len(), 2);
                Ok(())
            },
        )
        .unwrap();
    assert_eq!(recovered.state, ApiOperationState::Succeeded);
    assert!(matches!(
        reopened.api_recover_create(
            &api_context(),
            "api-op-1",
            reopened.snapshot().unwrap().revision,
            |_, _, _| panic!("terminal recovery replay")
        ),
        Err(LedgerError::Conflict)
    ));
}

#[test]
fn queued_history_and_exact_keys_do_not_create_authority() {
    let fixture = Fixture::new();
    let ledger = fixture.ledger_with(inventory(&[]));
    let op = queue(&ledger);
    assert!(matches!(
        ledger.begin_publication(&publication(&ledger, "publish-1", "policy-2")),
        Err(LedgerError::NotQuiescent)
    ));
    let mut retry = mutation();
    retry.operation_id = "unused-server-generated-id".into();
    assert_eq!(
        ledger
            .api_queue_create(
                &api_context(),
                &retry,
                &spec("other-env"),
                999,
                |_, _, _| panic!("retry dispatch")
            )
            .unwrap(),
        ApiQueued::Existing(Box::new(op.clone()))
    );
    retry.request_digest = format!("sha256:{}", "b".repeat(64));
    assert!(matches!(
        ledger.api_queue_create(&api_context(), &retry, &spec("env-a"), 0, |_, _, _| panic!(
            "changed request"
        )),
        Err(LedgerError::Conflict)
    ));
    let mut foreign = api_context();
    foreign.principal = "other-owner".into();
    assert!(matches!(
        ledger.api_lookup(&foreign, &mutation().key),
        Err(LedgerError::Conflict)
    ));
    assert!(
        ledger
            .api_operation(&foreign, "api-op-1")
            .unwrap()
            .is_none()
    );
    assert!(
        ledger
            .api_snapshot(&foreign)
            .unwrap()
            .environments
            .is_empty()
    );
    assert_eq!(
        ledger
            .api_environments(&authority("policy-1"))
            .unwrap()
            .len(),
        1
    );
    let mut absent = mutation().key;
    absent.key = "00000000-0000-4000-8000-000000000002".into();
    assert_eq!(
        ledger.api_lookup(&api_context(), &absent).unwrap(),
        ApiLookup::Absent
    );
    absent.epoch = "retired-epoch".into();
    assert_eq!(
        ledger.api_lookup(&api_context(), &absent).unwrap(),
        ApiLookup::Unknown
    );
    let mut restarted_context = api_context();
    restarted_context.epoch = "next-epoch".into();
    assert!(matches!(
        ledger.api_claim_create(
            &restarted_context,
            "api-op-1",
            &execution(),
            |_, _, _, _| Ok(())
        ),
        Ok(CreateClaim::New(_))
    ));
}

#[test]
fn both_budgets_and_physical_space_are_checked_before_any_durable_charge() {
    for physical_denial in [false, true] {
        let fixture = Fixture::new();
        let ledger = fixture.ledger_with(inventory(&[]));
        let before = ledger.snapshot().unwrap();
        let result = ledger.api_queue_create(
            &api_context(),
            &mutation(),
            &spec("env-a"),
            0,
            |_, _, spec| {
                if physical_denial {
                    return Err("insufficient full logical pool charge".into());
                }
                let mut current = policy(spec);
                current.policy.capacity.state_bytes = MIN_STORAGE_BYTES;
                current.policy.host_reserve.state_bytes = MIN_STORAGE_BYTES;
                Ok(current)
            },
        );
        assert!(matches!(
            result,
            Err(LedgerError::Context | LedgerError::StorageDenied(_))
        ));
        assert_eq!(ledger.snapshot().unwrap(), before);
        assert_eq!(
            ledger.api_lookup(&api_context(), &mutation().key).unwrap(),
            ApiLookup::Absent
        );
    }
}

#[test]
fn creation_rejects_unissued_grants_invalid_resources_and_native_collisions() {
    for case in 0..10 {
        let fixture = Fixture::new();
        let ledger = fixture.ledger_with(inventory(&[]));
        let mut wanted = spec("env-a");
        match case {
            0 => wanted.instance.resources.nano_cpus = 0,
            1 => wanted.instance.owner_id = "foreign".into(),
            2 => {
                wanted.grants[0].target = ApiTarget::Environment {
                    id: "foreign".into(),
                }
            }
            3 => wanted.grants[0].expires_at = Some(100),
            4 => wanted
                .instance
                .requested_capabilities
                .push(Capability::Model),
            5 => wanted.instance.storage[1].id = wanted.instance.storage[0].id.clone(),
            _ => {}
        }
        assert!(
            ledger
                .api_queue_create(&api_context(), &mutation(), &wanted, 0, |_, _, spec| {
                    let mut policy = policy(spec);
                    if case == 6 {
                        policy.create_grant.target = ApiTarget::Host {
                            id: "foreign".into(),
                        };
                    }
                    if case == 7 {
                        policy.issued_grants.clear();
                    }
                    if case == 8 {
                        policy.create_grant.expires_at = Some(200);
                    }
                    if case == 9 {
                        policy.policy.per_instance.nano_cpus =
                            spec.instance.resources.nano_cpus - 1;
                    }
                    Ok(policy)
                })
                .is_err()
        );
        assert_eq!(ledger.snapshot().unwrap().revision, 1);
    }
    let fixture = Fixture::new();
    let ledger = fixture.ledger_with(inventory(&[]));
    queue(&ledger);
    let (mut snapshot, mut request) = context("env-a", ledger.snapshot().unwrap().revision);
    snapshot.reservations = ledger.snapshot().unwrap();
    request.operation_id = "native-start".into();
    assert!(matches!(
        ledger.claim(&request, |_| Ok(RuntimeContext {
            snapshot: snapshot.clone(),
            now: 100,
            intent: None
        })),
        Err(LedgerError::Conflict)
    ));
    let storage = StorageRequest {
        request,
        allocation_id: "state-env-a".into(),
    };
    assert!(matches!(
        ledger.reserve_storage(&storage, |_| panic!("unlinked dynamic reserve")),
        Err(LedgerError::Conflict)
    ));
    assert!(matches!(
        ledger.with_present_storage(
            &authority("policy-1"),
            &storage,
            |_| panic!("unlinked owner policy"),
            |_| -> Result<(), String> { panic!("unlinked owner mutation") }
        ),
        Err(LedgerError::Conflict)
    ));
}

#[test]
fn failed_claim_write_yields_no_permit_and_preserves_queued_history() {
    let fixture = Fixture::new();
    let ledger = fixture.ledger_with(inventory(&[]));
    queue(&ledger);
    fs::create_dir(fixture.path().join(STAGING)).unwrap();
    assert!(matches!(
        ledger.api_claim_create(
            &api_context(),
            "api-op-1",
            &execution(),
            |_, _, _, _| Ok(())
        ),
        Err(LedgerError::Io(_))
    ));
    assert_eq!(
        ledger
            .api_operation(&api_context(), "api-op-1")
            .unwrap()
            .unwrap()
            .state,
        ApiOperationState::Queued
    );
    fs::remove_dir(fixture.path().join(STAGING)).unwrap();
    let permit = claim(&ledger);
    drop(permit);
    assert_eq!(
        ledger
            .api_operation(&api_context(), "api-op-1")
            .unwrap()
            .unwrap()
            .state,
        ApiOperationState::Unknown
    );
}

#[test]
fn supervisor_identity_and_revalidation_fail_before_claim() {
    let fixture = Fixture::new();
    let ledger = fixture.ledger_with(inventory(&[]));
    queue(&ledger);
    let revision = ledger.snapshot().unwrap().revision;
    let mut invalid = execution();
    invalid.supervisor.invocation_id = "0".repeat(32);
    assert!(matches!(
        ledger.api_claim_create(&api_context(), "api-op-1", &invalid, |_, _, _, _| panic!(
            "invalid identity"
        )),
        Err(LedgerError::Invalid(_))
    ));
    assert!(matches!(
        ledger.api_claim_create(&api_context(), "api-op-1", &execution(), |_, _, _, _| Err(
            "source generation changed".into()
        )),
        Err(LedgerError::Context)
    ));
    assert_eq!(ledger.snapshot().unwrap().revision, revision);
}

#[test]
#[ignore = "subprocess helper"]
fn crash_entry() {
    let root = PathBuf::from(std::env::var_os("TENTAFLAKE_API_TEST_ROOT").unwrap());
    let ledger = Ledger::open_in(fixture_directory(&root, "ledger").unwrap()).unwrap();
    let mut permit = claim(&ledger);
    if std::env::var("TENTAFLAKE_API_CRASH_PHASE").unwrap() == "partial" {
        permit
            .provision(StorageKind::Workspace, |_| {
                let mut marker = File::create(root.join("created-workspace")).unwrap();
                marker.write_all(b"created exactly once").unwrap();
                marker.sync_all().unwrap();
                Ok(())
            })
            .unwrap();
    }
    unsafe {
        libc::_exit(0);
    }
}

#[test]
fn process_death_after_claim_or_first_helper_preserves_intent_and_both_charges() {
    for phase in ["claim", "partial"] {
        let fixture = Fixture::new();
        let ledger = fixture.ledger_with(inventory(&[]));
        queue(&ledger);
        let output = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "ledger::tests::api_tests::crash_entry",
                "--ignored",
            ])
            .env("TENTAFLAKE_API_TEST_ROOT", &fixture.0)
            .env("TENTAFLAKE_API_CRASH_PHASE", phase)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let reopened = fixture.open("ledger").unwrap();
        let snapshot = reopened.snapshot().unwrap();
        assert_eq!(snapshot.storage.len(), 2);
        assert_eq!(
            snapshot
                .storage
                .iter()
                .filter(|row| row.state == StorageState::Present)
                .count(),
            usize::from(phase == "partial")
        );
        assert_eq!(
            fixture.0.join("created-workspace").exists(),
            phase == "partial"
        );
        let op = reopened
            .api_operation(&api_context(), "api-op-1")
            .unwrap()
            .unwrap();
        assert_eq!(op.state, ApiOperationState::Unknown);
        assert_eq!(op.execution, Some(execution()));
        assert!(matches!(
            reopened.api_claim_create(
                &api_context(),
                "api-op-1",
                &execution(),
                |_, _, _, _| panic!("crash replay")
            ),
            Ok(CreateClaim::Existing(_))
        ));
    }
}

#[test]
fn private_v3_without_dynamic_fields_loads_and_tampered_dynamic_records_refuse() {
    let fixture = Fixture::new();
    let ledger = fixture.ledger_with(inventory(&[]));
    let mut legacy: serde_json::Value =
        serde_json::from_slice(&fs::read(fixture.path().join(STATE)).unwrap()).unwrap();
    legacy.as_object_mut().unwrap().remove("api");
    fs::write(
        fixture.path().join(STATE),
        serde_json::to_vec(&legacy).unwrap(),
    )
    .unwrap();
    assert!(
        ledger
            .api_snapshot(&api_context())
            .unwrap()
            .environments
            .is_empty()
    );
    queue(&ledger);
    let original: serde_json::Value =
        serde_json::from_slice(&fs::read(fixture.path().join(STATE)).unwrap()).unwrap();
    for case in 0..9 {
        let mut broken = original.clone();
        match case {
            0 => broken["api"]["operations"][0]["mutation"]["requestDigest"] = "sha256:bad".into(),
            1 => broken["api"]["operations"][0]["storageOperations"][0] = "foreign".into(),
            2 => broken["api"]["environments"][0]["spec"]["instance"]["ownerId"] = "foreign".into(),
            3 => broken["api"]["operations"][0]["state"] = "succeeded".into(),
            4 => broken["api"]["grantsRevision"] = 1.into(),
            5 => broken["api"]["workspaces"][0]["allocationId"] = "foreign".into(),
            6 => broken["api"]["operations"][0]["authority"]["revision"] = "foreign".into(),
            7 => broken["api"]["operations"][0]["queuedRevision"] = 2.into(),
            _ => broken["api"]["environments"]
                .as_array_mut()
                .unwrap()
                .push(original["api"]["environments"][0].clone()),
        }
        fs::write(
            fixture.path().join(STATE),
            serde_json::to_vec(&broken).unwrap(),
        )
        .unwrap();
        assert!(
            fixture.open("ledger").is_err(),
            "accepted tamper case {case}"
        );
    }
}

#[test]
fn successful_helper_followed_by_failed_commit_poison_retains_original_charges() {
    let fixture = Fixture::new();
    let ledger = fixture.ledger_with(inventory(&[]));
    queue(&ledger);
    let mut permit = claim(&ledger);
    let before = fs::read(fixture.path().join(STATE)).unwrap();
    assert!(matches!(
        permit.provision(StorageKind::Workspace, |_| {
            fs::write(fixture.0.join("created-workspace"), b"helper completed").unwrap();
            fs::create_dir(fixture.path().join(STAGING)).unwrap();
            Ok(())
        }),
        Err(LedgerError::Io(_))
    ));
    assert!(fixture.0.join("created-workspace").exists());
    assert!(matches!(
        permit.provision(StorageKind::State, |_| panic!("write-failed permit replay")),
        Err(LedgerError::Context)
    ));
    assert!(matches!(permit.complete(101), Err(LedgerError::Context)));
    assert_eq!(fs::read(fixture.path().join(STATE)).unwrap(), before);
    let reopened = fixture.open("ledger").unwrap();
    assert!(
        reopened
            .snapshot()
            .unwrap()
            .storage
            .iter()
            .all(|row| row.state == StorageState::Unknown)
    );
    assert!(matches!(
        reopened.api_claim_create(
            &api_context(),
            "api-op-1",
            &execution(),
            |_, _, _, _| panic!("uncertain helper replay")
        ),
        Ok(CreateClaim::Existing(_))
    ));
}

#[test]
fn login_revision_collision_and_unlinked_native_allocations_are_not_loadable() {
    let fixture = Fixture::new();
    let ledger = fixture.ledger_with(inventory(&[]));
    queue(&ledger);
    ledger
        .consume_operator_login(&authority("policy-1"), &"a".repeat(64))
        .unwrap();
    let original: serde_json::Value =
        serde_json::from_slice(&fs::read(fixture.path().join(STATE)).unwrap()).unwrap();
    let mut collision = original.clone();
    collision["operatorLogins"][0]["consumedRevision"] =
        collision["api"]["operations"][0]["queuedRevision"].clone();
    collision["revision"] = collision["api"]["operations"][0]["queuedRevision"].clone();
    fs::write(
        fixture.path().join(STATE),
        serde_json::to_vec(&collision).unwrap(),
    )
    .unwrap();
    assert!(fixture.open("ledger").is_err());
    let mut hidden = original;
    let mut row = hidden["storage"][0].clone();
    row["request"] = serde_json::Value::Null;
    row["claimedRevision"] = 1.into();
    row["updatedRevision"] = 1.into();
    row["allocation"]["id"] = "hidden-allocation".into();
    hidden["storage"].as_array_mut().unwrap().push(row);
    fs::write(
        fixture.path().join(STATE),
        serde_json::to_vec(&hidden).unwrap(),
    )
    .unwrap();
    assert!(fixture.open("ledger").is_err());
}

#[test]
fn unknown_slot_cannot_fabricate_an_observed_update_after_dispatch() {
    let fixture = Fixture::new();
    let ledger = fixture.ledger_with(inventory(&[]));
    queue(&ledger);
    drop(claim(&ledger));
    let path = fixture.path().join(STATE);
    let mut value: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    let revision = value["revision"].as_u64().unwrap() + 1;
    value["revision"] = revision.into();
    value["storage"][0]["updatedRevision"] = revision.into();
    fs::write(path, serde_json::to_vec(&value).unwrap()).unwrap();
    assert!(fixture.open("ledger").is_err());
}

#[test]
fn request_uuid_case_is_one_identity_and_nil_is_valid_but_boot_identity_stays_strict() {
    for initial_key in [
        "ABCDEFAB-CDEF-4ABC-8ABC-ABCDEFABCDEF",
        "abcdefab-cdef-4abc-8abc-abcdefabcdef",
        "00000000-0000-0000-0000-000000000000",
    ] {
        let fixture = Fixture::new();
        let ledger = fixture.ledger_with(inventory(&[]));
        let mut request = mutation();
        request.key.key = initial_key.into();
        let ApiQueued::New(created) = ledger
            .api_queue_create(&api_context(), &request, &spec("env-a"), 0, |_, _, spec| {
                Ok(policy(spec))
            })
            .unwrap()
        else {
            panic!("new request");
        };
        assert_eq!(created.mutation.key.key, initial_key.to_ascii_lowercase());
        for key in [
            initial_key.to_ascii_uppercase(),
            initial_key.to_ascii_lowercase(),
        ] {
            request.key.key = key;
            assert_eq!(
                ledger.api_lookup(&api_context(), &request.key).unwrap(),
                ApiLookup::Found(created.clone())
            );
            assert_eq!(
                ledger
                    .api_queue_create(
                        &api_context(),
                        &request,
                        &spec("unused"),
                        999,
                        |_, _, _| panic!("UUID alias redispatch")
                    )
                    .unwrap(),
                ApiQueued::Existing(created.clone())
            );
        }
        request.request_digest = format!("sha256:{}", "b".repeat(64));
        assert!(matches!(
            ledger.api_queue_create(
                &api_context(),
                &request,
                &spec("unused"),
                0,
                |_, _, _| panic!("changed body")
            ),
            Err(LedgerError::Conflict)
        ));
        for bad_boot in [
            "00000000-0000-0000-0000-000000000000",
            "ABCDEFAB-CDEF-4ABC-8ABC-ABCDEFABCDEF",
            "not-a-uuid",
        ] {
            let mut identity = execution();
            identity.boot_id = bad_boot.into();
            assert!(matches!(
                ledger.api_claim_create(
                    &api_context(),
                    "api-op-1",
                    &identity,
                    |_, _, _, _| panic!("invalid boot")
                ),
                Err(LedgerError::Invalid(_))
            ));
        }
        let mut malformed = request.key.clone();
        malformed.key = "abcdefab-cdef-4abc-8abc-abcdefabcdeg".into();
        assert!(matches!(
            ledger.api_lookup(&api_context(), &malformed),
            Err(LedgerError::Invalid(_))
        ));
        assert!(matches!(
            ledger.api_queue_create(
                &api_context(),
                &ApiMutation {
                    key: malformed,
                    ..request
                },
                &spec("unused"),
                0,
                |_, _, _| panic!("malformed key")
            ),
            Err(LedgerError::Invalid(_))
        ));
        if initial_key.contains('A') {
            let path = fixture.path().join(STATE);
            let mut value: serde_json::Value =
                serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
            value["api"]["operations"][0]["mutation"]["key"]["key"] = initial_key.into();
            fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
            assert!(fixture.open("ledger").is_err());
        }
    }
}
