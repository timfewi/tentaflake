//! The test backend replaces only privileged installed-plan/ledger reads. HTTP,
//! Unix framing, cookie authentication, dispatch and public projection are real.
//! Durable storage and Pending semantics are tested in the admission crate.
use super::*;
use crate::api::{operations::Read, server};
use tentaflake_admission::ledger::api::{ApiOperation, ApiOperationState};

pub(in crate::api) struct Fixture {
    state: Mutex<State>,
}

struct State {
    current: Current,
    operation: ApiOperation,
    reads: usize,
    pending: bool,
    unavailable: bool,
    revoke_on_read: bool,
    grants: Vec<ActionGrant>,
    grants_revision: u64,
    grants_reads: usize,
    revoke_after_grants: Option<usize>,
    previews: usize,
    change_on_preview: bool,
    native_change_on_preview: bool,
    preview_failure: Option<AuthError>,
    created: Option<(
        tentaflake_admission::ledger::api::OwnedCreate,
        Arc<crate::admission_host::HostPlan>,
    )>,
    queues: usize,
    starts: usize,
    revoke_on_queue: bool,
    queue_wait_until: Option<std::time::Instant>,
    handoff_gate: Option<Arc<(Mutex<bool>, std::sync::Condvar)>>,
}

impl Fixture {
    // Only privileged ledger/unit observations are replaced here. These records
    // do not pretend to prove persistence or storage accounting; core/VM own it.
    pub(super) fn create_receipt(
        &self,
        context: &ApiContext,
        request: &crate::api::creation::Request,
    ) -> Result<Option<tentaflake_admission::ledger::api::OwnedCreate>, AuthError> {
        let mut state = self.state.lock().unwrap();
        if state.unavailable {
            return Err(AuthError::Unavailable);
        }
        assert_eq!(context.authority, state.current.authority);
        let Some((receipt, _)) = &state.created else {
            return Ok(None);
        };
        if receipt.operation.mutation.key != request.key {
            return Ok(None);
        }
        if receipt.operation.mutation.request_digest != request.digest {
            return Err(AuthError::IdempotencyConflict);
        }
        let mut receipt = receipt.clone();
        receipt.grants = state.grants.clone();
        if state.revoke_on_read {
            state.current.binding.push_str("-changed");
        }
        Ok(Some(receipt))
    }
    pub(super) fn original(
        &self,
        receipt: &tentaflake_admission::ledger::api::OwnedCreate,
    ) -> Result<Arc<crate::admission_host::HostPlan>, AuthError> {
        let state = self.state.lock().unwrap();
        let (stored, plan) = state.created.as_ref().ok_or(AuthError::Unavailable)?;
        assert_eq!(receipt.operation.authority, stored.operation.authority);
        Ok(plan.clone())
    }
    pub(super) fn queue(
        &self,
        context: &ApiContext,
        mutation: &tentaflake_admission::ledger::api::ApiMutation,
        prepared: &crate::api::planning::Prepared,
        authorize: impl FnOnce() -> Result<(), AuthError>,
    ) -> Result<tentaflake_admission::ledger::api::ApiQueued, AuthError> {
        use tentaflake_admission::ledger::api::{
            ApiQueued, EnvironmentRecord, EnvironmentState, OwnedCreate, WorkspaceRecord,
        };
        self.preview(prepared)?;
        if let Some(until) = self.state.lock().unwrap().queue_wait_until {
            let wait = until.saturating_duration_since(std::time::Instant::now());
            assert!(wait <= Duration::from_secs(2));
            std::thread::sleep(wait);
        }
        authorize()?;
        let mut state = self.state.lock().unwrap();
        assert!(state.created.is_none());
        let spec = &prepared.creation.environment;
        let operation = ApiOperation {
            mutation: mutation.clone(),
            authority: context.authority.clone(),
            environment_id: spec.instance.id.clone(),
            state: ApiOperationState::Queued,
            created_at: context.now,
            updated_at: context.now,
            queued_revision: 9,
            claimed_revision: None,
            updated_revision: 9,
            execution: None,
            storage_operations: vec![
                "private-storage-state".into(),
                "private-storage-workspace".into(),
            ],
            result: None,
            abort: None,
            recovery: None,
        };
        state.grants.extend(spec.grants.clone());
        state.grants_revision += 1;
        let receipt = OwnedCreate {
            operation: operation.clone(),
            environment: EnvironmentRecord {
                host: context.host.clone(),
                spec: spec.clone(),
                state: EnvironmentState::Provisioning,
                operation_id: mutation.operation_id.clone(),
                created_at: context.now,
                revision: 9,
            },
            workspace: WorkspaceRecord {
                id: spec.workspace_id.clone(),
                environment_id: spec.instance.id.clone(),
                owner_id: context.principal.clone(),
                allocation_id: spec
                    .instance
                    .storage
                    .iter()
                    .find(|row| row.kind == tentaflake_admission::StorageKind::Workspace)
                    .unwrap()
                    .id
                    .clone(),
                revision: 9,
            },
            grants: state.grants.clone(),
        };
        let plan = state.current.host_plan.clone();
        state.created = Some((receipt, plan));
        state.queues += 1;
        if state.revoke_on_queue {
            state.current.binding.push_str("-changed");
        }
        Ok(ApiQueued::New(Box::new(operation)))
    }
    pub(super) fn start(&self, operation: &str) {
        let mut state = self.state.lock().unwrap();
        assert_eq!(
            operation,
            state
                .created
                .as_ref()
                .unwrap()
                .0
                .operation
                .mutation
                .operation_id
        );
        state.starts += 1;
        let gate = state.handoff_gate.clone();
        drop(state);
        if let Some(gate) = gate {
            let (lock, changed) = &*gate;
            let (released, timeout) = changed
                .wait_timeout_while(lock.lock().unwrap(), Duration::from_secs(3), |released| {
                    !*released
                })
                .unwrap();
            assert!(
                *released && !timeout.timed_out(),
                "fixture handoff was not released"
            );
        }
        // No supervisor is run: the handoff's result can never mark success.
    }
    pub(in crate::api) fn creation_counts(&self) -> (usize, usize) {
        let state = self.state.lock().unwrap();
        (state.queues, state.starts)
    }

    pub(super) fn grants(
        &self,
        context: &ApiContext,
    ) -> Result<(u64, Vec<ActionGrant>), AuthError> {
        let mut state = self.state.lock().unwrap();
        assert_eq!(context.principal, "operator");
        assert_eq!(context.host, "host-fixture");
        assert_eq!(context.authority, state.current.authority);
        if state.unavailable {
            return Err(AuthError::Unavailable);
        }
        let grants = state.grants.clone();
        state.grants_reads += 1;
        if state.revoke_on_read || state.revoke_after_grants == Some(state.grants_reads) {
            state.current.binding.push_str("-changed");
        }
        Ok((state.grants_revision, grants))
    }
    pub(super) fn preview(
        &self,
        prepared: &crate::api::planning::Prepared,
    ) -> Result<(), AuthError> {
        let mut state = self.state.lock().unwrap();
        if state.pending || state.unavailable {
            return Err(AuthError::Unavailable);
        }
        assert_eq!(prepared.authority, state.current.authority);
        assert_eq!(prepared.host, "host-fixture");
        assert_eq!(prepared.principal, "operator");
        assert_eq!(prepared.native_grants_revision, state.grants_revision);
        prepared.check_source(&state.current.host_plan).unwrap();
        state.previews += 1;
        if state.change_on_preview {
            state.current.binding.push_str("-changed");
        }
        if state.native_change_on_preview {
            state.grants_revision += 1;
        }
        if let Some(error) = state.preview_failure {
            return Err(error);
        }
        Ok(())
    }
    pub(in crate::api) fn reads(&self) -> usize {
        self.state.lock().unwrap().reads
    }

    pub(super) fn current(&self) -> Result<Current, AuthError> {
        Ok(self.state.lock().unwrap().current.clone())
    }

    pub(super) fn lookup(
        &self,
        context: &ApiContext,
        request: &Read,
    ) -> Result<ApiLookup, AuthError> {
        let mut state = self.state.lock().unwrap();
        state.reads += 1;
        assert_eq!(context.host, "host-fixture");
        assert_eq!(context.principal, "operator");
        assert_eq!(context.authority, state.current.authority);
        assert_eq!(context.epoch, state.current.epoch);
        assert!(context.now > 0);
        if state.unavailable {
            return Err(AuthError::Unavailable);
        }
        let op = state
            .created
            .as_ref()
            .map(|(receipt, _)| &receipt.operation)
            .unwrap_or(&state.operation);
        let owned =
            op.mutation.key.principal == context.principal && op.mutation.key.host == context.host;
        let found = owned
            && match request {
                Read::Id(id) => id == &op.mutation.operation_id,
                Read::Key {
                    epoch,
                    method,
                    target,
                    key,
                } => {
                    epoch == &op.mutation.key.epoch
                        && method == &op.mutation.key.method
                        && target == &op.mutation.key.canonical_target
                        && key == &op.mutation.key.key
                }
            };
        let result = if found {
            Ok(ApiLookup::Found(Box::new(op.clone())))
        } else {
            match request {
                Read::Id(_) => Err(AuthError::NotFound),
                Read::Key { epoch, .. } if epoch == &context.epoch && !state.pending => {
                    Ok(ApiLookup::Absent)
                }
                Read::Key { .. } => Ok(ApiLookup::Unknown),
            }
        };
        if state.revoke_on_read {
            state.current.binding.push_str("-changed");
        }
        result
    }
}

pub(in crate::api) const KEY: &str = "12345678-1234-4234-8234-123456789abc";
pub(in crate::api) const EPOCH: &str = "epoch-fixture";

pub(in crate::api) fn fixture(state: ApiOperationState) -> (Arc<Api>, String, Arc<Fixture>) {
    let recorded: Value = serde_json::from_str(include_str!(
        "../../../tentaflake-admission/tests/recorded-start.json"
    ))
    .unwrap();
    let authority = PublishedAuthority {
        generation: "authority-fixture".into(),
        revision: format!("sha256:{}", "1".repeat(64)),
        manifest_path: "/nix/store/00000000000000000000000000000000-private-authority".into(),
    };
    let mut host_plan = crate::admission_oci::tests::template_plan_fixture();
    host_plan.policy = serde_json::from_value(recorded["snapshot"]["policy"].clone()).unwrap();
    let current = Current {
        authority: authority.clone(),
        policy: "policy-fixture".into(),
        binding: "binding-fixture".into(),
        epoch: EPOCH.into(),
        host_plan: Arc::new(host_plan),
        catalog: super::super::catalog::Catalog::default(),
    };
    let claimed = state != ApiOperationState::Queued;
    let operation: ApiOperation = serde_json::from_value(json!({
        "mutation":{"operationId":"operation-fixture","key":{"host":"host-fixture","principal":"operator",
            "epoch":EPOCH,"method":"POST","canonicalTarget":"/api/v1/environments","key":KEY},
            "requestDigest":format!("sha256:{}", "a".repeat(64))},
        "authority":authority,"environmentId":"environment-fixture","state":state,
        "createdAt":1800000000u64,"updatedAt":if claimed {1800000010u64} else {1800000000u64},
        "queuedRevision":4,"claimedRevision":if claimed {Some(5)} else {None},
        "updatedRevision":match state {ApiOperationState::Queued => 4, ApiOperationState::Unknown => 5, _ => 8},
        "execution":if claimed {json!({"bootId":"12345678-1234-4234-8234-123456789abc",
            "supervisor":{"unit":"private-supervisor.service","invocationId":"a".repeat(32),"pid":1234,"startTimeTicks":12345}})} else {Value::Null},
        "storageOperations":["private-storage-one","private-storage-two"],
        "result":if state == ApiOperationState::Succeeded {json!({"environmentId":"environment-fixture","environmentRevision":8,
            "workspaceId":"workspace-fixture","workspaceRevision":8})} else {Value::Null},
        "abort":if state == ApiOperationState::Aborted {json!({"expectedLedgerRevision":7})} else {Value::Null},
        "recovery":null,
    })).unwrap();
    let backend = Arc::new(Fixture {
        state: Mutex::new(State {
            current: current.clone(),
            operation,
            reads: 0,
            pending: false,
            unavailable: false,
            revoke_on_read: false,
            grants: vec![],
            grants_revision: 0,
            grants_reads: 0,
            revoke_after_grants: None,
            previews: 0,
            change_on_preview: false,
            native_change_on_preview: false,
            preview_failure: None,
            created: None,
            queues: 0,
            starts: 0,
            revoke_on_queue: false,
            queue_wait_until: None,
            handoff_gate: None,
        }),
    });
    let config: Config = serde_json::from_value(json!({
        "schemaVersion":1,"host":"host-fixture","origin":"https://host.example.invalid",
        "listen":"127.0.0.1:18181","principal":{"id":"operator","name":"Operator"},
        "operatorLogin":true,"cliBearer":false,
    }))
    .unwrap();
    let mut sessions = Sessions::new(super::super::session::AuthSettings {
        origin: config.origin.clone(),
        operator_credential: Some("/run/credentials/api-fixture/operator-token".into()),
        bearer_credential: None,
    })
    .unwrap();
    let login = sessions.fixture_login(current.context(&config));
    let cookie = login.set_cookie.split(';').next().unwrap().to_owned();
    (
        Arc::new(Api {
            config,
            configuration: "/uninstalled-api-fixture".into(),
            plan: "/uninstalled-plan-fixture".into(),
            sessions: Mutex::new(sessions),
            plans: Mutex::new(crate::api::planning::Cache::default()),
            work: Arc::new(Semaphore::new(1)),
            fixture: Some(backend.clone()),
        }),
        cookie,
        backend,
    )
}

pub(in crate::api) async fn http(api: Arc<Api>, cookie: &str, target: &str) -> String {
    server::tests::exchange(
        api,
        format!("GET {target} HTTP/1.1\r\nHost: host.example.invalid\r\nCookie: {cookie}\r\n\r\n")
            .into_bytes(),
    )
    .await
}

pub(in crate::api) fn catalog_fixture(enabled: bool) -> (Arc<Api>, String, Arc<Fixture>) {
    let (mut api, cookie, backend) = fixture(ApiOperationState::Succeeded);
    if enabled {
        Arc::get_mut(&mut api).unwrap().config.catalog_read_grant = Some(serde_json::from_value(json!({
            "id":"catalog-reader","action":"catalog.read","target":{"kind":"catalog","id":"fixture-catalog"},"expiresAt":null
        })).unwrap());
    }
    backend.state.lock().unwrap().current.catalog = crate::api::catalog::Catalog::fixture();
    (api, cookie, backend)
}

#[test]
fn catalog_http_requires_exact_installed_grant_and_pages_without_leaking_authority() {
    tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
        let (denied, cookie, backend) = catalog_fixture(false);
        body(&http(denied, &cookie, "/api/v1/catalog/images").await, 403);
        assert_eq!(backend.reads(), 0);
        let (api, cookie, backend) = catalog_fixture(true);
        let session = body(&http(api.clone(), &cookie, "/api/v1/session").await, 200);
        assert_eq!(session["grants"], json!([{"id":"catalog-reader","action":"catalog.read","target":{"kind":"catalog","id":"fixture-catalog"},"expiresAt":null}]));
        let before = serde_json::to_vec(&backend.state.lock().unwrap().operation).unwrap();
        let first = http(api.clone(), &cookie, "/api/v1/catalog/images?limit=1").await;
        let page = body(&first, 200);
        assert_eq!(page.as_array().unwrap().len(), 1);
        let cursor = first.lines().find_map(|line| line.strip_prefix("next-cursor: ")).unwrap();
        assert!(first.contains(&format!("etag: \"{}\"\r\n", session["revisions"]["catalog"].as_str().unwrap())));
        let second = http(api.clone(), &cookie, &format!("/api/v1/catalog/images?cursor={cursor}&limit=1")).await;
        assert_ne!(body(&second, 200)[0]["id"], page[0]["id"]);
        assert!(!second.contains("next-cursor:"));
        assert_eq!(body(&http(api.clone(), &cookie, "/api/v1/catalog/blocks").await, 200), json!([]));
        for target in [format!("/api/v1/catalog/blocks?cursor={cursor}"), "/api/v1/catalog/images?cursor=unknown".into()] {
            assert_eq!(body(&http(api.clone(), &cookie, &target).await, 409)["error"]["code"], "cursor_expired");
        }
        for target in ["/api/v1/catalog/images?limit=0", "/api/v1/catalog/images?limit=1&limit=2", "/api/v1/catalog/images?principal=other", "/api/v1/catalog/images?cursor=%00"] {
            body(&http(api.clone(), &cookie, target).await, 400);
        }
        for secret in ["private-authority", "private-supervisor", "private-storage", "sourceInstance", "principals", "command", "environmentFile"] {
            assert!(!first.contains(secret));
        }
        // Pending does not revoke the still-installed published catalog.
        backend.state.lock().unwrap().pending = true;
        body(&http(api.clone(), &cookie, "/api/v1/catalog/images").await, 200);
        assert_eq!(before, serde_json::to_vec(&backend.state.lock().unwrap().operation).unwrap());
        let after = body(&http(api.clone(), &cookie, "/api/v1/session").await, 200);
        assert_eq!(session["revisions"], after["revisions"]);
        backend.state.lock().unwrap().revoke_on_read = true;
        body(&http(api, &cookie, "/api/v1/catalog/images").await, 401);
    });
}

#[test]
fn public_grants_change_content_revision_and_overflow_only_blocks_session_output() {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            let (api, cookie, backend) = catalog_fixture(true);
            let initial = body(&http(api.clone(), &cookie, "/api/v1/session").await, 200);
            let page = http(api.clone(), &cookie, "/api/v1/catalog/images?limit=1").await;
            let cursor = page
                .lines()
                .find_map(|line| line.strip_prefix("next-cursor: "))
                .unwrap();
            backend.state.lock().unwrap().grants = (0..256)
                .map(|index| {
                    serde_json::from_value(json!({
            "id":format!("environment-reader-{index}"),"action":"environments.read",
            "target":{"kind":"environment","id":format!("environment-{index}")},"expiresAt":null
        })).unwrap()
                })
                .collect();
            body(&http(api.clone(), &cookie, "/api/v1/session").await, 503);
            body(
                &http(api.clone(), &cookie, "/api/v1/operations/operation-fixture").await,
                200,
            );
            body(
                &http(api.clone(), &cookie, "/api/v1/catalog/images").await,
                200,
            );
            body(
                &http(
                    api.clone(),
                    &cookie,
                    &format!("/api/v1/catalog/images?cursor={cursor}"),
                )
                .await,
                409,
            );
            backend.state.lock().unwrap().grants.truncate(1);
            let changed = body(&http(api.clone(), &cookie, "/api/v1/session").await, 200);
            assert_eq!(changed["grants"].as_array().unwrap().len(), 2);
            assert_ne!(
                changed["revisions"]["grants"],
                initial["revisions"]["grants"]
            );
            assert_eq!(
                changed["revisions"]["catalog"],
                initial["revisions"]["catalog"]
            );
            backend.state.lock().unwrap().grants.clear();
            assert_eq!(
                body(&http(api, &cookie, "/api/v1/session").await, 200)["revisions"],
                initial["revisions"]
            );
        });
}

fn body(reply: &str, status: u16) -> Value {
    assert!(reply.starts_with(&format!("HTTP/1.1 {status}")), "{reply}");
    assert!(reply.contains("cache-control: no-store\r\n"));
    serde_json::from_str(reply.split_once("\r\n\r\n").unwrap().1).unwrap()
}

fn recovery(epoch: &str, key: &str) -> String {
    format!(
        "/api/v1/operations?idempotencyKey={key}&mutationEpoch={epoch}&method=POST&target=%2Fapi%2Fv1%2Fenvironments"
    )
}

#[test]
fn real_http_unix_owner_projects_only_original_receipt_data() {
    tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
        for (state, expected, revision, updated_at) in [(ApiOperationState::Queued,"queued","r4",1800000000),
            (ApiOperationState::Unknown,"unknown","r5",1800000010),
            (ApiOperationState::Succeeded,"succeeded","r8",1800000010), (ApiOperationState::Aborted,"failed","r8",1800000010)] {
            let (api, cookie, backend) = fixture(state);
            let reply = http(api.clone(), &cookie, "/api/v1/operations/operation-fixture").await;
            let value = body(&reply, 200);
            assert!(reply.contains(&format!("etag: \"{revision}\"\r\n")));
            assert_eq!(value["state"], expected);
            assert_eq!(value["ownerId"], "operator");
            assert_eq!(value["kind"], "create");
            assert_eq!(value["target"], json!({"kind":"environment","id":"environment-fixture"}));
            assert_eq!(value["createdAt"], timestamp(1800000000).unwrap());
            assert_eq!(value["updatedAt"], timestamp(updated_at).unwrap());
            assert_eq!(value["revision"], revision);
            assert_eq!(value["steps"], json!([]));
            assert_eq!(value["requestDigest"], format!("sha256:{}", "a".repeat(64)));
            assert_eq!(value.as_object().unwrap().len(), 13);
            for secret in ["private-authority", "private-supervisor", "private-storage", "bootId", "execution", "claimedRevision"] {
                assert!(!reply.contains(secret), "private receipt field leaked");
            }
            if state == ApiOperationState::Succeeded {
                assert_eq!(value["result"], json!({"resources":[
                    {"kind":"environment","id":"environment-fixture","state":"created","revision":"r8"},
                    {"kind":"workspace","id":"workspace-fixture","state":"created","revision":"r8"},
                ]}));
                assert!(value["error"].is_null());
            } else {
                assert!(value["result"].is_null());
                if state == ApiOperationState::Aborted {
                    assert_eq!(value["error"]["code"], "unready");
                    assert!(value["error"]["message"].as_str().unwrap().contains("explicit operator recovery"));
                }
            }
            let found = body(&http(api.clone(), &cookie, &recovery(EPOCH, &KEY.to_uppercase())).await, 200);
            assert_eq!(found, json!({"state":"found","operation":value,"retrySafe":false}));
            assert_eq!(backend.state.lock().unwrap().reads, 2);
        }
    });
}

#[test]
fn owner_authentication_binding_and_lookup_failures_never_become_absence() {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            let (api, cookie, backend) = fixture(ApiOperationState::Succeeded);
            let missing = recovery(EPOCH, "00000000-0000-0000-0000-000000000000");
            assert_eq!(
                body(&http(api.clone(), &cookie, &missing).await, 200),
                json!({"state":"absent","operation":null,"retrySafe":true,"mutationEpoch":EPOCH})
            );
            assert_eq!(
                body(
                    &http(api.clone(), &cookie, &recovery("retired", KEY)).await,
                    200
                )["state"],
                "unknown"
            );
            backend.state.lock().unwrap().pending = true;
            assert_eq!(
                body(&http(api.clone(), &cookie, &missing).await, 200)["state"],
                "unknown"
            );
            assert_eq!(
                body(
                    &http(api.clone(), &cookie, &recovery(EPOCH, KEY)).await,
                    200
                )["state"],
                "found"
            );
            backend.state.lock().unwrap().operation.updated_at = u64::MAX;
            body(
                &http(api.clone(), &cookie, "/api/v1/operations/operation-fixture").await,
                503,
            );
            backend.state.lock().unwrap().operation.updated_at = 1800000010;
            let result = backend.state.lock().unwrap().operation.result.take();
            body(
                &http(api.clone(), &cookie, "/api/v1/operations/operation-fixture").await,
                503,
            );
            backend.state.lock().unwrap().operation.result = result;
            backend
                .state
                .lock()
                .unwrap()
                .operation
                .mutation
                .key
                .principal = "other-owner".into();
            let denied = body(
                &http(api.clone(), &cookie, "/api/v1/operations/operation-fixture").await,
                404,
            );
            assert_eq!(denied["error"]["code"], "not_found");
            assert!(!denied.to_string().contains("other-owner"));
            backend
                .state
                .lock()
                .unwrap()
                .operation
                .mutation
                .key
                .principal = "operator".into();
            backend.state.lock().unwrap().operation.mutation.key.host = "other-host".into();
            body(
                &http(api.clone(), &cookie, "/api/v1/operations/operation-fixture").await,
                404,
            );
            backend.state.lock().unwrap().unavailable = true;
            assert_eq!(
                body(&http(api.clone(), &cookie, &missing).await, 503)["error"]["code"],
                "unavailable"
            );
            backend.state.lock().unwrap().unavailable = false;
            backend.state.lock().unwrap().revoke_on_read = true;
            assert_eq!(
                body(&http(api.clone(), &cookie, &missing).await, 401)["error"]["code"],
                "session_revoked"
            );
            let reads = backend.state.lock().unwrap().reads;
            body(
                &http(api.clone(), "__Host-tentaflake-session=wrong", &missing).await,
                400,
            );
            let unknown_cookie = format!("__Host-tentaflake-session={}", "0".repeat(64));
            body(&http(api, &unknown_cookie, &missing).await, 401);
            assert_eq!(backend.state.lock().unwrap().reads, reads);
        });
}

#[test]
fn query_decodes_only_outer_encoding_and_never_normalizes_semantic_target() {
    let query = format!(
        "idempotencyKey={KEY}&mutationEpoch={EPOCH}&method=DELETE&target=%2Fapi%2Fv1%2Fenvironments%2Fenv-1%3Fworkspace%3Dretain%26name%3Da%2520b"
    );
    let parsed = crate::api::operations::parse("/api/v1/operations", Some(&query))
        .unwrap()
        .unwrap();
    assert_eq!(
        parsed,
        Read::Key {
            epoch: EPOCH.into(),
            method: tentaflake_admission::ledger::api::ApiMethod::Delete,
            target: "/api/v1/environments/env-1?workspace=retain&name=a%20b".into(),
            key: KEY.into()
        }
    );
    for target in [
        "%FF",
        "%",
        "%0",
        "%GG",
        "%2Fapi%2Fv1%2Fenvironments%2F..",
        "%2Fapi%2Fv1%2Fenv%5Cbad",
        "%2Fapi%2Fv1%2Fenv+bad",
    ] {
        let malformed =
            format!("idempotencyKey={KEY}&mutationEpoch={EPOCH}&method=POST&target={target}");
        assert!(
            crate::api::operations::parse("/api/v1/operations", Some(&malformed)).is_err(),
            "{target}"
        );
    }
}

#[test]
fn operation_queries_are_strict() {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            let (api, cookie, backend) = fixture(ApiOperationState::Queued);
            for target in [
                "/api/v1/operations",
                "/api/v1/operations/id?ignored=1",
                "/api/v1/operations/%69d",
                "/api/v1/operations/id/child",
                "/api/v1/operations?unknown=1",
                "/api/v1/session?ignored=1",
            ] {
                body(&http(api.clone(), &cookie, target).await, 400);
            }
            for suffix in [
                "&ownerId=other",
                "&method=POST",
                "&%6Dethod=POST",
                "&",
                "&target=%FF",
                "&target=%",
            ] {
                body(
                    &http(api.clone(), &cookie, &(recovery(EPOCH, KEY) + suffix)).await,
                    400,
                );
            }
            for (field, value) in [
                ("method=POST", "method=GET"),
                ("method=POST", "method=post"),
                ("mutationEpoch=epoch-fixture", "mutationEpoch="),
                (
                    "idempotencyKey=12345678-1234-4234-8234-123456789abc",
                    "idempotencyKey=invalid",
                ),
            ] {
                body(
                    &http(
                        api.clone(),
                        &cookie,
                        &recovery(EPOCH, KEY).replace(field, value),
                    )
                    .await,
                    400,
                );
            }
            assert_eq!(backend.state.lock().unwrap().reads, 0);
        });
}

// These fixtures replace only the privileged preview observation. The same
// installed-template reconstruction and HTTP/Unix/authentication path executes;
// real lock, physical-slot and atomic no-write behavior have separate core/VM tests.
pub(in crate::api) fn planning_fixture() -> (Arc<Api>, String, Arc<Fixture>) {
    let (api, cookie, backend) = fixture(ApiOperationState::Succeeded);
    let plan = crate::admission_template::tests::fixture_plan();
    let mut state = backend.state.lock().unwrap();
    state.current.catalog = crate::api::catalog::Catalog::project(&plan).unwrap();
    state.current.host_plan = Arc::new(plan);
    drop(state);
    (api, cookie, backend)
}

pub(in crate::api) fn plan_request(backend: &Fixture, revisions: Value) -> Value {
    let state = backend.state.lock().unwrap();
    let plan = &state.current.host_plan;
    let template = plan.creation_templates.values().next().unwrap();
    let (_, settings) = template
        .public_metadata(&plan.instances[&template.source_instance])
        .unwrap();
    json!({"name":"Example environment","config":settings,"workspace":{"kind":"new"},"revisions":revisions})
}

pub(in crate::api) fn prepared(clock: Clock) -> crate::api::planning::Prepared {
    let (api, _, backend) = planning_fixture();
    let current = backend.current().unwrap();
    let request = plan_request(&backend, current.revisions("grants-fixture"));
    let request =
        crate::api::planning::Request::parse(&serde_json::to_vec(&request).unwrap()).unwrap();
    crate::api::planning::prepare(&current.host_plan, &current, &api.config, request, 0, clock)
        .unwrap()
        .unwrap()
}

pub(in crate::api) async fn post_plan(
    api: Arc<Api>,
    cookie: &str,
    csrf: &str,
    payload: &[u8],
) -> String {
    let head = format!(
        "POST /api/v1/plans HTTP/1.1\r\nHost: host.example.invalid\r\nOrigin: https://host.example.invalid\r\nCookie: {cookie}\r\nX-Tentaflake-CSRF: {csrf}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n",
        payload.len()
    );
    server::tests::exchange(api, [head.as_bytes(), payload].concat()).await
}

#[test]
fn planning_http_uses_exact_template_grant_and_returns_no_reservation() {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            let (api, cookie, backend) = planning_fixture();
            assert!(api.config.catalog_read_grant.is_none());
            let session = body(&http(api.clone(), &cookie, "/api/v1/session").await, 200);
            let grants = session["grants"].as_array().unwrap();
            assert_eq!(grants.len(), 1);
            assert_eq!(grants[0]["action"], "environments.create");
            assert_eq!(
                grants[0]["target"],
                json!({"kind":"host","id":"host-fixture"})
            );
            let request = plan_request(&backend, session["revisions"].clone());
            let before = serde_json::to_value(&backend.state.lock().unwrap().operation).unwrap();
            let csrf = session["csrfToken"].as_str().unwrap();
            let mut previous = None;
            for _ in 0..2 {
                let reply = post_plan(
                    api.clone(),
                    &cookie,
                    csrf,
                    &serde_json::to_vec(&request).unwrap(),
                )
                .await;
                let plan = body(&reply, 200);
                assert_eq!(plan["valid"], true);
                assert_eq!(plan["ownerId"], "operator");
                assert_eq!(plan["request"], request);
                assert_eq!(plan["revisions"], session["revisions"]);
                assert_eq!(plan["resolved"]["grantIds"], json!([grants[0]["id"]]));
                assert_eq!(
                    plan["resolved"]["resources"],
                    request["config"]["resources"]
                );
                assert_eq!(plan["resolved"]["isolation"], "container");
                assert_eq!(plan["resolved"]["agentRuntime"], Value::Null);
                assert_eq!(plan["resolved"]["blocks"], json!([]));
                assert_eq!(plan["resolved"]["capabilities"], json!([]));
                assert!(
                    plan["resolved"]["environmentId"]
                        .as_str()
                        .unwrap()
                        .starts_with("env-")
                );
                assert!(
                    plan["resolved"]["workspaceId"]
                        .as_str()
                        .unwrap()
                        .starts_with("ws-")
                );
                let issued = plan["resolved"]["proposedGrants"].as_array().unwrap();
                assert_eq!(issued.len(), 2);
                for grant in issued {
                    let target = if grant["target"]["kind"] == "environment" {
                        "environmentId"
                    } else {
                        "workspaceId"
                    };
                    assert_eq!(grant["target"]["id"], plan["resolved"][target]);
                    assert_ne!(grant["action"], "environments.create");
                }
                for private in [
                    "agent-fixture",
                    "public-base",
                    "/nix/store/",
                    "/var/lib/",
                    "sourceRevision",
                    "helpers",
                    "principals",
                ] {
                    assert!(!reply.contains(private), "private projection: {private}");
                }
                if let Some(previous) = previous {
                    assert_ne!(plan["id"], previous);
                }
                previous = Some(plan["id"].clone());
                assert!(reply.contains("cache-control: no-store"));
            }
            let state = backend.state.lock().unwrap();
            assert_eq!(state.previews, 2);
            assert_eq!(state.grants_revision, 0);
            assert_eq!(serde_json::to_value(&state.operation).unwrap(), before);
            assert!(state.grants.is_empty());
        });
}

#[test]
fn planning_http_refuses_auth_staleness_pending_and_exactness_without_downgrades() {
    tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
        let (api,cookie,backend)=planning_fixture();
        let session=body(&http(api.clone(),&cookie,"/api/v1/session").await,200);
        let csrf=session["csrfToken"].as_str().unwrap();
        let exact=plan_request(&backend,session["revisions"].clone());
        let payload=serde_json::to_vec(&exact).unwrap();
        body(&post_plan(api.clone(),&cookie,"wrong",&payload).await,403);
        body(&post_plan(api.clone(),&format!("__Host-tentaflake-session={}","0".repeat(64)),csrf,&payload).await,401);
        body(&post_plan(api.clone(),&cookie,csrf,b"not-json").await,400);
        let mut stale=exact.clone(); stale["revisions"]["host"]=json!("old-epoch");
        assert_eq!(body(&post_plan(api.clone(),&cookie,csrf,&serde_json::to_vec(&stale).unwrap()).await,409)["error"]["code"],"plan_stale");
        for field in ["isolation","agentRuntime","tools","skills","capabilities","resources","workspace"] {
            let mut unsupported=exact.clone();
            match field {
                "isolation"=>unsupported["config"][field]=json!("guest-kernel"),
                "agentRuntime"=>unsupported["config"][field]=json!("uninstalled-runtime"),
                "tools"|"skills"|"capabilities"=>unsupported["config"][field]=json!(["not-installed"]),
                "resources"=>unsupported["config"][field]["cpu"]=json!(2),
                _=>unsupported[field]=json!({"kind":"retained","workspaceId":"private-existing","revision":"old"}),
            }
            let plan=body(&post_plan(api.clone(),&cookie,csrf,&serde_json::to_vec(&unsupported).unwrap()).await,200);
            assert_eq!(plan["valid"],false);
            assert!(plan["resolved"].is_null());
            assert_eq!(plan["request"],unsupported);
        }
        assert_eq!(backend.state.lock().unwrap().previews,0);
        backend.state.lock().unwrap().pending=true;
        body(&post_plan(api.clone(),&cookie,csrf,&payload).await,503);
        backend.state.lock().unwrap().pending=false;
        backend.state.lock().unwrap().preview_failure=Some(AuthError::QuotaExceeded);
        assert_eq!(body(&post_plan(api.clone(),&cookie,csrf,&payload).await,409)["error"]["code"],"quota_exceeded");
        backend.state.lock().unwrap().preview_failure=None;
        backend.state.lock().unwrap().native_change_on_preview=true;
        assert_eq!(body(&post_plan(api.clone(),&cookie,csrf,&payload).await,409)["error"]["code"],"plan_stale");
        backend.state.lock().unwrap().native_change_on_preview=false;
        backend.state.lock().unwrap().change_on_preview=true;
        body(&post_plan(api.clone(),&cookie,csrf,&payload).await,401);

        let (denied,cookie,backend)=catalog_fixture(true);
        let session=body(&http(denied.clone(),&cookie,"/api/v1/session").await,200);
        let mut request=exact.clone();request["revisions"]=session["revisions"].clone();
        body(&post_plan(denied,&cookie,session["csrfToken"].as_str().unwrap(),&serde_json::to_vec(&request).unwrap()).await,403);
        assert_eq!(backend.state.lock().unwrap().previews,0);
    });
}

#[test]
fn unsupported_plan_rechecks_auth_after_its_last_grant_read() {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            let (api, cookie, backend) = planning_fixture();
            let session = body(&http(api.clone(), &cookie, "/api/v1/session").await, 200);
            let mut request = plan_request(&backend, session["revisions"].clone());
            request["config"]["isolation"] = json!("guest-kernel");
            {
                let mut state = backend.state.lock().unwrap();
                state.revoke_after_grants = Some(state.grants_reads + 2);
            }
            body(
                &post_plan(
                    api,
                    &cookie,
                    session["csrfToken"].as_str().unwrap(),
                    &serde_json::to_vec(&request).unwrap(),
                )
                .await,
                401,
            );
            assert_eq!(backend.state.lock().unwrap().previews, 0);
        });
}

pub(in crate::api) async fn post_create(
    api: Arc<Api>,
    cookie: &str,
    csrf: &str,
    epoch: &str,
    key: &str,
    payload: &[u8],
) -> String {
    let head = format!(
        "POST /api/v1/environments HTTP/1.1\r\nHost: host.example.invalid\r\nOrigin: https://host.example.invalid\r\nCookie: {cookie}\r\nX-Tentaflake-CSRF: {csrf}\r\nContent-Type: application/json\r\nIdempotency-Epoch: {epoch}\r\nIdempotency-Key: {key}\r\nContent-Length: {}\r\n\r\n",
        payload.len()
    );
    server::tests::exchange(api, [head.as_bytes(), payload].concat()).await
}

pub(in crate::api) async fn creation_request(
    api: Arc<Api>,
    cookie: &str,
    backend: &Fixture,
) -> (Value, Value, Vec<u8>) {
    let session = body(&http(api.clone(), cookie, "/api/v1/session").await, 200);
    let request = plan_request(backend, session["revisions"].clone());
    let plan = body(
        &post_plan(
            api,
            cookie,
            session["csrfToken"].as_str().unwrap(),
            &serde_json::to_vec(&request).unwrap(),
        )
        .await,
        200,
    );
    let payload =
        serde_json::to_vec(&json!({"planId":plan["id"],"startAfterCreate":false})).unwrap();
    (session, plan, payload)
}

#[test]
fn stopped_create_http_queues_once_and_preserves_exact_history_without_cached_authority() {
    use tentaflake_admission::ledger::api::{CreateResult, EnvironmentState};
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            let (api, cookie, backend) = planning_fixture();
            let (session, plan, payload) = creation_request(api.clone(), &cookie, &backend).await;
            let csrf = session["csrfToken"].as_str().unwrap();
            let first = post_create(api.clone(), &cookie, csrf, EPOCH, KEY, &payload).await;
            let accepted = body(&first, 202);
            let op = accepted["operation"]["id"].as_str().unwrap();
            assert!(first.contains(&format!("operation-id: {op}\r\n")));
            assert_eq!(
                accepted["environment"]["id"],
                plan["resolved"]["environmentId"]
            );
            assert_eq!(
                accepted["environment"]["workspace"]["id"],
                plan["resolved"]["workspaceId"]
            );
            assert_eq!(accepted["environment"]["config"], plan["request"]["config"]);
            assert_eq!(accepted["environment"]["state"], "provisioning");
            assert_eq!(accepted["operation"]["state"], "queued");
            assert!(accepted["environment"]["usage"].is_null());
            assert!(accepted["environment"]["workspace"]["usedBytes"].is_null());
            assert!(accepted["environment"]["active"].is_null());
            let fresh = body(&http(api.clone(), &cookie, "/api/v1/session").await, 200);
            assert_eq!(accepted["environment"]["revisions"], fresh["revisions"]);
            assert_ne!(fresh["revisions"]["grants"], session["revisions"]["grants"]);
            for private in [
                "/nix/store/",
                "/var/lib/",
                "public-base",
                "sourceRevision",
                "claimedRevision",
                "private-storage",
                "supervisor",
            ] {
                assert!(!first.contains(private));
            }
            *api.plans.lock().unwrap() = crate::api::planning::Cache::default();
            assert_eq!(
                body(
                    &post_create(
                        api.clone(),
                        &cookie,
                        csrf,
                        EPOCH,
                        &KEY.to_uppercase(),
                        &payload
                    )
                    .await,
                    202
                ),
                accepted
            );
            // Same logical JSON with different accepted bytes is a different intent.
            let mut changed = payload.clone();
            changed.push(b' ');
            assert_eq!(
                body(
                    &post_create(api.clone(), &cookie, csrf, EPOCH, KEY, &changed).await,
                    409
                )["error"]["code"],
                "idempotency_conflict"
            );
            {
                let mut state = backend.state.lock().unwrap();
                state.pending = true;
                let (receipt, _) = state.created.as_mut().unwrap();
                receipt.operation.state = ApiOperationState::Unknown;
                receipt.operation.updated_revision = 10;
                receipt.environment.state = EnvironmentState::Unknown;
                receipt.environment.revision = 10;
            }
            let unknown = body(
                &post_create(api.clone(), &cookie, csrf, EPOCH, KEY, &payload).await,
                202,
            );
            assert_eq!(unknown["operation"]["state"], "unknown");
            assert_eq!(unknown["environment"]["state"], "unknown");
            {
                let mut state = backend.state.lock().unwrap();
                let (receipt, _) = state.created.as_mut().unwrap();
                receipt.operation.state = ApiOperationState::Succeeded;
                receipt.operation.updated_revision = 11;
                receipt.environment.state = EnvironmentState::Stopped;
                receipt.environment.revision = 11;
                receipt.workspace.revision = 11;
                receipt.operation.result = Some(CreateResult {
                    environment_id: receipt.environment.spec.instance.id.clone(),
                    environment_revision: 11,
                    workspace_id: receipt.workspace.id.clone(),
                    workspace_revision: 11,
                });
            }
            let complete = body(
                &post_create(api.clone(), &cookie, csrf, EPOCH, KEY, &payload).await,
                202,
            );
            assert_eq!(complete["environment"]["state"], "stopped");
            assert_eq!(
                complete["operation"]["result"]["resources"][0]["revision"],
                "r11"
            );
            {
                let mut state = backend.state.lock().unwrap();
                let mut removed = crate::admission_template::tests::fixture_plan();
                removed.creation_templates.clear();
                state.current.host_plan = Arc::new(removed);
                state.current.catalog = crate::api::catalog::Catalog::default();
                state.current.epoch = "later-epoch".into();
                state.current.binding = "later-binding".into();
                state.grants.clear();
                let (receipt, _) = state.created.as_mut().unwrap();
                receipt.operation.state = ApiOperationState::Aborted;
                receipt.operation.result = None;
                receipt.environment.state = EnvironmentState::Aborted;
            }
            // A fresh authenticated principal can read exact old intent after a
            // publication/revocation. No current template or create grant is used.
            let current = backend.current().unwrap();
            let login = api
                .sessions
                .lock()
                .unwrap()
                .fixture_login(current.context(&api.config));
            let cookie2 = login.set_cookie.split(';').next().unwrap();
            let aborted = body(
                &post_create(
                    api.clone(),
                    cookie2,
                    &login.auth.csrf_token,
                    EPOCH,
                    KEY,
                    &payload,
                )
                .await,
                202,
            );
            assert_eq!(aborted["operation"]["state"], "failed");
            assert_eq!(
                aborted["environment"]["config"],
                accepted["environment"]["config"]
            );
            assert_eq!(aborted["environment"]["revisions"]["host"], "later-epoch");
            assert_eq!(backend.creation_counts(), (1, 1));
            body(
                &post_create(api.clone(), &cookie, csrf, EPOCH, KEY, &payload).await,
                401,
            );
        });
}

#[test]
fn stopped_create_http_refuses_invalid_stale_foreign_or_pending_before_any_queue() {
    tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
        for case in 0..10 {
            let (api,cookie,backend)=planning_fixture();
            let (session,plan,mut payload)=creation_request(api.clone(),&cookie,&backend).await;
            let mut csrf=session["csrfToken"].as_str().unwrap().to_owned();
            let mut epoch=EPOCH;
            let expected=match case {
                0=>{csrf="wrong".into();403},
                1=>{payload=b"not JSON".to_vec();400},
                2=>{payload=serde_json::to_vec(&json!({"planId":plan["id"],"startAfterCreate":true})).unwrap();501},
                3=>{*api.plans.lock().unwrap()=Default::default();409},
                4=>{epoch="retired-epoch";409},
                5=>{backend.state.lock().unwrap().grants_revision+=1;409},
                6=>{backend.state.lock().unwrap().pending=true;503},
                7=>{let mut cache=api.plans.lock().unwrap();let mut prepared=cache.take(plan["id"].as_str().unwrap()).unwrap();prepared.principal="other-principal".into();cache.insert(prepared,Clock::now().unwrap()).unwrap();409},
                8=>{let mut changed=crate::admission_template::tests::fixture_plan();changed.creation_templates.clear();backend.state.lock().unwrap().current.host_plan=Arc::new(changed);403},
                _=>{payload=serde_json::to_vec(&json!({"planId":plan["id"],"startAfterCreate":false,"command":"unexpected"})).unwrap();400},
            };
            let reply=body(&post_create(api,&cookie,&csrf,epoch,KEY,&payload).await,expected);
            if case==4 { assert_eq!(reply["error"]["code"],"idempotency_expired"); }
            assert_eq!(backend.creation_counts(),(0,0),"case {case}");
        }
        let (api,cookie,backend)=planning_fixture();
        let session=body(&http(api.clone(),&cookie,"/api/v1/session").await,200);
        let current=backend.current().unwrap();
        let request=crate::api::planning::Request::parse(&serde_json::to_vec(&plan_request(&backend,session["revisions"].clone())).unwrap()).unwrap();
        let mut old=Clock::now().unwrap();old.unix_seconds-=301;old.monotonic-=Duration::from_secs(301);
        let expired=crate::api::planning::prepare(&current.host_plan,&current,&api.config,request,0,old).unwrap().unwrap();
        let plan=api.plans.lock().unwrap().insert(expired,old).unwrap();
        let payload=serde_json::to_vec(&json!({"planId":plan["id"],"startAfterCreate":false})).unwrap();
        assert_eq!(body(&post_create(api,&cookie,session["csrfToken"].as_str().unwrap(),EPOCH,KEY,&payload).await,409)["error"]["code"],"plan_stale");
        assert_eq!(backend.creation_counts(),(0,0));
    });
}

#[test]
fn stopped_create_expiry_during_observation_never_queues_or_issues_grants() {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            let (api, cookie, backend) = planning_fixture();
            let (session, _, payload) = creation_request(api.clone(), &cookie, &backend).await;
            let before = {
                let state = backend.state.lock().unwrap();
                (state.previews, state.grants.clone(), state.grants_revision)
            };
            let expires = api
                .sessions
                .lock()
                .unwrap()
                .fixture_expire_after(&cookie, Duration::from_secs(1));
            backend.state.lock().unwrap().queue_wait_until = Some(expires);
            let response = post_create(
                api,
                &cookie,
                session["csrfToken"].as_str().unwrap(),
                EPOCH,
                KEY,
                &payload,
            )
            .await;
            assert_eq!(body(&response, 401)["error"]["code"], "session_expired");
            let state = backend.state.lock().unwrap();
            assert_eq!(
                state.previews,
                before.0 + 1,
                "the request must reach the slow observation before expiring"
            );
            assert_eq!(
                (state.queues, state.starts),
                (0, 0),
                "expired authentication must not reach the durable queue boundary"
            );
            assert!(state.created.is_none());
            assert_eq!(state.grants, before.1);
            assert_eq!(state.grants_revision, before.2);
        });
}

#[test]
fn stopped_create_late_revocation_keeps_receipt_without_handoff_or_replay() {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            let (api, cookie, backend) = planning_fixture();
            let (session, _, payload) = creation_request(api.clone(), &cookie, &backend).await;
            backend.state.lock().unwrap().revoke_on_queue = true;
            body(
                &post_create(
                    api.clone(),
                    &cookie,
                    session["csrfToken"].as_str().unwrap(),
                    EPOCH,
                    KEY,
                    &payload,
                )
                .await,
                401,
            );
            assert_eq!(backend.creation_counts(), (1, 0));
            let current = backend.current().unwrap();
            let login = api
                .sessions
                .lock()
                .unwrap()
                .fixture_login(current.context(&api.config));
            let cookie = login.set_cookie.split(';').next().unwrap();
            let retained = body(
                &post_create(api, cookie, &login.auth.csrf_token, EPOCH, KEY, &payload).await,
                202,
            );
            assert_eq!(retained["operation"]["state"], "queued");
            assert_eq!(backend.creation_counts(), (1, 0));
        });
}

#[test]
fn stopped_create_unix_disconnect_never_releases_work_or_replays_the_accepted_queue() {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            let (api, cookie, backend) = planning_fixture();
            let (session, _, payload) = creation_request(api.clone(), &cookie, &backend).await;
            let gate = Arc::new((Mutex::new(false), std::sync::Condvar::new()));
            backend.state.lock().unwrap().handoff_gate = Some(gate.clone());
            let request = hyper::Request::builder()
                .method("POST")
                .uri("/api/v1/environments")
                .header("host", "host.example.invalid")
                .header("origin", "https://host.example.invalid")
                .header("cookie", &cookie)
                .header("x-tentaflake-csrf", session["csrfToken"].as_str().unwrap())
                .header("content-type", "application/json")
                .header("idempotency-key", KEY)
                .header("idempotency-epoch", EPOCH)
                .body(())
                .unwrap();
            let (parts, _) = request.into_parts();
            let client = tokio::spawn(super::exchange(
                api.clone(),
                parts,
                Bytes::from(payload.clone()),
            ));
            tokio::time::timeout(Duration::from_secs(2), async {
                while backend.creation_counts().1 == 0 {
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
            })
            .await
            .unwrap();
            client.abort();
            let _ = client.await;
            assert_eq!(api.work.available_permits(), 0);
            *gate.0.lock().unwrap() = true;
            gate.1.notify_all();
            tokio::time::timeout(Duration::from_secs(2), async {
                while api.work.available_permits() == 0 {
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
            })
            .await
            .unwrap();
            let retained = body(
                &post_create(
                    api.clone(),
                    &cookie,
                    session["csrfToken"].as_str().unwrap(),
                    EPOCH,
                    KEY,
                    &payload,
                )
                .await,
                202,
            );
            assert_eq!(retained["operation"]["state"], "queued");
            assert_eq!(backend.creation_counts(), (1, 1));
        });
}
