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
        json!({"object": "chat.completion.chunk", "id": "fixture",
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
        let directory = Directory::new();
        let upstream = TcpListener::bind("127.0.0.1:0").unwrap();
        let mut cfg = config(&directory, upstream.local_addr().unwrap());
        adjust(&mut cfg);
        cfg.validate().unwrap();
        let broker = Arc::new(Broker::new(cfg).unwrap());
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
        write!(client, "POST /v1/{route} HTTP/1.1\r\nContent-Type: application/json\r\nAuthorization: Bearer virtual-agent-key\r\nContent-Length: {}\r\n\r\n", body.len()).unwrap();
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
        self.broker.join().unwrap();
        self.provider.join().unwrap();
        let audit = self.directory.audit();
        let budget =
            serde_json::from_slice(&fs::read(self.directory.0.join("budget")).unwrap()).unwrap();
        (prefix, audit, budget)
    }
}

fn headers(socket: &mut TcpStream) {
    socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream; charset=utf-8\r\nConnection: close\r\n\r\n").unwrap();
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
    let (_, audit, _) = exchange.finish(Vec::new());
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
