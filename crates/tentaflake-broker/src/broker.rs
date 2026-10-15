use crate::config::Config;
use crate::http::{Request, Response};
use crate::policy::{ResolvedTarget, resolve_public_target};
use crate::routes::{Api, ModelPolicy, Route, Routes};
use crate::state::{Audit, Limits, read_secret};
use crate::streaming::{Protocol, StreamingResponse};
use reqwest::blocking::{Client, Response as UpstreamResponse};
use reqwest::header::CONTENT_TYPE;
use serde_json::{Value, json};
use std::io::Read;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Upstream request headers: the provider credential and the few client
/// headers forwarded after validation.
pub type Headers = Vec<(&'static str, String)>;

pub struct Broker {
    config: Config,
    routes: Routes,
    limits: Limits,
    audit: Arc<Audit>,
}

impl Broker {
    pub fn new(config: Config) -> Result<Self, String> {
        let limits = Limits::load(
            config.budget_state_file.clone(),
            config.rate_window_seconds,
            config.max_requests_per_window,
        )?;
        let routes = Routes::new(config.routes_file.clone());
        let count = routes.table()?.len();
        let audit = Arc::new(Audit::new(
            config.audit_file.clone(),
            config.max_audit_bytes,
        ));
        audit.check_ready()?;
        audit.record("-", "startup", "ready", json!({ "routes": count }))?;
        Ok(Self {
            config,
            routes,
            limits,
            audit,
        })
    }

    pub fn handle(&self, request: Request) -> Response {
        if request.method == "GET" && route_without_query(&request.path) == "/healthz" {
            return match self.health() {
                Ok(()) => Response::json(200, json!({ "status": "ready" })),
                Err(failure) => Response::json(failure.status, json!({ "error": failure.public })),
            };
        }
        let route = match self.authenticate(&request) {
            Ok(route) => route,
            Err(failure) => return self.deny("-", failure),
        };
        self.forward(&route, request)
            .unwrap_or_else(|failure| self.deny(&route.id, failure))
    }

    fn deny(&self, route: &str, failure: Failure) -> Response {
        let _ = self.audit.record(
            route,
            failure.kind,
            "denied",
            json!({ "reason": failure.public }),
        );
        Response::json(failure.status, json!({ "error": failure.public }))
    }

    fn health(&self) -> Result<(), Failure> {
        self.routes
            .table()
            .map_err(|_| Failure::new(503, "health", "routes are unavailable"))?;
        self.audit
            .check_ready()
            .map_err(|_| Failure::new(503, "health", "audit log is unavailable"))
    }

    /// The route of the environment token, sent as a bearer token (OpenAI
    /// clients) or as `x-api-key` (Anthropic clients).
    fn authenticate(&self, request: &Request) -> Result<Arc<Route>, Failure> {
        if request.method != "POST" {
            return Err(Failure::new(405, "request", "only POST is accepted"));
        }
        let token = request
            .headers
            .get("authorization")
            .and_then(|value| value.strip_prefix("Bearer "))
            .or_else(|| request.headers.get("x-api-key").map(String::as_str))
            .filter(|token| !token.is_empty())
            .ok_or_else(|| Failure::new(401, "auth", "broker credential is required"))?;
        let route = self
            .routes
            .find(token)
            .map_err(|_| Failure::new(503, "auth", "routes are unavailable"))?
            .ok_or_else(|| Failure::new(401, "auth", "broker credential is invalid"))?;
        if request
            .headers
            .get("content-type")
            .and_then(|value| value.split(';').next())
            .map(str::trim)
            .is_none_or(|value| !value.eq_ignore_ascii_case("application/json"))
        {
            return Err(Failure::new(
                400,
                "request",
                "content-type must be application/json",
            ));
        }
        Ok(route)
    }

    fn forward(&self, route: &Route, request: Request) -> Result<Response, Failure> {
        let path = route_without_query(&request.path);
        let protocol = match (route.api, path) {
            (Api::Openai, "/v1/chat/completions") => Protocol::Chat,
            (Api::Openai, "/v1/responses") => Protocol::Responses,
            (Api::Anthropic, "/v1/messages") => Protocol::Messages,
            _ => return Err(Failure::new(404, "llm", "route is not allowed")),
        };
        let mut headers = forwarded_headers(route.api, &request)?;
        let mut payload: Value = serde_json::from_slice(&request.body)
            .map_err(|_| Failure::new(400, "llm", "request body is not valid JSON"))?;
        let object = payload
            .as_object_mut()
            .ok_or_else(|| Failure::new(400, "llm", "request body must be a JSON object"))?;
        require_local_tools(object)?;
        let streaming = match object.get("stream") {
            None => false,
            Some(Value::Bool(value)) => *value,
            Some(_) => return Err(Failure::new(400, "llm", "stream must be a boolean")),
        };
        if streaming
            && object
                .get("n")
                .is_some_and(|value| value.as_u64() != Some(1))
        {
            return Err(Failure::new(
                400,
                "llm",
                "streaming requires one completion choice",
            ));
        }
        if streaming
            && object
                .get("background")
                .is_some_and(|value| value != &Value::Bool(false))
        {
            return Err(Failure::new(
                400,
                "llm",
                "background streaming is not allowed",
            ));
        }
        let model_name = object
            .get("model")
            .and_then(Value::as_str)
            .ok_or_else(|| Failure::new(400, "llm", "model is required"))?
            .to_string();
        let model = route
            .models
            .iter()
            .find(|candidate| candidate.name == model_name)
            .ok_or_else(|| Failure::new(403, "llm", "model is not allowed"))?;
        let token_field = match protocol {
            Protocol::Responses => "max_output_tokens",
            Protocol::Messages => "max_tokens",
            Protocol::Chat if object.contains_key("max_completion_tokens") => {
                if object.contains_key("max_tokens") {
                    return Err(Failure::new(
                        400,
                        "llm",
                        "use only one completion-token limit",
                    ));
                }
                "max_completion_tokens"
            }
            Protocol::Chat => "max_tokens",
        };
        let completion_tokens = match object.get(token_field) {
            Some(value) => value
                .as_u64()
                .ok_or_else(|| {
                    Failure::new(400, "llm", "completion-token limit must be an integer")
                })?
                .min(route.max_completion_tokens),
            None => route.max_completion_tokens,
        };
        if completion_tokens == 0 {
            return Err(Failure::new(
                400,
                "llm",
                "completion-token limit must be positive",
            ));
        }
        object.insert(token_field.into(), Value::from(completion_tokens));
        let input_tokens = conservative_input_tokens(request.body.len());
        let estimated_tokens = input_tokens.saturating_add(completion_tokens);
        let estimated_cost = estimate_cost(model, input_tokens, completion_tokens);

        let body = serde_json::to_vec(&payload)
            .map_err(|_| Failure::new(400, "llm", "request body cannot be encoded"))?;
        let upstream_url = reqwest::Url::parse(&route.upstream_base_url)
            .and_then(|base| base.join(path.strip_prefix("/v1/").expect("allowed path")))
            .map_err(|_| Failure::new(502, "llm", "configured upstream route is invalid"))?;
        let host = upstream_url
            .host_str()
            .ok_or_else(|| Failure::new(502, "llm", "configured upstream has no host"))?
            .to_ascii_lowercase();
        let test_mode = self.config.allow_plain_http_for_tests;
        let resolved = resolve_public_target(upstream_url.as_str(), &[host], test_mode, test_mode)
            .map_err(|_| {
                Failure::new(502, "llm", "upstream address policy rejected the provider")
            })?;
        let credential = read_secret(&route.credential_file)
            .map_err(|_| Failure::new(503, "llm", "provider credential is unavailable"))?;
        headers.push(match route.api {
            Api::Openai => ("authorization", format!("Bearer {credential}")),
            Api::Anthropic => ("x-api-key", credential),
        });
        self.limits
            .reserve(route, estimated_tokens, estimated_cost)
            .map_err(|reason| Failure::new(429, "llm", reason))?;
        self.audit
            .record(
                &route.id,
                "llm",
                "authorized",
                json!({
                    "provider": resolved.host,
                    "path": path,
                    "model": model_name,
                    "reserved_tokens": estimated_tokens,
                    "reserved_cost_microusd": estimated_cost,
                }),
            )
            .map_err(|_| Failure::new(503, "audit", "audit log is unavailable"))?;
        if streaming {
            return Ok(Response {
                status: 200,
                content_type: "text/event-stream".into(),
                body: Vec::new(),
                streaming: Some(Box::new(StreamingResponse {
                    target: resolved,
                    headers,
                    body,
                    policy: self.config.streaming.clone(),
                    connect_timeout_seconds: self.config.connect_timeout_seconds,
                    max_response_bytes: self.config.max_response_bytes,
                    audit: Arc::clone(&self.audit),
                    route: route.id.clone(),
                    path: path.to_string(),
                    protocol,
                    model: model_name,
                })),
            });
        }
        let client = self.client_for(&resolved)?;
        let started = Instant::now();
        let mut upstream = client
            .post(resolved.url.clone())
            .header(CONTENT_TYPE, "application/json")
            .body(body);
        for (name, value) in headers {
            upstream = upstream.header(name, value);
        }
        let response = upstream
            .send()
            .map_err(|_| Failure::new(502, "llm", "provider request failed"))?;
        let status = response.status().as_u16();
        let content_type = response_content_type(&response);
        let response_body = read_bounded(response, self.config.max_response_bytes)?;
        let usage = serde_json::from_slice::<Value>(&response_body)
            .ok()
            .and_then(|value| value.get("usage").cloned())
            .map(|value| {
                json!({
                    "input_tokens": value.get("input_tokens")
                        .or_else(|| value.get("prompt_tokens"))
                        .and_then(Value::as_u64),
                    "output_tokens": value.get("output_tokens")
                        .or_else(|| value.get("completion_tokens"))
                        .and_then(Value::as_u64),
                    "total_tokens": value.get("total_tokens")
                        .and_then(Value::as_u64),
                })
            })
            .unwrap_or(Value::Null);
        self.audit
            .record(
                &route.id,
                "llm",
                "completed",
                json!({
                    "path": path,
                    "provider": resolved.host,
                    "model": model_name,
                    "status": status,
                    "latency_ms": started.elapsed().as_millis(),
                    "usage": usage,
                }),
            )
            .map_err(|_| Failure::new(503, "audit", "audit log is unavailable"))?;
        Ok(Response {
            status,
            content_type,
            body: response_body,
            streaming: None,
        })
    }

    fn client_for(&self, target: &ResolvedTarget) -> Result<Client, Failure> {
        Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .no_proxy()
            .connect_timeout(Duration::from_secs(self.config.connect_timeout_seconds))
            .timeout(Duration::from_secs(self.config.timeout_seconds))
            .resolve_to_addrs(&target.host, &target.addresses)
            .build()
            .map_err(|_| Failure::new(503, "upstream", "cannot construct upstream client"))
    }
}

fn route_without_query(path: &str) -> &str {
    path.split('?').next().unwrap_or(path)
}

fn estimate_cost(model: &ModelPolicy, input_tokens: u64, output_tokens: u64) -> u64 {
    input_tokens
        .saturating_mul(model.input_microusd_per_million)
        .saturating_add(output_tokens.saturating_mul(model.output_microusd_per_million))
        .div_ceil(1_000_000)
}

// Client tools are typed `function`/`custom` (OpenAI) or untyped (Anthropic);
// provider-run tools and request extensions would act outside the environment.
fn require_local_tools(payload: &serde_json::Map<String, Value>) -> Result<(), Failure> {
    let local_tool = |tool: &Value| {
        matches!(
            tool.get("type").map(Value::as_str),
            None | Some(Some("function" | "custom"))
        )
    };
    let local_tools = |tools: &Value| {
        tools
            .as_array()
            .is_some_and(|tools| tools.iter().all(local_tool))
    };
    let provider_extension = [
        "web_search_options",
        "plugins",
        "extensions",
        "data_sources",
        "mcp_servers",
        "container",
    ]
    .iter()
    .any(|field| payload.contains_key(*field));
    let tools_allowed = payload.get("tools").is_none_or(local_tools);
    let choice_allowed = payload
        .get("tool_choice")
        .is_none_or(|choice| match choice {
            Value::Null => true,
            Value::String(choice) => matches!(choice.as_str(), "auto" | "none" | "required"),
            Value::Object(_) => match choice.get("type").and_then(Value::as_str) {
                Some("function" | "custom" | "auto" | "any" | "none" | "tool") => true,
                Some("allowed_tools") => choice.get("tools").is_some_and(local_tools),
                _ => false,
            },
            _ => false,
        });
    if provider_extension || !tools_allowed || !choice_allowed {
        return Err(Failure::new(
            403,
            "llm",
            "provider tools and extensions are disabled",
        ));
    }
    Ok(())
}

/// Validated client headers worth forwarding: OpenCode's session id (cache
/// affinity) and, for Anthropic, the API version and beta flags.
fn forwarded_headers(api: Api, request: &Request) -> Result<Headers, Failure> {
    let mut headers = Headers::new();
    let mut forward = |name: &'static str, max: usize, allowed: fn(u8) -> bool| {
        let Some(value) = request.headers.get(name) else {
            return Ok(());
        };
        if value.is_empty() || value.len() > max || !value.bytes().all(allowed) {
            return Err(Failure::new(400, "llm", format!("invalid {name} header")));
        }
        headers.push((name, value.clone()));
        Ok(())
    };
    forward("x-opencode-session", 128, |byte| {
        byte.is_ascii_alphanumeric() || b"_.:-".contains(&byte)
    })?;
    if api == Api::Anthropic {
        forward("anthropic-version", 32, |byte| {
            byte.is_ascii_digit() || byte == b'-'
        })?;
        forward("anthropic-beta", 512, |byte| {
            byte.is_ascii_alphanumeric() || b"-,".contains(&byte)
        })?;
        if !headers.iter().any(|(name, _)| *name == "anthropic-version") {
            headers.push(("anthropic-version", "2023-06-01".into()));
        }
    }
    Ok(headers)
}

fn conservative_input_tokens(body_bytes: usize) -> u64 {
    u64::try_from(body_bytes).unwrap_or(u64::MAX)
}

fn response_content_type(response: &UpstreamResponse) -> String {
    response
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("application/octet-stream")
        .to_string()
}

fn read_bounded(mut response: UpstreamResponse, limit: usize) -> Result<Vec<u8>, Failure> {
    if response
        .content_length()
        .is_some_and(|length| length > u64::try_from(limit).unwrap_or(u64::MAX))
    {
        return Err(Failure::new(
            502,
            "upstream",
            "upstream body exceeds configured limit",
        ));
    }
    let mut body = Vec::new();
    response
        .by_ref()
        .take(u64::try_from(limit).unwrap_or(u64::MAX).saturating_add(1))
        .read_to_end(&mut body)
        .map_err(|_| Failure::new(502, "upstream", "cannot read upstream body"))?;
    if body.len() > limit {
        return Err(Failure::new(
            502,
            "upstream",
            "upstream body exceeds configured limit",
        ));
    }
    Ok(body)
}

struct Failure {
    status: u16,
    kind: &'static str,
    public: String,
}

impl Failure {
    fn new(status: u16, kind: &'static str, public: impl Into<String>) -> Self {
        Self {
            status,
            kind,
            public: public.into(),
        }
    }
}

pub type SharedBroker = Arc<Broker>;

#[cfg(test)]
mod tests;
