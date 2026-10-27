use serde::{Deserialize, Serialize};
use serde_json::json;
use std::env;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Clone, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct BudgetState {
    day: u64,
    requests: u64,
    tokens: u64,
    cost_microusd: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    last_seen_seconds: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    unsettled: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pending_usage: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    checksum: Option<[u8; 32]>,
}

const STRICT_MARKER: &[u8] = b"tentaflake-strict-budget-v2\n";

struct StrictState {
    owner: File,
    lock_path: PathBuf,
    marker_path: PathBuf,
}

#[derive(Clone)]
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
    strict: Option<StrictState>,
    _legacy_owner: Option<File>,
    persistence_failed: AtomicBool,
    overage_pending: AtomicBool,
    // The third value counts live usage guards; persisted excess is orphaned.
    inner: Mutex<(BudgetState, RateState, u64)>,
}

// A strict reservation is a move-only completion capability. A known
// pre-dispatch failure may resolve uncertainty; a possibly dispatched request
// remains held unless its response meets the completion contract.
pub struct Reservation {
    limits: Arc<Limits>,
    dispatched: AtomicBool,
}

impl Reservation {
    pub fn mark_dispatched(&self) {
        self.dispatched.store(true, Ordering::Relaxed);
    }

    pub fn settle(self) -> Result<(), String> {
        self.mark_dispatched();
        self.limits.settle_with_clock(unix_seconds)
    }
}

impl Drop for Reservation {
    fn drop(&mut self) {
        if !self.dispatched.load(Ordering::Relaxed) {
            // No send was attempted. Keep the day's conservative charge, but
            // do not block future days solely because local preflight failed.
            let _ = self.limits.settle_with_clock(unix_seconds);
        }
    }
}

// Persisted before dispatch. If a process dies or an overage cannot be
// persisted, an orphaned guard prevents further admission after restart.
pub struct UsageGuard {
    limits: Arc<Limits>,
    reserved_tokens: u64,
    reserved_cost: u64,
    failed: AtomicBool,
}

impl UsageGuard {
    pub fn observe(&self, tokens: u64, cost: Option<u64>) -> Result<(), String> {
        if tokens > self.reserved_tokens || cost.is_none_or(|cost| cost > self.reserved_cost) {
            self.limits.overage_pending.store(true, Ordering::SeqCst);
            self.failed.store(true, Ordering::Relaxed);
            self.limits.exhaust_for_overage()?;
            self.failed.store(false, Ordering::Relaxed);
            self.limits.overage_pending.store(false, Ordering::SeqCst);
        }
        Ok(())
    }
}

impl Drop for UsageGuard {
    fn drop(&mut self) {
        self.limits
            .release_usage(!self.failed.load(Ordering::Relaxed));
    }
}

impl Limits {
    // This is an explicit, one-time provisioning operation, never a service
    // startup path. A partially provisioned state stays blocked.
    pub fn initialize_strict(path: &Path) -> Result<(), String> {
        Self::initialize_strict_at(path, unix_seconds())
    }

    fn initialize_strict_at(path: &Path, now: u64) -> Result<(), String> {
        if now == 0 {
            return Err("cannot initialize budget with an invalid clock".into());
        }
        let lock_path = state_sidecar(path, ".lock");
        let marker_path = state_sidecar(path, ".initialized");
        let owner = open_or_create_lock(&lock_path)?;
        lock_owner(&owner, true)?;
        match fs::symlink_metadata(path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Ok(_) => return Err("budget state already exists; refusing initialization".into()),
            Err(error) => return Err(format!("cannot inspect budget state: {error}")),
        }
        owner
            .sync_all()
            .map_err(|error| format!("cannot sync budget lock: {error}"))?;
        sync_parent(path)?;
        let mut marker = create_private(&marker_path)?;
        marker
            .write_all(STRICT_MARKER)
            .map_err(|error| format!("cannot write budget marker: {error}"))?;
        marker
            .sync_all()
            .map_err(|error| format!("cannot sync budget marker: {error}"))?;
        sync_parent(path)?;
        let mut state = create_private(path)?;
        let mut initial = BudgetState {
            day: now / 86_400,
            last_seen_seconds: Some(now),
            unsettled: Some(0),
            ..BudgetState::default()
        };
        initial.checksum = Some(strict_digest(&initial));
        serde_json::to_writer(&mut state, &initial)
            .map_err(|error| format!("cannot encode initial budget: {error}"))?;
        state
            .sync_all()
            .map_err(|error| format!("cannot sync initial budget: {error}"))?;
        sync_parent(path)
    }

    // Explicit operator recovery after the provider-side outcome and usage of
    // held requests were verified. It requires the broker to be stopped, clears
    // held reservations and orphaned usage holds, and never refunds counters.
    pub fn reconcile_strict(path: &Path) -> Result<(u64, u64), String> {
        Self::reconcile_strict_at(path, unix_seconds())
    }

    fn reconcile_strict_at(path: &Path, now: u64) -> Result<(u64, u64), String> {
        let _owner = StrictState::acquire(path)?;
        let mut budget = read_strict_budget(path)?;
        check_strict_clock(&budget, now)?;
        let resolved = (
            budget.unsettled.unwrap_or(0),
            budget.pending_usage.unwrap_or(0),
        );
        budget.unsettled = Some(0);
        budget.pending_usage = None;
        budget.last_seen_seconds = Some(now);
        atomic_json(path, &mut budget, true)?;
        Ok(resolved)
    }

    pub fn load(
        path: PathBuf,
        window_seconds: u64,
        max_requests_per_window: u64,
        daily_request_budget: u64,
        daily_token_budget: u64,
        daily_cost_microusd: u64,
        strict: bool,
    ) -> Result<Self, String> {
        Self::load_at(
            path,
            window_seconds,
            max_requests_per_window,
            daily_request_budget,
            daily_token_budget,
            daily_cost_microusd,
            strict.then(unix_seconds),
        )
    }

    fn load_at(
        path: PathBuf,
        window_seconds: u64,
        max_requests_per_window: u64,
        daily_request_budget: u64,
        daily_token_budget: u64,
        daily_cost_microusd: u64,
        now: Option<u64>,
    ) -> Result<Self, String> {
        let (strict_state, legacy_owner) = if now.is_some() {
            (Some(StrictState::acquire(&path)?), None)
        } else {
            // Ordinary brokers can still share legacy state, but they must hold
            // a shared lock so they cannot race strict provisioning/ownership.
            ensure_parent(&path)?;
            let owner = open_or_create_lock(&state_sidecar(&path, ".lock"))?;
            lock_owner(&owner, false)?;
            reject_strict_marker(&path)?;
            (None, Some(owner))
        };
        let budget = if let Some(now) = now {
            let mut budget = read_strict_budget(&path)?;
            check_strict_clock(&budget, now)?;
            checkpoint_clock(&path, &mut budget, now)?;
            budget
        } else {
            match fs::read(&path) {
                Ok(bytes) => serde_json::from_slice(&bytes)
                    .map_err(|error| format!("invalid budget state {}: {error}", path.display()))?,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    BudgetState::default()
                }
                Err(error) => {
                    return Err(format!(
                        "cannot read budget state {}: {error}",
                        path.display()
                    ));
                }
            }
        };
        Ok(Self {
            path,
            window_seconds,
            max_requests_per_window,
            daily_request_budget,
            daily_token_budget,
            daily_cost_microusd,
            strict: strict_state,
            _legacy_owner: legacy_owner,
            persistence_failed: AtomicBool::new(false),
            overage_pending: AtomicBool::new(false),
            inner: Mutex::new((
                budget,
                RateState {
                    window: 0,
                    requests: 0,
                },
                0,
            )),
        })
    }

    pub fn reserve(
        self: &Arc<Self>,
        tokens: u64,
        cost_microusd: u64,
    ) -> Result<Option<Reservation>, String> {
        self.reserve_with_clock(tokens, cost_microusd, false, unix_seconds)?;
        Ok(self.strict.as_ref().map(|_| Reservation {
            limits: Arc::clone(self),
            dispatched: AtomicBool::new(false),
        }))
    }

    pub fn reserve_llm(
        self: &Arc<Self>,
        tokens: u64,
        cost_microusd: u64,
    ) -> Result<(Option<Reservation>, Option<UsageGuard>), String> {
        self.reserve_with_clock(tokens, cost_microusd, self.strict.is_some(), unix_seconds)?;
        let reservation = self.strict.as_ref().map(|_| Reservation {
            limits: Arc::clone(self),
            dispatched: AtomicBool::new(false),
        });
        let guard = self.strict.as_ref().map(|_| UsageGuard {
            limits: Arc::clone(self),
            reserved_tokens: tokens,
            reserved_cost: cost_microusd,
            failed: AtomicBool::new(false),
        });
        Ok((reservation, guard))
    }

    #[cfg(test)]
    fn reserve_at(&self, tokens: u64, cost_microusd: u64, now: u64) -> Result<(), String> {
        self.reserve_with_clock(tokens, cost_microusd, false, || now)
    }

    fn reserve_with_clock(
        &self,
        tokens: u64,
        cost_microusd: u64,
        track_usage: bool,
        clock: impl FnOnce() -> u64,
    ) -> Result<(), String> {
        let mut state = self.inner.lock().map_err(|_| "budget state lock failed")?;
        if self.persistence_failed.load(Ordering::Relaxed) {
            return Err("budget persistence previously failed".into());
        }
        if self.overage_pending.load(Ordering::SeqCst) {
            return Err("usage overage accounting is in progress".into());
        }
        // Sample only after acquiring the mutex; queued callers must not compare
        // their stale pre-lock timestamp against a later caller's checkpoint.
        let now = clock();
        let day = now / 86_400;
        let window = now / self.window_seconds;
        if let Some(strict) = &self.strict {
            strict.verify_owner_and_marker()?;
            if read_strict_budget(&self.path)? != state.0 {
                return Err("strict budget state changed while the broker was running".into());
            }
            check_strict_clock(&state.0, now)?;
        } else {
            reject_strict_marker(&self.path)?;
        }
        let mut budget = state.0.clone();
        let mut rate = state.1.clone();
        if budget.pending_usage.unwrap_or(0) > state.2 {
            if self.strict.is_some() {
                checkpoint_clock(&self.path, &mut state.0, now)?;
            }
            return Err("unresolved usage accounting blocks admission".into());
        }
        if budget.day != day {
            if budget.pending_usage.is_some() {
                if self.strict.is_some() {
                    checkpoint_clock(&self.path, &mut state.0, now)?;
                }
                return Err("pending usage accounting blocks the next UTC day".into());
            }
            if self.strict.is_some() && budget.unsettled != Some(0) {
                checkpoint_clock(&self.path, &mut state.0, now)?;
                return Err("unsettled reservations block the next UTC day".into());
            }
            budget = BudgetState {
                day,
                unsettled: self.strict.as_ref().map(|_| 0),
                ..BudgetState::default()
            };
        }
        if rate.window != window {
            rate = RateState {
                window,
                requests: 0,
            };
        }
        let proposal = budget
            .requests
            .checked_add(1)
            .zip(budget.tokens.checked_add(tokens))
            .zip(budget.cost_microusd.checked_add(cost_microusd))
            .map(|((requests, tokens), cost)| (requests, tokens, cost));
        let denied = if rate.requests >= self.max_requests_per_window {
            Some("rate limit exceeded")
        } else if proposal.is_none_or(|(requests, tokens, cost)| {
            requests > self.daily_request_budget
                || tokens > self.daily_token_budget
                || cost > self.daily_cost_microusd
        }) {
            Some("daily budget exceeded")
        } else {
            None
        };
        if let Some(reason) = denied {
            if self.strict.is_some() {
                checkpoint_clock(&self.path, &mut state.0, now)?;
            }
            return Err(reason.into());
        }
        let (requests, tokens, cost) = proposal.expect("admitted budget was checked");
        budget.requests = requests;
        budget.tokens = tokens;
        budget.cost_microusd = cost;
        if self.strict.is_some() {
            budget.last_seen_seconds = Some(now);
            budget.unsettled = Some(
                budget
                    .unsettled
                    .ok_or("strict budget is missing unsettled reservations")?
                    .checked_add(1)
                    .ok_or("daily budget exceeded")?,
            );
        }
        let active = if track_usage {
            budget.pending_usage = Some(
                budget
                    .pending_usage
                    .unwrap_or(0)
                    .checked_add(1)
                    .ok_or("too many pending usage records")?,
            );
            state
                .2
                .checked_add(1)
                .ok_or("too many active usage records")?
        } else {
            state.2
        };
        rate.requests += 1;
        self.persist_budget(&mut budget)?;
        state.0 = budget;
        state.1 = rate;
        state.2 = active;
        Ok(())
    }

    fn persist_budget(&self, budget: &mut BudgetState) -> Result<(), String> {
        atomic_json(&self.path, budget, self.strict.is_some()).inspect_err(|_| {
            if self.strict.is_some() {
                self.persistence_failed.store(true, Ordering::Relaxed);
            }
        })
    }

    fn check_usage_state(&self, budget: &BudgetState, now: u64) -> Result<(), String> {
        if self.persistence_failed.load(Ordering::Relaxed) {
            return Err("budget persistence previously failed".into());
        }
        if let Some(strict) = &self.strict {
            strict.verify_owner_and_marker()?;
            if read_strict_budget(&self.path)? != *budget {
                return Err("strict budget state changed while the broker was running".into());
            }
            check_strict_clock(budget, now)?;
        } else {
            reject_strict_marker(&self.path)?;
        }
        Ok(())
    }

    fn exhaust_for_overage(&self) -> Result<(), String> {
        let mut state = self.inner.lock().map_err(|_| "budget state lock failed")?;
        let now = unix_seconds();
        self.check_usage_state(&state.0, now)?;
        if state.0.tokens == u64::MAX && state.0.cost_microusd == u64::MAX {
            return Ok(());
        }
        let mut budget = state.0.clone();
        // Never undercount an unrepresentable price or a reported overage.
        // No reservation is refunded, and admission remains blocked this day.
        budget.tokens = u64::MAX;
        budget.cost_microusd = u64::MAX;
        if self.strict.is_some() {
            budget.last_seen_seconds = Some(now);
        }
        self.persist_budget(&mut budget)?;
        state.0 = budget;
        Ok(())
    }

    fn release_usage(&self, clear: bool) {
        let Ok(mut state) = self.inner.lock() else {
            return;
        };
        state.2 = state.2.saturating_sub(1);
        if !clear {
            return;
        }
        let now = unix_seconds();
        if self.check_usage_state(&state.0, now).is_err() {
            return;
        }
        let mut budget = state.0.clone();
        let Some(pending) = budget.pending_usage.and_then(|count| count.checked_sub(1)) else {
            return;
        };
        budget.pending_usage = (pending != 0).then_some(pending);
        if self.strict.is_some() {
            budget.last_seen_seconds = Some(now);
        }
        if self.persist_budget(&mut budget).is_ok() {
            state.0 = budget;
        }
    }

    fn settle_with_clock(&self, clock: impl FnOnce() -> u64) -> Result<(), String> {
        let mut state = self.inner.lock().map_err(|_| "budget state lock failed")?;
        let now = clock();
        let strict = self
            .strict
            .as_ref()
            .ok_or("cannot settle a non-strict budget")?;
        strict.verify_owner_and_marker()?;
        if read_strict_budget(&self.path)? != state.0 {
            return Err("strict budget state changed while the broker was running".into());
        }
        check_strict_clock(&state.0, now)?;
        let mut budget = state.0.clone();
        let unsettled = budget
            .unsettled
            .ok_or("strict budget is missing unsettled reservations")?;
        budget.unsettled = Some(
            unsettled
                .checked_sub(1)
                .ok_or("strict reservation was already settled")?,
        );
        // The counters stay on their admission day. Once *all* old requests
        // have settled, the next admission starts a fresh UTC day; one
        // completion never releases other unresolved old holds.
        budget.last_seen_seconds = Some(now);
        self.persist_budget(&mut budget)?;
        state.0 = budget;
        Ok(())
    }

    #[cfg(test)]
    fn settle_at(&self, now: u64) -> Result<(), String> {
        self.settle_with_clock(|| now)
    }
}

impl StrictState {
    fn acquire(path: &Path) -> Result<Self, String> {
        let lock_path = state_sidecar(path, ".lock");
        let owner = open_private(&lock_path)?;
        lock_owner(&owner, true)?;
        let state = Self {
            owner,
            lock_path,
            marker_path: state_sidecar(path, ".initialized"),
        };
        state.verify_owner_and_marker()?;
        Ok(state)
    }

    fn verify_owner_and_marker(&self) -> Result<(), String> {
        let current = open_private(&self.lock_path)?;
        let held = self
            .owner
            .metadata()
            .map_err(|error| format!("cannot stat held budget lock: {error}"))?;
        let actual = current
            .metadata()
            .map_err(|error| format!("cannot stat budget lock: {error}"))?;
        if held.dev() != actual.dev() || held.ino() != actual.ino() {
            return Err("budget lock was replaced".into());
        }
        let marker = open_private(&self.marker_path)?;
        let mut contents = Vec::new();
        marker
            .take(128)
            .read_to_end(&mut contents)
            .map_err(|error| format!("cannot read budget marker: {error}"))?;
        if contents != STRICT_MARKER {
            return Err("invalid strict budget initialization marker".into());
        }
        Ok(())
    }
}

fn state_sidecar(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(suffix);
    PathBuf::from(name)
}

fn create_private(path: &Path) -> Result<File, String> {
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)
        .map_err(|error| format!("cannot create {}: {error}", path.display()))
}

fn open_or_create_lock(path: &Path) -> Result<File, String> {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK)
        .open(path)
        .map_err(|error| format!("cannot open budget lock {}: {error}", path.display()))?;
    let metadata = file
        .metadata()
        .map_err(|error| format!("cannot stat budget lock: {error}"))?;
    if !metadata.is_file() || metadata.permissions().mode() & 0o077 != 0 || metadata.nlink() != 1 {
        return Err("budget lock must be a private regular file with one link".into());
    }
    Ok(file)
}

fn reject_strict_marker(path: &Path) -> Result<(), String> {
    match fs::symlink_metadata(state_sidecar(path, ".initialized")) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Ok(_) => Err("strict budget state requires strict cost mode".into()),
        Err(error) => Err(format!("cannot inspect strict budget marker: {error}")),
    }
}

fn open_private(path: &Path) -> Result<File, String> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK)
        .open(path)
        .map_err(|error| format!("cannot open {}: {error}", path.display()))?;
    let metadata = file
        .metadata()
        .map_err(|error| format!("cannot stat {}: {error}", path.display()))?;
    if !metadata.is_file() || metadata.permissions().mode() & 0o077 != 0 || metadata.nlink() != 1 {
        return Err(format!(
            "{} must be a private regular file with one link",
            path.display()
        ));
    }
    Ok(file)
}

fn lock_owner(file: &File, exclusive: bool) -> Result<(), String> {
    // flock is tied to this open file description and released when it closes.
    let mode = if exclusive {
        libc::LOCK_EX
    } else {
        libc::LOCK_SH
    };
    // SAFETY: the descriptor belongs to a live File and flock does not retain
    // any Rust pointer; the kernel releases the lock when that descriptor closes.
    if unsafe { libc::flock(file.as_raw_fd(), mode | libc::LOCK_NB) } != 0 {
        return Err(format!(
            "strict budget already owned or lock unavailable: {}",
            std::io::Error::last_os_error()
        ));
    }
    Ok(())
}

// Canonical, versioned fixed-width encoding excludes only the checksum itself.
// This detects accidental corruption, not malicious edits or snapshot rollback.
fn strict_digest(budget: &BudgetState) -> [u8; 32] {
    let mut context = ring::digest::Context::new(&ring::digest::SHA256);
    context.update(b"tentaflake-strict-budget-v2\0");
    for value in [
        budget.day,
        budget.requests,
        budget.tokens,
        budget.cost_microusd,
    ] {
        context.update(&value.to_be_bytes());
    }
    for value in [budget.last_seen_seconds, budget.unsettled] {
        context.update(&[u8::from(value.is_some())]);
        context.update(&value.unwrap_or(0).to_be_bytes());
    }
    if let Some(pending) = budget.pending_usage {
        // Absent fields retain the original v2 checksum for existing states.
        context.update(b"pending-usage\0");
        context.update(&pending.to_be_bytes());
    }
    let mut digest = [0_u8; 32];
    digest.copy_from_slice(context.finish().as_ref());
    digest
}

fn read_strict_budget(path: &Path) -> Result<BudgetState, String> {
    let file = open_private(path)?;
    let mut bytes = Vec::new();
    file.take(4097)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("cannot read budget state: {error}"))?;
    if bytes.len() > 4096 {
        return Err("strict budget state is too large".into());
    }
    let budget: BudgetState = serde_json::from_slice(&bytes)
        .map_err(|error| format!("invalid budget state {}: {error}", path.display()))?;
    if budget.last_seen_seconds.is_none()
        || budget.unsettled.is_none()
        || budget.checksum != Some(strict_digest(&budget))
    {
        return Err("strict budget checksum or required fields are invalid".into());
    }
    if (budget.requests == 0 && (budget.tokens != 0 || budget.cost_microusd != 0))
        || (budget.requests != 0 && (budget.tokens == 0 || budget.cost_microusd == 0))
        || budget
            .unsettled
            .is_some_and(|count| count > budget.requests)
        || budget
            .pending_usage
            .is_some_and(|count| count == 0 || count > budget.requests)
    {
        return Err("strict budget counters are inconsistent".into());
    }
    Ok(budget)
}

fn check_strict_clock(budget: &BudgetState, now: u64) -> Result<(), String> {
    let last = budget
        .last_seen_seconds
        .ok_or("strict budget is missing its clock checkpoint")?;
    if last == 0 || budget.day > last / 86_400 {
        return Err("strict budget has an invalid clock checkpoint".into());
    }
    if now < last {
        return Err("clock rollback detected for strict budget".into());
    }
    Ok(())
}

fn checkpoint_clock(path: &Path, budget: &mut BudgetState, now: u64) -> Result<(), String> {
    if budget.last_seen_seconds != Some(now) {
        let mut checkpoint = budget.clone();
        checkpoint.last_seen_seconds = Some(now);
        atomic_json(path, &mut checkpoint, true)?;
        *budget = checkpoint;
    }
    Ok(())
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
            .map_err(|error| format!("cannot write audit: {error}"))?;
        file.sync_data()
            .map_err(|error| format!("cannot sync audit log: {error}"))?;
        // Creation and rotation change directory entries. Sync the parent too
        // so a durable terminal record remains reachable after a crash.
        let parent = self.path.parent().ok_or("audit path has no parent")?;
        fs::File::open(parent)
            .and_then(|directory| directory.sync_all())
            .map_err(|error| format!("cannot sync audit directory: {error}"))
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

fn atomic_json(path: &Path, value: &mut BudgetState, strict: bool) -> Result<(), String> {
    if strict {
        value.checksum = Some(strict_digest(value));
    } else {
        ensure_parent(path)?;
    }
    let temporary = if strict {
        state_sidecar(path, ".tmp")
    } else {
        path.with_extension("tmp")
    };
    let mut options = OpenOptions::new();
    options
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW);
    let mut file = options
        .open(&temporary)
        .map_err(|error| format!("cannot create state file: {error}"))?;
    let metadata = file
        .metadata()
        .map_err(|error| format!("cannot stat state file: {error}"))?;
    if !metadata.is_file() || metadata.nlink() != 1 {
        return Err("temporary budget state must be a regular file with one link".into());
    }
    file.set_len(0)
        .map_err(|error| format!("cannot truncate state: {error}"))?;
    file.set_permissions(fs::Permissions::from_mode(0o600))
        .map_err(|error| format!("cannot protect state: {error}"))?;
    serde_json::to_writer(&mut file, value)
        .map_err(|error| format!("cannot encode state: {error}"))?;
    file.sync_all()
        .map_err(|error| format!("cannot sync state: {error}"))?;
    if strict {
        // Exchange requires the original state to exist at the atomic update;
        // plain rename would silently recreate a state deleted after validation.
        replace_existing_strict(&temporary, path)?;
        fs::remove_file(&temporary)
            .map_err(|error| format!("cannot remove prior budget state: {error}"))?;
        sync_parent(path)?;
    } else {
        fs::rename(&temporary, path).map_err(|error| format!("cannot replace state: {error}"))?;
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn replace_existing_strict(temporary: &Path, path: &Path) -> Result<(), String> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    let temporary = CString::new(temporary.as_os_str().as_bytes())
        .map_err(|_| "invalid temporary budget path")?;
    let path = CString::new(path.as_os_str().as_bytes()).map_err(|_| "invalid budget path")?;
    // SAFETY: both NUL-terminated paths remain alive for the system call and
    // AT_FDCWD selects the current directory for the supplied absolute paths.
    let result = unsafe {
        libc::renameat2(
            libc::AT_FDCWD,
            temporary.as_ptr(),
            libc::AT_FDCWD,
            path.as_ptr(),
            libc::RENAME_EXCHANGE,
        )
    };
    if result != 0 {
        return Err(format!(
            "cannot exchange existing budget state: {}",
            std::io::Error::last_os_error()
        ));
    }
    Ok(())
}

#[cfg(not(target_os = "linux"))]
fn replace_existing_strict(_temporary: &Path, _path: &Path) -> Result<(), String> {
    Err("strict budget replacement requires Linux renameat2".into())
}

fn sync_parent(path: &Path) -> Result<(), String> {
    let parent = path.parent().ok_or("state path has no parent")?;
    File::open(parent)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| format!("cannot sync budget directory: {error}"))
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

    fn strict_fixture(name: &str) -> (PathBuf, PathBuf) {
        let directory = test_path(name);
        fs::create_dir(&directory).unwrap();
        let budget = directory.join("budget.json");
        (directory, budget)
    }

    fn strict_limits(path: &Path, now: u64) -> Result<Limits, String> {
        Limits::load_at(path.to_owned(), 60, 1, 5, 100, 100, Some(now))
    }

    #[test]
    fn strict_budget_initialization_ownership_and_held_reservation() {
        let (directory, budget) = strict_fixture("strict-owner");
        let now = 200_000;
        assert!(strict_limits(&budget, now).is_err());
        Limits::initialize_strict_at(&budget, now).unwrap();
        assert!(Limits::initialize_strict_at(&budget, now).is_err());
        let first = strict_limits(&budget, now).unwrap();
        assert!(strict_limits(&budget, now).is_err());
        first.reserve_at(37, 21, now).unwrap();
        drop(first);

        // An uncertain dispatch has no surviving completion capability.
        let restarted = strict_limits(&budget, now + 60).unwrap();
        assert_eq!(restarted.inner.lock().unwrap().0.tokens, 37);
        assert_eq!(restarted.inner.lock().unwrap().0.cost_microusd, 21);
        assert!(restarted.reserve_at(64, 1, now + 60).is_err());
        assert!(restarted.reserve_at(1, 80, now + 60).is_err());
        drop(restarted);
        fs::remove_file(&budget).unwrap();
        assert!(strict_limits(&budget, now + 60).is_err());
        assert!(Limits::initialize_strict_at(&budget, now + 60).is_err());
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn strict_usage_guards_allow_live_concurrency_without_refunds() {
        let (directory, budget) = strict_fixture("strict-usage-concurrency");
        Limits::initialize_strict(&budget).unwrap();
        let limits = Arc::new(Limits::load(budget.clone(), 60, 10, 5, 100, 100, true).unwrap());
        let (first, first_guard) = limits.reserve_llm(10, 5).unwrap();
        let (second, second_guard) = limits.reserve_llm(20, 7).unwrap();
        assert_eq!(read_strict_budget(&budget).unwrap().pending_usage, Some(2));
        drop(first_guard);
        drop(second_guard);
        drop(first);
        drop(second);
        let state = read_strict_budget(&budget).unwrap();
        assert_eq!(state.pending_usage, None);
        assert_eq!(
            (state.requests, state.tokens, state.cost_microusd),
            (2, 30, 12)
        );
        assert_eq!(state.unsettled, Some(0));
        drop(limits);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn reported_overage_blocks_admission_while_its_update_waits_for_the_lock() {
        use std::time::{Duration, Instant};

        let (directory, budget) = strict_fixture("strict-usage-admission-race");
        Limits::initialize_strict(&budget).unwrap();
        let limits = Arc::new(Limits::load(budget.clone(), 60, 10, 5, 100, 100, true).unwrap());
        let (reservation, guard) = limits.reserve_llm(10, 5).unwrap();
        let held = limits.inner.lock().unwrap();
        let worker = std::thread::spawn(move || guard.unwrap().observe(11, Some(5)));
        let deadline = Instant::now() + Duration::from_secs(5);
        while !limits.overage_pending.load(Ordering::SeqCst) {
            assert!(Instant::now() < deadline, "overage observer did not start");
            std::thread::yield_now();
        }
        drop(held);
        assert!(limits.reserve_llm(1, 1).is_err());
        worker.join().unwrap().unwrap();
        drop(reservation);
        assert_eq!(read_strict_budget(&budget).unwrap().tokens, u64::MAX);
        drop(limits);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn strict_failed_overage_persistence_blocks_admission_after_restart() {
        use std::os::unix::fs::symlink;

        let (directory, budget) = strict_fixture("strict-usage-write-failure");
        Limits::initialize_strict(&budget).unwrap();
        let limits = Arc::new(Limits::load(budget.clone(), 60, 10, 5, 100, 100, true).unwrap());
        let (reservation, guard) = limits.reserve_llm(20, 5).unwrap();
        let reservation = reservation.unwrap();
        reservation.mark_dispatched();
        let guard = guard.unwrap();
        let target = directory.join("unrelated");
        fs::write(&target, b"untouched").unwrap();
        let temporary = state_sidecar(&budget, ".tmp");
        symlink(&target, &temporary).unwrap();
        assert!(guard.observe(21, Some(5)).is_err());
        assert_eq!(fs::read(&target).unwrap(), b"untouched");
        fs::remove_file(temporary).unwrap();
        drop(guard);
        drop(reservation);
        assert!(limits.reserve_llm(1, 1).is_err());
        assert_eq!(read_strict_budget(&budget).unwrap().pending_usage, Some(1));
        drop(limits);
        let restarted = Arc::new(Limits::load(budget.clone(), 60, 10, 5, 100, 100, true).unwrap());
        assert!(restarted.reserve_llm(1, 1).is_err());
        assert_eq!(read_strict_budget(&budget).unwrap().tokens, 20);
        drop(restarted);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn strict_crash_and_clock_rollback_preserve_usage_holds() {
        let (directory, budget) = strict_fixture("strict-usage-crash");
        let now = unix_seconds();
        Limits::initialize_strict_at(&budget, now).unwrap();
        let first = strict_limits(&budget, now).unwrap();
        first.reserve_with_clock(20, 5, true, || now).unwrap();
        drop(first);
        let restarted = strict_limits(&budget, now).unwrap();
        assert!(
            restarted
                .reserve_at(1, 1, now)
                .unwrap_err()
                .contains("usage")
        );
        assert_eq!(read_strict_budget(&budget).unwrap().pending_usage, Some(1));
        drop(restarted);
        fs::remove_dir_all(directory).unwrap();

        let (directory, budget) = strict_fixture("strict-usage-rollback");
        let future = unix_seconds() + 300;
        Limits::initialize_strict_at(&budget, future).unwrap();
        let limits = Arc::new(strict_limits(&budget, future).unwrap());
        limits.reserve_with_clock(20, 5, true, || future).unwrap();
        let guard = UsageGuard {
            limits: Arc::clone(&limits),
            reserved_tokens: 20,
            reserved_cost: 5,
            failed: AtomicBool::new(false),
        };
        assert!(guard.observe(21, Some(5)).unwrap_err().contains("rollback"));
        drop(guard);
        assert!(limits.reserve_at(1, 1, future).is_err());
        drop(limits);
        let restarted = strict_limits(&budget, future).unwrap();
        assert!(restarted.reserve_at(1, 1, future).is_err());
        drop(restarted);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn only_definitely_undispatched_reservation_drop_clears_uncertainty() {
        let (directory, budget) = strict_fixture("strict-dispatch-phase");
        Limits::initialize_strict(&budget).unwrap();
        let limits = Arc::new(Limits::load(budget.clone(), 60, 10, 5, 100, 100, true).unwrap());
        let preflight = limits.reserve(10, 5).unwrap().unwrap();
        let attempted = limits.reserve(20, 7).unwrap().unwrap();
        assert_eq!(read_strict_budget(&budget).unwrap().unsettled, Some(2));
        drop(preflight);
        let state = read_strict_budget(&budget).unwrap();
        assert_eq!(
            (
                state.requests,
                state.tokens,
                state.cost_microusd,
                state.unsettled
            ),
            (2, 30, 12, Some(1))
        );
        attempted.mark_dispatched();
        drop(attempted);
        assert_eq!(read_strict_budget(&budget).unwrap().unsettled, Some(1));
        drop(limits);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn strict_provisioning_cannot_race_a_legacy_owner() {
        let (directory, budget) = strict_fixture("strict-legacy-owner");
        let now = 200_000;
        let legacy = Limits::load_at(budget.clone(), 60, 1, 5, 100, 100, None).unwrap();
        let second_legacy = Limits::load_at(budget.clone(), 60, 1, 5, 100, 100, None).unwrap();
        assert!(Limits::initialize_strict_at(&budget, now).is_err());
        drop(legacy);
        drop(second_legacy);
        Limits::initialize_strict_at(&budget, now).unwrap();
        let strict = strict_limits(&budget, now).unwrap();
        assert!(Limits::load_at(budget.clone(), 60, 1, 5, 100, 100, None).is_err());
        drop(strict);
        assert!(Limits::load_at(budget.clone(), 60, 1, 5, 100, 100, None).is_err());
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn strict_budget_rejects_clock_rollback_even_within_a_day_or_window() {
        let (directory, budget) = strict_fixture("strict-clock");
        let now = 200_000;
        Limits::initialize_strict_at(&budget, now).unwrap();
        let limits = strict_limits(&budget, now).unwrap();
        limits.reserve_at(10, 5, now + 20).unwrap();
        let saved = fs::read(&budget).unwrap();
        assert!(
            limits
                .reserve_at(1, 1, now + 19)
                .unwrap_err()
                .contains("rollback")
        );
        assert!(
            limits
                .reserve_at(1, 1, 86_399)
                .unwrap_err()
                .contains("rollback")
        );
        assert_eq!(fs::read(&budget).unwrap(), saved);
        // Even denied observations advance the durable clock checkpoint, so a
        // rollback cannot evade the rate window or reopen a prior spend day.
        assert!(limits.reserve_at(1, 1, now + 21).is_err());
        assert!(
            limits
                .reserve_at(1, 1, now + 20)
                .unwrap_err()
                .contains("rollback")
        );
        limits.settle_at(now + 21).unwrap();
        assert!(limits.reserve_at(101, 1, 259_200).is_err());
        let checkpoint = read_strict_budget(&budget).unwrap();
        assert_eq!(checkpoint.requests, 1);
        assert_eq!(checkpoint.tokens, 10);
        assert_eq!(checkpoint.cost_microusd, 5);
        assert_eq!(checkpoint.last_seen_seconds, Some(259_200));
        drop(limits);
        assert!(strict_limits(&budget, 259_199).is_err());
        let restarted = strict_limits(&budget, 259_200).unwrap();
        restarted.reserve_at(1, 1, 259_200).unwrap();
        drop(restarted);
        let observed_at_start = strict_limits(&budget, 259_210).unwrap();
        assert!(
            observed_at_start
                .reserve_at(1, 1, 259_209)
                .unwrap_err()
                .contains("rollback")
        );
        drop(observed_at_start);
        assert!(strict_limits(&budget, 259_209).is_err());
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn unresolved_reservation_blocks_rollover_across_restart() {
        let (directory, budget) = strict_fixture("strict-midnight-hold");
        let end = 259_199;
        Limits::initialize_strict_at(&budget, end).unwrap();
        let limits = strict_limits(&budget, end).unwrap();
        limits.reserve_at(20, 30, end).unwrap();
        // Simulate a lost or timed-out response and a process restart.
        drop(limits);
        let restarted = strict_limits(&budget, end + 1).unwrap();
        assert!(
            restarted
                .reserve_at(1, 1, end + 1)
                .unwrap_err()
                .contains("unsettled")
        );
        let held = read_strict_budget(&budget).unwrap();
        assert_eq!(
            (
                held.requests,
                held.tokens,
                held.cost_microusd,
                held.unsettled
            ),
            (1, 20, 30, Some(1))
        );
        drop(restarted);
        let restarted_again = strict_limits(&budget, end + 86_401).unwrap();
        assert!(restarted_again.reserve_at(1, 1, end + 86_401).is_err());
        assert_eq!(read_strict_budget(&budget).unwrap().cost_microusd, 30);
        drop(restarted_again);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn completion_after_midnight_does_not_release_another_older_hold() {
        let (directory, budget) = strict_fixture("strict-midnight-mixed");
        let end = 259_199;
        Limits::initialize_strict_at(&budget, end - 60).unwrap();
        let limits = strict_limits(&budget, end - 60).unwrap();
        limits.reserve_at(10, 20, end - 60).unwrap(); // A remains unresolved.
        limits.reserve_at(15, 30, end).unwrap(); // B completes after midnight.
        limits.settle_at(end + 1).unwrap();
        let held = read_strict_budget(&budget).unwrap();
        assert_eq!(held.day, end / 86_400);
        assert_eq!(
            (
                held.requests,
                held.tokens,
                held.cost_microusd,
                held.unsettled
            ),
            (2, 25, 50, Some(1))
        );
        assert!(
            limits
                .reserve_at(1, 1, end + 1)
                .unwrap_err()
                .contains("unsettled")
        );
        drop(limits);
        let restarted = strict_limits(&budget, end + 1).unwrap();
        assert!(
            restarted
                .reserve_at(1, 1, end + 1)
                .unwrap_err()
                .contains("unsettled")
        );
        assert_eq!(read_strict_budget(&budget).unwrap().cost_microusd, 50);
        drop(restarted);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn settled_spend_ages_out_including_cross_midnight_completion() {
        let (directory, budget) = strict_fixture("strict-midnight-settled");
        let end = 259_199;
        Limits::initialize_strict_at(&budget, end - 1).unwrap();
        let limits = strict_limits(&budget, end - 1).unwrap();
        limits.reserve_at(20, 30, end - 1).unwrap();
        limits.settle_at(end).unwrap();
        drop(limits);
        let restarted = strict_limits(&budget, end + 1).unwrap();
        restarted.reserve_at(5, 7, end + 1).unwrap();
        let state = read_strict_budget(&budget).unwrap();
        assert_eq!(
            (
                state.requests,
                state.tokens,
                state.cost_microusd,
                state.unsettled
            ),
            (1, 5, 7, Some(1))
        );
        drop(restarted);
        fs::remove_dir_all(directory).unwrap();

        let (directory, budget) = strict_fixture("strict-midnight-completing");
        Limits::initialize_strict_at(&budget, end).unwrap();
        let limits = strict_limits(&budget, end).unwrap();
        limits.reserve_at(20, 30, end).unwrap();
        assert!(
            limits
                .reserve_at(1, 1, end + 1)
                .unwrap_err()
                .contains("unsettled")
        );
        limits.settle_at(end + 1).unwrap();
        // The settled request stays charged to its admission day only.
        let settled = read_strict_budget(&budget).unwrap();
        assert_eq!(
            (settled.day, settled.cost_microusd, settled.unsettled),
            (end / 86_400, 30, Some(0))
        );
        limits.reserve_at(5, 7, end + 1).unwrap();
        let state = read_strict_budget(&budget).unwrap();
        assert_eq!(
            (
                state.day,
                state.requests,
                state.tokens,
                state.cost_microusd,
                state.unsettled
            ),
            ((end + 1) / 86_400, 1, 5, 7, Some(1))
        );
        drop(limits);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn cross_midnight_usage_hold_release_does_not_carry_counters() {
        let (directory, budget) = strict_fixture("strict-midnight-usage");
        // The usage release samples the real clock, so admit just before today.
        let end = unix_seconds() / 86_400 * 86_400 - 1;
        Limits::initialize_strict_at(&budget, end).unwrap();
        let limits = Arc::new(strict_limits(&budget, end).unwrap());
        limits.reserve_with_clock(90, 90, true, || end).unwrap();
        // Release after midnight, then settle the matching reservation.
        limits.release_usage(true);
        limits.settle_with_clock(unix_seconds).unwrap();
        let now = unix_seconds();
        limits.reserve_at(5, 7, now).unwrap();
        let state = read_strict_budget(&budget).unwrap();
        assert_eq!(
            (state.day, state.tokens, state.cost_microusd),
            (now / 86_400, 5, 7)
        );
        drop(limits);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn reconciliation_resolves_holds_without_refunds_and_requires_ownership() {
        let (directory, budget) = strict_fixture("strict-reconcile");
        let now = 200_000;
        assert!(Limits::reconcile_strict_at(&budget, now).is_err());
        Limits::initialize_strict_at(&budget, now).unwrap();
        let limits = strict_limits(&budget, now).unwrap();
        limits.reserve_with_clock(20, 5, true, || now).unwrap();
        // The running broker owns the state; reconciliation must not race it.
        assert!(Limits::reconcile_strict_at(&budget, now).is_err());
        drop(limits);
        assert!(
            Limits::reconcile_strict_at(&budget, now - 1)
                .unwrap_err()
                .contains("rollback")
        );
        assert_eq!(Limits::reconcile_strict_at(&budget, now + 1), Ok((1, 1)));
        let state = read_strict_budget(&budget).unwrap();
        assert_eq!(
            (
                state.requests,
                state.tokens,
                state.cost_microusd,
                state.unsettled,
                state.pending_usage
            ),
            (1, 20, 5, Some(0), None)
        );
        let restarted = strict_limits(&budget, now + 86_400).unwrap();
        restarted.reserve_at(1, 1, now + 86_400).unwrap();
        drop(restarted);
        fs::remove_file(state_sidecar(&budget, ".initialized")).unwrap();
        assert!(Limits::reconcile_strict_at(&budget, now + 86_400).is_err());
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn strict_digest_rejects_valid_json_counter_corruption_after_restart() {
        let (directory, budget) = strict_fixture("strict-checksum");
        let now = 200_000;
        Limits::initialize_strict_at(&budget, now).unwrap();
        let limits = strict_limits(&budget, now).unwrap();
        limits.reserve_at(10, 50, now).unwrap();
        drop(limits);
        let mut state: serde_json::Value =
            serde_json::from_slice(&fs::read(&budget).unwrap()).unwrap();
        // This remains valid JSON with nonzero, otherwise consistent counters.
        state["cost_microusd"] = json!(1);
        fs::write(&budget, serde_json::to_vec(&state).unwrap()).unwrap();
        let error = strict_limits(&budget, now).err().unwrap();
        assert!(error.contains("checksum"), "{error}");
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn reservation_samples_clock_only_after_acquiring_mutex() {
        use std::sync::mpsc;
        use std::time::Duration;

        let (directory, budget) = strict_fixture("strict-lock-clock");
        let now = 200_000;
        Limits::initialize_strict_at(&budget, now).unwrap();
        let limits = Arc::new(strict_limits(&budget, now).unwrap());
        let held = limits.inner.lock().unwrap();
        let (started_tx, started_rx) = mpsc::channel();
        let (clock_tx, clock_rx) = mpsc::channel();
        let worker = Arc::clone(&limits);
        let thread = std::thread::spawn(move || {
            started_tx.send(()).unwrap();
            worker.reserve_with_clock(1, 1, false, || {
                clock_tx.send(()).unwrap();
                now + 1
            })
        });
        started_rx.recv().unwrap();
        assert!(clock_rx.recv_timeout(Duration::from_millis(100)).is_err());
        drop(held);
        clock_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        thread.join().unwrap().unwrap();
        drop(limits);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn strict_budget_detects_missing_corrupt_replaced_and_symlink_state() {
        use std::os::unix::fs::symlink;

        let (directory, budget) = strict_fixture("strict-tamper");
        let now = 200_000;
        Limits::initialize_strict_at(&budget, now).unwrap();
        let limits = strict_limits(&budget, now).unwrap();
        let saved = fs::read(&budget).unwrap();
        fs::remove_file(&budget).unwrap();
        assert!(limits.reserve_at(1, 1, now).is_err());
        assert!(!budget.exists());
        let mut saved_state: BudgetState = serde_json::from_slice(&saved).unwrap();
        assert!(atomic_json(&budget, &mut saved_state, true).is_err());
        assert!(!budget.exists());
        fs::remove_file(state_sidecar(&budget, ".tmp")).unwrap();
        fs::write(&budget, b"not-json").unwrap();
        fs::set_permissions(&budget, fs::Permissions::from_mode(0o600)).unwrap();
        assert!(limits.reserve_at(1, 1, now).is_err());
        drop(limits);
        assert!(strict_limits(&budget, now).is_err());
        let mut inconsistent: serde_json::Value = serde_json::from_slice(&saved).unwrap();
        inconsistent["tokens"] = json!(1);
        fs::write(&budget, serde_json::to_vec(&inconsistent).unwrap()).unwrap();
        assert!(strict_limits(&budget, now).is_err());
        fs::write(&budget, &saved).unwrap();
        let limits = strict_limits(&budget, now).unwrap();
        let marker = state_sidecar(&budget, ".initialized");
        fs::write(&marker, b"bad marker").unwrap();
        assert!(limits.reserve_at(1, 1, now).is_err());
        fs::write(&marker, STRICT_MARKER).unwrap();
        let temporary = state_sidecar(&budget, ".tmp");
        let target = directory.join("target");
        fs::write(&target, b"untouched").unwrap();
        fs::remove_file(&marker).unwrap();
        symlink(&target, &marker).unwrap();
        assert!(limits.reserve_at(1, 1, now).is_err());
        assert_eq!(fs::read(&target).unwrap(), b"untouched");
        fs::remove_file(&marker).unwrap();
        fs::write(&marker, STRICT_MARKER).unwrap();
        fs::set_permissions(&marker, fs::Permissions::from_mode(0o600)).unwrap();
        symlink(&target, &temporary).unwrap();
        assert!(limits.reserve_at(1, 1, now).is_err());
        assert_eq!(fs::read(&target).unwrap(), b"untouched");
        fs::remove_file(&temporary).unwrap();
        fs::remove_file(&budget).unwrap();
        symlink(&target, &budget).unwrap();
        assert!(limits.reserve_at(1, 1, now).is_err());
        drop(limits);
        assert!(strict_limits(&budget, now).is_err());
        fs::remove_file(&budget).unwrap();
        fs::write(&budget, &saved).unwrap();
        fs::set_permissions(&budget, fs::Permissions::from_mode(0o600)).unwrap();
        let lock = state_sidecar(&budget, ".lock");
        let limits = strict_limits(&budget, now).unwrap();
        fs::remove_file(&lock).unwrap();
        fs::write(&lock, b"replacement").unwrap();
        fs::set_permissions(&lock, fs::Permissions::from_mode(0o600)).unwrap();
        assert!(limits.reserve_at(1, 1, now).is_err());
        drop(limits);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn constant_time_comparison_handles_lengths() {
        assert!(constant_time_eq(b"secret", b"secret"));
        assert!(!constant_time_eq(b"secret", b"secreu"));
        assert!(!constant_time_eq(b"secret", b"secret-long"));
    }

    #[test]
    fn audit_record_fails_when_durable_sync_is_unavailable() {
        // /dev/null accepts the complete JSON write but rejects fdatasync.
        // This detects a write-only implementation without mocking the filesystem.
        let audit = Audit::new(PathBuf::from("/dev/null"), u64::MAX);
        let error = audit
            .record("fixture", "llm", "completed", json!({ "streaming": true }))
            .unwrap_err();
        assert!(error.contains("cannot sync audit log"), "{error}");
    }

    #[test]
    fn durable_audit_records_survive_rotation() {
        let path = test_path("rotation");
        let audit = Audit::new(path.clone(), 1);
        for outcome in ["ready", "completed"] {
            audit.record("fixture", "llm", outcome, json!({})).unwrap();
        }
        let rotated = path.with_extension("jsonl.1");
        assert!(
            fs::read_to_string(&rotated)
                .unwrap()
                .contains("\"outcome\":\"ready\"")
        );
        assert!(
            fs::read_to_string(&path)
                .unwrap()
                .contains("\"outcome\":\"completed\"")
        );
        fs::remove_file(path).unwrap();
        fs::remove_file(rotated).unwrap();
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
