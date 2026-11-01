//! Optional, minimized telemetry. A publication failure never authorizes replay.
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::io::{Read, Write};
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::net::UnixStream;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

type Result<T, E = &'static str> = std::result::Result<T, E>;

pub fn require_root_config(file: &std::fs::File) -> Result<()> {
    use std::os::unix::fs::MetadataExt;
    let meta = file.metadata().map_err(|_| "activity config metadata")?;
    if !meta.is_file() || meta.uid() != 0 || meta.mode() & 0o022 != 0 {
        return Err("activity requires root-owned non-writable configuration");
    }
    Ok(())
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub socket: String,
    pub server_uid: u32,
    #[serde(default)]
    pub relay_socket: Option<String>,
    pub producer_uid: u32,
    pub producer_id: String,
    pub tenant_id: String,
    pub agent_id: String,
    /// Fixed host context, NOT evidence of a native runtime session.
    pub run_id: String,
    pub workload_id: String,
    pub generation: u64,
    pub policy_revision: String,
}
pub fn identifier(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 64
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b))
}
fn socket_path(s: &str) -> bool {
    s.starts_with("/run/")
        && s.len() < 108
        && s.split('/').skip(1).all(|part| {
            !part.is_empty()
                && !matches!(part, "." | "..")
                && part
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b))
        })
}
impl Config {
    pub fn validate(&self, agent: &str) -> Result<()> {
        let agent_id = if agent.starts_with("agent-") {
            agent.to_owned()
        } else {
            format!("agent-{agent}")
        };
        if self.server_uid == 0
            || self.producer_uid == 0
            || self.agent_id != agent_id
            || self.generation == 0
            || self.generation > 9_007_199_254_740_991
            || ![
                &self.producer_id,
                &self.tenant_id,
                &self.agent_id,
                &self.run_id,
                &self.workload_id,
                &self.policy_revision,
            ]
            .into_iter()
            .all(|s| identifier(s))
            || !socket_path(&self.socket)
        {
            return Err("invalid activity context");
        }
        if self.relay_socket.as_ref().is_some_and(|s| !socket_path(s)) {
            return Err("invalid activity relay");
        }
        Ok(())
    }
    pub fn emit(
        &self,
        operation_id: &str,
        operation: Operation,
        outcome: Outcome,
        duration: Duration,
        usage: u64,
    ) -> Result<()> {
        self.emit_until(
            operation_id,
            operation,
            outcome,
            duration,
            usage,
            Instant::now() + Duration::from_millis(100),
        )
    }
    fn emit_until(
        &self,
        operation_id: &str,
        operation: Operation,
        outcome: Outcome,
        duration: Duration,
        usage: u64,
        deadline: Instant,
    ) -> Result<()> {
        if !identifier(operation_id) {
            return Err("activity operation denied");
        }
        if let Some(socket) = &self.relay_socket {
            if unsafe { libc::geteuid() } != 0 {
                return Err("activity relay caller denied");
            }
            return exchange(
                socket,
                self.producer_uid,
                &json!({"operation_id":operation_id,"operation":operation,"outcome":outcome,"duration_ms":duration.as_millis().min(86_400_000) as u64,"usage":usage.min(1_000_000_000_000u64)}),
                Duration::from_millis(250),
            );
        }
        if unsafe { libc::geteuid() } != self.producer_uid {
            return Err("activity producer denied");
        }
        let event_id = unique_id()?;
        let occurred = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| "activity clock")?
            .as_millis()
            .min(9_007_199_254_740_991) as u64;
        let message = json!({"operation":"publish_activity","version":1,"event":{
            "version":1,"producer_id":self.producer_id,"event_id":event_id,"operation_id":operation_id,
            "tenant_id":self.tenant_id,"agent_id":self.agent_id,"run_id":self.run_id,"workload_id":self.workload_id,
            "generation":self.generation,"policy_revision":self.policy_revision,"operation":operation,
            "decision":if outcome == Outcome::Denied {"deny"} else {"allow"},"outcome":outcome,
            "occurred_at_ms":occurred,"duration_ms":duration.as_millis().min(86_400_000) as u64,"usage":usage.min(1_000_000_000_000u64)
        }});
        exchange_until(&self.socket, self.server_uid, &message, deadline)
    }
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Operation {
    Model,
    Tool,
    Enforcement,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    Succeeded,
    Failed,
    Uncertain,
    Denied,
}
/// Kernel randomness supplies restart-independent IDs without any caller payload.
pub fn unique_id() -> Result<String> {
    let mut bytes = [0u8; 16];
    let count =
        unsafe { libc::getrandom(bytes.as_mut_ptr().cast(), bytes.len(), libc::GRND_NONBLOCK) };
    if count != bytes.len() as isize {
        return Err("activity identity unavailable");
    }
    use std::fmt::Write;
    let mut id = String::with_capacity(32);
    for b in bytes {
        let _ = write!(id, "{b:02x}");
    }
    Ok(id)
}
/// One request owns one guard, transferred to streaming output if necessary.
/// Unwinding/disconnection after dispatch remains uncertain, never success.
pub struct Guard {
    config: Config,
    pub operation_id: String,
    start: Instant,
    outcome: Outcome,
}
impl Guard {
    pub fn new(config: &Config) -> Result<Self> {
        Ok(Self {
            config: config.clone(),
            operation_id: unique_id()?,
            start: Instant::now(),
            outcome: Outcome::Denied,
        })
    }
    pub fn dispatched(&mut self) {
        self.outcome = Outcome::Uncertain;
    }
    pub fn finish(&mut self, outcome: Outcome) {
        self.outcome = outcome;
    }
}
impl Drop for Guard {
    fn drop(&mut self) {
        if self
            .config
            .emit(
                &self.operation_id,
                Operation::Model,
                self.outcome,
                self.start.elapsed(),
                0,
            )
            .is_err()
        {
            eprintln!("tentaflake activity telemetry_gap");
        }
    }
}
fn ready(stream: &UnixStream, events: libc::c_short, deadline: Instant) -> Result<()> {
    loop {
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .ok_or("activity deadline")?;
        let mut poll = libc::pollfd {
            fd: stream.as_raw_fd(),
            events,
            revents: 0,
        };
        let ms = remaining.as_millis().min(i32::MAX as u128) as i32;
        if ms == 0 {
            return Err("activity deadline");
        }
        let status = unsafe { libc::poll(&mut poll, 1, ms) };
        if status > 0 && poll.revents & events != 0 {
            return Ok(());
        }
        if status < 0 && std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
            continue;
        }
        return Err("activity unavailable");
    }
}
fn connect(path: &str, deadline: Instant) -> Result<UnixStream> {
    let mut address: libc::sockaddr_un = unsafe { std::mem::zeroed() };
    if !path.starts_with('/')
        || path.as_bytes().contains(&0)
        || path.len() >= address.sun_path.len()
    {
        return Err("activity path");
    }
    address.sun_family = libc::AF_UNIX as libc::sa_family_t;
    for (a, b) in address.sun_path.iter_mut().zip(path.bytes()) {
        *a = b as libc::c_char;
    }
    let fd = unsafe {
        libc::socket(
            libc::AF_UNIX,
            libc::SOCK_STREAM | libc::SOCK_NONBLOCK | libc::SOCK_CLOEXEC,
            0,
        )
    };
    if fd < 0 {
        return Err("activity unavailable");
    }
    let stream = unsafe { UnixStream::from_raw_fd(fd) };
    if unsafe {
        libc::connect(
            fd,
            (&address as *const libc::sockaddr_un).cast(),
            std::mem::size_of_val(&address) as libc::socklen_t,
        )
    } != 0
    {
        if std::io::Error::last_os_error().raw_os_error() != Some(libc::EINPROGRESS) {
            return Err("activity unavailable");
        }
        ready(&stream, libc::POLLOUT, deadline)?;
        if stream
            .take_error()
            .map_err(|_| "activity unavailable")?
            .is_some()
        {
            return Err("activity unavailable");
        }
    }
    Ok(stream)
}
fn transfer(
    stream: &mut UnixStream,
    bytes: &mut [u8],
    writing: bool,
    deadline: Instant,
) -> Result<()> {
    let mut offset = 0;
    while offset < bytes.len() {
        ready(
            stream,
            if writing { libc::POLLOUT } else { libc::POLLIN },
            deadline,
        )?;
        let result = if writing {
            stream.write(&bytes[offset..])
        } else {
            stream.read(&mut bytes[offset..])
        };
        match result {
            Ok(0) => return Err("activity closed"),
            Ok(n) => offset += n,
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted
                ) => {}
            Err(_) => return Err("activity unavailable"),
        }
    }
    Ok(())
}
fn exchange(socket: &str, server_uid: u32, message: &Value, budget: Duration) -> Result<()> {
    exchange_until(socket, server_uid, message, Instant::now() + budget)
}
fn exchange_until(socket: &str, server_uid: u32, message: &Value, deadline: Instant) -> Result<()> {
    let mut bytes = serde_json::to_vec(message).map_err(|_| "activity encoding")?;
    if bytes.len() > 65536 {
        return Err("activity bounds");
    }
    let mut stream = connect(socket, deadline)?;
    let mut peer: libc::ucred = unsafe { std::mem::zeroed() };
    let mut length = std::mem::size_of_val(&peer) as libc::socklen_t;
    if unsafe {
        libc::getsockopt(
            stream.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            (&mut peer as *mut libc::ucred).cast(),
            &mut length,
        )
    } != 0
        || length as usize != std::mem::size_of_val(&peer)
        || peer.uid != server_uid
    {
        return Err("activity peer denied");
    }
    transfer(
        &mut stream,
        &mut (bytes.len() as u32).to_be_bytes(),
        true,
        deadline,
    )?;
    transfer(&mut stream, &mut bytes, true, deadline)?;
    let mut header = [0; 4];
    transfer(&mut stream, &mut header, false, deadline)?;
    let length = u32::from_be_bytes(header) as usize;
    if length == 0 || length > 65536 {
        return Err("activity bounds");
    }
    bytes.resize(length, 0);
    transfer(&mut stream, &mut bytes, false, deadline)?;
    let response: Value = serde_json::from_slice(&bytes).map_err(|_| "activity response")?;
    if response["version"] != 1 || response["status"] != "ok" {
        return Err("activity rejected");
    }
    Ok(())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RelayEvent {
    operation_id: String,
    operation: Operation,
    outcome: Outcome,
    duration_ms: u64,
    usage: u64,
}
fn relay_one(config: &Config, stream: &mut UnixStream) -> Result<()> {
    let deadline = Instant::now() + Duration::from_millis(200);
    let mut peer: libc::ucred = unsafe { std::mem::zeroed() };
    let mut length = std::mem::size_of_val(&peer) as libc::socklen_t;
    if unsafe {
        libc::getsockopt(
            stream.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            (&mut peer as *mut libc::ucred).cast(),
            &mut length,
        )
    } != 0
        || length as usize != std::mem::size_of_val(&peer)
        || peer.uid != 0
    {
        return Err("activity relay caller denied");
    }
    stream
        .set_nonblocking(true)
        .map_err(|_| "activity relay socket")?;
    let mut header = [0; 4];
    transfer(stream, &mut header, false, deadline)?;
    let length = u32::from_be_bytes(header) as usize;
    if length == 0 || length > 1024 {
        return Err("activity relay bounds");
    }
    let mut bytes = vec![0; length];
    transfer(stream, &mut bytes, false, deadline)?;
    let event: RelayEvent = serde_json::from_slice(&bytes).map_err(|_| "activity relay context")?;
    if !identifier(&event.operation_id)
        || event.duration_ms > 86_400_000
        || event.usage > 1_000_000_000_000
        || matches!(event.operation, Operation::Model)
    {
        return Err("activity relay event");
    }
    config.emit_until(
        &event.operation_id,
        event.operation,
        event.outcome,
        Duration::from_millis(event.duration_ms),
        event.usage,
        deadline,
    )?;
    let mut reply = br#"{"version":1,"status":"ok"}"#.to_vec();
    // The caller's 250ms total bound includes ingress + collector publication.
    transfer(
        stream,
        &mut (reply.len() as u32).to_be_bytes(),
        true,
        deadline,
    )?;
    transfer(stream, &mut reply, true, deadline)
}
/// Dedicated single-client relay: no queue, threads, credentials or engine access.
/// Root is the host TCB; a separate deployment socket binds each worker context.
pub fn serve_relay(mut config: Config) -> Result<()> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    use std::os::unix::net::UnixListener;
    if !config.agent_id.starts_with("agent-") {
        return Err("activity agent");
    }
    config.validate(&config.agent_id)?;
    if unsafe { libc::geteuid() } != config.producer_uid {
        return Err("activity relay identity");
    }
    let socket = config.relay_socket.take().ok_or("activity relay missing")?;
    let parent = std::path::Path::new(&socket)
        .parent()
        .ok_or("activity relay directory")?;
    let meta = std::fs::symlink_metadata(parent).map_err(|_| "activity relay directory")?;
    if !meta.is_dir() || meta.uid() != config.producer_uid || meta.mode() & 0o777 != 0o711 {
        return Err("activity relay directory");
    }
    // RuntimeDirectory is private to this unit and removed by systemd on stop.
    let listener = UnixListener::bind(&socket).map_err(|_| "activity relay bind")?;
    std::fs::set_permissions(&socket, std::fs::Permissions::from_mode(0o622))
        .map_err(|_| "activity relay permissions")?;
    loop {
        match listener.accept() {
            Ok((mut stream, _)) => {
                if relay_one(&config, &mut stream).is_err() {
                    eprintln!("tentaflake activity telemetry_gap");
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => return Err("activity relay accept"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::net::UnixListener;

    fn config(socket: String) -> Config {
        Config {
            socket,
            relay_socket: None,
            server_uid: unsafe { libc::geteuid() },
            producer_uid: unsafe { libc::geteuid() },
            producer_id: "broker-test".into(),
            tenant_id: "tenant".into(),
            agent_id: "agent-test".into(),
            run_id: "host-fixed-not-native".into(),
            workload_id: "work".into(),
            generation: 1,
            policy_revision: "policy".into(),
        }
    }
    fn collector() -> (Config, UnixListener) {
        let socket = format!("/tmp/tfa-{}.sock", unique_id().unwrap());
        let listener = UnixListener::bind(&socket).unwrap();
        (config(socket), listener)
    }
    fn capture(listener: UnixListener) -> std::thread::JoinHandle<Value> {
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(1)))
                .unwrap();
            let mut header = [0; 4];
            stream.read_exact(&mut header).unwrap();
            let mut bytes = vec![0; u32::from_be_bytes(header) as usize];
            stream.read_exact(&mut bytes).unwrap();
            let response = br#"{"version":1,"status":"ok"}"#;
            stream
                .write_all(&(response.len() as u32).to_be_bytes())
                .unwrap();
            stream.write_all(response).unwrap();
            serde_json::from_slice(&bytes).unwrap()
        })
    }
    #[test]
    fn denied_and_abandoned_dispatch_are_minimized_and_distinct() {
        for dispatched in [false, true] {
            let (config, listener) = collector();
            let capture = capture(listener);
            let mut guard = Guard::new(&config).unwrap();
            let id = guard.operation_id.clone();
            if dispatched {
                guard.dispatched();
            }
            drop(guard);
            let event = capture.join().unwrap();
            assert_eq!(event["event"]["operation_id"], id);
            assert_eq!(
                event["event"]["outcome"],
                if dispatched { "uncertain" } else { "denied" }
            );
            assert_eq!(event["event"]["run_id"], "host-fixed-not-native");
            assert_eq!(event["event"].as_object().unwrap().len(), 16);
            for forbidden in [
                "argv",
                "url",
                "payload",
                "result",
                "credential",
                "SECRET_CANARY",
            ] {
                assert!(!event.to_string().contains(forbidden));
            }
            std::fs::remove_file(config.socket).unwrap();
        }
    }
    #[test]
    fn stalled_collector_and_sdk_disconnect_have_absolute_bounds() {
        for disconnect in [false, true] {
            let (config, listener) = collector();
            let server = std::thread::spawn(move || {
                let (stream, _) = listener.accept().unwrap();
                if !disconnect {
                    std::thread::sleep(Duration::from_millis(300));
                }
                drop(stream);
            });
            let start = Instant::now();
            assert!(
                config
                    .emit(
                        "operation",
                        Operation::Tool,
                        Outcome::Uncertain,
                        Duration::ZERO,
                        0
                    )
                    .is_err()
            );
            assert!(start.elapsed() < Duration::from_millis(250));
            server.join().unwrap();
            std::fs::remove_file(config.socket).unwrap();
        }
    }
    #[test]
    fn peer_and_context_spoofing_are_rejected() {
        let (mut config, listener) = collector();
        config.server_uid = config.server_uid.wrapping_add(1);
        assert!(
            config
                .emit(
                    "operation",
                    Operation::Tool,
                    Outcome::Succeeded,
                    Duration::ZERO,
                    0
                )
                .is_err()
        );
        drop(listener);
        std::fs::remove_file(&config.socket).unwrap();
        assert!(serde_json::from_value::<RelayEvent>(json!({
            "operation_id":"job","operation":"tool","outcome":"succeeded","duration_ms":0,"usage":0,
            "agent_id":"agent-forged","payload":"SECRET_CANARY"
        })).is_err());
        let (mut server, client) = UnixStream::pair().unwrap();
        if unsafe { libc::geteuid() } != 0 {
            assert_eq!(
                relay_one(&config, &mut server),
                Err("activity relay caller denied")
            );
        }
        drop(client);
    }
}
