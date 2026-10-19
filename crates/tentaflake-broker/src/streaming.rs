//! Bounded, single-dispatch SSE forwarding. No provider body is written to audit.
use crate::config::StreamingPolicy;
use crate::policy::ResolvedTarget;
use crate::state::Audit;
use reqwest::header::{ACCEPT, CONTENT_TYPE};
use serde_json::{Value, json};
use std::future::{Future, poll_fn};
use std::io;
use std::net::{Shutdown, TcpStream};
use std::pin::pin;
use std::sync::Arc;
use std::task::Poll;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::time::{Instant, timeout_at};

pub struct StreamingResponse {
    pub target: ResolvedTarget,
    pub credential: String,
    pub session_id: Option<String>,
    pub body: Vec<u8>,
    pub policy: StreamingPolicy,
    pub connect_timeout_seconds: u64,
    pub max_response_bytes: usize,
    pub audit: Arc<Audit>,
    pub agent: String,
    pub route: String,
    pub model: String,
}

struct Failure {
    status: u16,
    reason: &'static str,
}

impl Failure {
    fn upstream(reason: &'static str) -> Self {
        Self {
            status: 502,
            reason,
        }
    }
}

// Poll the client alongside *every* network wait, including request dispatch,
// silent providers and backpressure. Dropping the future cancels the upstream.
async fn guarded<T>(
    reader: &mut OwnedReadHalf,
    deadline: Instant,
    operation: impl Future<Output = Result<T, Failure>>,
) -> Result<T, Failure> {
    let mut byte = [0];
    let mut disconnect = pin!(reader.read(&mut byte));
    let mut operation = pin!(operation);
    timeout_at(
        deadline,
        poll_fn(|cx| {
            if disconnect.as_mut().poll(cx).is_ready() {
                return Poll::Ready(Err(Failure {
                    status: 0,
                    reason: "client disconnected or sent additional request data",
                }));
            }
            operation.as_mut().poll(cx)
        }),
    )
    .await
    .map_err(|_| Failure {
        status: 504,
        reason: "stream deadline exceeded",
    })?
}

async fn write_bytes(
    reader: &mut OwnedReadHalf,
    writer: &mut OwnedWriteHalf,
    deadline: Instant,
    bytes: &[u8],
) -> Result<(), Failure> {
    guarded(reader, deadline, async {
        writer.write_all(bytes).await.map_err(|_| Failure {
            status: 0,
            reason: "client write failed",
        })
    })
    .await
}

impl StreamingResponse {
    pub fn write_to(self, socket: &mut TcpStream) -> io::Result<()> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        let cloned = socket.try_clone()?;
        cloned.set_nonblocking(true)?;
        let result = runtime.block_on(async {
            let socket = tokio::net::TcpStream::from_std(cloned)?;
            let (mut reader, mut writer) = socket.into_split();
            let mut headers_sent = false;
            let started = Instant::now();
            let total = started + Duration::from_secs(self.policy.total_timeout_seconds);
            if let Err(failure) = self.relay(&mut reader, &mut writer, &mut headers_sent, started).await {
                // Reasons are static broker strings, never upstream error text.
                let _ = self.audit.record(&self.agent, "llm", "stream_aborted", json!({
                    "route": self.route, "model": self.model,
                    "provider": self.target.host, "reason": failure.reason,
                }));
                if !headers_sent && failure.status != 0 && Instant::now() < total {
                    let body = serde_json::to_vec(&json!({"error": failure.reason})).unwrap();
                    let header = format!(
                        "HTTP/1.1 {} Error\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\nCache-Control: no-store\r\nX-Content-Type-Options: nosniff\r\n\r\n",
                        failure.status, body.len()
                    );
                    let deadline = total.min(Instant::now() + Duration::from_secs(self.policy.idle_timeout_seconds));
                    let _ = write_bytes(&mut reader, &mut writer, deadline, header.as_bytes()).await;
                    let _ = write_bytes(&mut reader, &mut writer, deadline, &body).await;
                }
                // After headers, close a truncated chunked body. Never fabricate
                // [DONE], a Responses terminal event, or a successful HTTP end.
                return Err(io::Error::other(failure.reason));
            }
            Ok(())
        });
        let _ = socket.shutdown(Shutdown::Both);
        result
    }

    async fn relay(
        &self,
        reader: &mut OwnedReadHalf,
        writer: &mut OwnedWriteHalf,
        headers_sent: &mut bool,
        started: Instant,
    ) -> Result<(), Failure> {
        let total = started + Duration::from_secs(self.policy.total_timeout_seconds);
        let first =
            total.min(started + Duration::from_secs(self.policy.first_event_timeout_seconds));
        let idle = Duration::from_secs(self.policy.idle_timeout_seconds);
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .no_proxy()
            .connect_timeout(Duration::from_secs(self.connect_timeout_seconds))
            .read_timeout(idle)
            .timeout(Duration::from_secs(self.policy.total_timeout_seconds))
            .resolve_to_addrs(&self.target.host, &self.target.addresses)
            .build()
            .map_err(|_| Failure::upstream("cannot construct streaming client"))?;
        self.audit.check_ready().map_err(|_| Failure {
            status: 503,
            reason: "audit log is unavailable",
        })?;
        let mut response = guarded(reader, first, async {
            let mut upstream = client
                .post(self.target.url.clone())
                .bearer_auth(&self.credential)
                .header(CONTENT_TYPE, "application/json")
                .header(ACCEPT, "text/event-stream")
                .body(self.body.clone());
            if let Some(session) = &self.session_id {
                upstream = upstream.header("x-opencode-session", session);
            }
            upstream.send().await.map_err(|error| Failure {
                status: if error.is_timeout() { 504 } else { 502 },
                reason: "provider streaming request failed",
            })
        })
        .await?;
        if !response.status().is_success() {
            return Err(Failure {
                status: response.status().as_u16(),
                reason: "provider rejected streaming request",
            });
        }
        let mime = response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.split(';').next())
            .unwrap_or("");
        if !mime.trim().eq_ignore_ascii_case("text/event-stream") {
            return Err(Failure::upstream("provider did not return an event stream"));
        }
        if response
            .content_length()
            .is_some_and(|size| size > self.max_response_bytes as u64)
        {
            return Err(Failure::upstream(
                "stream exceeds configured response limit",
            ));
        }
        let mut parser = SseParser::new(self.policy.max_event_bytes);
        let mut protocol = Protocol::new(self.route == "/v1/responses");
        let mut received = 0_usize;
        loop {
            let deadline = total.min(Instant::now() + idle);
            let deadline = if *headers_sent {
                deadline
            } else {
                deadline.min(first)
            };
            let chunk = guarded(reader, deadline, async {
                response.chunk().await.map_err(|error| Failure {
                    status: if error.is_timeout() { 504 } else { 502 },
                    reason: "provider stream read failed",
                })
            })
            .await?
            .ok_or_else(|| Failure::upstream("provider stream ended without a terminal event"))?;
            received = received
                .checked_add(chunk.len())
                .filter(|size| *size <= self.max_response_bytes)
                .ok_or_else(|| Failure::upstream("stream exceeds configured response limit"))?;
            for byte in chunk {
                let Some(frame) = parser.push(byte)? else {
                    continue;
                };
                let event = protocol.inspect(&frame)?;
                if !*headers_sent && !event.has_data {
                    continue; // Heartbeats cannot satisfy the first-event deadline.
                }
                self.audit.check_ready().map_err(|_| Failure {
                    status: 503,
                    reason: "audit log is unavailable",
                })?;
                let deadline = total.min(Instant::now() + idle);
                if !*headers_sent {
                    // Mark before writing: a partial header cannot become a JSON response.
                    *headers_sent = true;
                    write_bytes(reader, writer, deadline, b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\nConnection: close\r\nCache-Control: no-store\r\nX-Content-Type-Options: nosniff\r\n\r\n").await?;
                }
                if let Some(outcome) = event.terminal {
                    // Persist completion admission before releasing the terminal marker.
                    self.audit.record(&self.agent, "llm", outcome, json!({
                        "route": self.route, "provider": self.target.host,
                        "model": self.model, "streaming": true, "status": 200,
                        "response_bytes": received, "latency_ms": started.elapsed().as_millis(),
                        "usage": protocol.usage,
                    })).map_err(|_| Failure { status: 503, reason: "audit log is unavailable" })?;
                }
                let prefix = format!("{:x}\r\n", frame.len());
                write_bytes(reader, writer, deadline, prefix.as_bytes()).await?;
                write_bytes(reader, writer, deadline, &frame).await?;
                write_bytes(reader, writer, deadline, b"\r\n").await?;
                if event.terminal.is_some() {
                    write_bytes(reader, writer, deadline, b"0\r\n\r\n").await?;
                    return Ok(());
                }
            }
        }
    }
}

// Normalize SSE line endings while retaining field/data semantics. The only
// retained content is one bounded event, never the entire model response.
struct SseParser {
    frame: Vec<u8>,
    line_bytes: usize,
    skip_lf: bool,
    raw_bytes: usize,
    limit: usize,
}

impl SseParser {
    fn new(limit: usize) -> Self {
        Self {
            frame: Vec::new(),
            line_bytes: 0,
            skip_lf: false,
            raw_bytes: 0,
            limit,
        }
    }

    fn push(&mut self, byte: u8) -> Result<Option<Vec<u8>>, Failure> {
        if self.skip_lf {
            self.skip_lf = false;
            if byte == b'\n' {
                return Ok(None);
            }
        }
        self.raw_bytes += 1;
        if self.raw_bytes > self.limit {
            return Err(Failure::upstream("event exceeds configured event limit"));
        }
        if byte == b'\r' || byte == b'\n' {
            self.skip_lf = byte == b'\r';
            self.frame.push(b'\n');
            let empty = self.line_bytes == 0;
            self.line_bytes = 0;
            if empty {
                self.raw_bytes = 0;
                return Ok(Some(std::mem::take(&mut self.frame)));
            }
        } else {
            self.frame.push(byte);
            self.line_bytes += 1;
        }
        Ok(None)
    }
}

struct Event {
    has_data: bool,
    terminal: Option<&'static str>,
}

struct Protocol {
    responses: bool,
    chat_finished: bool,
    usage: Value,
    first_frame: bool,
}

impl Protocol {
    fn new(responses: bool) -> Self {
        Self {
            responses,
            chat_finished: false,
            usage: Value::Null,
            first_frame: true,
        }
    }

    fn inspect(&mut self, frame: &[u8]) -> Result<Event, Failure> {
        let text =
            std::str::from_utf8(frame).map_err(|_| Failure::upstream("event is not UTF-8"))?;
        let text = if self.first_frame {
            text.trim_start_matches('\u{feff}')
        } else {
            text
        };
        self.first_frame = false;
        let mut data = Vec::new();
        let mut name = None;
        for line in text.lines() {
            let (field, value) = line.split_once(':').unwrap_or((line, ""));
            let value = value.strip_prefix(' ').unwrap_or(value);
            match field {
                "data" => data.push(value),
                "event" => name = Some(value),
                _ => {}
            }
        }
        if data.is_empty() || data.iter().all(|part| part.is_empty()) {
            return Ok(Event {
                has_data: false,
                terminal: None,
            });
        }
        let data = data.join("\n");
        if !self.responses && data == "[DONE]" {
            if !self.chat_finished {
                return Err(Failure::upstream(
                    "completion ended without a finish reason",
                ));
            }
            return Ok(Event {
                has_data: true,
                terminal: Some("completed"),
            });
        }
        let value: Value = serde_json::from_str(&data)
            .map_err(|_| Failure::upstream("event data is not valid JSON"))?;
        if !value.is_object() {
            return Err(Failure::upstream("event data must be a JSON object"));
        }
        let terminal = if self.responses {
            let kind = value
                .get("type")
                .and_then(Value::as_str)
                .ok_or_else(|| Failure::upstream("Responses event has no type"))?;
            if name.is_some_and(|name| name != kind) {
                return Err(Failure::upstream(
                    "Responses event type does not match its SSE field",
                ));
            }
            let outcome = match kind {
                "response.completed" => Some("completed"),
                "response.failed" => Some("failed"),
                "response.incomplete" => Some("incomplete"),
                "error" => Some("failed"),
                _ => None,
            };
            if outcome.is_some() && kind != "error" {
                if value.pointer("/response/status").and_then(Value::as_str) != outcome {
                    return Err(Failure::upstream("Responses terminal status is invalid"));
                }
                self.usage = sanitize_usage(value.pointer("/response/usage"));
            }
            outcome
        } else if value.get("error").is_some() || name == Some("error") {
            Some("failed")
        } else {
            if value.get("object").and_then(Value::as_str) != Some("chat.completion.chunk") {
                return Err(Failure::upstream(
                    "provider returned an invalid completion chunk",
                ));
            }
            let choices = value
                .get("choices")
                .and_then(Value::as_array)
                .ok_or_else(|| Failure::upstream("completion chunk has no choices"))?;
            for choice in choices {
                if choice.get("index").and_then(Value::as_u64) != Some(0) {
                    return Err(Failure::upstream(
                        "completion choice is outside the requested limit",
                    ));
                }
                match choice.get("finish_reason") {
                    None | Some(Value::Null) => {}
                    Some(Value::String(reason))
                        if matches!(
                            reason.as_str(),
                            "stop" | "length" | "tool_calls" | "content_filter" | "function_call"
                        ) =>
                    {
                        self.chat_finished = true
                    }
                    Some(_) => {
                        return Err(Failure::upstream("completion finish reason is invalid"));
                    }
                }
            }
            if let Some(usage) = value.get("usage").filter(|usage| !usage.is_null()) {
                self.usage = sanitize_usage(Some(usage));
            }
            None
        };
        Ok(Event {
            has_data: true,
            terminal,
        })
    }
}

fn sanitize_usage(usage: Option<&Value>) -> Value {
    let Some(value) = usage else {
        return Value::Null;
    };
    let input = value
        .get("input_tokens")
        .or_else(|| value.get("prompt_tokens"))
        .and_then(Value::as_u64);
    let output = value
        .get("output_tokens")
        .or_else(|| value.get("completion_tokens"))
        .and_then(Value::as_u64);
    let total = value.get("total_tokens").and_then(Value::as_u64);
    match (input, output, total) {
        (Some(input), Some(output), Some(total)) if input.checked_add(output) == Some(total) => {
            json!({ "input_tokens": input, "output_tokens": output, "total_tokens": total })
        }
        _ => Value::Null,
    }
}

#[cfg(test)]
mod tests;
