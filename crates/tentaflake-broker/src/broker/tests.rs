use super::*;
use crate::http::read_request;
use crate::routes::sha256_hex;
use std::collections::HashMap;
use std::fs;
use std::io::Write;
use std::net::{SocketAddr, TcpListener};
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::thread::{self, JoinHandle};

/// A private directory with provider credentials, a routes file and broker state.
struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        static SEQUENCE: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "tentaflake-broker-{}-{}",
            std::process::id(),
            SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).unwrap();
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
        Self(root)
    }

    fn credential(&self, name: &str, value: &str) -> PathBuf {
        let path = self.0.join(name);
        fs::write(&path, value).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        path
    }

    /// Replaces the routes file the way the control service does: write, then rename.
    fn routes(&self, routes: Value) {
        let temporary = self.0.join("routes.tmp");
        fs::write(&temporary, json!({ "routes": routes }).to_string()).unwrap();
        fs::rename(temporary, self.0.join("routes")).unwrap();
    }

    fn route(&self, id: &str, api: &str, upstream: SocketAddr, extra: Value) -> Value {
        let mut route = json!({
            "id": id, "token_sha256": sha256_hex(&format!("token-{id}")), "api": api,
            "upstream_base_url": format!("http://{upstream}/{id}/"),
            "credential_file": self.credential(&format!("{id}.key"), &format!("provider-key-{id}")),
            "models": [{"name": format!("model-{id}")}], "max_completion_tokens": 16,
        });
        route
            .as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        route
    }

    fn broker(&self) -> Result<Broker, String> {
        Broker::new(
            serde_json::from_value(json!({
                "listen": "127.0.0.1:7810",
                "routes_file": self.0.join("routes"),
                "audit_file": self.0.join("audit"),
                "budget_state_file": self.0.join("budget"),
                "allow_plain_http_for_tests": true,
            }))
            .unwrap(),
        )
    }

    fn budget(&self) -> Value {
        fs::read(self.0.join("budget"))
            .map(|bytes| serde_json::from_slice(&bytes).unwrap())
            .unwrap_or(Value::Null)
    }

    /// The audit log, after checking it holds no prompt, token or provider key.
    fn audit(&self) -> String {
        let audit = fs::read_to_string(self.0.join("audit")).unwrap();
        for secret in ["SENSITIVE_PROMPT_FIXTURE", "token-", "provider-key-"] {
            assert!(!audit.contains(secret), "audit leaked {secret}");
        }
        audit
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// A provider that answers `count` requests and returns what it received.
fn provider(count: usize) -> (SocketAddr, JoinHandle<Vec<Request>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let handle = thread::spawn(move || {
        let received = (0..count).map(|_| answer(&listener, None)).collect();
        listener.set_nonblocking(true).unwrap();
        assert!(listener.accept().is_err(), "unexpected provider request");
        received
    });
    (address, handle)
}

/// Accepts one request and answers it, after `hold` reports and releases it.
fn answer(listener: &TcpListener, hold: Option<(mpsc::Sender<()>, mpsc::Receiver<()>)>) -> Request {
    let (mut stream, _) = listener.accept().unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let request = read_request(&mut stream, 32 * 1024, 1024 * 1024).unwrap();
    if let Some((received, release)) = hold {
        received.send(()).unwrap();
        release.recv_timeout(Duration::from_secs(5)).unwrap();
    }
    let body = br#"{"id":"fixture","usage":{"input_tokens":3,"output_tokens":2}}"#;
    write!(
        stream,
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    )
    .unwrap();
    stream.write_all(body).unwrap();
    request
}

fn call(broker: &Broker, path: &str, auth: (&str, &str), body: Value) -> Response {
    broker.handle(Request {
        method: "POST".into(),
        path: path.into(),
        headers: HashMap::from([
            (auth.0.into(), auth.1.into()),
            ("content-type".into(), "application/json".into()),
            (
                "anthropic-beta".into(),
                "interleaved-thinking-2025-05-14".into(),
            ),
            ("x-forged".into(), "not-forwarded".into()),
        ]),
        body: serde_json::to_vec(&body).unwrap(),
    })
}

fn chat(model: &str) -> Value {
    json!({"model": model, "max_tokens": 8, "messages": [{"role": "user", "content": "SENSITIVE_PROMPT_FIXTURE"}],
        "tools": [{"type": "function", "function": {"name": "local", "parameters": {"type": "object"}}}]})
}

fn messages(model: &str) -> Value {
    json!({"model": model, "max_tokens": 64000, "messages": [{"role": "user", "content": "SENSITIVE_PROMPT_FIXTURE"}],
        "tools": [{"name": "local", "input_schema": {"type": "object"}}], "tool_choice": {"type": "auto"}})
}

const BEARER_A: (&str, &str) = ("authorization", "Bearer token-a");
const API_KEY_B: (&str, &str) = ("x-api-key", "token-b");

#[test]
fn selects_the_route_by_token_and_sends_its_provider_credential() {
    let fixture = Fixture::new();
    let (upstream, provider) = provider(2);
    fixture.routes(json!([
        fixture.route("a", "openai", upstream, json!({})),
        fixture.route("b", "anthropic", upstream, json!({})),
    ]));
    let broker = fixture.broker().unwrap();

    let response = call(&broker, "/v1/chat/completions", BEARER_A, chat("model-a"));
    assert_eq!(response.status, 200);
    assert_eq!(
        call(&broker, "/v1/messages", API_KEY_B, messages("model-b")).status,
        200
    );
    // A route serves its own API and models only.
    assert_eq!(
        call(&broker, "/v1/chat/completions", BEARER_A, chat("model-b")).status,
        403
    );
    assert_eq!(
        call(&broker, "/v1/messages", BEARER_A, messages("model-a")).status,
        404
    );
    assert_eq!(
        call(&broker, "/v1/chat/completions", API_KEY_B, chat("model-b")).status,
        404
    );

    let received = provider.join().unwrap();
    let [openai, anthropic] = &received[..] else {
        panic!("expected two provider requests");
    };
    assert_eq!(openai.path, "/a/chat/completions");
    assert_eq!(openai.headers["authorization"], "Bearer provider-key-a");
    assert!(!openai.headers.contains_key("anthropic-beta"));
    assert_eq!(anthropic.path, "/b/messages");
    assert_eq!(anthropic.headers["x-api-key"], "provider-key-b");
    assert_eq!(anthropic.headers["anthropic-version"], "2023-06-01");
    assert_eq!(
        anthropic.headers["anthropic-beta"],
        "interleaved-thinking-2025-05-14"
    );
    assert!(!anthropic.headers.contains_key("authorization"));
    for request in &received {
        assert!(!request.headers.contains_key("x-forged"));
    }
    let body: Value = serde_json::from_slice(&anthropic.body).unwrap();
    assert_eq!(body["max_tokens"], 16, "clamped to the route limit");

    let budget = fixture.budget();
    assert_eq!(budget["a"]["requests"], 1);
    assert_eq!(budget["b"]["requests"], 1);
    let audit = fixture.audit();
    assert!(audit.contains("\"route\":\"a\"") && audit.contains("\"route\":\"b\""));
}

#[test]
fn unknown_or_missing_tokens_are_refused_before_any_dispatch() {
    let fixture = Fixture::new();
    let (upstream, provider) = provider(0);
    fixture.routes(json!([fixture.route("a", "openai", upstream, json!({}))]));
    let broker = fixture.broker().unwrap();
    for auth in [
        ("authorization", "Bearer token-unknown"),
        ("authorization", "Bearer "),
        ("authorization", "token-a"),
        ("x-forged", "token-a"),
    ] {
        let response = call(&broker, "/v1/chat/completions", auth, chat("model-a"));
        assert_eq!(response.status, 401, "{auth:?}");
    }
    provider.join().unwrap();
    assert_eq!(fixture.budget(), Value::Null);
    assert!(fixture.audit().contains("\"route\":\"-\""));
}

#[test]
fn an_exhausted_budget_refuses_only_its_own_route() {
    let fixture = Fixture::new();
    let (upstream, provider) = provider(2);
    let body = serde_json::to_vec(&chat("model-a")).unwrap();
    // Admission counts request bytes plus the completion limit (8 here).
    let one_request = body.len() as u64 + 8;
    fixture.routes(json!([
        fixture.route(
            "a",
            "openai",
            upstream,
            json!({"daily_token_budget": one_request})
        ),
        fixture.route("c", "openai", upstream, json!({})),
    ]));
    let broker = fixture.broker().unwrap();
    assert_eq!(
        call(&broker, "/v1/chat/completions", BEARER_A, chat("model-a")).status,
        200
    );
    let refused = call(&broker, "/v1/chat/completions", BEARER_A, chat("model-a"));
    assert_eq!(refused.status, 429);
    assert_eq!(
        serde_json::from_slice::<Value>(&refused.body).unwrap(),
        json!({"error": "daily budget exceeded"})
    );
    let other = ("authorization", "Bearer token-c");
    assert_eq!(
        call(&broker, "/v1/chat/completions", other, chat("model-c")).status,
        200
    );
    provider.join().unwrap();
    assert_eq!(fixture.budget()["a"]["tokens"], one_request);
}

#[test]
fn a_reload_keeps_the_request_in_flight_and_switches_new_ones() {
    let fixture = Fixture::new();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let upstream = listener.local_addr().unwrap();
    let (received_send, received) = mpsc::channel();
    let (release, release_receive) = mpsc::channel();
    let provider = thread::spawn(move || {
        let first = answer(&listener, Some((received_send, release_receive)));
        let second = answer(&listener, None);
        (first.path, second.path)
    });
    let route_a = fixture.route("a", "openai", upstream, json!({}));
    fixture.routes(json!([route_a]));
    let broker = Arc::new(fixture.broker().unwrap());
    let in_flight = {
        let broker = Arc::clone(&broker);
        thread::spawn(move || call(&broker, "/v1/chat/completions", BEARER_A, chat("model-a")))
    };
    received.recv_timeout(Duration::from_secs(5)).unwrap();

    // Route a is removed and route c added while a's request waits upstream.
    fixture.routes(json!([fixture.route("c", "openai", upstream, json!({}))]));
    assert_eq!(
        call(&broker, "/v1/chat/completions", BEARER_A, chat("model-a")).status,
        401
    );
    release.send(()).unwrap();
    assert_eq!(in_flight.join().unwrap().status, 200);
    let bearer_c = ("authorization", "Bearer token-c");
    assert_eq!(
        call(&broker, "/v1/chat/completions", bearer_c, chat("model-c")).status,
        200
    );
    assert_eq!(
        provider.join().unwrap(),
        ("/a/chat/completions".into(), "/c/chat/completions".into())
    );
}

#[test]
fn a_removed_credential_file_fails_closed_without_budget_or_dispatch() {
    let fixture = Fixture::new();
    let (upstream, provider) = provider(0);
    fixture.routes(json!([fixture.route("a", "openai", upstream, json!({}))]));
    let broker = fixture.broker().unwrap();
    fs::remove_file(fixture.0.join("a.key")).unwrap();
    let response = call(&broker, "/v1/chat/completions", BEARER_A, chat("model-a"));
    assert_eq!(response.status, 503);
    assert_eq!(
        serde_json::from_slice::<Value>(&response.body).unwrap(),
        json!({"error": "provider credential is unavailable"})
    );
    provider.join().unwrap();
    assert_eq!(fixture.budget(), Value::Null);
}

#[test]
fn invalid_or_writable_routes_files_fail_closed() {
    let fixture = Fixture::new();
    let (upstream, provider) = provider(0);
    // No file yet: the broker starts with no routes.
    let broker = fixture.broker().unwrap();
    assert_eq!(
        call(&broker, "/v1/chat/completions", BEARER_A, chat("model-a")).status,
        401
    );
    let valid = fixture.route("a", "openai", upstream, json!({}));
    for routes in [
        json!([valid, valid]),
        json!([fixture.route("a", "openai", upstream, json!({"models": []}))]),
        json!([fixture.route("a", "openai", upstream, json!({"token_sha256": "x"}))]),
        json!([fixture.route("a", "gemini", upstream, json!({}))]),
    ] {
        fixture.routes(routes);
        assert!(fixture.broker().is_err());
        let response = call(&broker, "/v1/chat/completions", BEARER_A, chat("model-a"));
        assert_eq!(response.status, 503);
    }
    fixture.routes(json!([valid]));
    assert_eq!(
        call(&broker, "/v1/chat/completions", BEARER_A, chat("model-c")).status,
        403
    );
    fs::set_permissions(fixture.0.join("routes"), fs::Permissions::from_mode(0o666)).unwrap();
    assert!(fixture.broker().is_err());
    // Without the test flag, plain HTTP upstreams are invalid.
    assert!(
        Routes::new(fixture.0.join("routes"), false)
            .table()
            .is_err(),
        "plain HTTP accepted"
    );
    provider.join().unwrap();
}

#[test]
fn provider_side_tools_and_extensions_are_refused_before_budget() {
    let fixture = Fixture::new();
    let (upstream, provider) = provider(0);
    fixture.routes(json!([
        fixture.route("a", "openai", upstream, json!({})),
        fixture.route("b", "anthropic", upstream, json!({})),
    ]));
    let broker = fixture.broker().unwrap();
    let features = [
        json!({"tools": [{"type": "web_search"}]}),
        json!({"tools": [{"type": "web_search_20250305", "name": "web_search"}]}),
        json!({"tools": [{"type": "mcp", "server_url": "https://connector.example.org"}]}),
        json!({"tools": [{"type": "code_interpreter"}]}),
        json!({"tool_choice": {"type": "web_search"}}),
        json!({"tool_choice": {"type": "allowed_tools", "tools": [{"type": "mcp"}]}}),
        json!({"web_search_options": {}}),
        json!({"plugins": [{"id": "web"}]}),
        json!({"mcp_servers": [{"type": "url", "url": "https://connector.example.org"}]}),
        json!({"container": "container-fixture"}),
    ];
    for feature in &features {
        for (path, auth, mut payload) in [
            ("/v1/chat/completions", BEARER_A, chat("model-a")),
            ("/v1/messages", API_KEY_B, messages("model-b")),
        ] {
            payload
                .as_object_mut()
                .unwrap()
                .extend(feature.as_object().unwrap().clone());
            assert_eq!(call(&broker, path, auth, payload).status, 403, "{feature}");
        }
    }
    provider.join().unwrap();
    assert_eq!(fixture.budget(), Value::Null);
    fixture.audit();
}

#[test]
fn input_budget_uses_a_conservative_byte_bound() {
    assert_eq!(conservative_input_tokens(1), 1);
    assert_eq!(conservative_input_tokens(4096), 4096);
}
