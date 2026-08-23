use serde::{Deserialize, Serialize};
use serde_json::json;
use std::env;
use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct BudgetState {
    day: u64,
    requests: u64,
    tokens: u64,
    cost_microusd: u64,
}

struct RateState {
    window: u64,
    requests: u64,
}

pub struct Limits {
    path: PathBuf,
    window_seconds: u64,
    max_requests_per_window: u64,
    daily_request_budget: u64,
    daily_token_budget: u64,
    daily_cost_microusd: u64,
    inner: Mutex<(BudgetState, RateState)>,
}

impl Limits {
    pub fn load(
        path: PathBuf,
        window_seconds: u64,
        max_requests_per_window: u64,
        daily_request_budget: u64,
        daily_token_budget: u64,
        daily_cost_microusd: u64,
    ) -> Result<Self, String> {
        let budget = match fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .map_err(|error| format!("invalid budget state {}: {error}", path.display()))?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => BudgetState::default(),
            Err(error) => {
                return Err(format!(
                    "cannot read budget state {}: {error}",
                    path.display()
                ));
            }
        };
        Ok(Self {
            path,
            window_seconds,
            max_requests_per_window,
            daily_request_budget,
            daily_token_budget,
            daily_cost_microusd,
            inner: Mutex::new((
                budget,
                RateState {
                    window: 0,
                    requests: 0,
                },
            )),
        })
    }

    pub fn reserve(&self, tokens: u64, cost_microusd: u64) -> Result<(), String> {
        let now = unix_seconds();
        let day = now / 86_400;
        let window = now / self.window_seconds;
        let mut state = self.inner.lock().map_err(|_| "budget state lock failed")?;
        if state.0.day != day {
            state.0 = BudgetState {
                day,
                ..BudgetState::default()
            };
        }
        if state.1.window != window {
            state.1 = RateState {
                window,
                requests: 0,
            };
        }
        if state.1.requests >= self.max_requests_per_window {
            return Err("rate limit exceeded".into());
        }
        if state.0.requests.saturating_add(1) > self.daily_request_budget
            || state.0.tokens.saturating_add(tokens) > self.daily_token_budget
            || state.0.cost_microusd.saturating_add(cost_microusd) > self.daily_cost_microusd
        {
            return Err("daily budget exceeded".into());
        }
        state.0.requests += 1;
        state.0.tokens = state.0.tokens.saturating_add(tokens);
        state.0.cost_microusd = state.0.cost_microusd.saturating_add(cost_microusd);
        state.1.requests += 1;
        atomic_json(&self.path, &state.0)
    }
}

pub struct Audit {
    path: PathBuf,
    max_bytes: u64,
    lock: Mutex<()>,
}

impl Audit {
    pub fn new(path: PathBuf, max_bytes: u64) -> Self {
        Self {
            path,
            max_bytes,
            lock: Mutex::new(()),
        }
    }

    pub fn record(
        &self,
        agent: &str,
        kind: &str,
        outcome: &str,
        detail: serde_json::Value,
    ) -> Result<(), String> {
        let _guard = self.lock.lock().map_err(|_| "audit lock failed")?;
        ensure_parent(&self.path)?;
        if fs::metadata(&self.path).map(|item| item.len()).unwrap_or(0) >= self.max_bytes {
            let rotated = self.path.with_extension("jsonl.1");
            if rotated.exists() {
                fs::remove_file(&rotated)
                    .map_err(|error| format!("cannot rotate audit log: {error}"))?;
            }
            fs::rename(&self.path, rotated)
                .map_err(|error| format!("cannot rotate audit log: {error}"))?;
        }
        let event = json!({
            "ts": unix_seconds(),
            "agent": agent,
            "kind": kind,
            "outcome": outcome,
            "detail": detail,
        });
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&self.path)
            .map_err(|error| format!("cannot open audit log: {error}"))?;
        serde_json::to_writer(&mut file, &event)
            .map_err(|error| format!("cannot encode audit: {error}"))?;
        file.write_all(b"\n")
            .map_err(|error| format!("cannot write audit: {error}"))
    }

    pub fn check_ready(&self) -> Result<(), String> {
        let _guard = self.lock.lock().map_err(|_| "audit lock failed")?;
        ensure_parent(&self.path)?;
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&self.path)
            .map_err(|error| format!("cannot open audit log: {error}"))?;
        let metadata = file
            .metadata()
            .map_err(|error| format!("cannot stat audit log: {error}"))?;
        if !metadata.is_file() || metadata.permissions().mode() & 0o077 != 0 {
            return Err("audit log must be a private regular file".into());
        }
        file.sync_data()
            .map_err(|error| format!("cannot sync audit log: {error}"))
    }
}

pub fn read_secret(path: &Path) -> Result<String, String> {
    let credentials_directory = env::var_os("CREDENTIALS_DIRECTORY").map(PathBuf::from);
    read_secret_from(path, credentials_directory.as_deref())
}

fn read_secret_from(path: &Path, credentials_directory: Option<&Path>) -> Result<String, String> {
    let mut file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
        .map_err(|error| format!("cannot open credential {}: {error}", path.display()))?;
    let metadata = file
        .metadata()
        .map_err(|error| format!("cannot stat credential {}: {error}", path.display()))?;
    if !metadata.is_file() {
        return Err("credential must be a regular file".into());
    }
    if metadata.len() == 0 || metadata.len() > 4096 {
        return Err("credential file must contain 1..4096 bytes".into());
    }
    let mode = metadata.permissions().mode();
    let is_systemd_credential = credentials_directory.is_some_and(|directory| {
        directory.is_absolute() && path.parent() == Some(directory) && path.file_name().is_some()
    });
    if mode & 0o007 != 0 || (mode & 0o070 != 0 && !is_systemd_credential) {
        return Err("credential file must not be accessible by group or other".into());
    }
    let mut value = String::with_capacity(metadata.len() as usize);
    file.read_to_string(&mut value)
        .map_err(|error| format!("cannot read credential {}: {error}", path.display()))?;
    let value = value.trim_end_matches(['\r', '\n']);
    if value.is_empty() || value.contains(['\r', '\n']) {
        return Err("credential must be one non-empty line".into());
    }
    Ok(value.to_string())
}

pub fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    let mut difference = left.len() ^ right.len();
    let length = left.len().max(right.len());
    for index in 0..length {
        difference |= usize::from(*left.get(index).unwrap_or(&0) ^ *right.get(index).unwrap_or(&0));
    }
    difference == 0
}

pub fn unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn atomic_json(path: &Path, value: &impl Serialize) -> Result<(), String> {
    ensure_parent(path)?;
    let temporary = path.with_extension("tmp");
    let mut options = OpenOptions::new();
    options
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW);
    let mut file = options
        .open(&temporary)
        .map_err(|error| format!("cannot create state file: {error}"))?;
    serde_json::to_writer(&mut file, value)
        .map_err(|error| format!("cannot encode state: {error}"))?;
    file.sync_all()
        .map_err(|error| format!("cannot sync state: {error}"))?;
    fs::rename(&temporary, path).map_err(|error| format!("cannot replace state: {error}"))?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
        .map_err(|error| format!("cannot protect state: {error}"))
}

fn ensure_parent(path: &Path) -> Result<(), String> {
    let parent = path.parent().ok_or("state path has no parent")?;
    fs::create_dir_all(parent).map_err(|error| format!("cannot create state directory: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_path(name: &str) -> PathBuf {
        let root = std::env::var_os("CARGO_TARGET_TMPDIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                Path::new(env!("CARGO_MANIFEST_DIR"))
                    .parent()
                    .and_then(Path::parent)
                    .expect("crate is in the workspace crates directory")
                    .join("target/test-tmp")
            });
        fs::create_dir_all(&root).unwrap();
        root.join(format!(
            "tentaflake-audit-test-{name}-{}-{}",
            std::process::id(),
            unix_seconds()
        ))
    }

    #[test]
    fn constant_time_comparison_handles_lengths() {
        assert!(constant_time_eq(b"secret", b"secret"));
        assert!(!constant_time_eq(b"secret", b"secreu"));
        assert!(!constant_time_eq(b"secret", b"secret-long"));
    }

    #[test]
    fn readiness_probe_does_not_grow_the_audit_log() {
        let path = test_path("ready");
        let audit = Audit::new(path.clone(), 1024 * 1024);
        audit
            .record("fixture", "startup", "ready", json!({ "mode": "fetch" }))
            .unwrap();
        let before = fs::metadata(&path).unwrap().len();
        audit.check_ready().unwrap();
        audit.check_ready().unwrap();
        assert_eq!(fs::metadata(&path).unwrap().len(), before);
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn readiness_probe_rejects_a_symlink() {
        use std::os::unix::fs::symlink;

        let target = test_path("target");
        let alias = test_path("alias");
        fs::write(&target, b"fixture\n").unwrap();
        fs::set_permissions(&target, fs::Permissions::from_mode(0o600)).unwrap();
        symlink(&target, &alias).unwrap();
        assert!(Audit::new(alias.clone(), 1024).check_ready().is_err());
        fs::remove_file(alias).unwrap();
        fs::remove_file(target).unwrap();
    }

    #[test]
    fn credential_reader_rejects_a_symlink() {
        use std::os::unix::fs::symlink;

        let target = test_path("credential-target");
        let alias = test_path("credential-alias");
        fs::write(&target, b"fixture-secret\n").unwrap();
        fs::set_permissions(&target, fs::Permissions::from_mode(0o600)).unwrap();
        symlink(&target, &alias).unwrap();
        assert!(read_secret(&alias).is_err());
        fs::remove_file(alias).unwrap();
        fs::remove_file(target).unwrap();
    }

    #[test]
    fn credential_reader_accepts_systemd_acl_mask_only_in_credential_directory() {
        let directory = test_path("credential-directory");
        fs::create_dir(&directory).unwrap();
        let credential = directory.join("agent-token");
        fs::write(&credential, b"fixture-secret\n").unwrap();
        fs::set_permissions(&credential, fs::Permissions::from_mode(0o440)).unwrap();

        assert!(read_secret_from(&credential, None).is_err());
        assert_eq!(
            read_secret_from(&credential, Some(&directory)).unwrap(),
            "fixture-secret"
        );
        fs::set_permissions(&credential, fs::Permissions::from_mode(0o444)).unwrap();
        assert!(read_secret_from(&credential, Some(&directory)).is_err());

        fs::remove_file(credential).unwrap();
        fs::remove_dir(directory).unwrap();
    }
}
