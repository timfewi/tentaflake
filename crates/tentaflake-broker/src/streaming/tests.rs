use super::*;
use crate::broker::Broker;
use crate::config::Config;
use crate::http::read_request;
use std::fs;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::thread::{self, JoinHandle};

struct Directory(PathBuf);

impl Directory {
    fn new() -> Self {
        static SEQUENCE: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "tentaflake-sse-{}-{}",
            std::process::id(),
            SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).unwrap();
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
        for (file, secret) in [
            ("token", "virtual-agent-key"),
            ("provider", "real-provider-key"),
        ] {
            fs::write(root.join(file), secret).unwrap();
            fs::set_permissions(root.join(file), fs::Permissions::from_mode(0o600)).unwrap();
        }
        Self(root)
    }

    fn audit(&self) -> String {
        let audit = fs::read_to_string(self.0.join("audit")).unwrap();
        for secret in [
            "real-provider-key",
            "virtual-agent-key",
            "SENSITIVE_PROMPT_FIXTURE",
            "PRIVATE_OUTPUT_FIXTURE",
            "conversation-stream",
            "conversation-other",
        ] {
            assert!(!audit.contains(secret), "audit leaked fixture content");
        }
        audit
    }
}

impl Drop for Directory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn config(directory: &Directory, upstream: std::net::SocketAddr) -> Config {
    serde_json::from_value(json!({
        "agent": "stream-fixture", "listen": "127.0.0.1:7810",
        "token_file": directory.0.join("token"),
        "audit_file": directory.0.join("audit"),
        "budget_state_file": directory.0.join("budget"),
        "max_response_bytes": 8192,
        "llm": {
            "upstream_base_url": format!("http://{upstream}/v1/"),
            "provider_credential_file": directory.0.join("provider"),
            "allowed_models": [{"name": "fixture-model", "input_microusd_per_million": 1000, "output_microusd_per_million": 2000}],
            "max_completion_tokens": 32, "allow_plain_http_for_tests": true,
            "streaming": {"enable": true, "max_event_bytes": 4096,
                "first_event_timeout_seconds": 2, "idle_timeout_seconds": 2, "total_timeout_seconds": 4}
        }
    })).unwrap()
}

fn chat(delta: Value, finish: Value) -> Vec<u8> {
    format!(
        "data: {}\n\n",
        json!({"object": "chat.completion.chunk", "id": "fixture", "model": "fixture-model",
        "choices": [{"index": 0, "delta": delta, "finish_reason": finish}]})
    )
    .into_bytes()
}

fn request(responses: bool) -> Value {
    if responses {
        json!({"model":"fixture-model","stream":true,"max_output_tokens":8,
            "input":"SENSITIVE_PROMPT_FIXTURE","tools":[{"type":"function","name":"local_tool","parameters":{"type":"object"}}]})
    } else {
        json!({"model":"fixture-model","stream":true,"max_completion_tokens":8,
            "messages":[{"role":"user","content":"SENSITIVE_PROMPT_FIXTURE"}],
            "tools":[{"type":"function","function":{"name":"local_tool","parameters":{"type":"object"}}}]})
    }
}

struct Exchange {
    client: TcpStream,
    broker: JoinHandle<()>,
    provider: JoinHandle<()>,
    directory: Directory,
}

impl Exchange {
    fn start(
        responses: bool,
        adjust: impl FnOnce(&mut Config),
        provider: impl FnOnce(TcpStream) + Send + 'static,
    ) -> Self {
        Self::start_with_session(responses, adjust, Some("conversation-stream"), provider)
    }

    fn start_with_session(
        responses: bool,
        adjust: impl FnOnce(&mut Config),
        session_id: Option<&str>,
        provider: impl FnOnce(TcpStream) + Send + 'static,
    ) -> Self {
        let directory = Directory::new();
        let upstream = TcpListener::bind("127.0.0.1:0").unwrap();
        let mut cfg = config(&directory, upstream.local_addr().unwrap());
        adjust(&mut cfg);
        cfg.validate().unwrap();
        if cfg.llm.as_ref().unwrap().strict_cost_envelope {
            crate::state::Limits::initialize_strict(&cfg.budget_state_file).unwrap();
        }
        let broker = Arc::new(Broker::new(cfg).unwrap());
        let session_header = session_id
            .map(|session| format!("X-OpenCode-Session: {session}\r\n"))
            .unwrap_or_default();
        let expected_session = session_id.map(str::to_owned);
        let provider = thread::spawn(move || {
            let (mut socket, _) = upstream.accept().unwrap();
            socket
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            socket
                .set_write_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let request = read_request(&mut socket, 32768, 1048576).unwrap();
            assert_eq!(request.headers["authorization"], "Bearer real-provider-key");
            assert_eq!(
                request.headers.get("x-opencode-session"),
                expected_session.as_ref()
            );
            assert!(!request.headers.contains_key("x-forged"));
            let body: Value = serde_json::from_slice(&request.body).unwrap();
            assert_eq!(body["stream"], true);
            assert_eq!(
                body[if responses {
                    "max_output_tokens"
                } else {
                    "max_completion_tokens"
                }],
                8
            );
            provider(socket);
            // No second dispatch after a partial response or provider failure.
            upstream.set_nonblocking(true).unwrap();
            assert!(upstream.accept().is_err());
        });
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let broker = thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            crate::handle_connection(&mut socket, broker, 32768, 1048576, Duration::from_secs(5));
        });
        let mut client = TcpStream::connect(address).unwrap();
        client
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let body = serde_json::to_vec(&request(responses)).unwrap();
        let route = if responses {
            "responses"
        } else {
            "chat/completions"
        };
        write!(client, "POST /v1/{route} HTTP/1.1\r\nContent-Type: application/json\r\nAuthorization: Bearer virtual-agent-key\r\n{session_header}X-Forged: not-forwarded\r\nContent-Length: {}\r\n\r\n", body.len()).unwrap();
        client.write_all(&body).unwrap();
        Self {
            client,
            broker,
            provider,
            directory,
        }
    }

    fn read_until(&mut self, marker: &[u8]) -> Vec<u8> {
        let mut result = Vec::new();
        while !result.windows(marker.len()).any(|part| part == marker) {
            let mut byte = [0];
            assert_eq!(
                self.client.read(&mut byte).unwrap(),
                1,
                "connection ended before marker"
            );
            result.push(byte[0]);
            assert!(result.len() < 32768);
        }
        result
    }

    fn finish(mut self, mut prefix: Vec<u8>) -> (Vec<u8>, String, Value) {
        self.client.read_to_end(&mut prefix).unwrap();
        let (audit, budget) = self.join();
        (prefix, audit, budget)
    }

    fn join(self) -> (String, Value) {
        self.broker.join().unwrap();
        self.provider.join().unwrap();
        let audit = self.directory.audit();
        let budget =
            serde_json::from_slice(&fs::read(self.directory.0.join("budget")).unwrap()).unwrap();
        (audit, budget)
    }
}

fn headers(socket: &mut TcpStream) {
    socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream; charset=utf-8\r\nConnection: close\r\n\r\n").unwrap();
}

#[test]
fn preserves_optional_session_metadata_on_both_streaming_routes() {
    for responses in [false, true] {
        for session in [None, Some("conversation-other")] {
            let exchange = Exchange::start_with_session(
                responses,
                |_| {},
                session,
                move |mut socket| {
                    headers(&mut socket);
                    if responses {
                        socket.write_all(b"event: response.completed\ndata: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\"}}\n\n").unwrap();
                    } else {
                        socket.write_all(&chat(json!({}), json!("stop"))).unwrap();
                        socket.write_all(b"data: [DONE]\n\n").unwrap();
                    }
                },
            );
            let (raw, audit, budget) = exchange.finish(Vec::new());
            assert!(raw.starts_with(b"HTTP/1.1 200 OK"));
            assert!(raw.ends_with(b"0\r\n\r\n"));
            assert!(audit.contains("\"outcome\":\"completed\""));
            assert_eq!(budget["requests"], 1);
        }
    }
}

#[test]
fn strict_cost_mode_forwards_bounded_chat_sse_and_holds_budget() {
    let exchange = Exchange::start_with_session(
        false,
        |cfg| cfg.llm.as_mut().unwrap().strict_cost_envelope = true,
        None,
        |mut socket| {
            headers(&mut socket);
            socket
                .write_all(&chat(json!({"content":"fixture"}), Value::Null))
                .unwrap();
            socket.write_all(&chat(json!({}), json!("stop"))).unwrap();
            socket.write_all(b"data: [DONE]\n\n").unwrap();
        },
    );
    let (raw, audit, budget) = exchange.finish(Vec::new());
    assert!(raw.starts_with(b"HTTP/1.1 200 OK"));
    assert!(raw.ends_with(b"0\r\n\r\n"));
    assert!(String::from_utf8_lossy(&raw).contains("[DONE]"));
    assert!(audit.contains("\"outcome\":\"completed\""));
    assert_eq!(budget["requests"], 1);
    assert_eq!(budget["unsettled"], 0);
    assert!(budget["cost_microusd"].as_u64().unwrap() > 0);
}

#[test]
fn strict_cost_mode_preserves_uncertain_stream_reservation_after_restart() {
    use crate::http::Request;
    use std::collections::HashMap;

    let mut exchange = Exchange::start_with_session(
        false,
        |cfg| cfg.llm.as_mut().unwrap().strict_cost_envelope = true,
        None,
        |mut socket| {
            headers(&mut socket);
            socket
                .write_all(&chat(json!({"content":"partial"}), Value::Null))
                .unwrap();
        },
    );
    let mut raw = Vec::new();
    exchange.client.read_to_end(&mut raw).unwrap();
    assert!(!raw.ends_with(b"0\r\n\r\n"));
    assert!(!String::from_utf8_lossy(&raw).contains("[DONE]"));
    exchange.broker.join().unwrap();
    exchange.provider.join().unwrap();
    assert!(exchange.directory.audit().contains("stream_aborted"));
    let saved = fs::read(exchange.directory.0.join("budget")).unwrap();
    let budget: Value = serde_json::from_slice(&saved).unwrap();
    assert_eq!(budget["requests"], 1);
    assert_eq!(budget["unsettled"], 1);

    let mut cfg = config(&exchange.directory, "127.0.0.1:1".parse().unwrap());
    cfg.llm.as_mut().unwrap().strict_cost_envelope = true;
    cfg.daily_token_budget = budget["tokens"].as_u64().unwrap();
    let restarted = Broker::new(cfg).unwrap();
    let response = restarted.handle(Request {
        method: "POST".into(),
        path: "/v1/chat/completions".into(),
        headers: HashMap::from([
            ("authorization".into(), "Bearer virtual-agent-key".into()),
            ("content-type".into(), "application/json".into()),
        ]),
        body: serde_json::to_vec(&request(false)).unwrap(),
    });
    assert_eq!(response.status, 429);
    let after: Value =
        serde_json::from_slice(&fs::read(exchange.directory.0.join("budget")).unwrap()).unwrap();
    for field in ["day", "requests", "tokens", "cost_microusd"] {
        assert_eq!(after[field], budget[field], "{field}");
    }
    assert!(after["last_seen_seconds"].as_u64() >= budget["last_seen_seconds"].as_u64());
}

#[test]
fn strict_stream_cancellation_keeps_reservation_unsettled() {
    let (cancel_send, cancel_receive) = mpsc::channel();
    let mut exchange = Exchange::start_with_session(
        false,
        |cfg| cfg.llm.as_mut().unwrap().strict_cost_envelope = true,
        None,
        move |mut socket| {
            headers(&mut socket);
            socket
                .write_all(&chat(
                    json!({"content":"PRIVATE_OUTPUT_FIXTURE"}),
                    Value::Null,
                ))
                .unwrap();
            let mut byte = [0];
            cancel_send
                .send(socket.read(&mut byte).unwrap() == 0)
                .unwrap();
        },
    );
    exchange.read_until(b"PRIVATE_OUTPUT_FIXTURE");
    exchange.client.shutdown(Shutdown::Both).unwrap();
    assert!(cancel_receive.recv_timeout(Duration::from_secs(1)).unwrap());
    let (audit, budget) = exchange.join();
    assert!(audit.contains("client disconnected"));
    assert_eq!(budget["unsettled"], 1);
}

#[test]
fn strict_stream_audit_failure_keeps_reservation_unsettled() {
    let (send, receive) = mpsc::channel();
    let mut exchange = Exchange::start_with_session(
        false,
        |cfg| cfg.llm.as_mut().unwrap().strict_cost_envelope = true,
        None,
        move |mut socket| {
            headers(&mut socket);
            socket
                .write_all(&chat(
                    json!({"content":"PRIVATE_OUTPUT_FIXTURE"}),
                    Value::Null,
                ))
                .unwrap();
            receive.recv_timeout(Duration::from_secs(3)).unwrap();
            let _ = socket.write_all(&chat(json!({}), json!("stop")));
            let _ = socket.write_all(b"data: [DONE]\n\n");
        },
    );
    let prefix = exchange.read_until(b"PRIVATE_OUTPUT_FIXTURE");
    fs::set_permissions(
        exchange.directory.0.join("audit"),
        fs::Permissions::from_mode(0o644),
    )
    .unwrap();
    send.send(()).unwrap();
    let (raw, audit, budget) = exchange.finish(prefix);
    assert!(!String::from_utf8_lossy(&raw).contains("[DONE]"));
    assert!(audit.contains("stream_aborted"));
    assert_eq!(budget["unsettled"], 1);
}

#[test]
fn strict_stream_failure_event_does_not_settle() {
    let exchange = Exchange::start_with_session(
        false,
        |cfg| cfg.llm.as_mut().unwrap().strict_cost_envelope = true,
        None,
        |mut socket| {
            headers(&mut socket);
            socket
                .write_all(b"data: {\"error\":{\"message\":\"fixture\"}}\n\n")
                .unwrap();
        },
    );
    let (raw, audit, budget) = exchange.finish(Vec::new());
    assert!(raw.starts_with(b"HTTP/1.1 200"));
    assert!(audit.contains("\"outcome\":\"failed\""));
    assert_eq!(budget["unsettled"], 1);
}

#[test]
fn strict_stream_settles_a_dated_snapshot_of_the_allowed_model() {
    let exchange = Exchange::start_with_session(
        false,
        |cfg| cfg.llm.as_mut().unwrap().strict_cost_envelope = true,
        None,
        |mut socket| {
            headers(&mut socket);
            let chunk = json!({"object":"chat.completion.chunk","id":"fixture",
                "model":"fixture-model-2024-08-06",
                "choices":[{"index":0,"delta":{"content":"ok"},"finish_reason":"stop"}]});
            let _ = write!(socket, "data: {chunk}\n\ndata: [DONE]\n\n");
        },
    );
    let (raw, audit, budget) = exchange.finish(Vec::new());
    assert!(String::from_utf8_lossy(&raw).contains("[DONE]"));
    assert!(audit.contains("\"outcome\":\"completed\""));
    assert_eq!(budget["unsettled"], 0);
}

#[test]
fn strict_stream_settles_only_a_complete_provider_error() {
    for (body, complete) in [
        (r#"{"error":{"code":"context_length_exceeded"}}"#, true),
        ("truncated", false),
    ] {
        let exchange = Exchange::start_with_session(
            false,
            |cfg| cfg.llm.as_mut().unwrap().strict_cost_envelope = true,
            None,
            move |mut socket| {
                // A truncated body announces more bytes than it sends.
                let length = if complete {
                    body.len()
                } else {
                    body.len() + 16
                };
                let _ = write!(
                    socket,
                    "HTTP/1.1 400 Bad Request\r\nContent-Type: application/json\r\nContent-Length: {length}\r\nConnection: close\r\n\r\n{body}"
                );
            },
        );
        let (raw, audit, budget) = exchange.finish(Vec::new());
        assert!(String::from_utf8_lossy(&raw).starts_with("HTTP/1.1 "));
        assert!(audit.contains("stream_aborted"));
        assert_eq!(budget["requests"], 1);
        assert_eq!(budget["unsettled"], if complete { 0 } else { 1 }, "{body}");
    }
}

#[test]
fn ordinary_stream_accepts_choice_metadata_without_a_delta() {
    let exchange = Exchange::start(
        false,
        |_| {},
        |mut socket| {
            headers(&mut socket);
            let filter = json!({"object":"chat.completion.chunk","id":"fixture","model":"fixture-model",
                "choices":[{"index":0,"finish_reason":null,"content_filter_results":{}}]});
            let _ = write!(socket, "data: {filter}\n\n");
            socket.write_all(&chat(json!({}), json!("stop"))).unwrap();
            socket.write_all(b"data: [DONE]\n\n").unwrap();
        },
    );
    let (raw, audit, _) = exchange.finish(Vec::new());
    assert!(String::from_utf8_lossy(&raw).contains("[DONE]"));
    assert!(audit.contains("\"outcome\":\"completed\""));
}

#[test]
fn strict_stream_does_not_settle_without_the_confirmed_priced_model() {
    for model in [None, Some("other-model"), Some("fixture-model-mini")] {
        let exchange = Exchange::start_with_session(
            false,
            |cfg| cfg.llm.as_mut().unwrap().strict_cost_envelope = true,
            None,
            move |mut socket| {
                headers(&mut socket);
                let mut chunk = json!({"object":"chat.completion.chunk","id":"fixture",
                    "choices":[{"index":0,"delta":{},"finish_reason":"stop"}]});
                if let Some(model) = model {
                    chunk["model"] = json!(model);
                }
                let _ = write!(socket, "data: {chunk}\n\ndata: [DONE]\n\n");
            },
        );
        let (raw, audit, budget) = exchange.finish(Vec::new());
        assert!(!String::from_utf8_lossy(&raw).contains("[DONE]"));
        assert!(audit.contains("stream_aborted"));
        assert_eq!(budget["unsettled"], 1);
    }
}

#[test]
fn strict_stream_requires_one_nonempty_completion_id_on_every_chunk() {
    for (first_id, finish_id) in [
        (None, Some("fixture")),
        (Some(""), Some("fixture")),
        (Some(" "), Some("fixture")),
        (Some("fixture"), None),
        (Some("fixture"), Some("")),
        (Some("fixture"), Some("different")),
    ] {
        let exchange = Exchange::start_with_session(
            false,
            |cfg| cfg.llm.as_mut().unwrap().strict_cost_envelope = true,
            None,
            move |mut socket| {
                headers(&mut socket);
                let mut first = json!({"object":"chat.completion.chunk","model":"fixture-model",
                    "choices":[{"index":0,"delta":{"content":"partial"},"finish_reason":null}]});
                if let Some(id) = first_id {
                    first["id"] = json!(id);
                }
                let _ = write!(socket, "data: {first}\n\n");
                let mut finish = json!({"object":"chat.completion.chunk","model":"fixture-model",
                    "choices":[{"index":0,"delta":{},"finish_reason":"stop"}]});
                if let Some(id) = finish_id {
                    finish["id"] = json!(id);
                }
                let _ = write!(socket, "data: {finish}\n\ndata: [DONE]\n\n");
            },
        );
        let (raw, audit, budget) = exchange.finish(Vec::new());
        assert!(!String::from_utf8_lossy(&raw).contains("[DONE]"));
        assert!(audit.contains("stream_aborted"));
        assert_eq!(budget["unsettled"], 1);
    }
}

#[test]
fn strict_stream_rejects_missing_or_malformed_delta_before_completion() {
    for delta in [
        None,
        Some(Value::Null),
        Some(json!("invalid")),
        Some(json!([])),
        Some(json!({"content": 42})),
        Some(json!({
            "tool_calls": [{"index": 0, "function": {"arguments": 42}}]
        })),
    ] {
        let exchange = Exchange::start_with_session(
            false,
            |cfg| cfg.llm.as_mut().unwrap().strict_cost_envelope = true,
            None,
            move |mut socket| {
                headers(&mut socket);
                let _ = socket.write_all(&chat(json!({"content": "partial"}), Value::Null));
                let mut finish = json!({"object":"chat.completion.chunk","id":"fixture",
                    "model":"fixture-model","choices":[{"index":0,"finish_reason":"stop"}]});
                if let Some(delta) = delta {
                    finish["choices"][0]["delta"] = delta;
                }
                let _ = write!(socket, "data: {finish}\n\ndata: [DONE]\n\n");
            },
        );
        let (raw, audit, budget) = exchange.finish(Vec::new());
        assert!(!String::from_utf8_lossy(&raw).contains("[DONE]"));
        assert!(!raw.ends_with(b"0\r\n\r\n"));
        assert!(audit.contains("stream_aborted"));
        assert_eq!(budget["unsettled"], 1);
    }
}

#[test]
fn strict_nonstream_settles_only_verified_audited_delivered_outcomes() {
    use crate::http::Request;
    use std::collections::HashMap;
    use std::os::unix::fs::symlink;

    let complete = json!({"id":"fixture","object":"chat.completion","model":"fixture-model",
        "choices":[{"index":0,"message":{"role":"assistant","content":"ok"},"finish_reason":"stop"}]});
    let mut incomplete = complete.clone();
    incomplete["choices"][0]["finish_reason"] = Value::Null;
    let mut wrong_model = complete.clone();
    wrong_model["model"] = json!("other-model");
    let mut snapshot_model = complete.clone();
    snapshot_model["model"] = json!("fixture-model-2024-08-06");
    let mut variant_model = complete.clone();
    variant_model["model"] = json!("fixture-model-mini");
    let mut no_output = complete.clone();
    no_output["choices"][0]["message"] = json!({"role":"assistant"});
    let mut empty_output = complete.clone();
    empty_output["choices"][0]["message"]["content"] = json!("");
    let mut tools = complete.clone();
    tools["choices"][0]["finish_reason"] = json!("tool_calls");
    tools["choices"][0]["message"] = json!({"role":"assistant", "content":null,
        "tool_calls":[{"id":"call-1", "type":"function",
            "function":{"name":"local_tool", "arguments":"{}"}}]});
    let mut filtered = complete.clone();
    filtered["choices"][0]["finish_reason"] = json!("content_filter");
    filtered["choices"][0]["message"] = json!({"role":"assistant", "refusal":"blocked"});
    let mut limited = complete.clone();
    limited["choices"][0]["finish_reason"] = json!("length");
    let mut withheld = complete.clone();
    withheld["choices"][0]["finish_reason"] = json!("content_filter");
    withheld["choices"][0]["message"] = json!({"role":"assistant", "content":null});
    let mut malformed_output = complete.clone();
    malformed_output["choices"][0]["message"]["content"] = json!(42);
    let cases = [
        (200, serde_json::to_vec(&complete).unwrap(), false, true, 0),
        (200, serde_json::to_vec(&no_output).unwrap(), false, true, 0),
        (
            200,
            serde_json::to_vec(&empty_output).unwrap(),
            false,
            true,
            0,
        ),
        (200, serde_json::to_vec(&withheld).unwrap(), false, true, 0),
        (
            200,
            serde_json::to_vec(&malformed_output).unwrap(),
            false,
            true,
            1,
        ),
        (
            200,
            serde_json::to_vec(&snapshot_model).unwrap(),
            false,
            true,
            0,
        ),
        (
            200,
            serde_json::to_vec(&variant_model).unwrap(),
            false,
            true,
            1,
        ),
        (200, serde_json::to_vec(&tools).unwrap(), false, true, 0),
        (200, serde_json::to_vec(&filtered).unwrap(), false, true, 0),
        (200, serde_json::to_vec(&limited).unwrap(), false, true, 0),
        (
            200,
            serde_json::to_vec(&incomplete).unwrap(),
            false,
            true,
            1,
        ),
        (
            200,
            serde_json::to_vec(&wrong_model).unwrap(),
            false,
            true,
            1,
        ),
        (200, b"not-json".to_vec(), false, true, 1),
        // Complete provider errors end the request; they are not refunded.
        (
            400,
            br#"{"error":{"code":"context_length_exceeded"}}"#.to_vec(),
            false,
            true,
            0,
        ),
        (429, b"rate limited".to_vec(), false, true, 0),
        (500, serde_json::to_vec(&complete).unwrap(), false, true, 0),
        (500, serde_json::to_vec(&complete).unwrap(), false, false, 1),
        (200, serde_json::to_vec(&complete).unwrap(), true, true, 1),
        (200, serde_json::to_vec(&complete).unwrap(), false, false, 1),
    ];
    for (status, response_body, fail_audit, deliver, unsettled) in cases {
        let directory = Directory::new();
        let upstream = TcpListener::bind("127.0.0.1:0").unwrap();
        let mut cfg = config(&directory, upstream.local_addr().unwrap());
        cfg.llm.as_mut().unwrap().strict_cost_envelope = true;
        crate::state::Limits::initialize_strict(&cfg.budget_state_file).unwrap();
        let broker = Arc::new(Broker::new(cfg.clone()).unwrap());
        let audit_path = directory.0.join("audit");
        let provider = thread::spawn(move || {
            let (mut socket, _) = upstream.accept().unwrap();
            socket
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let req = read_request(&mut socket, 32768, 1048576).unwrap();
            assert_eq!(req.headers["authorization"], "Bearer real-provider-key");
            if fail_audit {
                fs::remove_file(&audit_path).unwrap();
                symlink("/dev/null", &audit_path).unwrap();
            }
            write!(socket, "HTTP/1.1 {status} fixture\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", response_body.len()).unwrap();
            socket.write_all(&response_body).unwrap();
        });
        let mut payload = request(false);
        payload["stream"] = json!(false);
        let body = serde_json::to_vec(&payload).unwrap();
        if deliver {
            let ingress = TcpListener::bind("127.0.0.1:0").unwrap();
            let address = ingress.local_addr().unwrap();
            let worker = Arc::clone(&broker);
            let handler = thread::spawn(move || {
                let (mut socket, _) = ingress.accept().unwrap();
                crate::handle_connection(
                    &mut socket,
                    worker,
                    32768,
                    1048576,
                    Duration::from_secs(5),
                );
            });
            let mut client = TcpStream::connect(address).unwrap();
            client
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            write!(client, "POST /v1/chat/completions HTTP/1.1\r\nContent-Type: application/json\r\nAuthorization: Bearer virtual-agent-key\r\nContent-Length: {}\r\n\r\n", body.len()).unwrap();
            client.write_all(&body).unwrap();
            let mut response = Vec::new();
            client.read_to_end(&mut response).unwrap();
            let expected_status = if fail_audit { 503 } else { status };
            assert!(
                String::from_utf8_lossy(&response)
                    .starts_with(&format!("HTTP/1.1 {expected_status}"))
            );
            handler.join().unwrap();
        } else {
            let response = broker.handle(Request {
                method: "POST".into(),
                path: "/v1/chat/completions".into(),
                headers: HashMap::from([
                    ("authorization".into(), "Bearer virtual-agent-key".into()),
                    ("content-type".into(), "application/json".into()),
                ]),
                body,
            });
            assert_eq!(response.status, status);
            // A lost downstream response never consumes the completion capability.
            drop(response);
        }
        provider.join().unwrap();
        let state: Value =
            serde_json::from_slice(&fs::read(&cfg.budget_state_file).unwrap()).unwrap();
        assert_eq!(state["requests"], 1);
        assert_eq!(state["unsettled"], unsettled);
        if !fail_audit {
            directory.audit();
        }
    }
}

#[test]
fn strict_nonstream_usage_overages_block_admission_and_survive_restart() {
    use crate::http::{Request, write_response};
    use std::collections::HashMap;

    for (strict, responses) in [(true, false), (false, false), (false, true)] {
        for kind in [
            "tokens",
            "price",
            "price-overflow",
            "sum-overflow",
            "malformed",
            "missing",
        ] {
            let directory = Directory::new();
            let upstream = TcpListener::bind("127.0.0.1:0").unwrap();
            let mut cfg = config(&directory, upstream.local_addr().unwrap());
            cfg.llm.as_mut().unwrap().strict_cost_envelope = strict;
            cfg.daily_token_budget = u64::MAX;
            cfg.daily_cost_microusd = u64::MAX;
            let model = &mut cfg.llm.as_mut().unwrap().allowed_models[0];
            model.input_microusd_per_million = 1_000_000;
            model.output_microusd_per_million = 2_000_000;
            if kind == "price-overflow" {
                model.input_microusd_per_million = 1;
                model.output_microusd_per_million = u64::MAX;
            }
            if strict {
                crate::state::Limits::initialize_strict(&cfg.budget_state_file).unwrap();
            }
            let broker = Broker::new(cfg.clone()).unwrap();
            let mut payload = request(responses);
            payload["stream"] = json!(false);
            if kind == "price-overflow" {
                let padding = "x".repeat(1_000_020);
                if responses {
                    payload["input"] = json!(padding);
                } else {
                    payload["messages"][0]["content"] = json!(padding);
                }
            }
            let body = serde_json::to_vec(&payload).unwrap();
            if kind == "price-overflow" {
                assert!(1_000_001 < body.len() as u64 + 8);
            }
            let price_only_output = body.len() as u64 + 7;
            let usage = match kind {
                "tokens" => Some(json!({
                    "prompt_tokens": 5000, "completion_tokens": 1, "total_tokens": 5001
                })),
                "price" => Some(json!({
                    "prompt_tokens": 0, "completion_tokens": price_only_output,
                    "total_tokens": price_only_output
                })),
                "price-overflow" => Some(json!({
                    "prompt_tokens": 0, "completion_tokens": 1_000_001,
                    "total_tokens": 1_000_001
                })),
                "sum-overflow" => Some(json!({
                    "prompt_tokens": u64::MAX, "completion_tokens": 1, "total_tokens": u64::MAX
                })),
                "malformed" => Some(json!({
                    "prompt_tokens": 1, "completion_tokens": 2, "total_tokens": 4
                })),
                "missing" => None,
                _ => unreachable!(),
            };
            let mut answer = json!({"id":"fixture","object":"chat.completion","model":"fixture-model",
                "choices":[{"index":0,"message":{"role":"assistant","content":"ok"},"finish_reason":"stop"}]});
            if let Some(usage) = usage {
                answer["usage"] = usage;
            }
            let response_body = serde_json::to_vec(&answer).unwrap();
            let provider = thread::spawn(move || {
                let (mut socket, _) = upstream.accept().unwrap();
                read_request(&mut socket, 32768, 1048576).unwrap();
                write!(
                    socket,
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    response_body.len()
                )
                .unwrap();
                socket.write_all(&response_body).unwrap();
            });
            let route = if responses {
                "/v1/responses"
            } else {
                "/v1/chat/completions"
            };
            let make_request = || Request {
                method: "POST".into(),
                path: route.into(),
                headers: HashMap::from([
                    ("authorization".into(), "Bearer virtual-agent-key".into()),
                    ("content-type".into(), "application/json".into()),
                ]),
                body: body.clone(),
            };
            let response = broker.handle(make_request());
            assert_eq!(response.status, 200, "{kind}");
            let ingress = TcpListener::bind("127.0.0.1:0").unwrap();
            let mut client = TcpStream::connect(ingress.local_addr().unwrap()).unwrap();
            let (mut server, _) = ingress.accept().unwrap();
            write_response(&mut server, response).unwrap();
            server.shutdown(Shutdown::Both).unwrap();
            let mut delivered = Vec::new();
            client.read_to_end(&mut delivered).unwrap();
            assert!(String::from_utf8_lossy(&delivered).contains("\"content\":\"ok\""));
            provider.join().unwrap();
            let budget: Value =
                serde_json::from_slice(&fs::read(&cfg.budget_state_file).unwrap()).unwrap();
            let blocked = strict && !matches!(kind, "malformed" | "missing");
            if blocked {
                assert_eq!(budget["tokens"], u64::MAX, "{kind}");
                assert_eq!(budget["cost_microusd"], u64::MAX, "{kind}");
                assert_eq!(broker.handle(make_request()).status, 429, "{kind}");
            } else {
                assert_eq!(budget["tokens"], body.len() as u64 + 8, "{kind}");
            }
            if strict {
                assert_eq!(budget["unsettled"], 0, "{kind}");
            }
            drop(broker);
            if blocked {
                let restarted = Broker::new(cfg).unwrap();
                assert_eq!(restarted.handle(make_request()).status, 429, "{kind}");
            }
        }
    }
}

#[test]
fn strict_predispatch_failures_clear_uncertainty_but_retain_daily_charge() {
    use crate::http::Request;
    use std::collections::HashMap;
    use std::os::unix::fs::symlink;

    for streaming in [false, true] {
        for failure in ["address", "credential", "audit"] {
            let directory = Directory::new();
            let upstream = TcpListener::bind("127.0.0.1:0").unwrap();
            let mut cfg = config(&directory, upstream.local_addr().unwrap());
            let policy = cfg.llm.as_mut().unwrap();
            policy.strict_cost_envelope = true;
            if failure == "address" {
                // A synthetic non-public literal exercises the resolver's
                // rejection path without an external DNS or network call.
                policy.upstream_base_url =
                    format!("https://{}/v1/", upstream.local_addr().unwrap());
                policy.allow_plain_http_for_tests = false;
            }
            crate::state::Limits::initialize_strict(&cfg.budget_state_file).unwrap();
            let broker = Broker::new(cfg.clone()).unwrap();
            if failure == "credential" {
                fs::remove_file(directory.0.join("provider")).unwrap();
            }
            if failure == "audit" {
                fs::remove_file(directory.0.join("audit")).unwrap();
                symlink("/dev/null", directory.0.join("audit")).unwrap();
            }
            let mut payload = request(false);
            payload["stream"] = json!(streaming);
            let response = broker.handle(Request {
                method: "POST".into(),
                path: "/v1/chat/completions".into(),
                headers: HashMap::from([
                    ("authorization".into(), "Bearer virtual-agent-key".into()),
                    ("content-type".into(), "application/json".into()),
                ]),
                body: serde_json::to_vec(&payload).unwrap(),
            });
            assert_eq!(
                response.status,
                if failure == "address" { 502 } else { 503 }
            );
            let state: Value =
                serde_json::from_slice(&fs::read(&cfg.budget_state_file).unwrap()).unwrap();
            assert_eq!(state["requests"], 1, "{failure}, streaming={streaming}");
            assert!(state["cost_microusd"].as_u64().unwrap() > 0);
            assert_eq!(state["unsettled"], 0, "{failure}, streaming={streaming}");
            upstream.set_nonblocking(true).unwrap();
            assert!(upstream.accept().is_err());
        }
    }
}

#[test]
fn strict_stream_audit_preflight_failure_is_known_undispatched() {
    use crate::http::{Request, write_response};
    use std::collections::HashMap;

    let directory = Directory::new();
    let upstream = TcpListener::bind("127.0.0.1:0").unwrap();
    let mut cfg = config(&directory, upstream.local_addr().unwrap());
    cfg.llm.as_mut().unwrap().strict_cost_envelope = true;
    crate::state::Limits::initialize_strict(&cfg.budget_state_file).unwrap();
    let broker = Broker::new(cfg.clone()).unwrap();
    let response = broker.handle(Request {
        method: "POST".into(),
        path: "/v1/chat/completions".into(),
        headers: HashMap::from([
            ("authorization".into(), "Bearer virtual-agent-key".into()),
            ("content-type".into(), "application/json".into()),
        ]),
        body: serde_json::to_vec(&request(false)).unwrap(),
    });
    assert!(response.streaming.is_some());
    fs::set_permissions(directory.0.join("audit"), fs::Permissions::from_mode(0o644)).unwrap();
    let ingress = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = ingress.local_addr().unwrap();
    let handler = thread::spawn(move || {
        let (mut socket, _) = ingress.accept().unwrap();
        let _ = write_response(&mut socket, response);
    });
    let mut client = TcpStream::connect(address).unwrap();
    client
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let mut raw = Vec::new();
    client.read_to_end(&mut raw).unwrap();
    handler.join().unwrap();
    assert!(String::from_utf8_lossy(&raw).starts_with("HTTP/1.1 503"));
    let state: Value = serde_json::from_slice(&fs::read(&cfg.budget_state_file).unwrap()).unwrap();
    assert_eq!(state["requests"], 1);
    assert!(state["cost_microusd"].as_u64().unwrap() > 0);
    assert_eq!(state["unsettled"], 0);
    upstream.set_nonblocking(true).unwrap();
    assert!(upstream.accept().is_err());
}

#[test]
fn strict_request_with_lost_upstream_response_stays_unsettled() {
    use crate::http::Request;
    use std::collections::HashMap;

    let directory = Directory::new();
    let upstream = TcpListener::bind("127.0.0.1:0").unwrap();
    let mut cfg = config(&directory, upstream.local_addr().unwrap());
    cfg.llm.as_mut().unwrap().strict_cost_envelope = true;
    crate::state::Limits::initialize_strict(&cfg.budget_state_file).unwrap();
    let broker = Broker::new(cfg.clone()).unwrap();
    let provider = thread::spawn(move || {
        let (mut socket, _) = upstream.accept().unwrap();
        let _ = read_request(&mut socket, 32768, 1048576).unwrap();
        // Drop the connection after receiving the request, without a response.
    });
    let mut payload = request(false);
    payload["stream"] = json!(false);
    let response = broker.handle(Request {
        method: "POST".into(),
        path: "/v1/chat/completions".into(),
        headers: HashMap::from([
            ("authorization".into(), "Bearer virtual-agent-key".into()),
            ("content-type".into(), "application/json".into()),
        ]),
        body: serde_json::to_vec(&payload).unwrap(),
    });
    assert_eq!(response.status, 502);
    provider.join().unwrap();
    let state: Value = serde_json::from_slice(&fs::read(&cfg.budget_state_file).unwrap()).unwrap();
    assert_eq!(state["requests"], 1);
    assert_eq!(state["unsettled"], 1);
}

#[test]
fn strict_cost_mode_denies_unbounded_requests_before_reservation_or_dispatch() {
    use crate::http::Request;
    use std::collections::HashMap;

    let directory = Directory::new();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let mut cfg = config(&directory, listener.local_addr().unwrap());
    cfg.llm.as_mut().unwrap().strict_cost_envelope = true;
    crate::state::Limits::initialize_strict(&cfg.budget_state_file).unwrap();
    let broker = Broker::new(cfg.clone()).unwrap();
    let saved = fs::read(&cfg.budget_state_file).unwrap();
    let cases = [
        (json!({"n":2}), 400),
        (json!({"n":null}), 400),
        (json!({"future_billable_extension":true}), 400),
        (json!({"store":false}), 400),
        (json!({"previous_response_id":"remote"}), 400),
        (json!({"conversation":"remote"}), 400),
        (json!({"background":false}), 400),
        (json!({"parallel_tool_calls":true}), 400),
        (json!({"model":"unpriced-model"}), 403),
        (json!({"max_tokens":4}), 400),
        (json!({"stream":"true"}), 400),
        (json!({"stream_options":{"include_usage":"true"}}), 400),
        (
            json!({"stream_options":{"include_usage":true,"extra":true}}),
            400,
        ),
        (json!({"tools":[{"type":"web_search"}]}), 400),
        (
            json!({"tools":[{"type":"function","function":{"name":"local","remote":true}}]}),
            400,
        ),
        (json!({"tool_choice":{"type":"mcp"}}), 400),
        (
            json!({"messages":[{"role":"user","content":[{"type":"image_url","image_url":{"url":"https://example.invalid"}}]}]}),
            400,
        ),
        (
            json!({"messages":[{"role":"user","content":[{"type":"text","text":"x","file_id":"remote"}]}]}),
            400,
        ),
        (
            json!({"messages":[{"role":"user","content":"x","audio":{"id":"remote"}}]}),
            400,
        ),
        (json!({"messages":[{"role":"tool","content":"x"}]}), 400),
        (
            json!({"messages":[{"role":"assistant","content":null}]}),
            400,
        ),
        (
            json!({"messages":[{"role":"assistant","tool_calls":[{"id":"1","type":"function","function":{"name":"local","arguments":"{}","file_id":"remote"}}]}]}),
            400,
        ),
    ];
    for streaming in [false, true] {
        for (changes, status) in &cases {
            let mut payload = request(false);
            payload["stream"] = json!(streaming);
            payload
                .as_object_mut()
                .unwrap()
                .extend(changes.as_object().unwrap().clone());
            let response = broker.handle(Request {
                method: "POST".into(),
                path: "/v1/chat/completions".into(),
                headers: HashMap::from([
                    ("authorization".into(), "Bearer virtual-agent-key".into()),
                    ("content-type".into(), "application/json".into()),
                ]),
                body: serde_json::to_vec(&payload).unwrap(),
            });
            assert_eq!(response.status, *status, "{changes}");
            assert!(response.streaming.is_none());
            assert_eq!(
                fs::read(&cfg.budget_state_file).unwrap(),
                saved,
                "{changes}"
            );
        }
    }
    for (path, session, status) in [
        ("/v1/responses", None, 404),
        ("/v1/chat/completions", Some("remote-session"), 400),
    ] {
        let mut headers = HashMap::from([
            ("authorization".into(), "Bearer virtual-agent-key".into()),
            ("content-type".into(), "application/json".into()),
        ]);
        if let Some(session) = session {
            headers.insert("x-opencode-session".into(), session.into());
        }
        let response = broker.handle(Request {
            method: "POST".into(),
            path: path.into(),
            headers,
            body: serde_json::to_vec(&request(false)).unwrap(),
        });
        assert_eq!(response.status, status);
        assert_eq!(fs::read(&cfg.budget_state_file).unwrap(), saved);
    }
    let provider_key_is_not_agent_auth = broker.handle(Request {
        method: "POST".into(),
        path: "/v1/chat/completions".into(),
        headers: HashMap::from([
            ("authorization".into(), "Bearer real-provider-key".into()),
            ("content-type".into(), "application/json".into()),
        ]),
        body: serde_json::to_vec(&request(false)).unwrap(),
    });
    assert_eq!(provider_key_is_not_agent_auth.status, 401);
    assert_eq!(fs::read(&cfg.budget_state_file).unwrap(), saved);
    listener.set_nonblocking(true).unwrap();
    assert!(listener.accept().is_err());
    directory.audit();
}

#[test]
fn strict_cost_mode_refuses_lost_or_corrupt_state_before_dispatch() {
    use crate::http::Request;
    use std::collections::HashMap;

    let directory = Directory::new();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let mut cfg = config(&directory, listener.local_addr().unwrap());
    cfg.llm.as_mut().unwrap().strict_cost_envelope = true;
    crate::state::Limits::initialize_strict(&cfg.budget_state_file).unwrap();
    let broker = Broker::new(cfg.clone()).unwrap();
    fs::remove_file(&cfg.budget_state_file).unwrap();
    let make_request = || Request {
        method: "POST".into(),
        path: "/v1/chat/completions".into(),
        headers: HashMap::from([
            ("authorization".into(), "Bearer virtual-agent-key".into()),
            ("content-type".into(), "application/json".into()),
        ]),
        body: serde_json::to_vec(&request(false)).unwrap(),
    };
    assert_eq!(broker.handle(make_request()).status, 429);
    assert!(!cfg.budget_state_file.exists());
    fs::write(&cfg.budget_state_file, b"not-json").unwrap();
    fs::set_permissions(&cfg.budget_state_file, fs::Permissions::from_mode(0o600)).unwrap();
    assert_eq!(broker.handle(make_request()).status, 429);
    assert_eq!(fs::read(&cfg.budget_state_file).unwrap(), b"not-json");
    listener.set_nonblocking(true).unwrap();
    assert!(listener.accept().is_err());
    directory.audit();
}

#[test]
fn rejects_invalid_session_metadata_before_budget_or_upstream_dispatch() {
    use crate::http::Request;
    use std::collections::HashMap;
    let directory = Directory::new();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let broker = Broker::new(config(&directory, listener.local_addr().unwrap())).unwrap();
    for responses in [false, true] {
        for streaming in [false, true] {
            for invalid in [
                String::new(),
                "x".repeat(129),
                "bad\r\nheader".into(),
                "bad session".into(),
                "😀".into(),
            ] {
                let mut payload = request(responses);
                payload["stream"] = json!(streaming);
                let response = broker.handle(Request {
                    method: "POST".into(),
                    path: if responses {
                        "/v1/responses"
                    } else {
                        "/v1/chat/completions"
                    }
                    .into(),
                    headers: HashMap::from([
                        ("authorization".into(), "Bearer virtual-agent-key".into()),
                        ("content-type".into(), "application/json".into()),
                        ("x-opencode-session".into(), invalid),
                    ]),
                    body: serde_json::to_vec(&payload).unwrap(),
                });
                assert_eq!(response.status, 400);
                assert!(response.streaming.is_none());
                assert_eq!(
                    serde_json::from_slice::<Value>(&response.body).unwrap(),
                    json!({"error":"invalid provider session header"})
                );
                assert!(!directory.0.join("budget").exists());
            }
        }
    }
    listener.set_nonblocking(true).unwrap();
    assert!(listener.accept().is_err());
    let audit = directory.audit();
    for value in ["bad session", "bad\\r\\nheader", "😀"] {
        assert!(!audit.contains(value));
    }
}

#[test]
fn forwards_chat_before_completion_with_fragmented_utf8_tools_and_usage() {
    let (send, receive) = mpsc::channel();
    let mut exchange = Exchange::start(
        false,
        |_| {},
        move |mut socket| {
            headers(&mut socket);
            let first = chat(
                json!({"content":"PRIVATE_OUTPUT_FIXTURE 😀", "tool_calls":[{"index":0,"id":"call-1","type":"function","function":{"name":"local_tool","arguments":"{\"part\":"}}]}),
                Value::Null,
            );
            // Fragment at every byte, including inside the UTF-8 scalar.
            for byte in first {
                socket.write_all(&[byte]).unwrap();
            }
            receive.recv_timeout(Duration::from_secs(3)).unwrap();
            socket
                .write_all(&chat(
                    json!({"tool_calls":[{"index":0,"function":{"arguments":"1}"}}]}),
                    json!("tool_calls"),
                ))
                .unwrap();
            socket.write_all(b"data: {\"object\":\"chat.completion.chunk\",\"choices\":[],\"usage\":{\"prompt_tokens\":2,\"completion_tokens\":3,\"total_tokens\":5,\"ignored\":\"PRIVATE_OUTPUT_FIXTURE\"}}\n\ndata: [DONE]\n\n").unwrap();
        },
    );
    let prefix = exchange.read_until(b"PRIVATE_OUTPUT_FIXTURE");
    assert!(prefix.starts_with(b"HTTP/1.1 200"));
    assert!(String::from_utf8_lossy(&prefix).contains("Transfer-Encoding: chunked"));
    send.send(()).unwrap();
    let mut prefix = prefix;
    prefix.extend(exchange.read_until(b"data: [DONE]\n\n"));
    assert!(
        exchange
            .directory
            .audit()
            .contains("\"outcome\":\"completed\"")
    );
    let (raw, audit, budget) = exchange.finish(prefix);
    let raw = String::from_utf8(raw).unwrap();
    assert!(raw.contains("local_tool"));
    assert!(raw.contains("😀"));
    assert!(raw.contains("[DONE]"));
    assert!(raw.ends_with("0\r\n\r\n"));
    assert!(audit.contains("\"total_tokens\":5"));
    assert!(!audit.contains("ignored"));
    assert_eq!(
        budget["tokens"],
        serde_json::to_vec(&request(false)).unwrap().len() as u64 + 8
    );
}

#[test]
fn forwards_responses_function_deltas_and_terminal_usage() {
    let (send, receive) = mpsc::channel();
    let mut exchange = Exchange::start(
        true,
        |_| {},
        move |mut socket| {
            headers(&mut socket);
            socket.write_all(b"event: response.function_call_arguments.delta\r\ndata: {\"type\":\"response.function_call_arguments.delta\",\r\ndata: \"delta\":\"PRIVATE_OUTPUT_FIXTURE\"}\r\n\r\n").unwrap();
            receive.recv_timeout(Duration::from_secs(3)).unwrap();
            socket.write_all(b"event: response.completed\ndata: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\",\"usage\":{\"input_tokens\":2,\"output_tokens\":3,\"total_tokens\":5}}}\n\n").unwrap();
        },
    );
    let prefix = exchange.read_until(b"PRIVATE_OUTPUT_FIXTURE");
    send.send(()).unwrap();
    let mut prefix = prefix;
    prefix.extend(exchange.read_until(b"event: response.completed\n"));
    assert!(
        exchange
            .directory
            .audit()
            .contains("\"outcome\":\"completed\"")
    );
    let (raw, audit, budget) = exchange.finish(prefix);
    assert!(
        String::from_utf8(raw)
            .unwrap()
            .contains("event: response.completed")
    );
    assert!(audit.contains("\"total_tokens\":5"));
    assert_eq!(budget["requests"], 1);
}

#[test]
fn streamed_usage_overages_are_persisted_before_forwarding_or_abort() {
    for (strict, responses, terminal) in [
        (true, false, true),
        (false, false, true),
        (false, true, true),
        (true, false, false),
    ] {
        let exchange = Exchange::start_with_session(
            responses,
            move |cfg| cfg.llm.as_mut().unwrap().strict_cost_envelope = strict,
            None,
            move |mut socket| {
                headers(&mut socket);
                if responses {
                    let _ = socket.write_all(b"event: response.completed\ndata: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\",\"usage\":{\"input_tokens\":5000,\"output_tokens\":1,\"total_tokens\":5001}}}\n\n");
                } else {
                    let _ = socket.write_all(&chat(json!({"content":"ok"}), Value::Null));
                    let _ = socket.write_all(&chat(json!({}), json!("stop")));
                    let usage = json!({"object":"chat.completion.chunk","id":"fixture",
                        "model":"fixture-model","choices":[],
                        "usage":{"prompt_tokens":5000,"completion_tokens":1,"total_tokens":5001}});
                    let _ = write!(socket, "data: {usage}\n\n");
                    if terminal {
                        let _ = socket.write_all(b"data: [DONE]\n\n");
                    }
                }
            },
        );
        let (raw, audit, budget) = exchange.finish(Vec::new());
        if strict {
            assert_eq!(budget["tokens"], u64::MAX);
            assert_eq!(budget["cost_microusd"], u64::MAX);
        } else {
            assert_eq!(
                budget["tokens"],
                serde_json::to_vec(&request(responses)).unwrap().len() as u64 + 8
            );
        }
        assert!(String::from_utf8_lossy(&raw).contains("5001"));
        if terminal {
            assert!(raw.ends_with(b"0\r\n\r\n"));
            assert!(audit.contains("\"outcome\":\"completed\""));
        } else {
            assert!(!raw.ends_with(b"0\r\n\r\n"));
            assert!(audit.contains("stream_aborted"));
        }
        if strict {
            assert_eq!(budget["unsettled"], if terminal { 0 } else { 1 });
        }
    }
}

#[test]
fn strict_stream_budget_write_failure_withholds_usage_and_terminal() {
    use std::os::unix::fs::symlink;

    let (send, receive) = mpsc::channel();
    let mut exchange = Exchange::start_with_session(
        false,
        |cfg| cfg.llm.as_mut().unwrap().strict_cost_envelope = true,
        None,
        move |mut socket| {
            headers(&mut socket);
            socket
                .write_all(&chat(json!({"content":"partial"}), Value::Null))
                .unwrap();
            receive.recv_timeout(Duration::from_secs(3)).unwrap();
            let usage = json!({"object":"chat.completion.chunk","id":"fixture",
                "model":"fixture-model","choices":[],
                "usage":{"prompt_tokens":5000,"completion_tokens":1,"total_tokens":5001}});
            let _ = write!(socket, "data: {usage}\n\ndata: [DONE]\n\n");
        },
    );
    let prefix = exchange.read_until(b"partial");
    let target = exchange.directory.0.join("unrelated");
    fs::write(&target, b"untouched").unwrap();
    symlink(&target, exchange.directory.0.join("budget.tmp")).unwrap();
    send.send(()).unwrap();
    let (raw, audit, budget) = exchange.finish(prefix);
    assert!(!String::from_utf8_lossy(&raw).contains("5001"));
    assert!(!String::from_utf8_lossy(&raw).contains("[DONE]"));
    assert!(!raw.ends_with(b"0\r\n\r\n"));
    assert!(audit.contains("budget accounting is unavailable"));
    assert_eq!(budget["pending_usage"], 1);
    assert_eq!(budget["unsettled"], 1);
}

#[test]
fn premature_eof_never_adds_success_or_refunds_budget() {
    let exchange = Exchange::start(
        false,
        |_| {},
        |mut socket| {
            headers(&mut socket);
            socket
                .write_all(&chat(
                    json!({"content":"PRIVATE_OUTPUT_FIXTURE"}),
                    Value::Null,
                ))
                .unwrap();
        },
    );
    let (raw, audit, budget) = exchange.finish(Vec::new());
    assert!(!String::from_utf8_lossy(&raw).contains("[DONE]"));
    assert!(!raw.ends_with(b"0\r\n\r\n"));
    assert!(audit.contains("stream_aborted"));
    assert_eq!(budget["requests"], 1);
}

#[test]
fn rejects_non_sse_and_provider_http_errors_before_headers() {
    for response in [
        b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 2\r\n\r\n{}"
            .as_slice(),
        b"HTTP/1.1 429 Too Many Requests\r\nContent-Length: 0\r\n\r\n".as_slice(),
    ] {
        let exchange = Exchange::start(
            false,
            |_| {},
            move |mut socket| {
                socket.write_all(response).unwrap();
            },
        );
        let (raw, audit, _) = exchange.finish(Vec::new());
        assert!(!String::from_utf8_lossy(&raw).contains("Transfer-Encoding"));
        assert!(!raw.starts_with(b"HTTP/1.1 200"));
        assert!(audit.contains("stream_aborted"));
    }
}

#[test]
fn client_disconnect_cancels_a_silent_provider() {
    let (cancel_send, cancel_receive) = mpsc::channel();
    let mut exchange = Exchange::start(
        false,
        |_| {},
        move |mut socket| {
            headers(&mut socket);
            socket
                .write_all(&chat(
                    json!({"content":"PRIVATE_OUTPUT_FIXTURE"}),
                    Value::Null,
                ))
                .unwrap();
            let mut byte = [0];
            cancel_send
                .send(socket.read(&mut byte).unwrap() == 0)
                .unwrap();
        },
    );
    exchange.read_until(b"PRIVATE_OUTPUT_FIXTURE");
    exchange.client.shutdown(Shutdown::Both).unwrap();
    assert!(cancel_receive.recv_timeout(Duration::from_secs(1)).unwrap());
    // Provider EOF above proves timely cancellation. Reading this deliberately
    // closed client again may return ConnectionReset instead of EOF.
    let (audit, _) = exchange.join();
    assert!(audit.contains("client disconnected"));
}

#[test]
fn first_event_and_idle_deadlines_close_stalled_exchanges() {
    for first in [true, false] {
        let exchange = Exchange::start(
            false,
            |cfg| {
                let streaming = &mut cfg.llm.as_mut().unwrap().streaming;
                streaming.first_event_timeout_seconds = 1;
                streaming.idle_timeout_seconds = 1;
            },
            move |mut socket| {
                headers(&mut socket);
                if first {
                    socket.write_all(b": heartbeat\n\n").unwrap();
                } else {
                    socket
                        .write_all(&chat(
                            json!({"content":"PRIVATE_OUTPUT_FIXTURE"}),
                            Value::Null,
                        ))
                        .unwrap();
                }
                let mut byte = [0];
                assert_eq!(socket.read(&mut byte).unwrap(), 0);
            },
        );
        let (raw, audit, _) = exchange.finish(Vec::new());
        if first {
            assert!(raw.starts_with(b"HTTP/1.1 504"));
        } else {
            assert!(raw.starts_with(b"HTTP/1.1 200"));
        }
        assert!(!String::from_utf8_lossy(&raw).contains("[DONE]"));
        assert!(audit.contains("stream_aborted"));
    }
}

#[test]
fn rejects_event_overflow_and_malformed_data_without_success() {
    for body in [
        format!("data: {}\n\n", "x".repeat(4096)).into_bytes(),
        b"data: {broken\n\n".to_vec(),
        b"data: \xff\n\n".to_vec(),
        b"data: [DONE]\n\n".to_vec(),
    ] {
        let exchange = Exchange::start(
            false,
            |_| {},
            move |mut socket| {
                headers(&mut socket);
                socket.write_all(&body).unwrap();
            },
        );
        let (raw, audit, _) = exchange.finish(Vec::new());
        assert!(raw.starts_with(b"HTTP/1.1 502"));
        assert!(audit.contains("stream_aborted"));
    }
}

#[test]
fn response_limit_and_audit_failure_truncate_partial_output() {
    for audit_failure in [true, false] {
        let (send, receive) = mpsc::channel();
        let mut exchange = Exchange::start(
            false,
            |_| {},
            move |mut socket| {
                headers(&mut socket);
                socket
                    .write_all(&chat(
                        json!({"content":"PRIVATE_OUTPUT_FIXTURE"}),
                        Value::Null,
                    ))
                    .unwrap();
                receive.recv_timeout(Duration::from_secs(3)).unwrap();
                if audit_failure {
                    socket.write_all(&chat(json!({}), json!("stop"))).unwrap();
                    let _ = socket.write_all(b"data: [DONE]\n\n");
                } else {
                    let _ = socket.write_all(&vec![b' '; 8193]);
                }
            },
        );
        let prefix = exchange.read_until(b"PRIVATE_OUTPUT_FIXTURE");
        if audit_failure {
            fs::set_permissions(
                exchange.directory.0.join("audit"),
                fs::Permissions::from_mode(0o644),
            )
            .unwrap();
        }
        send.send(()).unwrap();
        let (raw, audit, budget) = exchange.finish(prefix);
        assert!(!String::from_utf8_lossy(&raw).contains("[DONE]"));
        assert!(!raw.ends_with(b"0\r\n\r\n"));
        assert!(audit.contains("stream_aborted"));
        assert_eq!(budget["requests"], 1);
    }
}

#[test]
fn parser_accepts_sse_line_endings_bom_and_preserves_multiline_data() {
    for newline in ["\n", "\r\n", "\r"] {
        let input = format!(
            "\u{feff}event: response.created{newline}data: {{\"type\":{newline}data: \"response.created\"}}{newline}{newline}"
        );
        let mut parser = SseParser::new(4096);
        let mut protocol = Protocol::new(true);
        let mut count = 0;
        for byte in input.bytes() {
            if let Some(frame) = parser.push(byte).unwrap_or_else(|_| panic!("parse failed")) {
                assert!(
                    protocol
                        .inspect(&frame)
                        .unwrap_or_else(|_| panic!("inspect failed"))
                        .has_data
                );
                count += 1;
            }
        }
        assert_eq!(count, 1);
    }
}

#[test]
fn responses_failure_and_incomplete_are_forwarded_as_their_real_outcomes() {
    for outcome in ["failed", "incomplete"] {
        let exchange = Exchange::start(
            true,
            |_| {},
            move |mut socket| {
                headers(&mut socket);
                write!(socket, "event: response.{outcome}\ndata: {{\"type\":\"response.{outcome}\",\"response\":{{\"status\":\"{outcome}\",\"usage\":null}}}}\n\n").unwrap();
            },
        );
        let (raw, audit, _) = exchange.finish(Vec::new());
        assert!(String::from_utf8_lossy(&raw).contains(&format!("response.{outcome}")));
        assert!(!String::from_utf8_lossy(&raw).contains("response.completed"));
        assert!(audit.contains(&format!("\"outcome\":\"{outcome}\"")));
        assert!(audit.contains("\"usage\":null"));
    }
}

#[test]
fn whole_exchange_deadline_stops_active_heartbeats() {
    let exchange = Exchange::start(
        false,
        |cfg| {
            let policy = &mut cfg.llm.as_mut().unwrap().streaming;
            policy.first_event_timeout_seconds = 1;
            policy.idle_timeout_seconds = 1;
            policy.total_timeout_seconds = 1;
        },
        |mut socket| {
            headers(&mut socket);
            socket
                .write_all(&chat(
                    json!({"content":"PRIVATE_OUTPUT_FIXTURE"}),
                    Value::Null,
                ))
                .unwrap();
            for _ in 0..30 {
                if socket.write_all(b": heartbeat\n\n").is_err() {
                    return;
                }
                thread::sleep(Duration::from_millis(100));
            }
            panic!("stream outlived total deadline");
        },
    );
    let started = std::time::Instant::now();
    let (raw, audit, _) = exchange.finish(Vec::new());
    assert!(started.elapsed() < Duration::from_secs(2));
    assert!(!raw.ends_with(b"0\r\n\r\n"));
    assert!(audit.contains("stream_aborted"));
}

#[test]
fn admission_rejects_invalid_stream_policy_model_tools_and_budget_before_dispatch() {
    use crate::http::Request;
    use std::collections::HashMap;
    let directory = Directory::new();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let cfg = config(&directory, listener.local_addr().unwrap());
    let broker = Broker::new(cfg.clone()).unwrap();
    for (field, value, status) in [
        ("stream", json!("true"), 400),
        ("model", json!("unlisted-model"), 403),
        ("tools", json!([{"type":"web_search"}]), 403),
        ("n", json!(2), 400),
        ("background", json!(true), 400),
    ] {
        let mut payload = request(false);
        payload[field] = value;
        let response = broker.handle(Request {
            method: "POST".into(),
            path: "/v1/chat/completions".into(),
            headers: HashMap::from([
                ("authorization".into(), "Bearer virtual-agent-key".into()),
                ("content-type".into(), "application/json".into()),
            ]),
            body: serde_json::to_vec(&payload).unwrap(),
        });
        assert_eq!(response.status, status);
        assert!(response.streaming.is_none());
        assert!(!directory.0.join("budget").exists());
    }
    let mut limited = cfg;
    limited.daily_token_budget = 1;
    let broker = Broker::new(limited).unwrap();
    let response = broker.handle(Request {
        method: "POST".into(),
        path: "/v1/chat/completions".into(),
        headers: HashMap::from([
            ("authorization".into(), "Bearer virtual-agent-key".into()),
            ("content-type".into(), "application/json".into()),
        ]),
        body: serde_json::to_vec(&request(false)).unwrap(),
    });
    assert_eq!(response.status, 429);
    assert!(!directory.0.join("budget").exists());
    listener.set_nonblocking(true).unwrap();
    assert!(listener.accept().is_err());
    directory.audit();
}

#[test]
fn validates_streaming_defaults_limits_and_unknown_fields() {
    let directory = Directory::new();
    let address = "127.0.0.1:1".parse().unwrap();
    let cfg = config(&directory, address);
    assert!(!StreamingPolicy::default().enable);
    for bad in [
        "max_event_bytes",
        "first_event_timeout_seconds",
        "idle_timeout_seconds",
        "total_timeout_seconds",
    ] {
        let mut policy = json!({"enable":true, "max_event_bytes":4096});
        policy[bad] = json!(0);
        let mut changed = cfg.clone();
        changed.llm.as_mut().unwrap().streaming = serde_json::from_value(policy).unwrap();
        assert!(changed.validate().is_err());
    }
    assert!(serde_json::from_value::<StreamingPolicy>(json!({"enabled":true})).is_err());
    assert_eq!(
        sanitize_usage(Some(
            &json!({"input_tokens":1,"output_tokens":2,"total_tokens":4})
        )),
        Value::Null
    );
}

#[test]
fn concurrency_slot_survives_partial_output_and_is_released_on_disconnect() {
    use crate::{ConnectionSettings, dispatch_connection};
    use std::sync::atomic::AtomicUsize;
    let directory = Directory::new();
    let upstream = TcpListener::bind("127.0.0.1:0").unwrap();
    let broker = Arc::new(Broker::new(config(&directory, upstream.local_addr().unwrap())).unwrap());
    let provider = thread::spawn(move || {
        let (mut socket, _) = upstream.accept().unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        read_request(&mut socket, 32768, 1048576).unwrap();
        headers(&mut socket);
        socket
            .write_all(&chat(
                json!({"content":"PRIVATE_OUTPUT_FIXTURE"}),
                Value::Null,
            ))
            .unwrap();
        assert_eq!(socket.read(&mut [0]).unwrap(), 0);
    });
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let active = Arc::new(AtomicUsize::new(0));
    let settings = ConnectionSettings {
        timeout: Duration::from_secs(3),
        max_concurrency: 1,
        header_limit: 32768,
        body_limit: 1048576,
    };
    let mut first = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
    first
        .set_read_timeout(Some(Duration::from_secs(3)))
        .unwrap();
    let body = serde_json::to_vec(&request(false)).unwrap();
    write!(first, "POST /v1/chat/completions HTTP/1.1\r\nContent-Type: application/json\r\nAuthorization: Bearer virtual-agent-key\r\nContent-Length: {}\r\n\r\n", body.len()).unwrap();
    first.write_all(&body).unwrap();
    let (socket, _) = listener.accept().unwrap();
    let handler =
        dispatch_connection(socket, Arc::clone(&broker), Arc::clone(&active), settings).unwrap();
    let mut bytes = Vec::new();
    while !String::from_utf8_lossy(&bytes).contains("PRIVATE_OUTPUT_FIXTURE") {
        let mut byte = [0];
        assert_eq!(first.read(&mut byte).unwrap(), 1);
        bytes.push(byte[0]);
    }
    assert_eq!(active.load(Ordering::Acquire), 1);
    let mut second = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
    second
        .set_read_timeout(Some(Duration::from_secs(3)))
        .unwrap();
    let (socket, _) = listener.accept().unwrap();
    assert!(
        dispatch_connection(socket, Arc::clone(&broker), Arc::clone(&active), settings).is_none()
    );
    let mut denied = String::new();
    second.read_to_string(&mut denied).unwrap();
    assert!(denied.starts_with("HTTP/1.1 503"));
    assert_eq!(active.load(Ordering::Acquire), 1);
    first.shutdown(Shutdown::Both).unwrap();
    handler.join().unwrap();
    provider.join().unwrap();
    assert_eq!(active.load(Ordering::Acquire), 0);
    let mut third = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
    third
        .set_read_timeout(Some(Duration::from_secs(3)))
        .unwrap();
    third.write_all(b"GET /healthz HTTP/1.1\r\n\r\n").unwrap();
    let (socket, _) = listener.accept().unwrap();
    let handler = dispatch_connection(socket, broker, Arc::clone(&active), settings).unwrap();
    let mut ready = String::new();
    third.read_to_string(&mut ready).unwrap();
    handler.join().unwrap();
    assert!(ready.starts_with("HTTP/1.1 200"));
    assert_eq!(active.load(Ordering::Acquire), 0);
    directory.audit();
}

#[test]
fn client_disconnect_cancels_dispatch_before_provider_headers() {
    let (ready_send, ready_receive) = mpsc::channel();
    let (cancel_send, cancel_receive) = mpsc::channel();
    let exchange = Exchange::start(
        false,
        |_| {},
        move |mut socket| {
            ready_send.send(()).unwrap();
            assert_eq!(socket.read(&mut [0]).unwrap(), 0);
            cancel_send.send(()).unwrap();
        },
    );
    ready_receive.recv_timeout(Duration::from_secs(1)).unwrap();
    exchange.client.shutdown(Shutdown::Both).unwrap();
    cancel_receive.recv_timeout(Duration::from_secs(1)).unwrap();
    let (_, audit, budget) = exchange.finish(Vec::new());
    assert!(audit.contains("client disconnected"));
    assert_eq!(budget["requests"], 1);
}

#[test]
fn downstream_backpressure_has_a_bounded_write_deadline() {
    let mut exchange = Exchange::start(
        false,
        |cfg| {
            cfg.max_response_bytes = 32 * 1024 * 1024;
            let policy = &mut cfg.llm.as_mut().unwrap().streaming;
            policy.max_event_bytes = 64 * 1024;
            policy.idle_timeout_seconds = 1;
        },
        |mut socket| {
            headers(&mut socket);
            let frame = chat(json!({"content":"x".repeat(60000)}), Value::Null);
            for _ in 0..256 {
                if socket.write_all(&frame).is_err() {
                    return;
                }
            }
        },
    );
    exchange.read_until(b"\r\n\r\n");
    // Keep the client connected without reading the body until the handler exits.
    let started = std::time::Instant::now();
    exchange.broker.join().unwrap();
    assert!(started.elapsed() < Duration::from_secs(3));
    exchange.provider.join().unwrap();
    let mut raw = Vec::new();
    exchange.client.read_to_end(&mut raw).unwrap();
    assert!(!raw.ends_with(b"0\r\n\r\n"));
    assert!(
        exchange
            .directory
            .audit()
            .contains("stream deadline exceeded")
    );
}

#[test]
fn responses_terminal_must_match_event_type_and_status() {
    for frame in [
        b"event: response.completed\ndata: {\"type\":\"response.failed\",\"response\":{\"status\":\"failed\"}}\n\n".as_slice(),
        b"data: {\"type\":\"response.completed\",\"response\":{\"status\":\"incomplete\"}}\n\n".as_slice(),
        b"data: [DONE]\n\n".as_slice(),
    ] {
        let exchange = Exchange::start(true, |_| {}, move |mut socket| {
            headers(&mut socket);
            socket.write_all(frame).unwrap();
        });
        let (raw, audit, _) = exchange.finish(Vec::new());
        assert!(raw.starts_with(b"HTTP/1.1 502"));
        assert!(!audit.contains("\"outcome\":\"completed\""));
    }
}
