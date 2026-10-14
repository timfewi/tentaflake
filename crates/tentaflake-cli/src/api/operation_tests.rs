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
}

impl Fixture {
    pub(super) fn grants(&self, context: &ApiContext) -> Result<Vec<ActionGrant>, AuthError> {
        let mut state = self.state.lock().unwrap();
        assert_eq!(context.principal, "operator");
        assert_eq!(context.host, "host-fixture");
        assert_eq!(context.authority, state.current.authority);
        if state.unavailable {
            return Err(AuthError::Unavailable);
        }
        let grants = state.grants.clone();
        if state.revoke_on_read {
            state.current.binding.push_str("-changed");
        }
        Ok(grants)
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
        let op = &state.operation;
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
    let current = Current {
        authority: authority.clone(),
        policy: "policy-fixture".into(),
        binding: "binding-fixture".into(),
        epoch: EPOCH.into(),
        host_policy: serde_json::from_value(recorded["snapshot"]["policy"].clone()).unwrap(),
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
