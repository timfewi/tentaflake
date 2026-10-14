//! Exact accepted-create receipts and the shared preview/queue storage observer.
use super::{
    Reply, config, json_reply, operations, owner, planning,
    session::{self, AuthError, Clock},
};
use crate::{
    admission_host::{HostPlan, load_plan},
    admission_runtime,
    admission_storage::{self, VolumeSpec},
    admission_template,
};
use hyper::http::{HeaderMap, HeaderValue, StatusCode};
use ring::digest;
use serde::Deserialize;
use serde_json::{Value, json};
use std::path::Path;
use tentaflake_admission::{
    ReservationSnapshot,
    host_fence::{self, HostFence},
    ledger::{
        Ledger, LedgerError, PublishedAuthority,
        api::{
            ApiContext, ApiKey, ApiMethod, ApiMutation, CreatePolicy, EnvironmentState, OwnedCreate,
        },
    },
};

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct Body {
    plan_id: String,
    start_after_create: bool,
}

pub(super) struct Request {
    pub plan_id: String,
    pub key: ApiKey,
    pub digest: String,
    pub start: bool,
}

impl Request {
    pub(super) fn parse(
        headers: &HeaderMap,
        bytes: &[u8],
        host: &str,
        principal: &str,
    ) -> Result<Self, AuthError> {
        let content_type = session::single(headers, "content-type")?.ok_or(AuthError::Invalid)?;
        let epoch = session::single(headers, "idempotency-epoch")?.ok_or(AuthError::Invalid)?;
        let key = session::single(headers, "idempotency-key")?.ok_or(AuthError::Invalid)?;
        // Create has no resource precondition in v1. Refuse rather than silently
        // ignoring one; the plan binds all relevant current revisions.
        let if_match = session::single(headers, "if-match")?;
        let if_none_match = session::single(headers, "if-none-match")?;
        if content_type != "application/json"
            || !config::id(epoch)
            || !super::client::uuid(key)
            || if_match.is_some()
            || if_none_match.is_some()
            || bytes.len() > super::MAX_BODY
        {
            return Err(AuthError::Invalid);
        }
        let body: Body = serde_json::from_slice(bytes).map_err(|_| AuthError::Invalid)?;
        if !config::id(&body.plan_id) {
            return Err(AuthError::Invalid);
        }
        // Versioned, unambiguous framing hashes exact accepted bytes, not a
        // reserialized JSON object. Method/target/owner are the separate key.
        let framed = serde_json::to_vec(&(
            "create-request/v1",
            content_type,
            if_match,
            if_none_match,
            bytes,
        ))
        .map_err(|_| AuthError::Unavailable)?;
        let hash = digest::digest(&digest::SHA256, &framed);
        let digest = format!(
            "sha256:{}",
            hash.as_ref()
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect::<String>()
        );
        Ok(Self {
            plan_id: body.plan_id,
            start: body.start_after_create,
            digest,
            key: ApiKey {
                host: host.into(),
                principal: principal.into(),
                epoch: epoch.into(),
                method: ApiMethod::Post,
                canonical_target: "/api/v1/environments".into(),
                key: key.to_ascii_lowercase(),
            },
        })
    }
    pub(super) fn mutation(&self, prepared: &planning::Prepared) -> ApiMutation {
        ApiMutation {
            operation_id: prepared.operation_id.clone(),
            key: self.key.clone(),
            request_digest: self.digest.clone(),
        }
    }
}

pub(super) fn receipt_error(error: LedgerError) -> AuthError {
    match error {
        LedgerError::Conflict => AuthError::IdempotencyConflict,
        LedgerError::Full => AuthError::Capacity,
        other => planning::preview_error(other),
    }
}

/// Concrete common owner for both read-only preview and new queue admission.
/// Core callbacks never acquire this fence or recursively open the ledger.
pub(super) struct Host {
    pub ledger: Ledger,
    pub fence: HostFence,
}
impl Host {
    pub(super) fn open() -> Result<Self, AuthError> {
        admission_runtime::require_root().map_err(|_| AuthError::Unavailable)?;
        let fence = host_fence::exclusive_existing().map_err(|_| AuthError::Unavailable)?;
        let ledger = Ledger::open(Path::new(admission_runtime::LEDGER))
            .map_err(|_| AuthError::Unavailable)?;
        Ok(Self { ledger, fence })
    }
    pub(super) fn check<'a>(
        &self,
        plan: &'a HostPlan,
        path: &'a Path,
        prepared: &'a planning::Prepared,
        started: Clock,
    ) -> Result<StorageCheck<'a>, AuthError> {
        let inventory = crate::admission_creation::maintenance_inventory(
            &self.ledger,
            &prepared.authority,
            &self.fence,
        )
        .map_err(|_| AuthError::Unavailable)?;
        let mut specs: Vec<_> = plan.volumes.iter().map(|v| v.spec.clone()).collect();
        specs.extend(inventory.volumes);
        specs.extend(prepared.creation.volumes.iter().cloned());
        Ok(StorageCheck {
            plan,
            path,
            prepared,
            started,
            specs,
        })
    }
}
pub(super) struct StorageCheck<'a> {
    plan: &'a HostPlan,
    path: &'a Path,
    prepared: &'a planning::Prepared,
    started: Clock,
    specs: Vec<VolumeSpec>,
}
impl StorageCheck<'_> {
    pub(super) fn observe(
        &self,
        authority: &PublishedAuthority,
        snapshot: &ReservationSnapshot,
    ) -> Result<CreatePolicy, String> {
        let now = Clock::now().map_err(|_| "creation clock unavailable")?;
        if authority != &self.prepared.authority
            || !self.prepared.live(now)
            || planning::finish_clock(self.started, now).is_err()
        {
            return Err("creation authority or deadline changed".into());
        }
        admission_runtime::installed(self.path)?;
        self.prepared.check_source(self.plan)?;
        admission_storage::check_creation_space(
            &self.specs,
            &snapshot.storage,
            admission_storage::CreationTargets::New(&self.prepared.creation.volumes),
            self.plan.physical_space_headroom_bytes,
        )?;
        admission_runtime::installed(self.path)?;
        let now = Clock::now().map_err(|_| "creation clock unavailable")?;
        if !self.prepared.live(now) || planning::finish_clock(self.started, now).is_err() {
            return Err("creation deadline exceeded".into());
        }
        Ok(CreatePolicy {
            policy: self.plan.policy.clone(),
            create_grant: self.prepared.create_grant.clone(),
            issued_grants: self.prepared.creation.environment.grants.clone(),
        })
    }
}

pub(super) fn original(receipt: &OwnedCreate) -> Result<HostPlan, AuthError> {
    let path = Path::new(&receipt.operation.authority.manifest_path);
    let plan = load_plan(path).map_err(|_| AuthError::Unavailable)?;
    if admission_runtime::authority(&plan, path) != receipt.operation.authority {
        return Err(AuthError::Unavailable);
    }
    Ok(plan)
}

pub(super) fn reply(
    receipt: &OwnedCreate,
    original: &HostPlan,
    revisions: Value,
) -> Result<Reply, AuthError> {
    let environment = &receipt.environment;
    let spec = &environment.spec;
    admission_template::verify_recorded(original, spec).map_err(|_| AuthError::Unavailable)?;
    let template = original
        .creation_templates
        .get(&spec.template_id)
        .ok_or(AuthError::Unavailable)?;
    let source = original
        .instances
        .get(&template.source_instance)
        .ok_or(AuthError::Unavailable)?;
    let (_, config) = template
        .public_metadata(source)
        .map_err(|_| AuthError::Unavailable)?;
    let (state, detail) = match environment.state {
        EnvironmentState::Provisioning => ("provisioning", None),
        EnvironmentState::Stopped => ("stopped", None),
        EnvironmentState::Unknown => (
            "unknown",
            Some("Creation requires explicit operator reconciliation."),
        ),
        EnvironmentState::Aborted => (
            "failed",
            Some("Creation was ended by explicit operator recovery."),
        ),
    };
    let value = json!({"operation":operations::project(&receipt.operation)?,"environment":{
        "id":spec.instance.id,"ownerId":spec.instance.owner_id,"name":spec.name,"state":state,"detail":detail,
        "config":config,"active":null,"presetId":null,"workspace":{"id":receipt.workspace.id,
            "ownerId":receipt.workspace.owner_id,"revision":format!("r{}",receipt.workspace.revision),
            "usedBytes":null,"quotaBytes":spec.instance.resources.workspace_bytes},
        "usage":null,"operation":receipt.operation.mutation.operation_id,
        "createdAt":owner::timestamp(environment.created_at)?,"revision":format!("r{}",environment.revision),
        "revisions":revisions,"availability":{"available":false,"state":"unsupported","reason":"Runtime start is not implemented by this API."},"evidence":[]
    }});
    if value.to_string().len() > super::MAX_BODY {
        return Err(AuthError::Unavailable);
    }
    let mut reply = json_reply(StatusCode::ACCEPTED, value);
    reply.headers_mut().insert(
        "operation-id",
        HeaderValue::from_str(&receipt.operation.mutation.operation_id)
            .map_err(|_| AuthError::Unavailable)?,
    );
    Ok(reply)
}

pub(super) fn context(
    authority: &PublishedAuthority,
    host: &str,
    principal: &str,
    epoch: &str,
) -> Result<ApiContext, AuthError> {
    Ok(ApiContext {
        authority: authority.clone(),
        host: host.into(),
        principal: principal.into(),
        epoch: epoch.into(),
        now: Clock::now()?.unix_seconds,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn candidate_collision_is_not_a_changed_idempotent_request() {
        // Core prepare_api_create uses Conflict for generated resource/native
        // operation collisions too. Only the keyed receipt lookup establishes
        // that an existing request's digest differs.
        assert_eq!(
            receipt_error(LedgerError::Conflict),
            AuthError::IdempotencyConflict
        );
        assert_eq!(
            planning::preview_error(LedgerError::Conflict),
            AuthError::PlanStale
        );
    }

    #[test]
    fn create_request_is_closed_and_hashes_exact_bytes_and_scope() {
        let mut headers = HeaderMap::new();
        headers.insert("content-type", HeaderValue::from_static("application/json"));
        headers.insert("idempotency-epoch", HeaderValue::from_static("epoch"));
        headers.insert(
            "idempotency-key",
            HeaderValue::from_static("12345678-1234-4234-8234-123456789ABC"),
        );
        let body = br#"{"planId":"plan-one","startAfterCreate":false}"#;
        let request = Request::parse(&headers, body, "host", "owner").unwrap();
        assert_eq!(request.key.key, "12345678-1234-4234-8234-123456789abc");
        assert!(request.digest.starts_with("sha256:"));
        assert_eq!(request.digest.len(), 71);
        let changed = [body.as_slice(), b" "].concat();
        assert_ne!(
            request.digest,
            Request::parse(&headers, &changed, "host", "owner")
                .unwrap()
                .digest
        );
        assert_ne!(
            request.key,
            Request::parse(&headers, body, "host", "other").unwrap().key
        );
        for body in [
            br#"{"planId":"plan-one"}"#.as_slice(),
            br#"{"planId":"plan-one","startAfterCreate":null}"#,
            br#"{"planId":"plan-one","startAfterCreate":false,"extra":1}"#,
            br#"{"planId":"plan-one","planId":"plan-two","startAfterCreate":false}"#,
        ] {
            assert!(Request::parse(&headers, body, "host", "owner").is_err());
        }
        for name in ["idempotency-key", "idempotency-epoch", "content-type"] {
            let mut duplicate = headers.clone();
            duplicate.append(name, headers[name].clone());
            assert!(Request::parse(&duplicate, body, "host", "owner").is_err());
        }
        for name in ["if-match", "if-none-match"] {
            let mut conditional = headers.clone();
            conditional.insert(name, HeaderValue::from_static("*"));
            assert!(Request::parse(&conditional, body, "host", "owner").is_err());
        }
    }
}
