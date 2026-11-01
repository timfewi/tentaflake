mod json;
pub mod process;
pub mod storage;
pub mod template;
pub mod transport;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;
use storage::{MAX_DOCUMENT, MAX_STATE, PrivateDir, Result};
use template::Builder;

pub fn digest(data: &[u8]) -> String {
    hex(&Sha256::digest(data))
}
fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    bytes
        .iter()
        .flat_map(|byte| {
            [
                char::from(DIGITS[(byte >> 4) as usize]),
                char::from(DIGITS[(byte & 15) as usize]),
            ]
        })
        .collect()
}

#[test]
fn sha256_has_canonical_lowercase_hex() {
    assert_eq!(
        digest(b"abc"),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
}
fn revision(value: &Value) -> String {
    digest(&serde_json::to_vec(value).expect("JSON value serializes"))
}
fn identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 96
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
}
fn hash(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Hashes {
    pub policy: String,
    pub compiler: String,
    pub inputs: String,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Peer {
    pub uid: u32,
    pub roles: Vec<Role>,
}
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    Read,
    Propose,
    Approve,
    Build,
}
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MinimumProfile {
    #[default]
    Balanced,
    Strict,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub source: PathBuf,
    pub normalizer: PathBuf,
    pub schema: PathBuf,
    pub state: PathBuf,
    pub socket: PathBuf,
    pub server_uid: u32,
    pub peers: Vec<Peer>,
    pub hashes: Hashes,
    #[serde(default)]
    pub minimum_profile: MinimumProfile,
    pub builder: Option<Builder>,
}
impl Config {
    pub fn load(path: &Path) -> Result<Self> {
        // NixOS installs /etc entries as symlinks to immutable store files.
        // Resolve only deployment configuration, never private state or candidate input.
        let resolved = fs::canonicalize(path).map_err(|_| "configuration unavailable")?;
        if resolved != path && !template::store_path(&resolved) {
            return Err("configuration link must resolve into the Nix store".into());
        }
        let file = storage::open_path(&resolved, false)?;
        let m = file.metadata().map_err(|_| "configuration unavailable")?;
        if m.uid() != 0 || m.mode() & 0o022 != 0 {
            return Err(
                "deployment configuration must be root owned and immutable to peers".into(),
            );
        }
        serde_json::from_slice(&storage::read_bounded(file, MAX_DOCUMENT)?)
            .map_err(|_| "invalid deployment configuration".into())
    }
    fn validate(&self) -> Result<()> {
        if self.server_uid == 0
            || self.server_uid != unsafe { libc::geteuid() }
            || self.peers.is_empty()
            || self.peers.len() > 32
            || ![
                &self.hashes.policy,
                &self.hashes.compiler,
                &self.hashes.inputs,
            ]
            .into_iter()
            .all(|v| hash(v))
            || !template::store_path(&self.normalizer)
            || !template::store_path(&self.schema)
            || self.source.file_name().and_then(|s| s.to_str()) != Some("tentaflake.toml")
            || self.socket.parent().is_none()
            || !self.socket.is_absolute()
        {
            return Err("invalid fixed deployment configuration".into());
        }
        for path in [&self.normalizer, &self.schema] {
            let metadata = storage::open_path(path, false)?
                .metadata()
                .map_err(|_| "immutable compiler input unavailable")?;
            if metadata.uid() != 0 || metadata.mode() & 0o222 != 0 {
                return Err("mutable compiler or schema refused".into());
            }
        }
        let mut uids = std::collections::BTreeSet::new();
        for peer in &self.peers {
            if !uids.insert(peer.uid) || peer.roles.is_empty() || peer.roles.len() > 4 {
                return Err("invalid peer policy".into());
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct DraftRequest {
    request_id: String,
    base_revision: String,
    configuration: Value,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ApproveRequest {
    request_id: String,
    base_revision: String,
    revision: String,
    hashes: Hashes,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct BuildRequest {
    request_id: String,
    proposal_id: String,
    approval_id: String,
    revision: String,
    hashes: Hashes,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Proposal {
    pub id: String,
    pub base_revision: String,
    pub revision: String,
    pub configuration: Value,
    pub changes: Vec<Value>,
    pub hashes: Hashes,
    pub state: String,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Candidate {
    proposal: Proposal,
    base_configuration: Value,
    toml: String,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Approval {
    pub id: String,
    pub proposal_id: String,
    pub base_revision: String,
    pub revision: String,
    pub hashes: Hashes,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Build {
    pub id: String,
    pub proposal_id: String,
    pub revision: String,
    pub state: String,
    pub output_path: Option<String>,
    pub error: Option<String>,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Intent {
    build: Build,
    approval_id: String,
    hashes: Hashes,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct RequestRecord {
    fingerprint: String,
    kind: String,
    id: String,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct State {
    schema_version: u8,
    proposals: BTreeMap<String, Candidate>,
    approvals: BTreeMap<String, Approval>,
    builds: BTreeMap<String, Intent>,
    requests: BTreeMap<String, RequestRecord>,
}
impl Default for State {
    fn default() -> Self {
        Self {
            schema_version: 1,
            proposals: BTreeMap::new(),
            approvals: BTreeMap::new(),
            builds: BTreeMap::new(),
            requests: BTreeMap::new(),
        }
    }
}
impl State {
    fn validate(&self) -> Result<()> {
        let invalid = || "invalid persisted receipts; refusing reset".to_owned();
        if self.schema_version != 1
            || self.proposals.len() > 64
            || self.requests.len() > 1024
            || self.approvals.len() > 1024
            || self.builds.len() > 1024
        {
            return Err(invalid());
        }
        for (id, candidate) in &self.proposals {
            let p = &candidate.proposal;
            let parsed: Value = toml_edit::de::from_str(&candidate.toml).map_err(|_| invalid())?;
            let mut changes = Vec::new();
            diff(
                &candidate.base_configuration,
                &p.configuration,
                "",
                &mut changes,
            );
            if !identifier(id)
                || id != &p.id
                || p.state != "draft"
                || parsed != p.configuration
                || revision(&p.configuration) != p.revision
                || revision(&candidate.base_configuration) != p.base_revision
                || changes != p.changes
                || ![&p.hashes.policy, &p.hashes.compiler, &p.hashes.inputs]
                    .into_iter()
                    .all(|h| hash(h))
            {
                return Err(invalid());
            }
        }
        for (id, a) in &self.approvals {
            let p = &self
                .proposals
                .get(&a.proposal_id)
                .ok_or_else(invalid)?
                .proposal;
            if !identifier(id)
                || id != &a.id
                || a.base_revision != p.base_revision
                || a.revision != p.revision
                || a.hashes != p.hashes
            {
                return Err(invalid());
            }
        }
        for (id, intent) in &self.builds {
            let b = &intent.build;
            let a = self
                .approvals
                .get(&intent.approval_id)
                .ok_or_else(invalid)?;
            if !identifier(id)
                || id != &b.id
                || a.proposal_id != b.proposal_id
                || a.revision != b.revision
                || a.hashes != intent.hashes
                || !matches!(
                    b.state.as_str(),
                    "queued" | "building" | "built" | "failed" | "unknown"
                )
                || (b.state == "built"
                    && !b.output_path.as_deref().is_some_and(template::output_path))
                || (b.state != "built" && b.output_path.is_some())
            {
                return Err(invalid());
            }
        }
        for (id, request) in &self.requests {
            if !identifier(id)
                || id.len() > 64
                || !hash(&request.fingerprint)
                || !match request.kind.as_str() {
                    "proposal" => self.proposals.contains_key(&request.id),
                    "approval" => self.approvals.contains_key(&request.id),
                    "build" => self.builds.contains_key(&request.id),
                    _ => false,
                }
            {
                return Err(invalid());
            }
        }
        Ok(())
    }
}
#[derive(Debug)]
pub struct ApiError {
    pub status: u16,
    pub code: &'static str,
}
impl ApiError {
    fn new(status: u16, code: &'static str) -> Self {
        Self { status, code }
    }
}
impl From<String> for ApiError {
    fn from(_: String) -> Self {
        Self::new(503, "unavailable")
    }
}
impl From<&str> for ApiError {
    fn from(_: &str) -> Self {
        Self::new(503, "unavailable")
    }
}
type ApiResult<T, E = ApiError> = std::result::Result<T, E>;
fn conflict() -> ApiError {
    ApiError::new(409, "revision_or_request_conflict")
}
fn invalid() -> ApiError {
    ApiError::new(400, "invalid_request")
}
fn decode<T: serde::de::DeserializeOwned>(bytes: &[u8]) -> ApiResult<T> {
    if bytes.len() > MAX_DOCUMENT {
        return Err(ApiError::new(413, "request_too_large"));
    }
    let value: json::UniqueValue = serde_json::from_slice(bytes).map_err(|_| invalid())?;
    serde_json::from_value(value.0).map_err(|_| invalid())
}
fn diff(before: &Value, after: &Value, path: &str, result: &mut Vec<Value>) {
    if before == after {
        return;
    }
    if let (Some(a), Some(b)) = (before.as_object(), after.as_object()) {
        let keys: std::collections::BTreeSet<_> = a.keys().chain(b.keys()).collect();
        for key in keys {
            let p = format!("{}/{}", path, key.replace('~', "~0").replace('/', "~1"));
            match (a.get(key), b.get(key)) {
                (Some(a), Some(b)) => diff(a, b, &p, result),
                (None, Some(_)) => result.push(json!({"path":p,"kind":"added"})),
                (Some(_), None) => result.push(json!({"path":p,"kind":"removed"})),
                _ => (),
            }
        }
    } else if let (Some(a), Some(b)) = (before.as_array(), after.as_array()) {
        for i in 0..a.len().max(b.len()) {
            let p = format!("{path}/{i}");
            match (a.get(i), b.get(i)) {
                (Some(a), Some(b)) => diff(a, b, &p, result),
                (None, Some(_)) => result.push(json!({"path":p,"kind":"added"})),
                (Some(_), None) => result.push(json!({"path":p,"kind":"removed"})),
                _ => (),
            }
        }
    } else {
        result.push(json!({"path":path,"kind":"changed"}));
    }
}

pub struct Engine {
    pub config: Config,
    store: PrivateDir,
    normalize_store: PrivateDir,
    state: State,
    poisoned: bool,
}
impl Engine {
    pub fn open(config: Config) -> Result<Self> {
        config.validate()?;
        Self::open_state(config)
    }
    fn open_state(config: Config) -> Result<Self> {
        let store = PrivateDir::lock(&config.state)?;
        let normalize = config.state.join("normalize");
        match fs::create_dir(&normalize) {
            Ok(()) => fs::set_permissions(&normalize, fs::Permissions::from_mode(0o700))
                .map_err(|_| "private directory unavailable")?,
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => (),
            Err(_) => return Err("private directory unavailable".into()),
        }
        let normalize_store = PrivateDir::lock(&normalize)?;
        let state = match store.read("receipts.json", MAX_STATE)? {
            Some(data) => serde_json::from_slice(&data)
                .map_err(|_| "invalid persisted receipts; refusing reset")?,
            None => State::default(),
        };
        state.validate()?;
        let mut engine = Self {
            config,
            store,
            normalize_store,
            state,
            poisoned: false,
        };
        for intent in engine.state.builds.values_mut() {
            if matches!(intent.build.state.as_str(), "queued" | "building") {
                intent.build.state = "unknown".into();
                intent.build.error = Some("service_restarted_no_redispatch".into());
            }
        }
        engine.persist()?;
        engine.current_hashes()?;
        Ok(engine)
    }
    fn persist(&mut self) -> Result<()> {
        if self.poisoned {
            return Err("state durability unavailable".into());
        }
        // Publish under the mutex. Any failed serialization/fsync permanently closes
        // this writer, so no caller can observe or dispatch an unpublished mutation.
        self.poisoned = true;
        let data = serde_json::to_vec(&self.state).map_err(|_| "state serialization failed")?;
        self.store.atomic("receipts.json", &data)?;
        self.poisoned = false;
        Ok(())
    }
    fn current_hashes(&self) -> Result<Hashes> {
        // The policy revision is deployment-owned; compiler bytes and locked inputs are checked, not client assertions.
        if digest(&storage::read_file(
            &self.config.normalizer,
            128 * 1024 * 1024,
        )?) != self.config.hashes.compiler
        {
            return Err("normalizer compiler drift".into());
        }
        if let Some(builder) = &self.config.builder {
            template::validate(builder, &self.config.hashes.inputs)?;
        }
        let mut hashes = self.config.hashes.clone();
        hashes.policy = revision(&json!({
            "revision": self.config.hashes.policy, "peers": self.config.peers,
            "builder": self.config.builder, "source": self.config.source,
            "server_uid": self.config.server_uid,
            "minimum_profile": self.config.minimum_profile,
            "schema": digest(&storage::read_file(&self.config.schema, MAX_DOCUMENT)?)
        }));
        Ok(hashes)
    }
    fn normalize(&self, bytes: &[u8]) -> Result<Value> {
        self.normalize_store.atomic("tentaflake.toml", bytes)?;
        let output = process::run(
            Command::new(&self.config.normalizer)
                .current_dir(&self.config.state)
                .arg("config")
                .arg("normalize")
                .arg(self.config.state.join("normalize/tentaflake.toml"))
                .arg("--json"),
            Duration::from_secs(10),
            MAX_DOCUMENT,
        )?;
        let value: Value =
            serde_json::from_slice(&output).map_err(|_| "invalid normalizer response")?;
        if !value.is_object() {
            return Err("invalid normalizer response".into());
        }
        Ok(value)
    }
    fn source_bytes(&self) -> Result<Vec<u8>> {
        // A legacy sibling is an ambiguity, even though normalization runs in an isolated directory.
        let parent = self
            .config
            .source
            .parent()
            .ok_or("invalid desired source")?;
        for name in ["agents.toml", "agents.json", "my-agents.nix"] {
            match fs::symlink_metadata(parent.join(name)) {
                Ok(_) => return Err("legacy desired source requires migration".into()),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
                Err(_) => return Err("desired source unavailable".into()),
            }
        }
        storage::read_file(&self.config.source, MAX_DOCUMENT)
    }
    fn current(&self) -> Result<Value> {
        self.normalize(&self.source_bytes()?)
    }
    fn enforce_profile(&self, candidate: &Value, base: &Value) -> ApiResult<()> {
        fn rank(value: &Value) -> Option<u8> {
            match value
                .pointer("/sandbox/profile")
                .and_then(Value::as_str)
                .unwrap_or("balanced")
            {
                "dev" => Some(0),
                "balanced" => Some(1),
                "strict" => Some(2),
                _ => None,
            }
        }
        let minimum = match self.config.minimum_profile {
            MinimumProfile::Balanced => 1,
            MinimumProfile::Strict => 2,
        };
        if rank(candidate).is_some_and(|r| r >= minimum && r >= rank(base).unwrap_or(2)) {
            Ok(())
        } else {
            Err(ApiError::new(403, "sandbox_policy_denied"))
        }
    }
    fn authorize(&self, uid: u32, role: Role) -> ApiResult<()> {
        if self
            .config
            .peers
            .iter()
            .any(|p| p.uid == uid && p.roles.contains(&role))
        {
            Ok(())
        } else {
            Err(ApiError::new(403, "forbidden"))
        }
    }
    fn replay(&self, id: &str, fingerprint: &str) -> ApiResult<Option<Value>> {
        if !identifier(id) || id.len() > 64 {
            return Err(invalid());
        }
        if let Some(record) = self.state.requests.get(id) {
            if record.fingerprint != fingerprint {
                return Err(conflict());
            }
            return self.response(&record.kind, &record.id).map(Some);
        }
        if self.state.requests.len() >= 1024 {
            return Err(ApiError::new(429, "receipt_capacity"));
        }
        Ok(None)
    }
    fn response(&self, kind: &str, id: &str) -> ApiResult<Value> {
        match kind {
            "proposal" => self
                .state
                .proposals
                .get(id)
                .map(|p| json!({"schema_version":1,"proposal":p.proposal})),
            "approval" => self
                .state
                .approvals
                .get(id)
                .map(|a| json!({"schema_version":1,"approval":a})),
            "build" => self
                .state
                .builds
                .get(id)
                .map(|b| json!({"schema_version":1,"build":b.build})),
            _ => None,
        }
        .ok_or(ApiError::new(404, "not_found"))
    }
    pub fn handle(
        &mut self,
        uid: u32,
        method: &str,
        route: &str,
        bytes: &[u8],
    ) -> ApiResult<Value> {
        if self.poisoned {
            return Err(ApiError::new(503, "unavailable"));
        }
        if method == "GET" && !bytes.is_empty() {
            return Err(invalid());
        }
        if method == "GET" && route == "/v1/system" {
            self.authorize(uid, Role::Read)?;
            let hashes = self.current_hashes()?;
            let configuration = self.current()?;
            let schema: Value =
                serde_json::from_slice(&storage::read_file(&self.config.schema, MAX_DOCUMENT)?)
                    .map_err(|_| ApiError::new(503, "unavailable"))?;
            return Ok(
                json!({"schema_version":1,"revision":revision(&configuration),"configuration":configuration,"schema":schema,"hashes":hashes,"capabilities":{"propose":self.authorize(uid, Role::Propose).is_ok(),"build":self.config.builder.is_some() && self.authorize(uid, Role::Build).is_ok(),"activate":false}}),
            );
        }
        if method == "GET" {
            self.authorize(uid, Role::Read)?;
            if let Some(id) = route
                .strip_prefix("/v1/system/builds/")
                .filter(|s| identifier(s))
            {
                return self.response("build", id);
            }
            return Err(ApiError::new(404, "not_found"));
        }
        if method != "POST" {
            return Err(ApiError::new(405, "method_not_allowed"));
        }
        if route == "/v1/system/proposals" {
            self.authorize(uid, Role::Propose)?;
            let request: DraftRequest = decode(bytes)?;
            let fp = revision(&json!([route, &request]));
            if let Some(response) = self.replay(&request.request_id, &fp)? {
                return Ok(response);
            }
            if self.state.proposals.len() >= 64 {
                return Err(ApiError::new(429, "proposal_capacity"));
            }
            let hashes = self.current_hashes()?;
            let base = self.current()?;
            if revision(&base) != request.base_revision {
                return Err(conflict());
            }
            self.enforce_profile(&request.configuration, &base)?;
            let toml = toml_edit::ser::to_document(&request.configuration)
                .map_err(|_| invalid())?
                .to_string();
            if toml.len() > MAX_DOCUMENT {
                return Err(invalid());
            }
            let configuration = self
                .normalize(toml.as_bytes())
                .map_err(|_| ApiError::new(422, "invalid_configuration"))?;
            if configuration != request.configuration {
                return Err(ApiError::new(422, "normalization_changed_meaning"));
            }
            let id = format!("p_{}", request.request_id);
            let mut changes = Vec::new();
            diff(&base, &configuration, "", &mut changes);
            let proposal = Proposal {
                id: id.clone(),
                base_revision: request.base_revision,
                revision: revision(&configuration),
                configuration,
                changes,
                hashes,
                state: "draft".into(),
            };
            if serde_json::to_vec(&proposal).map_err(|_| invalid())?.len() > MAX_DOCUMENT * 2 {
                return Err(ApiError::new(413, "proposal_response_too_large"));
            }
            self.state.proposals.insert(
                id.clone(),
                Candidate {
                    proposal,
                    base_configuration: base,
                    toml,
                },
            );
            self.state.requests.insert(
                request.request_id,
                RequestRecord {
                    fingerprint: fp,
                    kind: "proposal".into(),
                    id: id.clone(),
                },
            );
            self.persist()?;
            return self.response("proposal", &id);
        }
        if let Some(id) = route
            .strip_prefix("/v1/system/proposals/")
            .and_then(|s| s.strip_suffix("/approve"))
            .filter(|s| identifier(s))
        {
            self.authorize(uid, Role::Approve)?;
            let request: ApproveRequest = decode(bytes)?;
            let fp = revision(&json!([route, &request]));
            if let Some(response) = self.replay(&request.request_id, &fp)? {
                return Ok(response);
            }
            let hashes = self.current_hashes()?;
            let current = self.current()?;
            let base = revision(&current);
            let proposal = &self
                .state
                .proposals
                .get(id)
                .ok_or(ApiError::new(404, "not_found"))?
                .proposal;
            self.enforce_profile(&proposal.configuration, &current)?;
            if request.base_revision != base
                || proposal.base_revision != base
                || request.revision != proposal.revision
                || request.hashes != hashes
                || proposal.hashes != hashes
            {
                return Err(conflict());
            }
            let approval_id = format!("a_{}", request.request_id);
            let approval = Approval {
                id: approval_id.clone(),
                proposal_id: id.into(),
                base_revision: base,
                revision: proposal.revision.clone(),
                hashes,
            };
            self.state.approvals.insert(approval_id.clone(), approval);
            self.state.requests.insert(
                request.request_id,
                RequestRecord {
                    fingerprint: fp,
                    kind: "approval".into(),
                    id: approval_id.clone(),
                },
            );
            self.persist()?;
            return self.response("approval", &approval_id);
        }
        if route == "/v1/system/builds" {
            self.authorize(uid, Role::Build)?;
            let request: BuildRequest = decode(bytes)?;
            let fp = revision(&json!([route, &request]));
            if let Some(response) = self.replay(&request.request_id, &fp)? {
                return Ok(response);
            }
            let approval = self
                .state
                .approvals
                .get(&request.approval_id)
                .ok_or(ApiError::new(409, "approval_required"))?;
            if self.config.builder.is_none() {
                return Err(ApiError::new(503, "build_disabled"));
            }
            let hashes = self.current_hashes()?;
            let current = self.current()?;
            let base = revision(&current);
            let candidate = self
                .state
                .proposals
                .get(&request.proposal_id)
                .ok_or(conflict())?;
            self.enforce_profile(&candidate.proposal.configuration, &current)?;
            if approval.proposal_id != request.proposal_id
                || approval.base_revision != base
                || approval.revision != request.revision
                || approval.hashes != hashes
                || request.hashes != hashes
                || candidate.proposal.hashes != hashes
                || candidate.proposal.base_revision != base
                || candidate.proposal.revision != request.revision
            {
                return Err(conflict());
            }
            if self
                .state
                .builds
                .values()
                .filter(|b| matches!(b.build.state.as_str(), "queued" | "building"))
                .count()
                >= 4
            {
                return Err(ApiError::new(429, "build_queue_full"));
            }
            // An approval is a one-shot build grant, even when a client changes the request ID.
            if self
                .state
                .builds
                .values()
                .any(|b| b.approval_id == request.approval_id)
            {
                return Err(conflict());
            }
            let id = format!("b_{}", request.request_id);
            let build = Build {
                id: id.clone(),
                proposal_id: request.proposal_id,
                revision: request.revision,
                state: "queued".into(),
                output_path: None,
                error: None,
            };
            self.state.builds.insert(
                id.clone(),
                Intent {
                    build,
                    approval_id: request.approval_id,
                    hashes,
                },
            );
            self.state.requests.insert(
                request.request_id,
                RequestRecord {
                    fingerprint: fp,
                    kind: "build".into(),
                    id: id.clone(),
                },
            );
            self.persist()?;
            return self.response("build", &id);
        }
        Err(ApiError::new(404, "not_found"))
    }
    /// Claim durably under the only writer lock, then release the mutex before executing.
    pub fn claim(&mut self) -> Result<Option<Work>> {
        if self.poisoned {
            return Err("state unavailable".into());
        }
        if self
            .state
            .builds
            .values()
            .any(|b| b.build.state == "building")
        {
            return Ok(None);
        }
        let Some(intent) = self
            .state
            .builds
            .values()
            .find(|b| b.build.state == "queued")
            .cloned()
        else {
            return Ok(None);
        };
        let hashes = self.current_hashes();
        let source = match self.source_bytes() {
            Ok(bytes) => bytes,
            Err(_) => {
                self.finish(&intent.build.id, Err("source_unavailable".into()))?;
                return Ok(None);
            }
        };
        let base = self.normalize(&source).map(|v| revision(&v));
        let candidate = &self.state.proposals[&intent.build.proposal_id];
        if hashes.as_ref().ok() != Some(&intent.hashes)
            || base.as_ref().ok() != Some(&candidate.proposal.base_revision)
        {
            self.finish(&intent.build.id, Err("approval_invalidated".into()))?;
            return Ok(None);
        }
        let work = Work {
            id: intent.build.id.clone(),
            destination: self.config.state.join(&intent.build.id),
            toml: candidate.toml.clone(),
            builder: self.config.builder.clone().ok_or("build disabled")?,
            source: self.config.source.clone(),
            source_digest: digest(&source),
            inputs_hash: intent.hashes.inputs.clone(),
        };
        self.state
            .builds
            .get_mut(&work.id)
            .ok_or("receipt unavailable")?
            .build
            .state = "building".into();
        self.persist()?;
        Ok(Some(work))
    }
    pub fn finish(&mut self, id: &str, result: Result<String>) -> Result<()> {
        let build = &mut self
            .state
            .builds
            .get_mut(id)
            .ok_or("receipt unavailable")?
            .build;
        match result {
            Ok(path) => {
                build.state = "built".into();
                build.output_path = Some(path);
            }
            Err(_) => {
                build.state = "failed".into();
                build.error = Some("build_failed_or_approval_invalidated".into());
            }
        }
        self.persist()
    }
}
pub struct Work {
    id: String,
    destination: PathBuf,
    toml: String,
    builder: Builder,
    source: PathBuf,
    source_digest: String,
    inputs_hash: String,
}
impl Work {
    pub fn execute(&self) -> Result<String> {
        template::materialize(&self.builder, &self.destination, &self.toml)?;
        if digest(&storage::read_file(
            &self.destination.join("flake.lock"),
            1024 * 1024,
        )?) != self.inputs_hash
        {
            return Err("locked inputs changed before dispatch".into());
        }
        if digest(&storage::read_file(&self.source, MAX_DOCUMENT)?) != self.source_digest {
            return Err("base changed before dispatch".into());
        }
        let target = format!(
            "{}#nixosConfigurations.{}.config.system.build.toplevel",
            self.destination.display(),
            self.builder.target
        );
        let bytes = process::run(
            Command::new(&self.builder.nix)
                .current_dir(
                    self.destination
                        .parent()
                        .ok_or("candidate state unavailable")?,
                )
                .args([
                    "build",
                    "--no-link",
                    "--json",
                    "--no-write-lock-file",
                    &target,
                ]),
            Duration::from_secs(self.builder.deadline_seconds),
            MAX_DOCUMENT,
        )?;
        let result: Value = serde_json::from_slice(&bytes).map_err(|_| "invalid builder output")?;
        let outputs = result
            .as_array()
            .filter(|v| v.len() == 1)
            .and_then(|v| v[0].get("outputs"))
            .and_then(Value::as_object)
            .filter(|v| v.len() == 1)
            .ok_or("invalid builder output")?;
        let path = outputs
            .get("out")
            .and_then(Value::as_str)
            .filter(|p| template::output_path(p))
            .ok_or("invalid output path")?;
        Ok(path.into())
    }
}
pub fn worker(engine: std::sync::Arc<std::sync::Mutex<Engine>>) {
    loop {
        let work = match engine.lock() {
            Ok(mut e) => match e.claim() {
                Ok(w) => w,
                Err(_) => return,
            },
            Err(_) => return,
        };
        if let Some(work) = work {
            let result = work.execute();
            match engine.lock() {
                Ok(mut e) => {
                    if e.finish(&work.id, result).is_err() {
                        return;
                    }
                }
                Err(_) => return,
            }
        } else {
            std::thread::sleep(Duration::from_millis(100));
        }
    }
}

#[cfg(test)]
mod tests;
