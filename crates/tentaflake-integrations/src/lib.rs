//! Host-owned, read-only connector boundary. Provider content never becomes authority.
pub mod advisory;
pub mod front;
pub mod review;

use reqwest::Url;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::io::Read;
use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum Provider {
    Drive,
    Gmail,
    HubSpot,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum ReadMethod {
    DriveMetadata,
    DriveFile,
    GmailMessage,
    HubSpotContact,
}
impl ReadMethod {
    fn provider(self) -> Provider {
        match self {
            Self::DriveMetadata | Self::DriveFile => Provider::Drive,
            Self::GmailMessage => Provider::Gmail,
            Self::HubSpotContact => Provider::HubSpot,
        }
    }
}

/// Untrusted input: no URL, token, scopes, HTTP method, or policy fields exist.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReadRequest {
    pub tenant: String,
    pub account: String,
    pub resource: String,
    pub method: ReadMethod,
    pub generation: u64,
}

/// Trusted host input, never deserialized from a tool call. Each grant binds one
/// tenant/account; multiple accounts require separately authenticated gateways.
pub struct Grant {
    pub agent: String,
    pub tenant: String,
    pub account: String,
    pub provider: Provider,
    pub resources: BTreeSet<String>,
    pub methods: BTreeSet<ReadMethod>,
    pub credential_reference: String,
    pub generation: u64,
    pub expires_unix: u64,
    pub max_body_bytes: usize,
    pub max_concurrency: usize,
    pub deadline: Duration,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum Error {
    Denied,
    Expired,
    Revoked,
    Busy,
    InvalidPolicy,
    InvalidRequest,
    CredentialUnavailable,
    Network,
    Redirect,
    ProviderFailure,
    TooLarge,
    Malformed,
    Deadline,
}

/// Runtime-only credential. Intentionally no Debug/Serialize implementation.
pub struct Credential(String, u64);
impl Credential {
    pub fn new(value: String) -> Result<Self, Error> {
        if value.is_empty() || value.len() > 8192 || !value.bytes().all(|b| b.is_ascii_graphic()) {
            return Err(Error::CredentialUnavailable);
        }
        Ok(Self(value, 0))
    }
}
/// Host must resolve an opaque runtime reference, verifying provider audience,
/// consented read-only scopes and exact tenant/account. No refresh/login here.
pub trait CredentialSource: Send + Sync {
    fn load(
        &self,
        reference: &str,
        provider: Provider,
        tenant: &str,
        account: &str,
    ) -> Result<Credential, Error>;
    /// Recheck after provider I/O; rotating/revoking a runtime lease discards output.
    fn validate(&self, _credential: &Credential) -> Result<(), Error> {
        Ok(())
    }
}

/// Trusted transport seam for synthetic fixtures. Implementors must respect the
/// deadline and cap; Gateway independently checks the returned bytes/status.
pub trait Transport: Send + Sync {
    fn get(
        &self,
        url: &Url,
        credential: &Credential,
        cap: usize,
        deadline: Duration,
    ) -> Result<Response, Error>;
}
pub struct Response {
    pub status: u16,
    pub body: Vec<u8>,
}
pub enum Content {
    Json(serde_json::Value),
    File(Vec<u8>),
}

/// Minimized audit: excludes tokens, resource/account IDs, URLs and content.
#[derive(Debug, Serialize)]
pub struct Audit {
    pub producer: &'static str,
    pub schema_version: u8,
    pub generation: u64,
    pub method: ReadMethod,
    pub outcome: Result<(), Error>,
    pub duration_ms: u64,
}
pub trait AuditSink: Send + Sync {
    fn record(&self, event: Audit);
}

pub struct Gateway<T, C, A> {
    grant: Grant,
    transport: T,
    credentials: C,
    audit: A,
    generation: AtomicU64,
    active: AtomicUsize,
}
impl<T: Transport, C: CredentialSource, A: AuditSink> Gateway<T, C, A> {
    pub fn new(grant: Grant, transport: T, credentials: C, audit: A) -> Result<Self, Error> {
        if !label(&grant.agent)
            || !label(&grant.tenant)
            || !label(&grant.account)
            || !label(&grant.credential_reference)
            || grant.generation == 0
            || grant.resources.is_empty()
            || grant.resources.len() > 4096
            || !grant.resources.iter().all(|s| label(s))
            || grant.methods.is_empty()
            || grant.methods.iter().any(|m| m.provider() != grant.provider)
            || !(1..=4 * 1024 * 1024).contains(&grant.max_body_bytes)
            || !(1..=16).contains(&grant.max_concurrency)
            || grant.deadline.is_zero()
            || grant.deadline > Duration::from_secs(60)
        {
            return Err(Error::InvalidPolicy);
        }
        let generation = AtomicU64::new(grant.generation);
        Ok(Self {
            grant,
            transport,
            credentials,
            audit,
            generation,
            active: AtomicUsize::new(0),
        })
    }
    /// Irreversible for this gateway. Replacement grants need a new gateway.
    pub fn revoke(&self) {
        self.generation.store(0, Ordering::SeqCst);
    }
    pub fn read(&self, request: &ReadRequest) -> Result<Content, Error> {
        let started = Instant::now();
        let result = self.read_inner(request);
        self.audit.record(Audit {
            producer: "tentaflake-integrations",
            schema_version: 1,
            generation: self.grant.generation,
            method: request.method,
            outcome: result.as_ref().map(|_| ()).map_err(|e| *e),
            duration_ms: started.elapsed().as_millis().min(86400000) as u64,
        });
        result
    }
    /// Bound raw JSON before allocation-heavy deserialization.
    pub fn read_json(&self, bytes: &[u8]) -> Result<Content, Error> {
        if bytes.len() > 4096 {
            return Err(Error::TooLarge);
        }
        let request = serde_json::from_slice(bytes).map_err(|_| Error::InvalidRequest)?;
        self.read(&request)
    }
    fn authorize(&self, r: &ReadRequest) -> Result<(), Error> {
        if self.generation.load(Ordering::SeqCst) == 0 {
            return Err(Error::Revoked);
        }
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| Error::Expired)?
            .as_secs();
        if now >= self.grant.expires_unix {
            return Err(Error::Expired);
        }
        if r.generation != self.grant.generation
            || r.tenant != self.grant.tenant
            || r.account != self.grant.account
            || !self.grant.resources.contains(&r.resource)
            || !self.grant.methods.contains(&r.method)
        {
            return Err(Error::Denied);
        }
        Ok(())
    }
    fn read_inner(&self, r: &ReadRequest) -> Result<Content, Error> {
        self.authorize(r)?;
        self.active
            .try_update(Ordering::SeqCst, Ordering::SeqCst, |n| {
                (n < self.grant.max_concurrency).then_some(n + 1)
            })
            .map_err(|_| Error::Busy)?;
        let _slot = Slot(&self.active);
        let started = Instant::now();
        let url = route(r)?;
        let credential = self
            .credentials
            .load(
                &self.grant.credential_reference,
                self.grant.provider,
                &self.grant.tenant,
                &self.grant.account,
            )
            .map_err(|_| Error::CredentialUnavailable)?;
        self.authorize(r)?;
        let remaining = self
            .grant
            .deadline
            .checked_sub(started.elapsed())
            .ok_or(Error::Deadline)?;
        let response =
            self.transport
                .get(&url, &credential, self.grant.max_body_bytes, remaining)?;
        self.authorize(r)?; // discard data after revocation/expiry, including in-flight calls
        self.credentials.validate(&credential)?;
        if started.elapsed() > self.grant.deadline {
            return Err(Error::Deadline);
        }
        if (300..400).contains(&response.status) {
            return Err(Error::Redirect);
        }
        if response.status != 200 {
            return Err(Error::ProviderFailure);
        }
        if response.body.len() > self.grant.max_body_bytes {
            return Err(Error::TooLarge);
        }
        if r.method == ReadMethod::DriveFile {
            return Ok(Content::File(response.body));
        }
        let value: serde_json::Value =
            serde_json::from_slice(&response.body).map_err(|_| Error::Malformed)?;
        if !value.is_object()
            || value.get("id").and_then(|v| v.as_str()) != Some(r.resource.as_str())
        {
            return Err(Error::Malformed);
        }
        Ok(Content::Json(value))
    }
}
struct Slot<'a>(&'a AtomicUsize);
impl Drop for Slot<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}
fn label(s: &str) -> bool {
    !s.is_empty() && s.len() <= 256 && !s.chars().any(char::is_control)
}
fn origin(p: Provider) -> &'static str {
    match p {
        Provider::Drive => "www.googleapis.com",
        Provider::Gmail => "gmail.googleapis.com",
        Provider::HubSpot => "api.hubapi.com",
    }
}
fn route(r: &ReadRequest) -> Result<Url, Error> {
    if !label(&r.resource)
        || !label(&r.account)
        || matches!(r.resource.as_str(), "." | "..")
        || matches!(r.account.as_str(), "." | "..")
    {
        return Err(Error::InvalidRequest);
    }
    let mut url = Url::parse(&format!("https://{}/", origin(r.method.provider())))
        .map_err(|_| Error::InvalidRequest)?;
    {
        let mut path = url.path_segments_mut().map_err(|_| Error::InvalidRequest)?;
        path.clear();
        match r.method {
            ReadMethod::DriveMetadata | ReadMethod::DriveFile => {
                path.extend(["drive", "v3", "files", &r.resource]);
            }
            ReadMethod::GmailMessage => {
                path.extend(["gmail", "v1", "users", &r.account, "messages", &r.resource]);
            }
            ReadMethod::HubSpotContact => {
                path.extend(["crm", "v3", "objects", "contacts", &r.resource]);
            }
        }
    }
    match r.method {
        ReadMethod::DriveMetadata => {
            url.query_pairs_mut()
                .append_pair("fields", "id,name,mimeType,size,modifiedTime");
        }
        ReadMethod::DriveFile => {
            url.query_pairs_mut().append_pair("alt", "media");
        }
        ReadMethod::GmailMessage => {
            url.query_pairs_mut().append_pair("format", "full");
        }
        ReadMethod::HubSpotContact => {
            url.query_pairs_mut()
                .append_pair("properties", "email,firstname,lastname")
                .append_pair("archived", "false");
        }
    }
    Ok(url)
}

/// Pinned reviewed addresses avoid a second DNS resolution/rebinding and keep
/// DNS outside the request deadline. Host provisioning must refresh pins; failure
/// never falls back to DNS, another origin or a proxy. TLS still verifies hostname.
pub struct HttpsTransport {
    provider: Provider,
    client: reqwest::blocking::Client,
}
impl HttpsTransport {
    pub fn new(provider: Provider, addresses: &[IpAddr]) -> Result<Self, Error> {
        if addresses.is_empty()
            || addresses.len() > 16
            || addresses.iter().any(|a| !public_address(*a))
        {
            return Err(Error::InvalidPolicy);
        }
        let sockets: Vec<_> = addresses
            .iter()
            .map(|ip| SocketAddr::new(*ip, 443))
            .collect();
        let client = reqwest::blocking::Client::builder()
            .https_only(true)
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .resolve_to_addrs(origin(provider), &sockets)
            .build()
            .map_err(|_| Error::Network)?;
        Ok(Self { provider, client })
    }
}
impl Transport for HttpsTransport {
    fn get(
        &self,
        url: &Url,
        credential: &Credential,
        cap: usize,
        deadline: Duration,
    ) -> Result<Response, Error> {
        if url.scheme() != "https"
            || url.host_str() != Some(origin(self.provider))
            || url.port_or_known_default() != Some(443)
            || !url.username().is_empty()
            || url.password().is_some()
            || url.fragment().is_some()
        {
            return Err(Error::Denied);
        }
        let mut response = self
            .client
            .get(url.clone())
            .bearer_auth(&credential.0)
            .timeout(deadline)
            .send()
            .map_err(|_| Error::Network)?;
        let status = response.status().as_u16();
        if (300..400).contains(&status) {
            return Err(Error::Redirect);
        }
        if status != 200 {
            return Err(Error::ProviderFailure);
        }
        if response.content_length().is_some_and(|n| n > cap as u64) {
            return Err(Error::TooLarge);
        }
        let mut body = Vec::new();
        response
            .by_ref()
            .take(cap as u64 + 1)
            .read_to_end(&mut body)
            .map_err(|_| Error::Network)?;
        if body.len() > cap {
            return Err(Error::TooLarge);
        }
        Ok(Response { status, body })
    }
}
fn public_address(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => {
            let [a, b, c, _] = ip.octets();
            !(a == 0
                || a == 10
                || a == 127
                || a >= 224
                || (a == 100 && (64..=127).contains(&b))
                || (a == 169 && b == 254)
                || (a == 172 && (16..=31).contains(&b))
                || (a == 192 && (b == 168 || b == 0 || (b == 88 && c == 99)))
                || (a == 198 && (b == 18 || b == 19 || (b == 51 && c == 100)))
                || (a == 203 && b == 0 && c == 113))
        }
        // Conservatively allow only global unicast, excluding special 2001 ranges,
        // documentation, transition/6to4 and all mapped/local/multicast addresses.
        IpAddr::V6(ip) => {
            let s = ip.segments();
            s[0] & 0xe000 == 0x2000
                && s[0] != 0x2001
                && s[0] != 0x2002
                && !(s[0] == 0x3fff && s[1] <= 0x0fff)
        }
    }
}

#[cfg(test)]
mod tests;
