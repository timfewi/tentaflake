use serde::Deserialize;
use std::fs;
use std::net::{IpAddr, SocketAddr};
use std::path::{Path, PathBuf};

/// The broker process configuration. The per-environment routes live in
/// `routes_file`, which the control service rewrites (see `routes.rs`).
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub listen: SocketAddr,
    pub routes_file: PathBuf,
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
    #[serde(default = "default_audit_bytes")]
    pub max_audit_bytes: u64,
    #[serde(default)]
    pub streaming: StreamingPolicy,
    /// Lets routes use plain HTTP and non-public upstream addresses. Tests only.
    #[serde(default)]
    pub allow_plain_http_for_tests: bool,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct StreamingPolicy {
    pub max_event_bytes: usize,
    pub first_event_timeout_seconds: u64,
    pub idle_timeout_seconds: u64,
    pub total_timeout_seconds: u64,
}

impl Default for StreamingPolicy {
    fn default() -> Self {
        Self {
            max_event_bytes: 64 * 1024,
            first_event_timeout_seconds: 30,
            idle_timeout_seconds: 30,
            total_timeout_seconds: 600,
        }
    }
}

impl Config {
    pub fn load(path: &Path) -> Result<Self, String> {
        let text = fs::read_to_string(path)
            .map_err(|error| format!("cannot read broker config {}: {error}", path.display()))?;
        let config: Self = serde_json::from_str(&text)
            .map_err(|error| format!("invalid broker config {}: {error}", path.display()))?;
        config.validate()?;
        Ok(config)
    }

    pub fn validate(&self) -> Result<(), String> {
        if !is_private_listener(self.listen.ip()) {
            return Err(
                "listen must use loopback or a dedicated private broker-network address".into(),
            );
        }
        for path in [&self.routes_file, &self.audit_file, &self.budget_state_file] {
            require_absolute_path(path)?;
        }
        if self.max_request_bytes == 0
            || self.max_response_bytes == 0
            || self.max_header_bytes == 0
            || self.timeout_seconds == 0
            || self.connect_timeout_seconds == 0
            || self.max_concurrency == 0
            || self.rate_window_seconds == 0
            || self.max_requests_per_window == 0
            || self.max_audit_bytes == 0
        {
            return Err("broker limits must all be positive".into());
        }
        let streaming = &self.streaming;
        if streaming.max_event_bytes == 0
            || streaming.first_event_timeout_seconds == 0
            || streaming.idle_timeout_seconds == 0
            || streaming.total_timeout_seconds == 0
            || streaming.first_event_timeout_seconds > streaming.total_timeout_seconds
            || streaming.idle_timeout_seconds > streaming.total_timeout_seconds
            || streaming.max_event_bytes > self.max_response_bytes
        {
            return Err(
                "streaming limits must be positive and fit the response/time ceilings".into(),
            );
        }
        Ok(())
    }
}

fn is_private_listener(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => ip.is_loopback() || ip.is_private(),
        IpAddr::V6(ip) => ip.is_loopback() || (ip.segments()[0] & 0xfe00) == 0xfc00,
    }
}

pub fn require_absolute_path(path: &Path) -> Result<(), String> {
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
    64
}
fn default_window_seconds() -> u64 {
    60
}
fn default_requests_per_window() -> u64 {
    30
}
fn default_audit_bytes() -> u64 {
    32 * 1024 * 1024
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn base_config() -> Config {
        serde_json::from_value(json!({
            "listen": "127.0.0.1:7810",
            "routes_file": "/run/tentaflake/broker/routes.json",
            "audit_file": "/var/lib/tentaflake/audit.jsonl",
            "budget_state_file": "/var/lib/tentaflake/budget.json",
        }))
        .unwrap()
    }

    #[test]
    fn accepts_defaults_and_rejects_zero_limits() {
        base_config().validate().unwrap();
        let mut config = base_config();
        config.max_concurrency = 0;
        assert!(config.validate().is_err());
    }

    #[test]
    fn rejects_public_listener_relative_paths_and_unknown_fields() {
        let mut config = base_config();
        config.listen = "0.0.0.0:7810".parse().unwrap();
        assert!(config.validate().is_err());
        let mut config = base_config();
        config.routes_file = "routes.json".into();
        assert!(config.validate().is_err());
        assert!(
            serde_json::from_value::<Config>(json!({
                "listen": "127.0.0.1:7810", "routes_file": "/r", "audit_file": "/a",
                "budget_state_file": "/b", "token_file": "/t",
            }))
            .is_err()
        );
    }
}
