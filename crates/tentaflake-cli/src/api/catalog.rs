//! Public metadata from installed creation templates, never private source config.
use super::{MAX_BODY, Reply, json_reply, operations::decode, owner, session::AuthError};
use crate::admission_host::HostPlan;
use hyper::http::{HeaderValue, StatusCode};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use tentaflake_admission::ledger::api::ActionGrant;

#[derive(Clone, Default)]
pub(super) struct Catalog {
    images: Vec<Value>,
}

impl Catalog {
    #[cfg(test)]
    pub(super) fn fixture() -> Self {
        let fixtures: Value =
            serde_json::from_str(include_str!("../../../../api/v1/fixtures.json")).unwrap();
        let first = fixtures["exchanges"]
            .as_array()
            .unwrap()
            .iter()
            .find(|v| v["name"] == "listImages")
            .unwrap()["response"]["body"][0]
            .clone();
        let mut second = first.clone();
        second["id"] = json!("second-image");
        Self {
            images: vec![first, second],
        }
    }
    /// load_plan has validated every template against its original immutable
    /// catalog. This projection never exposes source IDs, argv, env or paths.
    pub(super) fn project(plan: &HostPlan) -> Result<Self, AuthError> {
        let mut images = BTreeMap::new();
        for template in plan.creation_templates.values() {
            let instance = plan
                .instances
                .get(&template.source_instance)
                .ok_or(AuthError::Unavailable)?;
            let (image, _) = template
                .public_metadata(instance)
                .map_err(|_| AuthError::Unavailable)?;
            if images
                .insert(template.image.id.clone(), image.clone())
                .is_some_and(|previous| previous != image)
            {
                return Err(AuthError::Unavailable);
            }
        }
        Ok(Self {
            images: images.into_values().collect(),
        })
    }

    pub(super) fn revision(&self) -> String {
        // No block provider is installed by this offline creation-template owner.
        owner::revision(
            json!(["catalog/v1", {"images":self.images,"blocks":[]}])
                .to_string()
                .as_bytes(),
        )
    }
}

pub(super) fn grants(
    installed: Option<&ActionGrant>,
    mut durable: Vec<ActionGrant>,
) -> Result<Vec<Value>, AuthError> {
    durable.extend(installed.cloned());
    durable.sort_by(|a, b| a.id.cmp(&b.id));
    if durable.windows(2).any(|pair| pair[0].id == pair[1].id) {
        return Err(AuthError::Unavailable);
    }
    durable
        .into_iter()
        .map(|grant| {
            grant.validate().map_err(|_| AuthError::Unavailable)?;
            Ok(
                json!({"id":grant.id,"action":grant.action,"target":grant.target,
            "expiresAt":grant.expires_at.map(owner::timestamp).transpose()?}),
            )
        })
        .collect()
}

pub(super) fn grants_revision(host: &str, principal: &str, grants: &[Value]) -> String {
    owner::revision(
        json!(["effective-grants/v1", host, principal, grants])
            .to_string()
            .as_bytes(),
    )
}

pub(super) struct Page {
    images: bool,
    cursor: Option<String>,
    limit: usize,
}

pub(super) fn parse(path: &str, query: Option<&str>) -> Result<Option<Page>, AuthError> {
    let images = match path {
        "/api/v1/catalog/images" => true,
        "/api/v1/catalog/blocks" => false,
        _ => return Ok(None),
    };
    let mut fields = BTreeMap::new();
    if let Some(query) = query {
        if query.is_empty() || query.len() > 8192 {
            return Err(AuthError::Invalid);
        }
        for pair in query.split('&') {
            let (name, value) = pair.split_once('=').ok_or(AuthError::Invalid)?;
            let name = decode(name)?;
            if !matches!(name.as_str(), "cursor" | "limit")
                || fields.insert(name, decode(value)?).is_some()
            {
                return Err(AuthError::Invalid);
            }
        }
    }
    let limit = if let Some(value) = fields.remove("limit") {
        let limit: usize = value.parse().map_err(|_| AuthError::Invalid)?;
        if !(1..=500).contains(&limit) || limit.to_string() != value {
            return Err(AuthError::Invalid);
        }
        limit
    } else {
        100
    };
    let cursor = fields.remove("cursor");
    if cursor
        .as_ref()
        .is_some_and(|v| v.is_empty() || v.len() > 2048 || v.chars().any(char::is_control))
    {
        return Err(AuthError::Invalid);
    }
    Ok(Some(Page {
        images,
        cursor,
        limit,
    }))
}

impl Page {
    pub(super) fn reply(&self, catalog: &Catalog, scope: &Value) -> Result<Reply, AuthError> {
        let revision = catalog.revision();
        let scope = owner::revision(
            json!(["catalog-page/v1", scope, revision, self.images])
                .to_string()
                .as_bytes(),
        );
        let items = if self.images {
            catalog.images.as_slice()
        } else {
            &[]
        };
        let start = if let Some(cursor) = &self.cursor {
            let (bound, offset) = cursor.rsplit_once('.').ok_or(AuthError::CursorExpired)?;
            let offset: usize = offset.parse().map_err(|_| AuthError::CursorExpired)?;
            if bound != scope
                || offset == 0
                || offset >= items.len()
                || cursor != &format!("{bound}.{offset}")
            {
                return Err(AuthError::CursorExpired);
            }
            offset
        } else {
            0
        };
        // The transport bound applies even when the requested row limit is large.
        let mut end = start;
        let mut bytes = 2;
        while end < items.len() && end - start < self.limit {
            let next = items[end].to_string().len() + usize::from(end != start);
            if bytes + next > MAX_BODY {
                break;
            }
            bytes += next;
            end += 1;
        }
        if end == start && start < items.len() {
            return Err(AuthError::Unavailable);
        }
        let mut reply = json_reply(StatusCode::OK, json!(&items[start..end]));
        reply.headers_mut().insert(
            "etag",
            HeaderValue::from_str(&format!("\"{revision}\""))
                .map_err(|_| AuthError::Unavailable)?,
        );
        if end < items.len() {
            reply.headers_mut().insert(
                "next-cursor",
                HeaderValue::from_str(&format!("{scope}.{end}"))
                    .map_err(|_| AuthError::Unavailable)?,
            );
        }
        Ok(reply)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tentaflake_admission::ledger::api::{ApiAction, ApiTarget};

    #[test]
    fn projection_deduplicates_exact_metadata_and_never_serializes_sources() {
        let mut plan = crate::admission_oci::tests::template_plan_fixture();
        let source = "agent-fixture";
        let helpers = json!({"create":"/private/create","verify":"/private/verify","owner":"/private/owner","mount":"/private/mount","unmount":"/private/unmount"});
        let template = json!({"sourceInstance":source,"sourceRevision":"private-revision","revision":"private-template",
            "reference":"private-registry","image":{"id":"image-one","distro":"debian","release":"13","label":"Public image","digest":format!("sha256:{}","1".repeat(64))},
            "principals":["private-reader"],"issuedActions":["environments.read"],"helpers":{"state":helpers,"workspace":helpers}});
        for id in ["private-template-one", "private-template-two"] {
            plan.creation_templates
                .insert(id.into(), serde_json::from_value(template.clone()).unwrap());
        }
        let catalog = Catalog::project(&plan).unwrap();
        assert_eq!(catalog.images.len(), 1);
        let image = &catalog.images[0];
        assert_eq!(image.as_object().unwrap().len(), 7);
        assert_eq!(image["evidence"], json!([]));
        assert_eq!(image["availability"]["available"], false);
        assert!(!image.to_string().contains("private"));
        assert!(!image.to_string().contains(source));
        plan.creation_templates
            .get_mut("private-template-two")
            .unwrap()
            .image
            .label = "conflicting".into();
        assert!(Catalog::project(&plan).is_err());
        plan.creation_templates.clear();
        assert!(Catalog::project(&plan).unwrap().images.is_empty());
    }

    #[test]
    fn effective_grants_have_canonical_owner_scoped_revisions_and_reject_collisions() {
        let read = ActionGrant {
            id: "catalog-one".into(),
            action: ApiAction::CatalogRead,
            target: ApiTarget::Catalog {
                id: "installed".into(),
            },
            expires_at: None,
        };
        let resource = ActionGrant {
            id: "environment-one".into(),
            action: ApiAction::EnvironmentsRead,
            target: ApiTarget::Environment {
                id: "env-one".into(),
            },
            expires_at: None,
        };
        let a = grants(Some(&read), vec![resource.clone()]).unwrap();
        let b = grants(None, vec![resource.clone(), read.clone()]).unwrap();
        assert_eq!(a, b);
        assert_eq!(
            grants_revision("host", "owner", &a),
            grants_revision("host", "owner", &b)
        );
        assert_ne!(
            grants_revision("host", "owner", &a),
            grants_revision("host", "other", &a)
        );
        assert_ne!(
            grants_revision("host", "owner", &a),
            grants_revision("host", "owner", &grants(None, vec![resource]).unwrap())
        );
        assert!(grants(Some(&read), vec![read.clone()]).is_err());
    }

    #[test]
    fn page_queries_are_closed_and_transport_size_can_limit_requested_rows() {
        for query in [
            "",
            "limit=0",
            "limit=501",
            "limit=01",
            "limit=1&limit=2",
            "cursor=x&%63ursor=y",
            "cursor=",
            "owner=other",
            "limit=%GG",
            "cursor=%FF",
            "cursor=%00",
        ] {
            assert!(
                parse("/api/v1/catalog/images", Some(query)).is_err(),
                "{query}"
            );
        }
        let mut catalog = Catalog::fixture();
        catalog.images[0]["label"] = json!("x".repeat(MAX_BODY / 2));
        catalog.images[1]["label"] = json!("x".repeat(MAX_BODY / 2));
        let reply = parse("/api/v1/catalog/images", Some("limit=500"))
            .unwrap()
            .unwrap()
            .reply(&catalog, &json!("scope"))
            .unwrap();
        assert!(reply.headers().contains_key("next-cursor"));
        assert_eq!(reply.headers()["cache-control"], "no-store");
    }
}
