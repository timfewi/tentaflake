//! Explicit private-HTTPS client. Resource authority and wire schemas stay on
//! the server; transport failure never authorizes a mutation retry.
use super::MAX_BODY;
use super::config::id;
use super::credential::{self, runtime_path as credential_path};
use reqwest::blocking::{Client, ClientBuilder};
use reqwest::header::{HeaderMap, HeaderValue};
use reqwest::{Method, Url};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::fs::{File, OpenOptions};
use std::io::Read;
use std::os::fd::AsRawFd;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Component, Path, PathBuf};
use std::time::Duration;
use tentaflake_admission::protected_path::open_at;

const MAX_RESPONSE: usize = 1024 * 1024;
const CREDENTIAL_ERROR: &str = "cannot read protected current-user API bearer credential";
const USAGE: &str = "usage: tentaflake api --origin https://HOST [--bearer-file FILE] meta|session|capabilities|environments [--cursor C --limit N]|environment ID|plan FILE|create FILE --epoch E --key UUID|start|stop ID --epoch E --key UUID --revision R|operation ID|recover --epoch E --key UUID --method POST|PUT|PATCH|DELETE --target /api/v1/...";

struct Command {
    origin: Url,
    bearer: Option<PathBuf>,
    request: Request,
}

struct Request {
    method: Method,
    target: String,
    body_file: Option<PathBuf>,
    headers: HeaderMap,
    recovery: Option<Recovery>,
    response: ResponseKind,
}

enum ResponseKind {
    Object,
    List,
    Create,
    Lifecycle {
        kind: &'static str,
        environment: String,
    },
    Recovery {
        epoch: String,
    },
}

#[derive(Debug)]
struct Reply {
    value: Value,
    next_cursor: Option<String>,
}

struct Recovery {
    epoch: String,
    key: String,
    method: String,
    target: String,
}

impl Recovery {
    fn query(&self, url: &mut Url) {
        url.query_pairs_mut()
            .append_pair("idempotencyKey", &self.key)
            .append_pair("mutationEpoch", &self.epoch)
            .append_pair("method", &self.method)
            .append_pair("target", &self.target);
    }

    fn describe(&self) -> String {
        json!({"mutationEpoch": self.epoch, "idempotencyKey": self.key,
            "method": self.method, "target": self.target})
        .to_string()
    }
}

impl Command {
    fn parse(args: &[String]) -> Result<Self, String> {
        let mut options = BTreeMap::new();
        let mut positional = Vec::new();
        let mut index = 0;
        while index < args.len() {
            let arg = args[index].as_str();
            if arg.starts_with('-') {
                if ![
                    "--origin",
                    "--bearer-file",
                    "--cursor",
                    "--limit",
                    "--epoch",
                    "--key",
                    "--revision",
                    "--method",
                    "--target",
                ]
                .contains(&arg)
                {
                    return Err(USAGE.into());
                }
                let value = args
                    .get(index + 1)
                    .filter(|v| !v.starts_with('-'))
                    .ok_or(USAGE)?;
                if options.insert(arg, value.as_str()).is_some() {
                    return Err("duplicate API client option".into());
                }
                index += 2;
            } else {
                positional.push(arg);
                index += 1;
            }
        }
        let origin = origin(options.remove("--origin").ok_or(USAGE)?)?;
        let bearer = options.remove("--bearer-file").map(PathBuf::from);
        let mut request = Request {
            method: Method::GET,
            target: String::new(),
            body_file: None,
            headers: HeaderMap::new(),
            recovery: None,
            response: ResponseKind::Object,
        };
        match positional.as_slice() {
            ["meta"] => request.target = "/api/meta".into(),
            ["session"] => request.target = "/api/v1/session".into(),
            ["capabilities"] => request.target = "/api/v1/capabilities".into(),
            ["environments"] => {
                request.response = ResponseKind::List;
                let mut url = origin.join("/api/v1/environments").map_err(|_| USAGE)?;
                if let Some(cursor) = options.remove("--cursor") {
                    if cursor.is_empty()
                        || cursor.len() > 2048
                        || cursor.chars().any(char::is_control)
                    {
                        return Err("invalid API page cursor".into());
                    }
                    url.query_pairs_mut().append_pair("cursor", cursor);
                }
                if let Some(limit) = options.remove("--limit") {
                    let parsed = limit.parse::<u16>().map_err(|_| "invalid API page limit")?;
                    if !(1..=500).contains(&parsed) || parsed.to_string() != limit {
                        return Err("invalid API page limit".into());
                    }
                    url.query_pairs_mut().append_pair("limit", limit);
                }
                request.target = relative(&url);
            }
            ["environment", value] | ["operation", value] => {
                if !id(value) {
                    return Err("invalid API resource identifier".into());
                }
                let collection = if positional[0] == "environment" {
                    "environments"
                } else {
                    "operations"
                };
                request.target = format!("/api/v1/{collection}/{value}");
            }
            ["plan", file] | ["create", file] => {
                request.method = Method::POST;
                request.body_file = Some(PathBuf::from(file));
                request.target = if positional[0] == "plan" {
                    "/api/v1/plans"
                } else {
                    "/api/v1/environments"
                }
                .into();
                if positional[0] == "create" {
                    request.response = ResponseKind::Create;
                }
            }
            [action @ ("start" | "stop"), value] => {
                if !id(value) {
                    return Err("invalid API resource identifier".into());
                }
                let revision = options
                    .remove("--revision")
                    .filter(|v| id(v))
                    .ok_or("an exact unquoted --revision is required")?;
                request.headers.insert(
                    "if-match",
                    HeaderValue::from_str(&format!("\"{revision}\""))
                        .map_err(|_| "invalid revision")?,
                );
                request.method = Method::POST;
                request.target = format!("/api/v1/environments/{value}/{action}");
                request.response = ResponseKind::Lifecycle {
                    kind: if *action == "start" { "start" } else { "stop" },
                    environment: (*value).into(),
                };
            }
            ["recover"] => {
                let epoch = options
                    .remove("--epoch")
                    .filter(|v| id(v))
                    .ok_or("invalid or missing epoch")?;
                let key = options
                    .remove("--key")
                    .filter(|v| uuid(v))
                    .ok_or("invalid or missing UUID key")?;
                let method = options
                    .remove("--method")
                    .filter(|v| ["POST", "PUT", "PATCH", "DELETE"].contains(v))
                    .ok_or("invalid or missing recovery method")?;
                let target = options
                    .remove("--target")
                    .filter(|v| canonical_target(v))
                    .ok_or("invalid or missing canonical recovery target")?;
                let recovery = Recovery {
                    epoch: epoch.into(),
                    key: key.into(),
                    method: method.into(),
                    target: target.into(),
                };
                let mut url = origin.join("/api/v1/operations").map_err(|_| USAGE)?;
                recovery.query(&mut url);
                request.target = relative(&url);
                request.response = ResponseKind::Recovery {
                    epoch: epoch.into(),
                };
            }
            _ => return Err(USAGE.into()),
        }
        if matches!(
            request.response,
            ResponseKind::Create | ResponseKind::Lifecycle { .. }
        ) {
            let epoch = options
                .remove("--epoch")
                .filter(|v| id(v))
                .ok_or("invalid or missing epoch")?;
            let key = options
                .remove("--key")
                .filter(|v| uuid(v))
                .ok_or("invalid or missing UUID key")?;
            request.headers.insert(
                "idempotency-epoch",
                HeaderValue::from_str(epoch).map_err(|_| USAGE)?,
            );
            request.headers.insert(
                "idempotency-key",
                HeaderValue::from_str(key).map_err(|_| USAGE)?,
            );
            request.recovery = Some(Recovery {
                epoch: epoch.into(),
                key: key.into(),
                method: request.method.to_string(),
                target: request.target.clone(),
            });
        }
        if !options.is_empty() {
            return Err("API option is not valid for this command".into());
        }
        if request.target == "/api/meta" {
            if bearer.is_some() {
                return Err("meta does not accept a bearer credential".into());
            }
        } else if bearer.as_ref().is_none_or(|p| !credential_path(p)) {
            return Err("a normalized absolute runtime --bearer-file is required".into());
        }
        Ok(Self {
            origin,
            bearer,
            request,
        })
    }
}

fn origin(input: &str) -> Result<Url, String> {
    let url = Url::parse(input).map_err(|_| "invalid HTTPS API origin")?;
    if input.len() > 2048
        || !input.is_ascii()
        || url.scheme() != "https"
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.path() != "/"
        || url.port() == Some(0)
        || url.origin().ascii_serialization() != input
    {
        return Err(
            "API origin must be a canonical HTTPS origin without path or credentials".into(),
        );
    }
    Ok(url)
}

fn relative(url: &Url) -> String {
    match url.query() {
        Some(query) => format!("{}?{query}", url.path()),
        None => url.path().into(),
    }
}

fn uuid(value: &str) -> bool {
    value.len() == 36
        && value.bytes().enumerate().all(|(i, b)| {
            if [8, 13, 18, 23].contains(&i) {
                b == b'-'
            } else {
                b.is_ascii_hexdigit()
            }
        })
}

fn canonical_target(value: &str) -> bool {
    if !value.starts_with("/api/v1/")
        || value.len() > 4096
        || !value.bytes().all(|b| b.is_ascii_graphic())
        || value.contains(['#', '\\'])
    {
        return false;
    }
    // Do not reinterpret the semantic query supplied for recovery. URL parsing
    // must preserve the caller's exact canonical target, including escapes.
    let Ok(url) = Url::parse(&format!("https://recovery.invalid{value}")) else {
        return false;
    };
    relative(&url) == value && !url.path().split('/').skip(1).any(|part| part.is_empty())
}

fn client_builder() -> ClientBuilder {
    Client::builder()
        .https_only(true)
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .retry(reqwest::retry::never())
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(15))
        .pool_max_idle_per_host(0)
}

pub(super) fn run(args: &[String], mode: crate::OutputMode) -> Result<u8, String> {
    if mode.hide {
        return Err("API client outputs server JSON; --hide is not supported".into());
    }
    let command = Command::parse(args)?;
    let body = command
        .request
        .body_file
        .as_ref()
        .map(|path| read_body(path))
        .transpose()?;
    let secret = command
        .bearer
        .as_ref()
        .map(|path| read_credential(path))
        .transpose()?;
    let client = client_builder()
        .build()
        .map_err(|_| "cannot initialize HTTPS API client")?;
    let result = execute(
        &client,
        &command.origin,
        &command.request,
        body,
        secret.as_deref(),
    );
    let reply = result.map_err(|message| match &command.request.recovery {
        Some(scope) => format!("{message}; mutation outcome is unconfirmed; use api recover with {}; never choose a new key to bypass uncertainty", scope.describe()),
        None => message,
    })?;
    println!(
        "{}",
        serde_json::to_string_pretty(&reply.value).map_err(|_| "cannot encode API response")?
    );
    if let Some(cursor) = reply.next_cursor {
        // Keep the contract's bare array on stdout. JSON quoting prevents an
        // opaque remote cursor from injecting terminal control sequences.
        eprintln!(
            "Next-Cursor: {}",
            serde_json::to_string(&cursor).map_err(|_| "cannot encode API page cursor")?
        );
    }
    Ok(0)
}

fn read_body(path: &Path) -> Result<Vec<u8>, String> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
        .open(path)
        .map_err(|_| "cannot read API JSON body file")?;
    let metadata = file
        .metadata()
        .map_err(|_| "cannot inspect API body file")?;
    if !metadata.is_file() || metadata.len() > MAX_BODY as u64 {
        return Err("API JSON body must be a regular file of at most 32 KiB".into());
    }
    let mut bytes = Vec::new();
    file.take(MAX_BODY as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "cannot read API body file")?;
    if bytes.len() > MAX_BODY
        || !serde_json::from_slice::<Value>(&bytes).is_ok_and(|v| v.is_object())
    {
        return Err("API body must be one bounded JSON object".into());
    }
    // Do not re-encode: exact accepted bytes belong to the idempotency digest.
    Ok(bytes)
}

fn execute(
    client: &Client,
    origin: &Url,
    request: &Request,
    body: Option<Vec<u8>>,
    secret: Option<&[u8]>,
) -> Result<Reply, String> {
    let url = origin
        .join(&request.target)
        .map_err(|_| "invalid API request target")?;
    let mut builder = client
        .request(request.method.clone(), url)
        .headers(request.headers.clone())
        .header("accept", "application/json");
    if let Some(secret) = secret {
        let mut value = b"Bearer ".to_vec();
        value.extend_from_slice(secret);
        let mut value = HeaderValue::from_bytes(&value).map_err(|_| CREDENTIAL_ERROR)?;
        value.set_sensitive(true);
        builder = builder.header("authorization", value);
    }
    if let Some(body) = body {
        builder = builder
            .header("content-type", "application/json")
            .body(body);
    }
    let mut response = builder
        .send()
        .map_err(|_| "API transport failed; no request was retried")?;
    let status = response.status().as_u16();
    let headers = response.headers().clone();
    if single_header(&headers, "content-type")? != "application/json"
        || single_header(&headers, "cache-control")? != "no-store"
        || response
            .content_length()
            .is_some_and(|len| len > MAX_RESPONSE as u64)
    {
        return Err("API response has invalid metadata or exceeds the size bound".into());
    }
    let mut bytes = Vec::new();
    (&mut response)
        .take(MAX_RESPONSE as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "API response was incomplete or exceeded its deadline")?;
    if bytes.len() > MAX_RESPONSE {
        return Err("API response exceeds 1 MiB".into());
    }
    let value: Value =
        serde_json::from_slice(&bytes).map_err(|_| "API response is not valid JSON")?;
    if secret.is_some_and(|s| contains_secret(&value, s)) {
        return Err("API response is invalid or contains credential material".into());
    }
    let expected = if request.recovery.is_some() { 202 } else { 200 };
    if status != expected {
        // No remote message, detail, redirect target or reqwest diagnostic is
        // printed: any of those can contain private server data or credentials.
        let code = value
            .get("error")
            .and_then(|v| v.get("code"))
            .and_then(Value::as_str)
            .filter(|code| {
                matches!(
                    *code,
                    "unauthenticated"
                        | "session_expired"
                        | "session_revoked"
                        | "csrf_failed"
                        | "forbidden"
                        | "not_found"
                        | "already_exists"
                        | "revision_conflict"
                        | "precondition_required"
                        | "read_only"
                        | "quota_exceeded"
                        | "too_large"
                        | "invalid"
                        | "plan_stale"
                        | "environment_running"
                        | "operation_pending"
                        | "unsupported"
                        | "unready"
                        | "cursor_expired"
                        | "idempotency_conflict"
                        | "idempotency_expired"
                        | "outcome_unknown"
                        | "rate_limited"
                        | "unavailable"
                        | "internal"
                )
            })
            .unwrap_or("invalid_response");
        return Err(format!("API request refused (HTTP {status}, {code})"));
    }
    validate_response(request, &headers, &value)?;
    let next_cursor =
        if matches!(request.response, ResponseKind::List) && headers.contains_key("next-cursor") {
            let cursor = single_header(&headers, "next-cursor")?;
            if cursor.is_empty()
                || cursor.len() > 2048
                || cursor.chars().any(char::is_control)
                || secret.is_some_and(|s| cursor.as_bytes().windows(s.len()).any(|part| part == s))
            {
                return Err("API response contains an invalid page cursor".into());
            }
            Some(cursor.into())
        } else {
            None
        };
    Ok(Reply { value, next_cursor })
}

fn single_header<'a>(headers: &'a HeaderMap, name: &str) -> Result<&'a str, String> {
    let mut values = headers.get_all(name).iter();
    let value = values
        .next()
        .and_then(|v| v.to_str().ok())
        .ok_or("API response is missing a valid required header")?;
    if values.next().is_some() {
        return Err("API response repeats a singleton header".into());
    }
    Ok(value)
}

fn validate_response(request: &Request, headers: &HeaderMap, value: &Value) -> Result<(), String> {
    let invalid = || "API response contradicts its operation or recovery identity".to_owned();
    if !matches!(request.response, ResponseKind::List) && !value.is_object() {
        return Err("API response must be a JSON object".into());
    }
    match &request.response {
        ResponseKind::Object => {}
        ResponseKind::List => {
            if !value
                .as_array()
                .is_some_and(|items| items.len() <= 500 && items.iter().all(Value::is_object))
            {
                return Err("API environment list must be a bounded JSON array".into());
            }
        }
        ResponseKind::Create | ResponseKind::Lifecycle { .. } => {
            let operation = if matches!(request.response, ResponseKind::Create) {
                &value["operation"]
            } else {
                value
            };
            let operation_id = single_header(headers, "operation-id")?;
            if !id(operation_id)
                || operation["id"].as_str() != Some(operation_id)
                || !operation["environmentId"].as_str().is_some_and(id)
                || !operation["state"].as_str().is_some_and(|s| {
                    [
                        "queued",
                        "running",
                        "succeeded",
                        "failed",
                        "unknown",
                        "interrupted",
                    ]
                    .contains(&s)
                })
            {
                return Err(invalid());
            }
            match &request.response {
                ResponseKind::Create => {
                    if operation["kind"] != "create"
                        || operation["environmentId"] != value["environment"]["id"]
                    {
                        return Err(invalid());
                    }
                }
                ResponseKind::Lifecycle { kind, environment } => {
                    if operation["kind"].as_str() != Some(kind)
                        || operation["environmentId"].as_str() != Some(environment)
                    {
                        return Err(invalid());
                    }
                    let etag = single_header(headers, "etag")?;
                    if etag.len() < 3
                        || !etag.starts_with('"')
                        || !etag.ends_with('"')
                        || !id(&etag[1..etag.len() - 1])
                    {
                        return Err(invalid());
                    }
                }
                _ => unreachable!(),
            }
        }
        ResponseKind::Recovery { epoch } => {
            let fields: Vec<_> = value
                .as_object()
                .ok_or_else(invalid)?
                .keys()
                .map(String::as_str)
                .collect();
            let valid = match value["state"].as_str() {
                Some("absent") => {
                    fields == ["mutationEpoch", "operation", "retrySafe", "state"]
                        && value["operation"].is_null()
                        && value["retrySafe"] == true
                        && value["mutationEpoch"].as_str() == Some(epoch)
                }
                Some("unknown") => {
                    fields == ["operation", "reason", "retrySafe", "state"]
                        && value["operation"].is_null()
                        && value["retrySafe"] == false
                        && value["reason"].as_str().is_some_and(|v| v.len() <= 2048)
                }
                Some("found") => {
                    fields == ["operation", "retrySafe", "state"]
                        && value["retrySafe"] == false
                        && value["operation"]["id"].as_str().is_some_and(id)
                }
                _ => false,
            };
            if !valid {
                return Err(invalid());
            }
        }
    }
    Ok(())
}

fn contains_secret(value: &Value, secret: &[u8]) -> bool {
    let Ok(secret) = std::str::from_utf8(secret) else {
        return true;
    };
    match value {
        Value::String(value) => value.contains(secret),
        Value::Array(values) => values.iter().any(|v| contains_secret(v, secret.as_bytes())),
        Value::Object(values) => values
            .iter()
            .any(|(k, v)| k.contains(secret) || contains_secret(v, secret.as_bytes())),
        _ => false,
    }
}

fn protected_parent(anchor: &File, relative: &Path, owner: u32) -> Result<File, String> {
    let mut directory = anchor.try_clone().map_err(|_| CREDENTIAL_ERROR)?;
    check_directory(&directory, owner)?;
    for part in relative.components() {
        let Component::Normal(name) = part else {
            return Err(CREDENTIAL_ERROR.into());
        };
        directory = open_at(
            directory.as_raw_fd(),
            name,
            libc::O_RDONLY | libc::O_DIRECTORY,
        )
        .map_err(|_| CREDENTIAL_ERROR)?;
        check_directory(&directory, owner)?;
    }
    Ok(directory)
}

fn check_directory(file: &File, owner: u32) -> Result<(), String> {
    let metadata = file.metadata().map_err(|_| CREDENTIAL_ERROR)?;
    if !metadata.is_dir() || ![0, owner].contains(&metadata.uid()) || metadata.mode() & 0o022 != 0 {
        return Err(CREDENTIAL_ERROR.into());
    }
    Ok(())
}

fn read_credential(path: &Path) -> Result<Vec<u8>, String> {
    if !credential_path(path) {
        return Err(CREDENTIAL_ERROR.into());
    }
    let root = open_at(
        libc::AT_FDCWD,
        OsStr::new("/"),
        libc::O_RDONLY | libc::O_DIRECTORY,
    )
    .map_err(|_| CREDENTIAL_ERROR)?;
    read_credential_at(
        &root,
        path.strip_prefix("/").map_err(|_| CREDENTIAL_ERROR)?,
        tentaflake_admission::effective_uid(),
    )
}

fn read_credential_at(anchor: &File, path: &Path, owner: u32) -> Result<Vec<u8>, String> {
    let parent = path.parent().ok_or(CREDENTIAL_ERROR)?;
    let name = path.file_name().ok_or(CREDENTIAL_ERROR)?;
    let directory = protected_parent(anchor, parent, owner)?;
    credential::read_at(&directory, name, owner, || {
        protected_parent(anchor, parent, owner).map_err(|_| credential::Error)
    })
    .map_err(|_| CREDENTIAL_ERROR.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::io::Write;
    use std::net::TcpListener;
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt, symlink};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::thread;
    use std::time::Instant;

    // These public, synthetic test keys authenticate only our temporary local
    // listener. The production client never installs this CA or skips TLS checks.
    const CA: &[u8] = include_bytes!("../../tests/fixtures/api-client/synthetic-ca.der");
    const CERT: &[u8] = include_bytes!("../../tests/fixtures/api-client/synthetic-localhost.der");
    const KEY: &[u8] =
        include_bytes!("../../tests/fixtures/api-client/synthetic-localhost-key.der");
    const TOKEN: &[u8] = b"synthetic-independent-cli-bearer-0123456789";
    const UUID: &str = "12345678-1234-4234-8234-123456789abc";

    fn parse(command: &[&str]) -> Result<Command, String> {
        let mut args = vec![
            "--origin",
            "https://api.example.invalid",
            "--bearer-file",
            "/run/user/1000/api-bearer",
        ];
        args.extend_from_slice(command);
        Command::parse(&args.into_iter().map(String::from).collect::<Vec<_>>())
    }

    #[test]
    fn commands_preserve_exact_mutation_scope_and_strong_revision() {
        let start = parse(&[
            "start",
            "env-1",
            "--epoch",
            "epoch-1",
            "--key",
            UUID,
            "--revision",
            "rev-1",
        ])
        .unwrap();
        assert_eq!(start.request.headers["if-match"], "\"rev-1\"");
        assert_eq!(start.request.headers["idempotency-key"], UUID);
        let scope = start.request.recovery.as_ref().unwrap();
        assert_eq!(scope.target, "/api/v1/environments/env-1/start");
        let recovery = parse(&[
            "recover",
            "--epoch",
            "epoch-1",
            "--key",
            UUID,
            "--method",
            "POST",
            "--target",
            &scope.target,
        ])
        .unwrap();
        let url = recovery.origin.join(&recovery.request.target).unwrap();
        let fields: BTreeMap<_, _> = url.query_pairs().into_owned().collect();
        assert_eq!(fields["target"], scope.target);
        assert_eq!(fields["idempotencyKey"], UUID);
        assert!(recovery.request.recovery.is_none());
        assert_eq!(recovery.request.method, Method::GET);
        for command in [
            vec!["session"],
            vec!["capabilities"],
            vec!["environments"],
            vec!["environment", "env-1"],
            vec!["operation", "op-1"],
            vec!["plan", "body.json"],
            vec!["create", "body.json", "--epoch", "epoch-1", "--key", UUID],
            vec![
                "stop",
                "env-1",
                "--epoch",
                "epoch-1",
                "--key",
                UUID,
                "--revision",
                "rev-1",
            ],
        ] {
            assert!(parse(&command).is_ok());
        }
    }

    #[test]
    fn parser_refuses_ambiguous_authority_arguments_and_mutations() {
        for url in [
            "http://api.example.invalid",
            "https://user:secret@api.example.invalid",
            "https://api.example.invalid/",
            "https://api.example.invalid/path",
            "https://api.example.invalid?x=1",
            "https://api.example.invalid#x",
            "https://api.example.invalid:0",
            "https://API.example.invalid",
            "https://api.example.invalid:443",
        ] {
            assert!(origin(url).is_err());
        }
        assert!(origin("https://[::1]:8443").is_ok());
        for command in [
            vec!["session", "--key", UUID],
            vec!["session", "--token", "never-accepted"],
            vec!["create", "body.json"],
            vec![
                "start",
                "../bad",
                "--epoch",
                "epoch",
                "--key",
                UUID,
                "--revision",
                "rev",
            ],
            vec![
                "start",
                "env",
                "--epoch",
                "epoch",
                "--key",
                UUID,
                "--revision",
                "\"rev\"",
            ],
            vec![
                "stop",
                "env",
                "--epoch",
                "epoch",
                "--key",
                "bad",
                "--revision",
                "rev",
            ],
            vec!["session", "--origin", "https://other.invalid"],
            vec!["meta"],
            vec!["environments", "--limit", "501"],
            vec!["environments", "--limit", "001"],
            vec!["environments", "--cursor", "x", "--cursor", "y"],
        ] {
            assert!(parse(&command).is_err());
        }
        for target in [
            "https://other.invalid/api/v1/x",
            "//other.invalid/api/v1/x",
            "/api/v1/x/../y",
            "/api/v1/x#part",
            "/api/v1//x",
        ] {
            assert!(!canonical_target(target));
        }
        let list = parse(&["environments", "--cursor", "a&b=c", "--limit", "25"]).unwrap();
        let url = list.origin.join(&list.request.target).unwrap();
        assert_eq!(url.query_pairs().next().unwrap().1, "a&b=c");
    }

    struct Fixture {
        path: PathBuf,
        anchor: File,
    }
    impl Fixture {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!(
                "tentaflake-api-client-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::DirBuilder::new().mode(0o700).create(&path).unwrap();
            let anchor = File::open(&path).unwrap();
            Self { path, anchor }
        }
        fn write(&self, name: &str, bytes: &[u8]) {
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(self.path.join(name))
                .unwrap();
            file.write_all(bytes).unwrap();
        }
        fn read(&self, name: &str) -> Result<Vec<u8>, String> {
            read_credential_at(
                &self.anchor,
                Path::new(name),
                tentaflake_admission::effective_uid(),
            )
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.path).unwrap();
        }
    }

    #[test]
    fn credentials_require_private_owned_nofollow_files_and_ancestors() {
        let fixture = Fixture::new();
        fixture.write("token", TOKEN);
        assert_eq!(fixture.read("token").unwrap(), TOKEN);
        assert!(
            read_credential_at(
                &fixture.anchor,
                Path::new("token"),
                tentaflake_admission::effective_uid() + 1
            )
            .is_err()
        );
        for mode in [0o644, 0o640, 0o660, 0o700] {
            fs::set_permissions(fixture.path.join("token"), fs::Permissions::from_mode(mode))
                .unwrap();
            assert!(fixture.read("token").is_err());
        }
        fs::set_permissions(
            fixture.path.join("token"),
            fs::Permissions::from_mode(0o400),
        )
        .unwrap();
        symlink("token", fixture.path.join("alias")).unwrap();
        assert!(fixture.read("alias").is_err());
        fs::hard_link(fixture.path.join("token"), fixture.path.join("linked")).unwrap();
        assert!(fixture.read("token").is_err());
        fs::remove_file(fixture.path.join("linked")).unwrap();
        fs::DirBuilder::new()
            .mode(0o700)
            .create(fixture.path.join("private"))
            .unwrap();
        fixture.write("private/token", TOKEN);
        assert_eq!(fixture.read("private/token").unwrap(), TOKEN);
        symlink("private", fixture.path.join("directory-alias")).unwrap();
        assert!(fixture.read("directory-alias/token").is_err());
        fs::set_permissions(
            fixture.path.join("private"),
            fs::Permissions::from_mode(0o770),
        )
        .unwrap();
        assert!(fixture.read("private/token").is_err());
        assert!(fixture.read("private").is_err());
        assert!(fixture.read("absent").is_err());
        for path in [
            "relative",
            "/nix/store/hash-token",
            "/run//token",
            "/run/../token",
            "/run/token/",
        ] {
            assert!(!credential_path(Path::new(path)));
        }
    }

    #[test]
    fn credential_and_body_bounds_preserve_exact_request_bytes() {
        let fixture = Fixture::new();
        for (name, bytes) in [
            ("short", vec![b'a'; 31]),
            ("large", vec![b'a'; 4097]),
            ("whitespace", vec![b' '; 32]),
        ] {
            fixture.write(name, &bytes);
            assert!(fixture.read(name).is_err());
        }
        let mut newline = TOKEN.to_vec();
        newline.push(b'\n');
        fixture.write("newline", &newline);
        assert_eq!(fixture.read("newline").unwrap(), TOKEN);
        let body = b"{ \n  \"planId\": \"plan-1\", \"startAfterCreate\": false }\n";
        fixture.write("body", body);
        assert_eq!(read_body(&fixture.path.join("body")).unwrap(), body);
        fixture.write("bad-body", b"[]");
        fixture.write("large-body", &vec![b' '; MAX_BODY + 1]);
        assert!(read_body(&fixture.path.join("bad-body")).is_err());
        assert!(read_body(&fixture.path.join("large-body")).is_err());
        symlink("body", fixture.path.join("body-alias")).unwrap();
        assert!(read_body(&fixture.path.join("body-alias")).is_err());
    }

    fn tls_fixture(response: Option<Vec<u8>>) -> (Url, thread::JoinHandle<Vec<Vec<u8>>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let origin = Url::parse(&format!("https://{}", listener.local_addr().unwrap())).unwrap();
        listener.set_nonblocking(true).unwrap();
        let config = rustls::ServerConfig::builder_with_provider(Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(
            vec![rustls::pki_types::CertificateDer::from(CERT.to_vec())],
            rustls::pki_types::PrivatePkcs8KeyDer::from(KEY.to_vec()).into(),
        )
        .unwrap();
        let handle = thread::spawn(move || {
            let mut requests = Vec::new();
            let mut deadline = Instant::now() + Duration::from_secs(3);
            while Instant::now() < deadline {
                let (socket, _) = match listener.accept() {
                    Ok(pair) => pair,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(2));
                        continue;
                    }
                    Err(_) => break,
                };
                socket
                    .set_read_timeout(Some(Duration::from_secs(2)))
                    .unwrap();
                socket
                    .set_write_timeout(Some(Duration::from_secs(2)))
                    .unwrap();
                let mut stream = rustls::StreamOwned::new(
                    rustls::ServerConnection::new(Arc::new(config.clone())).unwrap(),
                    socket,
                );
                let mut bytes = Vec::new();
                let mut block = [0; 2048];
                while bytes.len() <= MAX_BODY + 32768 {
                    match stream.read(&mut block) {
                        Ok(0) | Err(_) => break,
                        Ok(count) => bytes.extend_from_slice(&block[..count]),
                    }
                    if let Some(end) = bytes.windows(4).position(|v| v == b"\r\n\r\n") {
                        let header = std::str::from_utf8(&bytes[..end]).unwrap();
                        let length = header
                            .lines()
                            .find_map(|line| line.strip_prefix("content-length: "))
                            .map(|v| v.parse::<usize>().unwrap())
                            .unwrap_or(0);
                        if bytes.len() >= end + 4 + length {
                            break;
                        }
                    }
                }
                if !bytes.is_empty() {
                    requests.push(bytes);
                }
                if let Some(response) = &response {
                    let _ = stream.write_all(response);
                    let _ = stream.flush();
                }
                drop(stream);
                deadline = Instant::now() + Duration::from_millis(150);
            }
            requests
        });
        (origin, handle)
    }

    fn trusted_client() -> Client {
        client_builder()
            .add_root_certificate(reqwest::Certificate::from_der(CA).unwrap())
            .build()
            .unwrap()
    }

    fn reply(status: u16, extra: &str, value: Value) -> Vec<u8> {
        let body = serde_json::to_vec(&value).unwrap();
        let mut bytes = format!("HTTP/1.1 {status} Fixture\r\nContent-Type: application/json\r\nCache-Control: no-store\r\nContent-Length: {}\r\nConnection: close\r\n{extra}\r\n", body.len()).into_bytes();
        bytes.extend_from_slice(&body);
        bytes
    }

    #[test]
    fn real_https_requires_trust_and_sends_bearer_only_in_header() {
        let command = parse(&["session"]).unwrap();
        let (origin, handle) = tls_fixture(Some(reply(200, "", json!({"user":{"id":"fixture"}}))));
        assert!(
            execute(
                &client_builder().build().unwrap(),
                &origin,
                &command.request,
                None,
                Some(TOKEN)
            )
            .is_err()
        );
        assert!(handle.join().unwrap().is_empty());
        let (origin, handle) = tls_fixture(Some(reply(200, "", json!({"user":{"id":"fixture"}}))));
        assert!(
            execute(
                &trusted_client(),
                &origin,
                &command.request,
                None,
                Some(TOKEN)
            )
            .is_ok()
        );
        let requests = handle.join().unwrap();
        assert_eq!(requests.len(), 1);
        let wire = std::str::from_utf8(&requests[0]).unwrap();
        assert!(wire.starts_with("GET /api/v1/session HTTP/1.1\r\n"));
        assert!(wire.contains(&format!(
            "authorization: Bearer {}\r\n",
            std::str::from_utf8(TOKEN).unwrap()
        )));
        assert_eq!(wire.matches(std::str::from_utf8(TOKEN).unwrap()).count(), 1);
    }

    #[test]
    fn real_https_mutation_is_not_replayed_after_disconnect_redirect_or_refusal() {
        let command = parse(&["create", "body.json", "--epoch", "epoch-1", "--key", UUID]).unwrap();
        let body = b"{ \"planId\": \"p1\", \"startAfterCreate\": false }\n".to_vec();
        let responses = [
            None,
            Some(reply(
                307,
                "Location: https://other.invalid/never\r\n",
                json!({"error":{"code":"unavailable"}}),
            )),
            Some(reply(
                503,
                "",
                json!({"error":{"code":"unavailable","message":"private-server-message"}}),
            )),
        ];
        for response in responses {
            let (origin, handle) = tls_fixture(response);
            let error = execute(
                &trusted_client(),
                &origin,
                &command.request,
                Some(body.clone()),
                Some(TOKEN),
            )
            .unwrap_err();
            assert!(!error.contains("private-server-message"));
            assert!(!error.contains("other.invalid"));
            assert!(!error.contains(std::str::from_utf8(TOKEN).unwrap()));
            let requests = handle.join().unwrap();
            assert_eq!(requests.len(), 1);
            assert!(requests[0].ends_with(&body));
            let wire = std::str::from_utf8(&requests[0]).unwrap();
            assert!(wire.contains(&format!("idempotency-key: {UUID}\r\n")));
            assert!(wire.contains("idempotency-epoch: epoch-1\r\n"));
        }
    }

    #[test]
    fn response_bounds_and_reflected_credentials_never_become_output() {
        let command = parse(&["session"]).unwrap();
        let responses = [
            b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nCache-Control: no-store\r\nContent-Length: 2000000\r\n\r\n".to_vec(),
            b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nCache-Control: no-store\r\nContent-Length: 4\r\n\r\n{".to_vec(),
            reply(200, "", json!({"reflected":std::str::from_utf8(TOKEN).unwrap()})),
            reply(200, "Content-Type: application/json\r\n", json!({})),
            reply(500, "", json!({"error":{"code":"private-diagnostic", "message":"private-endpoint"}})),
        ];
        for response in responses {
            let (origin, handle) = tls_fixture(Some(response));
            let error = execute(
                &trusted_client(),
                &origin,
                &command.request,
                None,
                Some(TOKEN),
            )
            .unwrap_err();
            assert!(!error.contains("private-diagnostic"));
            assert!(!error.contains("private-endpoint"));
            assert!(!error.contains(std::str::from_utf8(TOKEN).unwrap()));
            assert_eq!(handle.join().unwrap().len(), 1);
        }
    }

    #[test]
    fn real_https_lists_contract_array_and_preserves_next_page_cursor() {
        let fixtures: Value =
            serde_json::from_str(include_str!("../../../../api/v1/fixtures.json")).unwrap();
        let body = fixtures["exchanges"]
            .as_array()
            .unwrap()
            .iter()
            .find(|exchange| exchange["name"] == "listEnvironments")
            .unwrap()["response"]["body"]
            .clone();
        let command = parse(&["environments", "--limit", "1"]).unwrap();
        let (origin, handle) = tls_fixture(Some(reply(
            200,
            "Next-Cursor: page&next=2\r\n",
            body.clone(),
        )));
        let result = execute(
            &trusted_client(),
            &origin,
            &command.request,
            None,
            Some(TOKEN),
        )
        .unwrap();
        assert_eq!(result.value, body);
        assert_eq!(result.next_cursor.as_deref(), Some("page&next=2"));
        assert_eq!(handle.join().unwrap().len(), 1);
        let command = parse(&[
            "environments",
            "--cursor",
            result.next_cursor.as_deref().unwrap(),
        ])
        .unwrap();
        let (origin, handle) = tls_fixture(Some(reply(200, "", json!([]))));
        let last = execute(
            &trusted_client(),
            &origin,
            &command.request,
            None,
            Some(TOKEN),
        )
        .unwrap();
        assert_eq!(last.value, json!([]));
        assert!(last.next_cursor.is_none());
        let requests = handle.join().unwrap();
        assert_eq!(requests.len(), 1);
        assert!(
            std::str::from_utf8(&requests[0])
                .unwrap()
                .starts_with("GET /api/v1/environments?cursor=page%26next%3D2 HTTP/1.1\r\n")
        );
    }

    #[test]
    fn cursor_headers_are_bounded_unique_and_cannot_echo_credentials() {
        let command = parse(&["environments"]).unwrap();
        for header in [
            "Next-Cursor: a\r\nNext-Cursor: b\r\n".to_owned(),
            "Next-Cursor: \r\n".to_owned(),
            format!("Next-Cursor: {}\r\n", "a".repeat(2049)),
            format!("Next-Cursor: {}\r\n", std::str::from_utf8(TOKEN).unwrap()),
            "Next-Cursor: a\tb\r\n".to_owned(),
        ] {
            let (origin, handle) = tls_fixture(Some(reply(200, &header, json!([]))));
            let error = execute(
                &trusted_client(),
                &origin,
                &command.request,
                None,
                Some(TOKEN),
            )
            .unwrap_err();
            assert!(!error.contains(std::str::from_utf8(TOKEN).unwrap()));
            assert_eq!(handle.join().unwrap().len(), 1);
        }
    }

    #[test]
    fn recovery_requires_authoritative_absence_in_the_exact_epoch() {
        let command = parse(&[
            "recover",
            "--epoch",
            "epoch-1",
            "--key",
            UUID,
            "--method",
            "POST",
            "--target",
            "/api/v1/environments",
        ])
        .unwrap();
        for valid in [
            json!({"state":"absent","operation":null,"retrySafe":true,"mutationEpoch":"epoch-1"}),
            json!({"state":"unknown","operation":null,"retrySafe":false,"reason":"record unavailable"}),
            json!({"state":"found","operation":{"id":"op-1"},"retrySafe":false}),
        ] {
            assert!(validate_response(&command.request, &HeaderMap::new(), &valid).is_ok());
        }
        for invalid in [
            json!({"state":"absent","operation":null,"retrySafe":true,"mutationEpoch":"old-epoch"}),
            json!({"state":"unknown","operation":null,"retrySafe":true,"reason":"record unavailable"}),
            json!({"state":"absent","retrySafe":true,"mutationEpoch":"epoch-1"}),
            json!({"state":"found","operation":{"id":"op-1"},"retrySafe":true}),
        ] {
            assert!(validate_response(&command.request, &HeaderMap::new(), &invalid).is_err());
        }
    }

    #[test]
    fn mutation_acceptance_requires_matching_operation_and_environment() {
        let command = parse(&[
            "start",
            "env-1",
            "--epoch",
            "epoch-1",
            "--key",
            UUID,
            "--revision",
            "rev-1",
        ])
        .unwrap();
        let mut headers = HeaderMap::new();
        headers.insert("operation-id", HeaderValue::from_static("op-1"));
        headers.insert("etag", HeaderValue::from_static("\"rev-2\""));
        let valid = json!({"id":"op-1","environmentId":"env-1","kind":"start","state":"queued"});
        assert!(validate_response(&command.request, &headers, &valid).is_ok());
        for (key, value) in [
            ("id", "different"),
            ("environmentId", "other"),
            ("kind", "stop"),
            ("state", "invented"),
        ] {
            let mut invalid = valid.clone();
            invalid[key] = value.into();
            assert!(validate_response(&command.request, &headers, &invalid).is_err());
        }
        headers.remove("operation-id");
        assert!(validate_response(&command.request, &headers, &valid).is_err());
    }
}
