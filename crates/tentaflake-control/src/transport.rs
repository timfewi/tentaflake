use crate::{
    ApiError, Config, Engine,
    storage::{self, MAX_DOCUMENT, Result},
};
use serde_json::{Value, json};
use std::io::{Read, Write};
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const MAX_HEADERS: usize = 8192;
const MAX_RESPONSE: usize = 2 * MAX_DOCUMENT + 65536;
const IO_DEADLINE: Duration = Duration::from_secs(5);

pub fn peer_uid(stream: &UnixStream) -> Result<u32> {
    let mut credential: libc::ucred = unsafe { std::mem::zeroed() };
    let mut size = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    if unsafe {
        libc::getsockopt(
            stream.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            &mut credential as *mut _ as *mut libc::c_void,
            &mut size,
        )
    } != 0
        || size as usize != std::mem::size_of::<libc::ucred>()
    {
        return Err("peer credentials unavailable".into());
    }
    Ok(credential.uid)
}
fn setup(stream: &UnixStream) -> Result<()> {
    stream
        .set_read_timeout(Some(Duration::from_millis(200)))
        .map_err(|_| "socket unavailable")?;
    stream
        .set_write_timeout(Some(IO_DEADLINE))
        .map_err(|_| "socket unavailable".into())
}
fn read_bytes(
    stream: &mut UnixStream,
    buffer: &mut Vec<u8>,
    length: usize,
    start: Instant,
    deadline: Duration,
) -> Result<()> {
    let mut part = [0u8; 4096];
    while buffer.len() < length {
        if start.elapsed() >= deadline {
            return Err("request deadline exceeded".into());
        }
        let remaining = (length - buffer.len()).min(part.len());
        match stream.read(&mut part[..remaining]) {
            Ok(0) => return Err("incomplete HTTP message".into()),
            Ok(n) => buffer.extend_from_slice(&part[..n]),
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock
                        | std::io::ErrorKind::TimedOut
                        | std::io::ErrorKind::Interrupted
                ) => {}
            Err(_) => return Err("HTTP transport unavailable".into()),
        }
    }
    Ok(())
}
struct Message {
    first: String,
    body: Vec<u8>,
}
fn read_message(stream: &mut UnixStream, limit: usize, request: bool) -> Result<Message> {
    let start = Instant::now();
    let deadline = if request {
        IO_DEADLINE
    } else {
        Duration::from_secs(30)
    };
    let mut header = Vec::new();
    // Exact header reads prevent accidental consumption of a pipelined request/body.
    while !header.ends_with(b"\r\n\r\n") {
        if header.len() >= MAX_HEADERS {
            return Err("headers exceed limit".into());
        }
        let next = header.len() + 1;
        read_bytes(stream, &mut header, next, start, deadline)?;
    }
    let text = std::str::from_utf8(&header).map_err(|_| "invalid HTTP header")?;
    let mut lines = text[..text.len() - 4].split("\r\n");
    let first = lines.next().ok_or("missing HTTP line")?.to_owned();
    let mut length = None;
    let mut content_type = false;
    let mut names = std::collections::BTreeSet::new();
    for line in lines {
        let (key, value) = line.split_once(':').ok_or("invalid HTTP header")?;
        if key.is_empty()
            || !key.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
            || value.bytes().any(|b| b < 32 || b == 127)
        {
            return Err("invalid HTTP header".into());
        }
        let key = key.to_ascii_lowercase();
        if !names.insert(key.clone()) {
            return Err("duplicate HTTP header".into());
        }
        let value = value.trim();
        match key.as_str() {
            "content-length" => {
                if value.is_empty() || !value.bytes().all(|b| b.is_ascii_digit()) {
                    return Err("invalid content length".into());
                }
                length = Some(
                    value
                        .parse::<usize>()
                        .map_err(|_| "invalid content length")?,
                );
            }
            "content-type" => {
                if value != "application/json" {
                    return Err("unsupported content type".into());
                }
                content_type = true;
            }
            "connection" if value.eq_ignore_ascii_case("close") => (),
            "host" if !value.is_empty() => (),
            "accept" if value == "application/json" => (),
            _ => return Err("unsupported HTTP header".into()),
        }
    }
    let length = length
        .or_else(|| (request && first.starts_with("GET ")).then_some(0))
        .ok_or("content length required")?;
    if length > limit || (length > 0 && !content_type) {
        return Err("invalid body length or type".into());
    }
    if request && !first.starts_with("GET ") && !first.starts_with("POST ") {
        return Err("unsupported method".into());
    }
    let mut body = Vec::with_capacity(length);
    read_bytes(stream, &mut body, length, start, deadline)?;
    Ok(Message { first, body })
}
fn reply(stream: &mut UnixStream, status: u16, value: &Value) -> Result<()> {
    let body = serde_json::to_vec(value).map_err(|_| "response unavailable")?;
    if body.len() > MAX_RESPONSE {
        return Err("response exceeds limit".into());
    }
    let reason = if status == 200 {
        "OK"
    } else {
        "Request Refused"
    };
    write!(stream, "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len()).and_then(|_| stream.write_all(&body)).map_err(|_| "response unavailable".into())
}
pub fn serve_connection(mut stream: UnixStream, engine: &Arc<Mutex<Engine>>) -> Result<()> {
    setup(&stream)?;
    let uid = peer_uid(&stream)?;
    // Reject unknown peers before spending any request parsing/normalizer resources.
    let known = engine
        .lock()
        .map_err(|_| "service unavailable")?
        .config
        .peers
        .iter()
        .any(|p| p.uid == uid);
    let response = if !known {
        Err(ApiError {
            status: 403,
            code: "forbidden",
        })
    } else {
        match read_message(&mut stream, MAX_DOCUMENT, true) {
            Ok(message) => {
                let parts: Vec<_> = message.first.split(' ').collect();
                if parts.len() != 3 || parts[2] != "HTTP/1.1" || parts[1].len() > 256 {
                    Err(ApiError {
                        status: 400,
                        code: "invalid_request",
                    })
                } else {
                    engine.lock().map_err(|_| "service unavailable")?.handle(
                        uid,
                        parts[0],
                        parts[1],
                        &message.body,
                    )
                }
            }
            Err(_) => Err(ApiError {
                status: 400,
                code: "invalid_request",
            }),
        }
    };
    match response {
        Ok(value) => reply(&mut stream, 200, &value),
        Err(error) => reply(
            &mut stream,
            error.status,
            &json!({"schema_version":1,"error":{"code":error.code,"message":error.code}}),
        ),
    }
}
pub fn serve(engine: Engine) -> Result<()> {
    let parent = engine.config.socket.parent().ok_or("invalid socket path")?;
    storage::secure_ancestry(parent)?;
    let dir = storage::open_path(parent, true)?;
    let meta = dir.metadata().map_err(|_| "socket directory unavailable")?;
    // Search-only access for other UIDs is allowed; no other principal may rename the socket.
    if meta.uid() != engine.config.server_uid || meta.mode() & 0o022 != 0 {
        return Err("unsafe socket directory".into());
    }
    match std::fs::symlink_metadata(&engine.config.socket) {
        Ok(meta) if meta.file_type().is_socket() && meta.uid() == engine.config.server_uid => {
            std::fs::remove_file(&engine.config.socket).map_err(|_| "stale socket unavailable")?;
        }
        Ok(_) => return Err("unsafe existing socket".into()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
        Err(_) => return Err("socket unavailable".into()),
    }
    let listener = UnixListener::bind(&engine.config.socket).map_err(|_| "socket bind failed")?;
    // DAC permits connection only; every operation still requires exact SO_PEERCRED UID/role.
    std::fs::set_permissions(
        &engine.config.socket,
        std::fs::Permissions::from_mode(0o666),
    )
    .map_err(|_| "socket permissions unavailable")?;
    let engine = Arc::new(Mutex::new(engine));
    let worker = Arc::clone(&engine);
    std::thread::spawn(move || crate::worker(worker));
    // One HTTP connection at a time, one builder, four durable queue slots. Kernel backlog is bounded.
    if unsafe { libc::listen(listener.as_raw_fd(), 16) } != 0 {
        return Err("socket backlog unavailable".into());
    }
    for stream in listener.incoming() {
        let stream = stream.map_err(|_| "listener unavailable")?;
        let _ = serve_connection(stream, &engine);
    }
    Err("listener stopped".into())
}
fn connect(path: &std::path::Path) -> Result<UnixStream> {
    let bytes = path.as_os_str().as_encoded_bytes();
    let mut address: libc::sockaddr_un = unsafe { std::mem::zeroed() };
    if !path.is_absolute() || bytes.len() >= address.sun_path.len() || bytes.contains(&0) {
        return Err("invalid deployment socket".into());
    }
    address.sun_family = libc::AF_UNIX as libc::sa_family_t;
    for (slot, byte) in address.sun_path.iter_mut().zip(bytes) {
        *slot = *byte as libc::c_char;
    }
    let fd = unsafe {
        libc::socket(
            libc::AF_UNIX,
            libc::SOCK_STREAM | libc::SOCK_CLOEXEC | libc::SOCK_NONBLOCK,
            0,
        )
    };
    if fd < 0 {
        return Err("control service unavailable".into());
    }
    let stream = unsafe { UnixStream::from_raw_fd(fd) };
    if unsafe {
        libc::connect(
            fd,
            &address as *const _ as *const libc::sockaddr,
            std::mem::size_of_val(&address) as libc::socklen_t,
        )
    } != 0
    {
        if std::io::Error::last_os_error().raw_os_error() != Some(libc::EINPROGRESS) {
            // AF_UNIX reports EAGAIN for a full backlog; do not wait or retry indefinitely.
            return Err("control service unavailable".into());
        }
        let mut poll = libc::pollfd {
            fd,
            events: libc::POLLOUT,
            revents: 0,
        };
        if unsafe { libc::poll(&mut poll, 1, 5000) } <= 0 {
            return Err("control connection deadline".into());
        }
        let mut error: libc::c_int = 0;
        let mut size = std::mem::size_of_val(&error) as libc::socklen_t;
        if unsafe {
            libc::getsockopt(
                fd,
                libc::SOL_SOCKET,
                libc::SO_ERROR,
                &mut error as *mut _ as *mut libc::c_void,
                &mut size,
            )
        } != 0
            || error != 0
        {
            return Err("control service unavailable".into());
        }
    }
    stream
        .set_nonblocking(false)
        .map_err(|_| "control service unavailable")?;
    Ok(stream)
}
/// Deployment selects the socket and expected server UID, never browser request data.
pub fn client(config: &Config, method: &str, route: &str, body: &[u8]) -> Result<(u16, Value)> {
    let allowed = (method == "GET"
        && (matches!(route, "/v1/system" | "/v1/system/source")
            || route
                .strip_prefix("/v1/system/builds/")
                .is_some_and(crate::identifier)))
        || (method == "POST"
            && (matches!(
                route,
                "/v1/system/proposals" | "/v1/system/builds" | "/v1/system/validate"
            ) || route
                .strip_prefix("/v1/system/proposals/")
                .and_then(|s| s.strip_suffix("/approve"))
                .is_some_and(crate::identifier)));
    if !allowed || body.len() > MAX_DOCUMENT {
        return Err("request route or size refused".into());
    }
    let mut stream = connect(&config.socket)?;
    setup(&stream)?;
    if peer_uid(&stream)? != config.server_uid {
        return Err("unexpected control server UID".into());
    }
    write!(stream, "{method} {route} HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len()).and_then(|_| stream.write_all(body)).map_err(|_| "request unavailable")?;
    let message = read_message(&mut stream, MAX_RESPONSE, false)?;
    let parts: Vec<_> = message.first.splitn(3, ' ').collect();
    if parts.len() != 3 || parts[0] != "HTTP/1.1" {
        return Err("invalid control response".into());
    }
    let status = parts[1]
        .parse::<u16>()
        .map_err(|_| "invalid control response")?;
    let value: Value =
        serde_json::from_slice(&message.body).map_err(|_| "invalid control response")?;
    if value.get("schema_version") != Some(&json!(1)) {
        return Err("unsupported control response".into());
    }
    Ok((status, value))
}
