//! Fixed-grant Unix front door. Runtime credentials disappear on restart.
use crate::*;
use serde_json::{Value, json};
use std::io::{Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;
use std::sync::{Arc, Mutex};
mod activity;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub socket: String,
    pub caller_uid: u32,
    pub reviewer_uid: u32,
    pub agent: String,
    pub tenant: String,
    pub account: String,
    pub provider: Provider,
    pub resources: BTreeSet<String>,
    pub methods: BTreeSet<ReadMethod>,
    pub generation: u64,
    pub expires_unix: u64,
    pub addresses: Vec<IpAddr>,
    pub max_body_bytes: usize,
    pub deadline_seconds: u64,
    pub advisory: Option<AdvisoryConfig>,
    pub activity: Option<ActivityConfig>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AdvisoryConfig {
    pub observer_uid: u32,
    pub policy: advisory::Policy,
    pub state_directory: String,
}
struct Lease {
    token: String,
    epoch: u64,
    expires: u64,
}
#[derive(Clone)]
pub struct RuntimeCredentials {
    lease: Arc<Mutex<Option<Lease>>>,
    epoch: Arc<AtomicU64>,
    provider: Provider,
    tenant: String,
    account: String,
    generation: u64,
}
impl RuntimeCredentials {
    fn new(config: &Config) -> Self {
        Self {
            lease: Arc::new(Mutex::new(None)),
            epoch: Arc::new(AtomicU64::new(0)),
            provider: config.provider,
            tenant: config.tenant.clone(),
            account: config.account.clone(),
            generation: config.generation,
        }
    }
    pub fn rotate(
        &self,
        token: String,
        generation: u64,
        expires: u64,
        consent: bool,
    ) -> Result<(), Error> {
        let credential = Credential::new(token)?;
        if !consent
            || generation != self.generation
            || expires <= now()?
            || expires > now()?.saturating_add(86400)
        {
            return Err(Error::Denied);
        }
        let mut slot = self
            .lease
            .lock()
            .map_err(|_| Error::CredentialUnavailable)?;
        let epoch = self
            .epoch
            .fetch_add(1, Ordering::SeqCst)
            .checked_add(1)
            .ok_or(Error::Revoked)?;
        *slot = Some(Lease {
            token: credential.0,
            epoch,
            expires,
        });
        Ok(())
    }
    pub fn revoke(&self) -> Result<(), Error> {
        let mut slot = self
            .lease
            .lock()
            .map_err(|_| Error::CredentialUnavailable)?;
        self.epoch.fetch_add(1, Ordering::SeqCst);
        *slot = None;
        Ok(())
    }
}
fn now() -> Result<u64, Error> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|t| t.as_secs())
        .map_err(|_| Error::Expired)
}
impl CredentialSource for RuntimeCredentials {
    fn load(
        &self,
        reference: &str,
        provider: Provider,
        tenant: &str,
        account: &str,
    ) -> Result<Credential, Error> {
        if reference != "runtime"
            || provider != self.provider
            || tenant != self.tenant
            || account != self.account
        {
            return Err(Error::Denied);
        }
        let guard = self
            .lease
            .lock()
            .map_err(|_| Error::CredentialUnavailable)?;
        let lease = guard.as_ref().ok_or(Error::CredentialUnavailable)?;
        if now()? >= lease.expires {
            return Err(Error::Expired);
        }
        Ok(Credential(lease.token.clone(), lease.epoch))
    }
    fn validate(&self, credential: &Credential) -> Result<(), Error> {
        let guard = self
            .lease
            .lock()
            .map_err(|_| Error::CredentialUnavailable)?;
        let lease = guard.as_ref().ok_or(Error::Revoked)?;
        if lease.epoch != credential.1 {
            return Err(Error::Revoked);
        }
        if now()? >= lease.expires {
            return Err(Error::Expired);
        }
        Ok(())
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ActivityConfig {
    pub socket: String,
    pub server_uid: u32,
    pub producer_id: String,
    pub run_id: String,
    pub workload_id: String,
    pub policy_revision: String,
}
struct Journal {
    activity: Option<ActivityConfig>,
    tenant: String,
    agent: String,
    sequence: AtomicU64,
    session: String,
}
impl AuditSink for Journal {
    fn record(&self, event: Audit) {
        if let Ok(line) = serde_json::to_string(&event) {
            eprintln!("{line}");
        }
        let Some(config) = &self.activity else { return };
        let id = format!(
            "{}-{}",
            self.session,
            self.sequence.fetch_add(1, Ordering::SeqCst)
        );
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|t| t.as_millis())
            .unwrap_or(0);
        let denied = matches!(
            event.outcome,
            Err(Error::Denied | Error::Expired | Error::Revoked | Error::CredentialUnavailable)
        );
        let message = json!({"version":1,"operation":"publish_activity","event":{
            "version":1,"producer_id":config.producer_id,"event_id":id,"operation_id":id,
            "tenant_id":self.tenant,"agent_id":self.agent,"run_id":config.run_id,"workload_id":config.workload_id,
            "generation":event.generation,"policy_revision":config.policy_revision,
            "operation":"connector_read","decision":if denied {"deny"} else {"allow"},
            "outcome":if event.outcome.is_ok() {"succeeded"} else if denied {"denied"} else {"failed"},
            "occurred_at_ms":timestamp,"duration_ms":event.duration_ms,"usage":0
        }});
        // A failed projection is explicitly a telemetry gap, never a provider retry.
        if activity::publish(config, &message, Duration::from_secs(1)).is_err() {
            eprintln!("connector_activity_gap");
        }
    }
}
type LiveGateway = Gateway<HttpsTransport, RuntimeCredentials, Journal>;
pub struct Server {
    config: Config,
    gateway: LiveGateway,
    credentials: RuntimeCredentials,
    advisory: Option<(u32, String, advisory::Advisory, Mutex<review::Store>)>,
}
impl Server {
    pub fn new(mut config: Config, enable_advisory: bool) -> Result<Self, &'static str> {
        if config.caller_uid == 0
            || config.reviewer_uid == 0
            || config.caller_uid == config.reviewer_uid
            || !Path::new(&config.socket).is_absolute()
        {
            return Err("invalid identity policy");
        }
        let credentials = RuntimeCredentials::new(&config);
        let transport = HttpsTransport::new(config.provider, &config.addresses)
            .map_err(|_| "invalid transport policy")?;
        let mut random = [0u8; 16];
        std::fs::File::open("/dev/urandom")
            .and_then(|mut f| f.read_exact(&mut random))
            .map_err(|_| "random unavailable")?;
        let session: String = random.iter().map(|b| format!("{b:02x}")).collect();
        let journal = Journal {
            activity: config.activity.take(),
            tenant: config.tenant.clone(),
            agent: config.agent.clone(),
            sequence: AtomicU64::new(0),
            session,
        };
        let gateway = Gateway::new(
            Grant {
                agent: config.agent.clone(),
                tenant: config.tenant.clone(),
                account: config.account.clone(),
                provider: config.provider,
                resources: config.resources.clone(),
                methods: config.methods.clone(),
                credential_reference: "runtime".into(),
                generation: config.generation,
                expires_unix: config.expires_unix,
                max_body_bytes: config.max_body_bytes,
                max_concurrency: 8,
                deadline: Duration::from_secs(config.deadline_seconds),
            },
            transport,
            credentials.clone(),
            journal,
        )
        .map_err(|_| "invalid grant")?;
        let advisory = match (enable_advisory, config.advisory.take()) {
            (true, Some(a)) => {
                if a.observer_uid == 0
                    || a.observer_uid == config.caller_uid
                    || a.observer_uid == config.reviewer_uid
                {
                    return Err("invalid observer identity");
                }
                let name = a.policy.observer.clone();
                let context = json!({
                    "tenant": config.tenant,
                    "account": config.account,
                    "agent": config.agent,
                    "provider": config.provider,
                    "resources": config.resources,
                    "methods": config.methods,
                    "caller_uid": config.caller_uid,
                    "generation": config.generation,
                    "reviewer_uid": config.reviewer_uid,
                    "observer_uid": a.observer_uid,
                    "system": a.policy.system,
                    "observer": a.policy.observer,
                    "admitted": a.policy.admitted,
                    "advisory_resources": a.policy.resources,
                    "max_evidence": a.policy.max_evidence,
                    "max_content_bytes": a.policy.max_content_bytes,
                });
                Some((
                    a.observer_uid,
                    name,
                    advisory::Advisory::new(a.policy).map_err(|_| "invalid advisory policy")?,
                    Mutex::new(review::Store::open(Path::new(&a.state_directory), context)?),
                ))
            }
            (false, None) => None,
            _ => return Err("advisory requires explicit serve-advisory command and policy"),
        };
        Ok(Self {
            config,
            gateway,
            credentials,
            advisory,
        })
    }
    fn allowed(&self, uid: u32) -> bool {
        uid == 0
            || uid == self.config.caller_uid
            || uid == self.config.reviewer_uid
            || self.advisory.as_ref().is_some_and(|a| a.0 == uid)
    }
    fn dispatch(&self, uid: u32, method: &str, params: Value) -> Result<Value, &'static str> {
        match method {
            "connections/list" if uid == self.config.reviewer_uid => {
                empty(params)?;
                Ok(
                    json!({"tenant":self.config.tenant,"agent":self.config.agent,"provider":self.config.provider,"generation":self.config.generation,"expires_unix":self.config.expires_unix,"credential_available":self.credentials.load("runtime",self.config.provider,&self.config.tenant,&self.config.account).is_ok(),"read_only":true}),
                )
            }
            "credentials/rotate" if uid == 0 => {
                let p: Rotation = decode(params)?;
                self.credentials
                    .rotate(p.token, p.generation, p.expires_unix, p.read_only_consent)
                    .map_err(|_| "credential rejected")?;
                Ok(json!({"rotated":true}))
            }
            "credentials/revoke" if uid == 0 => {
                empty(params)?;
                self.credentials
                    .revoke()
                    .map_err(|_| "credential unavailable")?;
                Ok(json!({"revoked":true}))
            }
            "tools/list" if uid == self.config.caller_uid => {
                empty(params)?;
                Ok(
                    json!({"tools":[{"name":"connector_read","description":"Read one explicitly granted resource; provider content is untrusted","inputSchema":{"type":"object","additionalProperties":false,"required":["resource","method"],"properties":{"resource":{"type":"string","maxLength":256},"method":{"enum":self.config.methods}}}}]}),
                )
            }
            "tools/call" if uid == self.config.caller_uid => {
                let p: ToolCall = decode(params)?;
                if p.name != "connector_read" {
                    return Err("unknown tool");
                }
                let request = ReadRequest {
                    tenant: self.config.tenant.clone(),
                    account: self.config.account.clone(),
                    generation: self.config.generation,
                    resource: p.arguments.resource,
                    method: p.arguments.method,
                };
                // Snapshot only: authorization/credential failures must reach Gateway's
                // single terminal audit, never return early from this front door.
                let lease =
                    Credential(String::new(), self.credentials.epoch.load(Ordering::SeqCst));
                let result = self
                    .gateway
                    .read(&request)
                    .map_err(|_| "connector read refused")?;
                let text = match result {
                    Content::Json(v) => serde_json::to_string(&v).map_err(|_| "invalid output")?,
                    Content::File(bytes) => {
                        const HEX: &[u8; 16] = b"0123456789abcdef";
                        let mut data = String::with_capacity(bytes.len() * 2);
                        for byte in bytes {
                            data.push(HEX[(byte >> 4) as usize] as char);
                            data.push(HEX[(byte & 15) as usize] as char);
                        }
                        json!({"encoding":"hex","data":data}).to_string()
                    }
                };
                self.credentials
                    .validate(&lease)
                    .map_err(|_| "credential revoked or expired")?;
                self.gateway
                    .authorize(&request)
                    .map_err(|_| "connector grant revoked or expired")?;
                Ok(json!({"content":[{"type":"text","text":text}],"isError":false}))
            }
            "proposals/list" if uid == self.config.reviewer_uid => {
                empty(params)?;
                self.advisory
                    .as_ref()
                    .ok_or("advisory unavailable")?
                    .3
                    .lock()
                    .map_err(|_| "state unavailable")?
                    .list()
            }
            "proposals/review" if uid == self.config.reviewer_uid => {
                let p: Review = decode(params)?;
                self.advisory
                    .as_ref()
                    .ok_or("advisory unavailable")?
                    .3
                    .lock()
                    .map_err(|_| "state unavailable")?
                    .review(&p.id, p.decision)
            }
            "proposals/submit" => {
                let a = self.advisory.as_ref().ok_or("advisory unavailable")?;
                if uid != a.0 {
                    return Err("denied");
                }
                let p: Submission = decode(params)?;
                let proposal =
                    a.2.propose(&a.1, &p.telemetry, Some(p.finding))
                        .map_err(|_| "proposal refused")?;
                a.3.lock()
                    .map_err(|_| "state unavailable")?
                    .submit(proposal)
            }
            _ => Err("denied or unknown operation"),
        }
    }
    pub fn serve(self) -> Result<(), &'static str> {
        // Never unlink an existing endpoint; systemd owns the fresh runtime directory.
        let listener = UnixListener::bind(&self.config.socket).map_err(|_| "socket unavailable")?;
        std::fs::set_permissions(&self.config.socket, std::fs::Permissions::from_mode(0o666))
            .map_err(|_| "socket permissions unavailable")?;
        let server = Arc::new(self);
        let active = Arc::new(AtomicUsize::new(0));
        for stream in listener.incoming() {
            let Ok(stream) = stream else { continue };
            let Ok(uid) = peer_uid(&stream) else { continue };
            if !server.allowed(uid)
                || active
                    .try_update(Ordering::SeqCst, Ordering::SeqCst, |n| {
                        (n < if uid == 0 { 18 } else { 16 }).then_some(n + 1)
                    })
                    .is_err()
            {
                continue;
            }
            let server = server.clone();
            let active = active.clone();
            std::thread::spawn(move || {
                let _slot = Slot(&active);
                let _ = server.connection(stream, uid);
            });
        }
        Err("listener unavailable")
    }
    fn connection(&self, mut stream: UnixStream, uid: u32) -> Result<(), &'static str> {
        stream
            .set_read_timeout(Some(Duration::from_millis(200)))
            .map_err(|_| "socket unavailable")?;
        stream
            .set_write_timeout(Some(Duration::from_millis(200)))
            .map_err(|_| "socket unavailable")?;
        let mut phase = 0;
        let mut ids = BTreeSet::new();
        for _ in 0..64 {
            let bytes = frame(&mut stream)?;
            let request: Rpc = serde_json::from_slice(&bytes).map_err(|_| "malformed request")?;
            if request.jsonrpc != "2.0" {
                return Err("invalid protocol");
            }
            if request.method == "notifications/initialized" && request.id.is_none() && phase == 1 {
                empty(request.params)?;
                phase = 2;
                continue;
            }
            let id = request.id.ok_or("missing id")?;
            if !(id.is_u64() || id.as_str().is_some_and(|s| !s.is_empty() && s.len() <= 128))
                || !ids.insert(id.to_string())
            {
                return Err("invalid or duplicate id");
            }
            let lease_epoch = self.credentials.epoch.load(Ordering::SeqCst);
            let result = if request.method == "initialize" && phase == 0 {
                let p: Initialize = decode(request.params)?;
                if p.protocol_version != "2024-11-05"
                    || !p.capabilities.is_object()
                    || !p.client_info.is_object()
                {
                    return Err("unsupported initialization");
                }
                phase = 1;
                Ok(
                    json!({"protocolVersion":"2024-11-05","capabilities":{"tools":{"listChanged":false}},"serverInfo":{"name":"tentaflake-integrations","version":env!("CARGO_PKG_VERSION")}}),
                )
            } else if phase == 2 {
                self.dispatch(uid, &request.method, request.params)
            } else {
                Err("not initialized")
            };
            let reply = match result {
                Ok(result) => json!({"jsonrpc":"2.0","id":id,"result":result}),
                Err(message) => {
                    json!({"jsonrpc":"2.0","id":id,"error":{"code":-32600,"message":message}})
                }
            };
            let provider_output = request.method == "tools/call" && reply.get("result").is_some();
            let mut output = serde_json::to_vec(&reply).map_err(|_| "output unavailable")?;
            if output.len() > 32 * 1024 * 1024 {
                return Err("output bounds");
            }
            output.push(b'\n');
            let start = Instant::now();
            let mut remaining = output.as_slice();
            while !remaining.is_empty() {
                if start.elapsed() >= Duration::from_secs(5) {
                    return Err("output deadline");
                }
                if provider_output {
                    self.credentials
                        .validate(&Credential(String::new(), lease_epoch))
                        .map_err(|_| "credential revoked or expired")?;
                    if now().map_err(|_| "clock unavailable")? >= self.config.expires_unix {
                        return Err("grant expired");
                    }
                }
                match stream.write(remaining) {
                    Ok(0) => return Err("output closed"),
                    Ok(n) => remaining = &remaining[n..],
                    Err(e)
                        if matches!(
                            e.kind(),
                            std::io::ErrorKind::TimedOut
                                | std::io::ErrorKind::WouldBlock
                                | std::io::ErrorKind::Interrupted
                        ) => {}
                    Err(_) => return Err("output unavailable"),
                }
            }
        }
        Ok(())
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Rpc {
    jsonrpc: String,
    id: Option<Value>,
    method: String,
    #[serde(default = "object")]
    params: Value,
}
fn object() -> Value {
    json!({})
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct Initialize {
    protocol_version: String,
    capabilities: Value,
    client_info: Value,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Rotation {
    token: String,
    generation: u64,
    expires_unix: u64,
    read_only_consent: bool,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ToolCall {
    name: String,
    arguments: Arguments,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Arguments {
    resource: String,
    method: ReadMethod,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Review {
    id: String,
    decision: review::Decision,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Submission {
    telemetry: Vec<advisory::Telemetry>,
    finding: advisory::Finding,
}
fn decode<T: serde::de::DeserializeOwned>(value: Value) -> Result<T, &'static str> {
    serde_json::from_value(value).map_err(|_| "invalid parameters")
}
fn empty(value: Value) -> Result<(), &'static str> {
    if value == json!({}) {
        Ok(())
    } else {
        Err("invalid parameters")
    }
}
pub fn peer_uid(stream: &UnixStream) -> Result<u32, &'static str> {
    let mut cred: libc::ucred = unsafe { std::mem::zeroed() };
    let mut size = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    if unsafe {
        libc::getsockopt(
            stream.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            &mut cred as *mut _ as *mut libc::c_void,
            &mut size,
        )
    } != 0
        || size as usize != std::mem::size_of::<libc::ucred>()
    {
        return Err("peer unavailable");
    }
    Ok(cred.uid)
}
fn frame(stream: &mut UnixStream) -> Result<Vec<u8>, &'static str> {
    let start = Instant::now();
    let mut bytes = Vec::new();
    let mut byte = [0];
    loop {
        if bytes.len() >= 65536 || start.elapsed() >= Duration::from_secs(5) {
            return Err("frame bounds");
        }
        match stream.read(&mut byte) {
            Ok(1) if byte[0] == b'\n' => return Ok(bytes),
            Ok(1) => bytes.push(byte[0]),
            Ok(_) => return Err("closed"),
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::TimedOut
                        | std::io::ErrorKind::WouldBlock
                        | std::io::ErrorKind::Interrupted
                ) => {}
            Err(_) => return Err("read failed"),
        }
    }
}
pub fn load_config(path: &Path) -> Result<Config, &'static str> {
    if !path.is_absolute() || path.starts_with("/nix/store") {
        return Err("configuration must be runtime-only");
    }
    for ancestor in path.ancestors().skip(1) {
        let metadata =
            std::fs::symlink_metadata(ancestor).map_err(|_| "configuration unavailable")?;
        if !metadata.is_dir() || metadata.uid() != 0 || metadata.mode() & 0o022 != 0 {
            return Err("unsafe configuration parent");
        }
    }
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
        .map_err(|_| "configuration unavailable")?;
    let metadata = file.metadata().map_err(|_| "configuration unavailable")?;
    if !metadata.is_file() || metadata.uid() != 0 || metadata.mode() & 0o022 != 0 {
        return Err("unsafe configuration");
    }
    let mut bytes = Vec::new();
    file.take(65537)
        .read_to_end(&mut bytes)
        .map_err(|_| "configuration unavailable")?;
    if bytes.len() > 65536 {
        return Err("configuration bounds");
    }
    serde_json::from_slice(&bytes).map_err(|_| "invalid configuration")
}

#[cfg(test)]
mod tests;
