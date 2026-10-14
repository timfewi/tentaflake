//! Bounded owner-local previews. No cache entry is a reservation or permit.
use super::{
    config, owner,
    session::{AuthError, Clock},
};
use crate::{
    admission_host::HostPlan,
    admission_template::{self, ReconstructedCreation},
};
use ring::rand::{SecureRandom, SystemRandom};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::time::Duration;
use tentaflake_admission::ledger::{PublishedAuthority, api::ActionGrant};

const LIFETIME: Duration = Duration::from_secs(300);
const MAX_PLANS: usize = 64;
const MAX_CACHE_BYTES: usize = 2 * 1024 * 1024;
pub(super) const DEADLINE: Duration = Duration::from_secs(10);

#[derive(Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(super) struct Request {
    pub name: String,
    pub config: Environment,
    pub workspace: Workspace,
    pub revisions: Revisions,
}

#[derive(Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(super) struct Environment {
    image: String,
    isolation: Isolation,
    #[serde(deserialize_with = "required_option")]
    agent_runtime: Option<String>,
    tools: Vec<String>,
    skills: Vec<String>,
    capabilities: Vec<String>,
    resources: Resources,
}

fn required_option<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<String>, D::Error> {
    Option::deserialize(deserializer)
}

#[derive(Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
enum Isolation {
    Container,
    GuestKernel,
}

#[derive(Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields, tag = "kind", rename_all = "camelCase")]
pub(super) enum Workspace {
    New {},
    Retained {
        #[serde(rename = "workspaceId")]
        workspace_id: String,
        revision: String,
    },
}

#[derive(Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct Resources {
    cpu: u64,
    #[serde(rename = "memoryMiB")]
    memory_mib: u64,
    #[serde(rename = "storageMiB")]
    storage_mib: u64,
}

#[derive(Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct Revisions {
    policy: String,
    catalog: String,
    grants: String,
    host: String,
}

impl Request {
    pub(super) fn parse(body: &[u8]) -> Result<Self, AuthError> {
        if body.len() > super::MAX_BODY {
            return Err(AuthError::Invalid);
        }
        let request: Self = serde_json::from_slice(body).map_err(|_| AuthError::Invalid)?;
        let environment = &request.config;
        let resources = &environment.resources;
        if request.name.is_empty()
            || request.name.chars().count() > 160
            || request.name.chars().any(char::is_control)
            || !config::id(&environment.image)
            || environment
                .agent_runtime
                .as_ref()
                .is_some_and(|id| !config::id(id))
            || [
                &environment.tools,
                &environment.skills,
                &environment.capabilities,
            ]
            .iter()
            .any(|rows| {
                rows.len() > 64
                    || rows.iter().any(|id| !config::id(id))
                    || rows.iter().collect::<BTreeSet<_>>().len() != rows.len()
            })
            || !(1..=1024).contains(&resources.cpu)
            || !(128..=1_048_576).contains(&resources.memory_mib)
            || !(128..=1_048_576).contains(&resources.storage_mib)
            || [
                &request.revisions.policy,
                &request.revisions.catalog,
                &request.revisions.grants,
                &request.revisions.host,
            ]
            .iter()
            .any(|id| !config::id(id))
            || matches!(&request.workspace,Workspace::Retained{workspace_id,revision} if !config::id(workspace_id)||!config::id(revision))
        {
            return Err(AuthError::Invalid);
        }
        Ok(request)
    }

    pub(super) fn matches_revisions(&self, revisions: &Value) -> bool {
        serde_json::to_value(&self.revisions).ok().as_ref() == Some(revisions)
    }
}

pub(super) fn finish_clock(started: Clock, now: Clock) -> Result<(), AuthError> {
    if now.unix_seconds < started.unix_seconds
        || now.unix_seconds - started.unix_seconds >= LIFETIME.as_secs()
        || now
            .monotonic
            .checked_duration_since(started.monotonic)
            .is_none_or(|elapsed| elapsed >= DEADLINE)
    {
        return Err(AuthError::Unavailable);
    }
    Ok(())
}

fn random<const N: usize>() -> Result<[u8; N], AuthError> {
    let mut bytes = [0; N];
    SystemRandom::new()
        .fill(&mut bytes)
        .map_err(|_| AuthError::Unavailable)?;
    Ok(bytes)
}

fn opaque(prefix: &str) -> Result<String, AuthError> {
    use std::fmt::Write;
    let mut id = prefix.to_owned();
    for byte in random::<32>()? {
        write!(&mut id, "{byte:02x}").expect("string formatting");
    }
    Ok(id)
}

pub(super) struct Prepared {
    id: String,
    pub operation_id: String,
    pub request: Request,
    pub authority: PublishedAuthority,
    pub binding: String,
    pub host: String,
    pub principal: String,
    pub native_grants_revision: u64,
    pub creation: ReconstructedCreation,
    pub create_grant: ActionGrant,
    source_revision: String,
    image: Value,
    created: Clock,
    expires_at: u64,
}

pub(super) fn prepare(
    plan: &HostPlan,
    context: &super::owner::Current,
    config: &config::Config,
    request: Request,
    native_grants_revision: u64,
    created: Clock,
) -> Result<Option<Prepared>, AuthError> {
    let grant = admission_template::creation_grant(plan, &config.host, &config.principal.id)
        .map_err(|_| AuthError::Unavailable)?
        .ok_or(AuthError::Forbidden)?;
    if request.workspace != (Workspace::New {}) {
        return Ok(None);
    }
    let signature = serde_json::to_value(&request.config).map_err(|_| AuthError::Invalid)?;
    let mut selected = None;
    for (id, template) in &plan.creation_templates {
        if !template.principals.contains(&config.principal.id) {
            continue;
        }
        let source = plan
            .instances
            .get(&template.source_instance)
            .ok_or(AuthError::Unavailable)?;
        let (image, settings) = template
            .public_metadata(source)
            .map_err(|_| AuthError::Unavailable)?;
        if settings == signature {
            if selected.is_some() {
                return Err(AuthError::Unavailable);
            }
            selected = Some((id, template, image));
        }
    }
    let Some((template_id, template, image)) = selected else {
        return Ok(None);
    };
    let creation = admission_template::reconstruct(
        plan,
        template_id,
        random()?,
        &config.principal.id,
        &request.name,
    )
    .map_err(|_| AuthError::Unavailable)?;
    Ok(Some(Prepared {
        id: opaque("plan-")?,
        operation_id: opaque("plan-op-")?,
        request,
        authority: context.authority.clone(),
        binding: context.binding.clone(),
        host: config.host.clone(),
        principal: config.principal.id.clone(),
        native_grants_revision,
        creation,
        create_grant: grant,
        source_revision: template.source_revision.clone(),
        image,
        created,
        expires_at: created
            .unix_seconds
            .checked_add(LIFETIME.as_secs())
            .ok_or(AuthError::Unavailable)?,
    }))
}

impl Prepared {
    fn bytes(&self) -> Result<usize, AuthError> {
        serde_json::to_vec(&(
            &self.id,
            &self.operation_id,
            &self.request,
            &self.authority,
            &self.binding,
            &self.host,
            &self.principal,
            &self.creation.environment,
            &self.creation.volumes,
            &self.create_grant,
            &self.source_revision,
            &self.image,
        ))
        .map(|bytes| bytes.len())
        .map_err(|_| AuthError::Unavailable)
    }
    pub(super) fn check_source(&self, plan: &HostPlan) -> Result<(), String> {
        let template = plan
            .creation_templates
            .get(&self.creation.environment.template_id)
            .ok_or("missing preview template")?;
        if template.source_revision != self.source_revision
            || template.revision != self.creation.environment.template_revision
            || admission_template::creation_grant(plan, &self.host, &self.principal)?.as_ref()
                != Some(&self.create_grant)
        {
            return Err("preview source policy changed".into());
        }
        let reconstructed = admission_template::verify_recorded(plan, &self.creation.environment)?;
        if reconstructed.volumes != self.creation.volumes {
            return Err("preview volume binding changed".into());
        }
        Ok(())
    }

    fn live(&self, clock: Clock) -> bool {
        clock.unix_seconds >= self.created.unix_seconds
            && clock.unix_seconds < self.expires_at
            && clock
                .monotonic
                .checked_duration_since(self.created.monotonic)
                .is_some_and(|age| age < LIFETIME)
    }

    fn reply(&self) -> Result<Value, AuthError> {
        Ok(
            json!({"id":self.id,"ownerId":self.principal,"valid":true,"expiresAt":owner::timestamp(self.expires_at)?,
            "request":self.request,"revisions":self.request.revisions,"problems":[],
            "resolved":{"image":{"label":self.image["label"],"digest":self.image["digest"]},
                "isolation":"container","agentRuntime":null,"blocks":[],"capabilities":[],"resources":self.request.config.resources,
                "workspace":"New empty workspace","grantIds":[self.create_grant.id],
                "environmentId":self.creation.environment.instance.id,"workspaceId":self.creation.environment.workspace_id,
                "proposedGrants":super::catalog::grants(None,self.creation.environment.grants.clone())?}}),
        )
    }
}

pub(super) fn unsupported(
    request: Request,
    principal: &str,
    clock: Clock,
) -> Result<Value, AuthError> {
    Ok(
        json!({"id":opaque("plan-")?,"ownerId":principal,"valid":false,
        "expiresAt":owner::timestamp(clock.unix_seconds.checked_add(LIFETIME.as_secs()).ok_or(AuthError::Unavailable)?)?,
        "revisions":request.revisions,"request":request,"resolved":null,
        "problems":[{"severity":"error","code":"unsupported","message":"No authorized installed template exactly supports this configuration and workspace choice."}]}),
    )
}

#[derive(Default)]
pub(super) struct Cache {
    entries: Vec<Prepared>,
}

impl Cache {
    pub(super) fn room(
        &mut self,
        binding: &str,
        revisions: &Value,
        native: u64,
        clock: Clock,
    ) -> Result<(), AuthError> {
        self.entries.retain(|entry| {
            entry.live(clock)
                && entry.binding == binding
                && entry.native_grants_revision == native
                && entry.request.matches_revisions(revisions)
        });
        if self.entries.len() >= MAX_PLANS {
            return Err(AuthError::Capacity);
        }
        Ok(())
    }

    pub(super) fn insert(&mut self, prepared: Prepared, clock: Clock) -> Result<Value, AuthError> {
        if !prepared.live(clock)
            || self.entries.len() >= MAX_PLANS
            || self.entries.iter().any(|entry| {
                entry.id == prepared.id
                    || entry.operation_id == prepared.operation_id
                    || entry.creation.environment.instance.id
                        == prepared.creation.environment.instance.id
                    || entry.creation.environment.workspace_id
                        == prepared.creation.environment.workspace_id
            })
        {
            return Err(AuthError::Unavailable);
        }
        let bytes = self
            .entries
            .iter()
            .try_fold(prepared.bytes()?, |bytes, entry| {
                bytes
                    .checked_add(entry.bytes()?)
                    .ok_or(AuthError::Unavailable)
            })?;
        if bytes > MAX_CACHE_BYTES {
            return Err(AuthError::Capacity);
        }
        let reply = prepared.reply()?;
        if reply.to_string().len() > super::MAX_BODY {
            return Err(AuthError::Unavailable);
        }
        self.entries.push(prepared);
        Ok(reply)
    }
}

// Only a proven native capacity denial becomes a public quota error. No native
// subject or diagnostic crosses this projection boundary.
pub(super) fn preview_error(error: tentaflake_admission::ledger::LedgerError) -> AuthError {
    use tentaflake_admission::{ReasonCode, ledger::LedgerError};
    match error {
        LedgerError::StaleRevision | LedgerError::StaleAuthority => AuthError::PlanStale,
        LedgerError::StorageDenied(decision)
            if decision
                .reasons
                .iter()
                .any(|reason| reason.code == ReasonCode::ResourceExhausted) =>
        {
            AuthError::QuotaExceeded
        }
        _ => AuthError::Unavailable,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::owner::operation_tests::prepared;

    #[test]
    fn final_clock_refuses_late_observations_and_wall_clock_changes() {
        let started = Clock::now().unwrap();
        finish_clock(started, started).unwrap();
        for case in 0..4 {
            let mut now = started;
            match case {
                0 => now.monotonic += DEADLINE,
                1 => now.unix_seconds -= 1,
                2 => now.unix_seconds += LIFETIME.as_secs(),
                _ => now.monotonic -= Duration::from_secs(1),
            }
            assert_eq!(finish_clock(started, now), Err(AuthError::Unavailable));
        }
    }

    #[test]
    fn request_is_closed_and_preserves_exact_wire_values() {
        let now = Clock::now().unwrap();
        let valid = serde_json::to_value(prepared(now).request).unwrap();
        let mut unicode = valid.clone();
        unicode["name"] = json!("é".repeat(160));
        assert!(Request::parse(&serde_json::to_vec(&unicode).unwrap()).is_ok());
        for case in 0..11 {
            let mut changed = valid.clone();
            match case {
                0 => {
                    changed["config"]
                        .as_object_mut()
                        .unwrap()
                        .remove("agentRuntime");
                }
                1 => changed["config"]["resources"]["cpu"] = json!(1.5),
                2 => changed["config"]["resources"]["memoryMiB"] = json!(127),
                3 => changed["config"]["resources"]["storageMiB"] = json!(1_048_577),
                4 => changed["workspace"]["path"] = json!("/private"),
                5 => changed["config"]["env"] = json!({}),
                6 => changed["config"]["tools"] = json!(["same", "same"]),
                7 => changed["revisions"]["unknown"] = json!("extra"),
                8 => changed["name"] = json!("é".repeat(161)),
                9 => changed["name"] = json!("control\nname"),
                _ => changed["unexpected"] = json!(true),
            }
            assert!(
                Request::parse(&serde_json::to_vec(&changed).unwrap()).is_err(),
                "case {case}"
            );
        }
        assert!(Request::parse(&vec![b' '; super::super::MAX_BODY + 1]).is_err());
        let encoded = serde_json::to_string(&valid).unwrap();
        let repeated = encoded.replacen("{", "{\"name\":\"duplicate\",", 1);
        assert!(Request::parse(repeated.as_bytes()).is_err());
    }

    #[test]
    fn cache_expiry_and_binding_changes_cannot_renew_or_reconstruct_candidates() {
        let now = Clock::now().unwrap();
        for case in 0..6 {
            let entry = prepared(now);
            let mut binding = entry.binding.clone();
            let mut revisions = serde_json::to_value(&entry.request.revisions).unwrap();
            let mut native = entry.native_grants_revision;
            let mut clock = now;
            let mut cache = Cache::default();
            let reply = cache.insert(entry, now).unwrap();
            assert_eq!(
                reply["expiresAt"],
                owner::timestamp(now.unix_seconds + 300).unwrap()
            );
            match case {
                0 => clock.monotonic += LIFETIME,
                1 => clock.unix_seconds += 300,
                2 => clock.unix_seconds -= 1,
                3 => binding.push_str("-changed"),
                4 => revisions["policy"] = json!("different-policy"),
                _ => native += 1,
            }
            cache.room(&binding, &revisions, native, clock).unwrap();
            assert!(cache.entries.is_empty(), "expired/stale case {case}");
        }
        let mut cache = Cache::default();
        let entry = prepared(now);
        let mut later = now;
        later.monotonic += LIFETIME;
        assert_eq!(
            cache.insert(entry, later).unwrap_err(),
            AuthError::Unavailable
        );
        assert!(cache.entries.is_empty());
        // A new owner has no persisted cache to reconstruct, even though the
        // template and principal are still installed.
        assert!(Cache::default().entries.is_empty());
    }

    #[test]
    fn cache_bounds_preserve_candidates_and_private_binding_checks() {
        let now = Clock::now().unwrap();
        let mut cache = Cache::default();
        for _ in 0..MAX_PLANS {
            cache.insert(prepared(now), now).unwrap();
        }
        let first = &cache.entries[0];
        let binding = first.binding.clone();
        let revisions = serde_json::to_value(&first.request.revisions).unwrap();
        assert_eq!(
            cache.room(&binding, &revisions, 0, now).unwrap_err(),
            AuthError::Capacity
        );
        assert_eq!(cache.entries.len(), MAX_PLANS);
        let mut oversized = prepared(now);
        oversized.creation.environment.name = "x".repeat(MAX_CACHE_BYTES);
        let mut empty = Cache::default();
        assert_eq!(
            empty.insert(oversized, now).unwrap_err(),
            AuthError::Capacity
        );
        assert!(empty.entries.is_empty());
        let mut changed = prepared(now);
        let original = crate::admission_template::tests::fixture_plan();
        changed.check_source(&original).unwrap();
        changed.creation.environment.instance.owner_id = "other-principal".into();
        assert!(changed.check_source(&original).is_err());
        let mut removed = crate::admission_template::tests::fixture_plan();
        removed.creation_templates.clear();
        assert!(prepared(now).check_source(&removed).is_err());
    }

    #[test]
    fn only_native_resource_exhaustion_is_a_public_quota_refusal() {
        use tentaflake_admission::{Reason, ReasonCode, StorageDecision, ledger::LedgerError};
        for (code, expected) in [
            (ReasonCode::ResourceExhausted, AuthError::QuotaExceeded),
            (ReasonCode::StorageUnknown, AuthError::Unavailable),
        ] {
            let error = LedgerError::StorageDenied(Box::new(StorageDecision {
                allowed: false,
                allocation: None,
                reasons: vec![Reason {
                    code,
                    subject: "private-source".into(),
                }],
            }));
            assert_eq!(preview_error(error), expected);
        }
        assert_eq!(
            preview_error(LedgerError::TransitionPending),
            AuthError::Unavailable
        );
        assert_eq!(
            preview_error(LedgerError::StaleRevision),
            AuthError::PlanStale
        );
        assert_eq!(preview_error(LedgerError::Context), AuthError::Unavailable);
    }
}
