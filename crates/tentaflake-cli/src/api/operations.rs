//! Minimal original-principal receipts, never resource or dispatch authority.
use super::{Reply, config, json_reply, owner, session::AuthError};
use hyper::http::{HeaderValue, StatusCode};
use serde_json::{Value, json};
use tentaflake_admission::ledger::{
    Ledger,
    api::{ApiContext, ApiKey, ApiLookup, ApiMethod, ApiOperation, ApiOperationState},
};

#[derive(Debug, PartialEq, Eq)]
pub(super) enum Read {
    Id(String),
    Key {
        epoch: String,
        method: ApiMethod,
        target: String,
        key: String,
    },
}

/// Decode the outer query exactly once. Invalid escapes/UTF-8 and duplicate
/// fields are errors; the target's own semantic query is not reinterpreted.
pub(super) fn decode(value: &str) -> Result<String, AuthError> {
    let mut bytes = Vec::with_capacity(value.len());
    let mut input = value.bytes();
    while let Some(byte) = input.next() {
        bytes.push(match byte {
            b'%' => {
                let high = input.next().and_then(|v| char::from(v).to_digit(16));
                let low = input.next().and_then(|v| char::from(v).to_digit(16));
                match (high, low) {
                    (Some(high), Some(low)) => ((high << 4) | low) as u8,
                    _ => return Err(AuthError::Invalid),
                }
            }
            b'+' => b' ',
            value => value,
        });
    }
    String::from_utf8(bytes).map_err(|_| AuthError::Invalid)
}

pub(super) fn parse(path: &str, query: Option<&str>) -> Result<Option<Read>, AuthError> {
    if let Some(id) = path.strip_prefix("/api/v1/operations/") {
        if !config::id(id) || query.is_some() {
            return Err(AuthError::Invalid);
        }
        return Ok(Some(Read::Id(id.into())));
    }
    if path != "/api/v1/operations" {
        return Ok(None);
    }
    let query = query
        .filter(|query| query.len() <= 16 * 1024)
        .ok_or(AuthError::Invalid)?;
    let mut fields = std::collections::BTreeMap::new();
    for pair in query.split('&') {
        let (name, value) = pair.split_once('=').ok_or(AuthError::Invalid)?;
        let name = decode(name)?;
        if !matches!(
            name.as_str(),
            "idempotencyKey" | "mutationEpoch" | "method" | "target"
        ) || fields.insert(name, decode(value)?).is_some()
        {
            return Err(AuthError::Invalid);
        }
    }
    if fields.len() != 4 {
        return Err(AuthError::Invalid);
    }
    let epoch = fields.remove("mutationEpoch").ok_or(AuthError::Invalid)?;
    let mut key = fields.remove("idempotencyKey").ok_or(AuthError::Invalid)?;
    let target = fields.remove("target").ok_or(AuthError::Invalid)?;
    let method = match fields.remove("method").as_deref() {
        Some("POST") => ApiMethod::Post,
        Some("PUT") => ApiMethod::Put,
        Some("PATCH") => ApiMethod::Patch,
        Some("DELETE") => ApiMethod::Delete,
        _ => return Err(AuthError::Invalid),
    };
    if !config::id(&epoch)
        || !super::client::uuid(&key)
        || !super::client::canonical_target(&target)
    {
        return Err(AuthError::Invalid);
    }
    key.make_ascii_lowercase();
    Ok(Some(Read::Key {
        epoch,
        method,
        target,
        key,
    }))
}

impl Read {
    pub(super) fn lookup(
        &self,
        ledger: &Ledger,
        context: &ApiContext,
    ) -> Result<ApiLookup, AuthError> {
        match self {
            Self::Id(id) => ledger
                .api_operation(context, id)
                .map_err(|_| AuthError::Unavailable)?
                .map(|operation| ApiLookup::Found(Box::new(operation)))
                .ok_or(AuthError::NotFound),
            Self::Key {
                epoch,
                method,
                target,
                key,
            } => ledger
                .api_lookup(
                    context,
                    &ApiKey {
                        host: context.host.clone(),
                        principal: context.principal.clone(),
                        epoch: epoch.clone(),
                        method: method.clone(),
                        canonical_target: target.clone(),
                        key: key.clone(),
                    },
                )
                .map_err(|_| AuthError::Unavailable),
        }
    }

    pub(super) fn reply(&self, outcome: ApiLookup) -> Result<Reply, AuthError> {
        let value = match outcome {
            ApiLookup::Found(operation) => {
                let value = project(&operation)?;
                if matches!(self, Self::Id(_)) {
                    let mut reply = json_reply(StatusCode::OK, value);
                    reply.headers_mut().insert(
                        "etag",
                        HeaderValue::from_str(&format!("\"r{}\"", operation.updated_revision))
                            .map_err(|_| AuthError::Unavailable)?,
                    );
                    return Ok(reply);
                }
                json!({"state":"found","operation":value,"retrySafe":false})
            }
            ApiLookup::Absent => {
                let Self::Key { epoch, .. } = self else {
                    return Err(AuthError::NotFound);
                };
                json!({"state":"absent","operation":null,"retrySafe":true,"mutationEpoch":epoch})
            }
            ApiLookup::Unknown => {
                if matches!(self, Self::Id(_)) {
                    return Err(AuthError::Unavailable);
                }
                json!({"state":"unknown","operation":null,"retrySafe":false,
                    "reason":"The host cannot establish safe current-epoch absence."})
            }
        };
        Ok(json_reply(StatusCode::OK, value))
    }
}

pub(super) fn project(operation: &ApiOperation) -> Result<Value, AuthError> {
    let (state, error) = match operation.state {
        ApiOperationState::Queued => ("queued", Value::Null),
        ApiOperationState::Unknown => (
            "unknown",
            json!({"code":"outcome_unknown",
            "message":"Creation has an unresolved durable claim; operator reconciliation is required."}),
        ),
        ApiOperationState::Succeeded => ("succeeded", Value::Null),
        ApiOperationState::Aborted => (
            "failed",
            json!({"code":"unready",
            "message":"Creation was ended by explicit operator recovery. Storage may remain retained; this operation must not be replayed."}),
        ),
    };
    let result = if operation.state == ApiOperationState::Succeeded {
        let result = operation.result.as_ref().ok_or(AuthError::Unavailable)?;
        json!({"resources":[
            {"kind":"environment","id":result.environment_id,"state":"created","revision":format!("r{}", result.environment_revision)},
            {"kind":"workspace","id":result.workspace_id,"state":"created","revision":format!("r{}", result.workspace_revision)},
        ]})
    } else {
        Value::Null
    };
    Ok(json!({
        "id":operation.mutation.operation_id,"ownerId":operation.mutation.key.principal,
        "kind":"create","environmentId":operation.environment_id,
        "target":{"kind":"environment","id":operation.environment_id},
        "state":state,"steps":[],"error":error,
        "createdAt":owner::timestamp(operation.created_at)?,
        "updatedAt":owner::timestamp(operation.updated_at)?,
        "revision":format!("r{}", operation.updated_revision),
        "requestDigest":operation.mutation.request_digest,"result":result,
    }))
}
