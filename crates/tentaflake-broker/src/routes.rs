//! Per-environment routes, read from the file the control service replaces
//! atomically. Every request reads the file and parses it again when its
//! content changed; requests already in flight keep the route they started with.
use crate::config::require_absolute_path;
use serde::Deserialize;
use std::collections::{HashMap, HashSet};
use std::fs::OpenOptions;
use std::io::Read;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::PathBuf;
use std::sync::{Arc, RwLock};

/// The provider API family a route speaks; it fixes paths and auth headers.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Api {
    /// OpenAI Chat Completions and Responses with a bearer key.
    Openai,
    /// Anthropic Messages with an `x-api-key`.
    Anthropic,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Route {
    /// The environment id; it names the route in audit and budget state.
    pub id: String,
    /// SHA-256 (hex) of the environment's broker token.
    pub token_sha256: String,
    pub api: Api,
    pub upstream_base_url: String,
    /// The provider key, one line in a private file.
    pub credential_file: PathBuf,
    pub models: Vec<ModelPolicy>,
    #[serde(default = "default_max_completion_tokens")]
    pub max_completion_tokens: u64,
    #[serde(default = "default_daily_requests")]
    pub daily_request_budget: u64,
    #[serde(default = "default_daily_tokens")]
    pub daily_token_budget: u64,
    #[serde(default = "default_daily_cost")]
    pub daily_cost_microusd: u64,
}

/// A model a route may use. Without prices its calls cost nothing against
/// the cost budget; the token budget still applies.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelPolicy {
    pub name: String,
    #[serde(default)]
    pub input_microusd_per_million: u64,
    #[serde(default)]
    pub output_microusd_per_million: u64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RoutesFile {
    routes: Vec<Route>,
}

type Table = Arc<HashMap<String, Arc<Route>>>;

pub struct Routes {
    path: PathBuf,
    /// The file content last parsed (None: no file) and its routes by token hash.
    loaded: RwLock<(Option<String>, Table)>,
}

impl Routes {
    /// A missing file means no routes yet; an invalid one is an error.
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            loaded: RwLock::new((None, Arc::default())),
        }
    }

    /// The route whose token this is, from the current routes file.
    pub fn find(&self, token: &str) -> Result<Option<Arc<Route>>, String> {
        Ok(self.table()?.get(&sha256_hex(token)).cloned())
    }

    pub fn table(&self) -> Result<Table, String> {
        let text = self.read()?;
        {
            let loaded = self.loaded.read().map_err(|_| "routes lock failed")?;
            if loaded.0 == text {
                return Ok(Arc::clone(&loaded.1));
            }
        }
        let table = match &text {
            Some(text) => Arc::new(self.parse(text)?),
            None => Table::default(),
        };
        *self.loaded.write().map_err(|_| "routes lock failed")? = (text, Arc::clone(&table));
        Ok(table)
    }

    fn read(&self) -> Result<Option<String>, String> {
        let mut file = match OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&self.path)
        {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(format!("cannot open routes file: {error}")),
        };
        let meta = file
            .metadata()
            .map_err(|error| format!("cannot stat routes file: {error}"))?;
        // The file decides where provider keys are sent.
        if !meta.is_file() || meta.permissions().mode() & 0o022 != 0 {
            return Err("routes file must be a regular file writable only by its owner".into());
        }
        let mut text = String::new();
        file.read_to_string(&mut text)
            .map_err(|error| format!("cannot read routes file: {error}"))?;
        Ok(Some(text))
    }

    fn parse(&self, text: &str) -> Result<HashMap<String, Arc<Route>>, String> {
        let parsed: RoutesFile =
            serde_json::from_str(text).map_err(|error| format!("invalid routes file: {error}"))?;
        let mut ids = HashSet::new();
        let mut table = HashMap::new();
        for route in parsed.routes {
            route.validate()?;
            if !ids.insert(route.id.clone()) {
                return Err(format!("duplicate route id: {}", route.id));
            }
            if table
                .insert(route.token_sha256.clone(), Arc::new(route))
                .is_some()
            {
                return Err("two routes share one token".into());
            }
        }
        Ok(table)
    }
}

impl Route {
    // Checks the shape only. Whether the upstream may be reached (HTTPS on a
    // public address) is decided per request, so such a route fails alone
    // instead of invalidating every route in the file.
    fn validate(&self) -> Result<(), String> {
        let invalid = |reason: &str| Err(format!("route {}: {reason}", self.id));
        if self.id.is_empty()
            || self.id.len() > 64
            || !self
                .id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        {
            return Err("route ids are 1-64 ASCII letters, digits or hyphens".into());
        }
        if self.token_sha256.len() != 64
            || !self
                .token_sha256
                .bytes()
                .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
        {
            return invalid("token_sha256 must be 64 lowercase hex digits");
        }
        require_absolute_path(&self.credential_file)?;
        let Ok(url) = reqwest::Url::parse(&self.upstream_base_url) else {
            return invalid("upstream_base_url is not a valid URL");
        };
        if !matches!(url.scheme(), "http" | "https")
            || url.host_str().is_none()
            || url.cannot_be_a_base()
            || !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
            || !url.path().ends_with('/')
        {
            return invalid(
                "the upstream URL needs http(s), a host and a trailing slash, without credentials, query or fragment",
            );
        }
        let mut names = HashSet::new();
        if self.models.is_empty()
            || !self.models.iter().all(|model| {
                !model.name.is_empty()
                    && model.name.len() <= 200
                    && !model.name.chars().any(char::is_control)
                    && names.insert(&model.name)
            })
        {
            return invalid("models must be a non-empty list of unique bounded names");
        }
        if self.max_completion_tokens == 0
            || self.daily_request_budget == 0
            || self.daily_token_budget == 0
            || self.daily_cost_microusd == 0
        {
            return invalid("token limits and budgets must be positive");
        }
        Ok(())
    }
}

pub fn sha256_hex(value: &str) -> String {
    ring::digest::digest(&ring::digest::SHA256, value.as_bytes())
        .as_ref()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn default_max_completion_tokens() -> u64 {
    32_000
}
fn default_daily_requests() -> u64 {
    5_000
}
fn default_daily_tokens() -> u64 {
    10_000_000
}
fn default_daily_cost() -> u64 {
    u64::MAX
}
