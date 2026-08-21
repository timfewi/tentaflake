use serde::Deserialize;
use std::collections::HashSet;
use std::env;
use std::fs;
use std::net::{IpAddr, SocketAddr};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub agent: String,
    pub listen: SocketAddr,
    pub token_file: PathBuf,
    pub audit_file: PathBuf,
    pub budget_state_file: PathBuf,
    #[serde(default = "default_request_bytes")]
    pub max_request_bytes: usize,
    #[serde(default = "default_response_bytes")]
    pub max_response_bytes: usize,
    #[serde(default = "default_header_bytes")]
    pub max_header_bytes: usize,
    #[serde(default = "default_timeout_seconds")]
    pub timeout_seconds: u64,
    #[serde(default = "default_connect_timeout_seconds")]
    pub connect_timeout_seconds: u64,
    #[serde(default = "default_concurrency")]
    pub max_concurrency: usize,
    #[serde(default = "default_window_seconds")]
    pub rate_window_seconds: u64,
    #[serde(default = "default_requests_per_window")]
    pub max_requests_per_window: u64,
    #[serde(default = "default_daily_requests")]
    pub daily_request_budget: u64,
    #[serde(default = "default_daily_tokens")]
    pub daily_token_budget: u64,
    #[serde(default = "default_daily_cost")]
    pub daily_cost_microusd: u64,
    #[serde(default = "default_audit_bytes")]
    pub max_audit_bytes: u64,
    #[serde(default)]
    pub llm: Option<LlmPolicy>,
    #[serde(default)]
    pub fetch: Option<FetchPolicy>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LlmPolicy {
    pub upstream_base_url: String,
    pub provider_credential_file: PathBuf,
    pub allowed_models: Vec<ModelPolicy>,
    #[serde(default = "default_max_completion_tokens")]
    pub max_completion_tokens: u64,
    #[serde(default)]
    pub allow_plain_http_for_tests: bool,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelPolicy {
    pub name: String,
    #[serde(default)]
    pub input_microusd_per_million: u64,
    #[serde(default)]
    pub output_microusd_per_million: u64,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FetchPolicy {
    pub allowed_hosts: Vec<String>,
    #[serde(default = "default_content_types")]
    pub allowed_content_types: Vec<String>,
    pub quarantine_dir: PathBuf,
    #[serde(default = "default_redirects")]
    pub max_redirects: usize,
}

impl Config {
    pub fn load(path: &Path) -> Result<Self, String> {
        let text = fs::read_to_string(path)
            .map_err(|error| format!("cannot read broker config {}: {error}", path.display()))?;
        let mut config: Self = serde_json::from_str(&text)
            .map_err(|error| format!("invalid broker config {}: {error}", path.display()))?;
        config.expand_credential_paths()?;
        config.validate()?;
        Ok(config)
    }

    fn expand_credential_paths(&mut self) -> Result<(), String> {
        expand_credential_path(&mut self.token_file)?;
        if let Some(policy) = &mut self.llm {
            expand_credential_path(&mut policy.provider_credential_file)?;
        }
        Ok(())
    }

    pub fn validate(&self) -> Result<(), String> {
        if !valid_agent_name(&self.agent) {
            return Err(
                "agent must contain only lowercase ASCII letters, digits, and hyphens".into(),
            );
        }
        if !is_private_listener(self.listen.ip()) {
            return Err(
                "listen must use loopback or a dedicated private broker-network address".into(),
            );
        }
        for path in [&self.token_file, &self.audit_file, &self.budget_state_file] {
            require_absolute_runtime_path(path)?;
        }
        if self.max_request_bytes == 0
            || self.max_response_bytes == 0
            || self.max_header_bytes == 0
            || self.timeout_seconds == 0
            || self.connect_timeout_seconds == 0
            || self.max_concurrency == 0
            || self.rate_window_seconds == 0
            || self.max_requests_per_window == 0
            || self.daily_request_budget == 0
            || self.daily_token_budget == 0
            || self.daily_cost_microusd == 0
            || self.max_audit_bytes == 0
        {
            return Err("broker limits must all be positive".into());
        }
        match (&self.llm, &self.fetch) {
            (Some(_), Some(_)) => {
                return Err("one broker process may be llm or fetch, never both".into());
            }
            (None, None) => return Err("broker config requires exactly one of llm or fetch".into()),
            _ => {}
        }
        if let Some(policy) = &self.llm {
            policy.validate()?;
        }
        if let Some(policy) = &self.fetch {
            policy.validate()?;
        }
        Ok(())
    }
}

fn expand_credential_path(path: &mut PathBuf) -> Result<(), String> {
    const PREFIX: &str = "$CREDENTIALS_DIRECTORY/";
    let Some(raw) = path.to_str() else {
        return Err("credential path is not UTF-8".into());
    };
    let Some(relative) = raw.strip_prefix(PREFIX) else {
        return Ok(());
    };
    if relative.is_empty() || relative.contains('/') || relative.contains("..") {
        return Err("credential path substitution requires one safe file name".into());
    }
    let directory = env::var_os("CREDENTIALS_DIRECTORY")
        .ok_or("CREDENTIALS_DIRECTORY is required by broker config")?;
    *path = PathBuf::from(directory).join(relative);
    Ok(())
}

fn is_private_listener(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => ip.is_loopback() || ip.is_private(),
        IpAddr::V6(ip) => ip.is_loopback() || (ip.segments()[0] & 0xfe00) == 0xfc00,
    }
}

impl LlmPolicy {
    fn validate(&self) -> Result<(), String> {
        require_absolute_runtime_path(&self.provider_credential_file)?;
        let url = reqwest::Url::parse(&self.upstream_base_url)
            .map_err(|_| "llm upstream_base_url is not a valid URL")?;
        if url.scheme() != "https" && !(self.allow_plain_http_for_tests && url.scheme() == "http") {
            return Err("llm upstream requires HTTPS".into());
        }
        if url.host_str().is_none()
            || url.cannot_be_a_base()
            || url.username() != ""
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
            || !url.path().ends_with('/')
        {
            return Err(
                "llm upstream URL requires a host and trailing slash, without credentials, query, or fragment"
                    .into(),
            );
        }
        if self.allowed_models.is_empty() || self.max_completion_tokens == 0 {
            return Err("llm policy requires models and a positive completion-token limit".into());
        }
        let mut model_names = HashSet::new();
        for model in &self.allowed_models {
            if model.name.is_empty()
                || model.name.len() > 200
                || model.name.chars().any(char::is_control)
                || !model_names.insert(&model.name)
                || model.input_microusd_per_million == 0
                || model.output_microusd_per_million == 0
            {
                return Err(
                    "llm models require unique bounded names and positive input/output prices"
                        .into(),
                );
            }
        }
        Ok(())
    }
}

impl FetchPolicy {
    fn validate(&self) -> Result<(), String> {
        require_absolute_runtime_path(&self.quarantine_dir)?;
        if self.allowed_hosts.is_empty() || self.allowed_content_types.is_empty() {
            return Err("fetch policy requires non-empty host and content-type allowlists".into());
        }
        for host in &self.allowed_hosts {
            if !valid_hostname(host) {
                return Err(format!("invalid exact fetch hostname: {host}"));
            }
        }
        for content_type in &self.allowed_content_types {
            if content_type.is_empty()
                || content_type != &content_type.to_ascii_lowercase()
                || content_type.contains('*')
                || content_type.contains(';')
                || !content_type.contains('/')
            {
                return Err(format!(
                    "fetch content types must be exact lowercase media types: {content_type}"
                ));
            }
        }
        if self.max_redirects > 10 {
            return Err("fetch max_redirects may not exceed 10".into());
        }
        Ok(())
    }
}

fn require_absolute_runtime_path(path: &Path) -> Result<(), String> {
    if !path.is_absolute()
        || path
            .components()
            .any(|part| matches!(part, std::path::Component::ParentDir))
    {
        return Err(format!(
            "broker path must be absolute without traversal: {}",
            path.display()
        ));
    }
    Ok(())
}

fn valid_agent_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 63
        && value.bytes().enumerate().all(|(index, byte)| {
            byte.is_ascii_lowercase()
                || byte.is_ascii_digit()
                || (byte == b'-' && index != 0 && index + 1 != value.len())
        })
}

fn valid_hostname(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 253
        && value == value.to_ascii_lowercase()
        && value.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        })
}

fn default_request_bytes() -> usize {
    1024 * 1024
}
fn default_response_bytes() -> usize {
    8 * 1024 * 1024
}
fn default_header_bytes() -> usize {
    32 * 1024
}
fn default_timeout_seconds() -> u64 {
    60
}
fn default_connect_timeout_seconds() -> u64 {
    10
}
fn default_concurrency() -> usize {
    4
}
fn default_window_seconds() -> u64 {
    60
}
fn default_requests_per_window() -> u64 {
    30
}
fn default_daily_requests() -> u64 {
    1000
}
fn default_daily_tokens() -> u64 {
    1_000_000
}
fn default_daily_cost() -> u64 {
    10_000_000
}
fn default_audit_bytes() -> u64 {
    32 * 1024 * 1024
}
fn default_max_completion_tokens() -> u64 {
    4096
}
fn default_redirects() -> usize {
    3
}
fn default_content_types() -> Vec<String> {
    vec![
        "application/json".into(),
        "application/xml".into(),
        "text/html".into(),
        "text/plain".into(),
        "text/xml".into(),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base_config() -> Config {
        Config {
            agent: "coding".into(),
            listen: "127.0.0.1:7810".parse().unwrap(),
            token_file: "/run/tentaflake/token".into(),
            audit_file: "/var/lib/tentaflake/audit.jsonl".into(),
            budget_state_file: "/var/lib/tentaflake/budget.json".into(),
            max_request_bytes: 1024,
            max_response_bytes: 2048,
            max_header_bytes: 1024,
            timeout_seconds: 5,
            connect_timeout_seconds: 2,
            max_concurrency: 2,
            rate_window_seconds: 60,
            max_requests_per_window: 10,
            daily_request_budget: 100,
            daily_token_budget: 1000,
            daily_cost_microusd: 100_000,
            max_audit_bytes: 4096,
            llm: Some(LlmPolicy {
                upstream_base_url: "https://api.example.com/v1/".into(),
                provider_credential_file: "/run/credentials/provider".into(),
                allowed_models: vec![ModelPolicy {
                    name: "example/model".into(),
                    input_microusd_per_million: 10,
                    output_microusd_per_million: 20,
                }],
                max_completion_tokens: 100,
                allow_plain_http_for_tests: false,
            }),
            fetch: None,
        }
    }

    #[test]
    fn accepts_one_fail_closed_mode() {
        base_config().validate().unwrap();
    }

    #[test]
    fn rejects_dual_mode_and_zero_limits() {
        let mut config = base_config();
        config.fetch = Some(FetchPolicy {
            allowed_hosts: vec!["example.com".into()],
            allowed_content_types: default_content_types(),
            quarantine_dir: "/var/lib/tentaflake/quarantine".into(),
            max_redirects: 2,
        });
        assert!(config.validate().is_err());
        config.fetch = None;
        config.max_concurrency = 0;
        assert!(config.validate().is_err());
    }

    #[test]
    fn rejects_public_listener_credentials_and_wildcard_hosts() {
        let mut config = base_config();
        config.listen = "0.0.0.0:7810".parse().unwrap();
        assert!(config.validate().is_err());
        config.listen = "127.0.0.1:7810".parse().unwrap();
        config.llm.as_mut().unwrap().upstream_base_url =
            "https://user:password@example.com/v1".into();
        assert!(config.validate().is_err());
        config.llm = None;
        config.fetch = Some(FetchPolicy {
            allowed_hosts: vec!["*.example.com".into()],
            allowed_content_types: default_content_types(),
            quarantine_dir: "/var/lib/tentaflake/quarantine".into(),
            max_redirects: 2,
        });
        assert!(config.validate().is_err());
    }

    #[test]
    fn rejects_unpriced_llm_models() {
        let mut config = base_config();
        config.llm.as_mut().unwrap().allowed_models[0].input_microusd_per_million = 0;
        assert!(config.validate().is_err());
    }
}
