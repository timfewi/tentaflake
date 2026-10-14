//! Private-origin authentication. Grants remain the server's live authority;
//! only consumed operator-token fingerprints belong in the durable ledger.
use super::credential::{self, runtime_path, secret_shape};
use hyper::http::{HeaderMap, Uri};
use ring::{
    digest, hmac,
    rand::{SecureRandom, SystemRandom},
};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tentaflake_admission::protected_path::open_root;

const COOKIE: &str = "__Host-tentaflake-session";
pub(crate) const EXPIRED_COOKIE: &str =
    "__Host-tentaflake-session=; Secure; HttpOnly; SameSite=Strict; Path=/; Max-Age=0";
const MAX_SESSIONS: usize = 256;
const IDLE: Duration = Duration::from_secs(15 * 60);
const ABSOLUTE: Duration = Duration::from_secs(8 * 60 * 60);
const LOGIN_WINDOW: Duration = Duration::from_secs(60);
const LOGIN_ATTEMPTS: u8 = 10;

pub(crate) struct AuthSettings {
    pub origin: String,
    pub operator_credential: Option<PathBuf>,
    pub bearer_credential: Option<PathBuf>,
}

#[derive(Clone, Copy)]
pub(crate) struct AuthContext<'a> {
    pub principal_id: &'a str,
    pub auth_binding: &'a str,
    pub mutation_epoch: &'a str,
    pub enabled: bool,
}

#[derive(Clone, Copy)]
pub(crate) struct Clock {
    pub monotonic: Instant,
    pub unix_seconds: u64,
}

impl Clock {
    pub(crate) fn now() -> Result<Self, AuthError> {
        Ok(Self {
            monotonic: Instant::now(),
            unix_seconds: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_err(|_| AuthError::Unavailable)?
                .as_secs(),
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AuthError {
    Invalid,
    Unauthenticated,
    SessionExpired,
    SessionRevoked,
    Csrf,
    Origin,
    RateLimited,
    Capacity,
    Unsupported,
    NotFound,
    Unavailable,
}

impl AuthError {
    pub(crate) fn code(self) -> &'static str {
        match self {
            Self::Invalid => "invalid",
            Self::Unauthenticated => "unauthenticated",
            Self::SessionExpired => "session_expired",
            Self::SessionRevoked => "session_revoked",
            Self::Csrf => "csrf_failed",
            Self::Origin => "forbidden",
            Self::RateLimited | Self::Capacity => "rate_limited",
            Self::Unsupported => "unsupported",
            Self::NotFound => "not_found",
            Self::Unavailable => "unavailable",
        }
    }

    pub(crate) fn status(self) -> u16 {
        match self {
            Self::Invalid => 400,
            Self::Unauthenticated | Self::SessionExpired | Self::SessionRevoked => 401,
            Self::Csrf | Self::Origin => 403,
            Self::RateLimited | Self::Capacity => 429,
            Self::Unsupported => 501,
            Self::NotFound => 404,
            Self::Unavailable => 503,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum RequestKind {
    Read,
    Mutation,
    Stream,
}

// Neither credentials nor CSRF/session values implement Debug or Serialize.
pub(crate) struct Authenticated {
    pub principal_id: String,
    pub csrf_token: String,
    pub expires_at: u64,
    binding: String,
    epoch: String,
    identity: Identity,
}

enum Identity {
    Cookie([u8; 32]),
    Bearer(hmac::Tag),
}

pub(crate) struct NewSession {
    pub auth: Authenticated,
    pub set_cookie: String,
}

struct Session {
    principal: String,
    binding: String,
    epoch: String,
    csrf: String,
    issued: Clock,
    touched: Instant,
}

impl Session {
    fn active(&self, now: Instant) -> bool {
        now.checked_duration_since(self.issued.monotonic)
            .is_some_and(|age| age < ABSOLUTE)
            && now
                .checked_duration_since(self.touched)
                .is_some_and(|age| age < IDLE)
    }

    fn bound(&self, current: AuthContext<'_>) -> bool {
        current.enabled
            && self.principal == current.principal_id
            && self.binding == current.auth_binding
            && self.epoch == current.mutation_epoch
    }

    fn authenticated(&self, key: [u8; 32]) -> Authenticated {
        let idle_end = self
            .touched
            .duration_since(self.issued.monotonic)
            .saturating_add(IDLE);
        Authenticated {
            principal_id: self.principal.clone(),
            csrf_token: self.csrf.clone(),
            expires_at: self
                .issued
                .unix_seconds
                .saturating_add(idle_end.min(ABSOLUTE).as_secs()),
            binding: self.binding.clone(),
            epoch: self.epoch.clone(),
            identity: Identity::Cookie(key),
        }
    }
}

pub(crate) struct Sessions {
    settings: AuthSettings,
    authority: String,
    comparisons: hmac::Key,
    bearer_csrf: String,
    sessions: BTreeMap<[u8; 32], Session>,
    login_window: Option<Instant>,
    login_attempts: u8,
}

impl Sessions {
    #[cfg(test)]
    pub(super) fn fixture_login(&mut self, current: AuthContext<'_>) -> NewSession {
        let mut headers = HeaderMap::new();
        headers.insert("host", self.authority.parse().unwrap());
        headers.insert("origin", self.settings.origin.parse().unwrap());
        headers.insert("content-type", "application/json".parse().unwrap());
        let secret = b"synthetic-operation-login-0123456789";
        self.login_with(&headers,
            &serde_json::to_vec(&serde_json::json!({"method":"operator-token","secret":std::str::from_utf8(secret).unwrap()})).unwrap(),
            current, Clock::now().unwrap(), |_| Ok(secret.to_vec()), |_| Ok(())).unwrap()
    }

    pub(crate) fn new(settings: AuthSettings) -> Result<Self, AuthError> {
        let origin: Uri = settings.origin.parse().map_err(|_| AuthError::Invalid)?;
        let authority = origin.authority().ok_or(AuthError::Invalid)?;
        let port = authority
            .as_str()
            .strip_prefix(authority.host())
            .ok_or(AuthError::Invalid)?;
        let valid_port = port.is_empty()
            || port
                .strip_prefix(':')
                .and_then(|port| port.parse::<u16>().ok())
                .is_some_and(|port| port != 0);
        if origin.scheme_str() != Some("https")
            || authority.host().is_empty()
            || !valid_port
            || authority.as_str().contains('@')
            || settings.origin != format!("https://{authority}")
            || settings.origin.len() > 2048
            || settings
                .operator_credential
                .iter()
                .chain(&settings.bearer_credential)
                .any(|path| !runtime_path(path))
            || settings
                .operator_credential
                .as_ref()
                .is_some_and(|path| Some(path) == settings.bearer_credential.as_ref())
        {
            return Err(AuthError::Invalid);
        }
        let authority = authority.as_str().to_owned();
        let mut key = [0; 32];
        SystemRandom::new()
            .fill(&mut key)
            .map_err(|_| AuthError::Unavailable)?;
        Ok(Self {
            settings,
            authority,
            comparisons: hmac::Key::new(hmac::HMAC_SHA256, &key),
            bearer_csrf: random_token()?,
            sessions: BTreeMap::new(),
            login_window: None,
            login_attempts: 0,
        })
    }

    /// This applies to discovery too. Forwarded headers never select authority.
    pub(crate) fn validate_public_request(&self, headers: &HeaderMap) -> Result<(), AuthError> {
        if single(headers, "host")? != Some(self.authority.as_str()) {
            return Err(AuthError::Origin);
        }
        if single(headers, "origin")?.is_some_and(|origin| origin != self.settings.origin) {
            return Err(AuthError::Origin);
        }
        Ok(())
    }

    fn require_origin(&self, headers: &HeaderMap) -> Result<(), AuthError> {
        self.validate_public_request(headers)?;
        if single(headers, "origin")? != Some(self.settings.origin.as_str()) {
            return Err(AuthError::Origin);
        }
        Ok(())
    }

    fn throttle(&mut self, now: Instant) -> Result<(), AuthError> {
        if self.login_window.is_none_or(|start| {
            now.checked_duration_since(start)
                .is_some_and(|elapsed| elapsed >= LOGIN_WINDOW)
        }) {
            self.login_window = Some(now);
            self.login_attempts = 0;
        }
        if self.login_attempts >= LOGIN_ATTEMPTS {
            return Err(AuthError::RateLimited);
        }
        self.login_attempts += 1;
        Ok(())
    }

    pub(crate) fn login(
        &mut self,
        headers: &HeaderMap,
        body: &[u8],
        current: AuthContext<'_>,
        clock: Clock,
        consume: impl FnOnce(&str) -> Result<(), AuthError>,
    ) -> Result<NewSession, AuthError> {
        self.login_with(headers, body, current, clock, read_secret, consume)
    }

    fn login_with(
        &mut self,
        headers: &HeaderMap,
        body: &[u8],
        current: AuthContext<'_>,
        clock: Clock,
        mut read: impl FnMut(&Path) -> Result<Vec<u8>, AuthError>,
        consume: impl FnOnce(&str) -> Result<(), AuthError>,
    ) -> Result<NewSession, AuthError> {
        self.require_origin(headers)?;
        if single(headers, "content-type")? != Some("application/json") || body.len() > 32768 {
            return Err(AuthError::Invalid);
        }
        let credentials = credentials(headers)?;
        if credentials.bearer.is_some() {
            return Err(AuthError::Invalid);
        }
        self.throttle(clock.monotonic)?;
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Login {
            method: String,
            secret: String,
        }
        let login: Login = serde_json::from_slice(body).map_err(|_| AuthError::Invalid)?;
        if login.method != "operator-token" || !secret_shape(login.secret.as_bytes()) {
            return Err(AuthError::Unauthenticated);
        }
        valid_context(current)?;
        let path = self
            .settings
            .operator_credential
            .as_ref()
            .ok_or(AuthError::Unsupported)?;
        let expected =
            self.read_independent(path, self.settings.bearer_credential.as_deref(), &mut read)?;
        if !secret_shape(&expected)
            || !matches_secret(&self.comparisons, &expected, login.secret.as_bytes())
        {
            return Err(AuthError::Unauthenticated);
        }
        self.sessions
            .retain(|_, session| session.active(clock.monotonic) && session.bound(current));
        let previous = credentials.cookie.map(|cookie| sha256(cookie.as_bytes()));
        if self.sessions.len() >= MAX_SESSIONS
            && !previous.is_some_and(|key| self.sessions.contains_key(&key))
        {
            return Err(AuthError::Capacity);
        }
        let cookie = random_token()?;
        let key = sha256(cookie.as_bytes());
        if self.sessions.contains_key(&key) {
            return Err(AuthError::Unavailable);
        }
        let session = Session {
            principal: current.principal_id.into(),
            binding: current.auth_binding.into(),
            epoch: current.mutation_epoch.into(),
            csrf: random_token()?,
            issued: clock,
            touched: clock.monotonic,
        };
        let result = NewSession {
            auth: session.authenticated(key),
            set_cookie: format!("{COOKIE}={cookie}; Secure; HttpOnly; SameSite=Strict; Path=/"),
        };
        let mut digest = digest::Context::new(&digest::SHA256);
        digest.update(b"tentaflake/operator-login/v1\0");
        digest.update(login.secret.as_bytes());
        // A crash after this callback burns the credential. No success or new
        // in-memory session exists before its durable single-use tombstone.
        consume(&hex(digest.finish().as_ref()))?;
        if let Some(previous) = previous {
            self.sessions.remove(&previous);
        }
        self.sessions.insert(key, session);
        Ok(result)
    }

    pub(crate) fn authenticate(
        &mut self,
        headers: &HeaderMap,
        kind: RequestKind,
        current: AuthContext<'_>,
        clock: Clock,
    ) -> Result<Authenticated, AuthError> {
        self.authenticate_with(headers, kind, current, clock, read_secret)
    }

    fn authenticate_with(
        &mut self,
        headers: &HeaderMap,
        kind: RequestKind,
        current: AuthContext<'_>,
        clock: Clock,
        mut read: impl FnMut(&Path) -> Result<Vec<u8>, AuthError>,
    ) -> Result<Authenticated, AuthError> {
        self.validate_public_request(headers)?;
        valid_context(current)?;
        let credentials = credentials(headers)?;
        if let Some(bearer) = credentials.bearer {
            let path = self
                .settings
                .bearer_credential
                .as_ref()
                .ok_or(AuthError::Unauthenticated)?;
            let expected = self.read_independent(
                path,
                self.settings.operator_credential.as_deref(),
                &mut read,
            )?;
            if !secret_shape(&expected)
                || !matches_secret(&self.comparisons, &expected, bearer.as_bytes())
            {
                return Err(AuthError::Unauthenticated);
            }
            return Ok(Authenticated {
                principal_id: current.principal_id.into(),
                csrf_token: self.bearer_csrf.clone(),
                expires_at: clock.unix_seconds.saturating_add(IDLE.as_secs()),
                binding: current.auth_binding.into(),
                epoch: current.mutation_epoch.into(),
                identity: Identity::Bearer(hmac::sign(&self.comparisons, bearer.as_bytes())),
            });
        }
        let cookie = credentials.cookie.ok_or(AuthError::Unauthenticated)?;
        if kind != RequestKind::Read {
            self.require_origin(headers)?;
        }
        let key = sha256(cookie.as_bytes());
        let session = self
            .sessions
            .get_mut(&key)
            .ok_or(AuthError::Unauthenticated)?;
        if !session.bound(current) {
            self.sessions.remove(&key);
            return Err(AuthError::SessionRevoked);
        }
        if !session.active(clock.monotonic) {
            self.sessions.remove(&key);
            return Err(AuthError::SessionExpired);
        }
        if kind == RequestKind::Mutation {
            let csrf = single(headers, "x-tentaflake-csrf")?.ok_or(AuthError::Csrf)?;
            if !matches_secret(&self.comparisons, session.csrf.as_bytes(), csrf.as_bytes()) {
                return Err(AuthError::Csrf);
            }
        }
        session.touched = clock.monotonic;
        Ok(session.authenticated(key))
    }

    /// Check each stream delivery/input without turning heartbeats into user
    /// activity. Root configuration and ledger authority are reobserved by caller.
    pub(crate) fn check_active(
        &self,
        auth: &Authenticated,
        current: AuthContext<'_>,
        clock: Clock,
    ) -> Result<(), AuthError> {
        valid_context(current)?;
        if auth.principal_id != current.principal_id
            || auth.binding != current.auth_binding
            || auth.epoch != current.mutation_epoch
        {
            return Err(AuthError::SessionRevoked);
        }
        match &auth.identity {
            Identity::Cookie(key) => {
                let session = self.sessions.get(key).ok_or(AuthError::SessionRevoked)?;
                if !session.bound(current) {
                    return Err(AuthError::SessionRevoked);
                }
                if !session.active(clock.monotonic) {
                    return Err(AuthError::SessionExpired);
                }
            }
            Identity::Bearer(tag) => {
                if clock.unix_seconds >= auth.expires_at {
                    return Err(AuthError::SessionExpired);
                }
                let path = self
                    .settings
                    .bearer_credential
                    .as_ref()
                    .ok_or(AuthError::SessionRevoked)?;
                let secret = self.read_independent(
                    path,
                    self.settings.operator_credential.as_deref(),
                    read_secret,
                )?;
                if hmac::verify(&self.comparisons, &secret, tag.as_ref()).is_err() {
                    return Err(AuthError::SessionRevoked);
                }
            }
        }
        Ok(())
    }

    /// Idempotent local revocation; caller expires the protected browser cookie.
    pub(crate) fn logout(&mut self, auth: &Authenticated) -> Result<(), AuthError> {
        match &auth.identity {
            Identity::Cookie(key) => {
                self.sessions.remove(key);
                Ok(())
            }
            Identity::Bearer(_) => Err(AuthError::Unsupported),
        }
    }

    fn read_independent(
        &self,
        path: &Path,
        other: Option<&Path>,
        mut read: impl FnMut(&Path) -> Result<Vec<u8>, AuthError>,
    ) -> Result<Vec<u8>, AuthError> {
        let secret = read(path)?;
        if !secret_shape(&secret) {
            return Err(AuthError::Unavailable);
        }
        if let Some(other) = other {
            let other = read(other)?;
            if !secret_shape(&other) || matches_secret(&self.comparisons, &secret, &other) {
                return Err(AuthError::Unavailable);
            }
        }
        Ok(secret)
    }
}

fn valid_context(current: AuthContext<'_>) -> Result<(), AuthError> {
    if !current.enabled {
        return Err(AuthError::SessionRevoked);
    }
    if !super::config::id(current.principal_id)
        || !super::config::id(current.mutation_epoch)
        || current.auth_binding.is_empty()
        || current.auth_binding.len() > 256
        || !current.auth_binding.bytes().all(|b| b.is_ascii_graphic())
    {
        return Err(AuthError::Unavailable);
    }
    Ok(())
}

fn single<'a>(headers: &'a HeaderMap, name: &str) -> Result<Option<&'a str>, AuthError> {
    let mut values = headers.get_all(name).iter();
    let value = values
        .next()
        .map(|value| value.to_str().map_err(|_| AuthError::Invalid))
        .transpose()?;
    if values.next().is_some() {
        return Err(AuthError::Invalid);
    }
    Ok(value)
}

struct Credentials<'a> {
    cookie: Option<&'a str>,
    bearer: Option<&'a str>,
}

fn credentials(headers: &HeaderMap) -> Result<Credentials<'_>, AuthError> {
    let mut cookie = None;
    if let Some(value) = single(headers, "cookie")? {
        if value.len() > 8192 {
            return Err(AuthError::Invalid);
        }
        for part in value.split(';') {
            let (name, value) = part.trim().split_once('=').ok_or(AuthError::Invalid)?;
            if name == COOKIE {
                if cookie.is_some() || !opaque(value) {
                    return Err(AuthError::Invalid);
                }
                cookie = Some(value);
            }
        }
    }
    let bearer = single(headers, "authorization")?
        .map(|value| {
            value
                .strip_prefix("Bearer ")
                .filter(|token| secret_shape(token.as_bytes()))
                .ok_or(AuthError::Unauthenticated)
        })
        .transpose()?;
    if cookie.is_some() && bearer.is_some() {
        return Err(AuthError::Invalid);
    }
    Ok(Credentials { cookie, bearer })
}

fn opaque(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn matches_secret(key: &hmac::Key, expected: &[u8], supplied: &[u8]) -> bool {
    hmac::verify(key, supplied, hmac::sign(key, expected).as_ref()).is_ok()
}

fn sha256(bytes: &[u8]) -> [u8; 32] {
    digest::digest(&digest::SHA256, bytes)
        .as_ref()
        .try_into()
        .expect("SHA256 length")
}

fn random_token() -> Result<String, AuthError> {
    let mut bytes = [0; 32];
    SystemRandom::new()
        .fill(&mut bytes)
        .map_err(|_| AuthError::Unavailable)?;
    Ok(hex(&bytes))
}

fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8] = b"0123456789abcdef";
    bytes
        .iter()
        .flat_map(|b| {
            [
                DIGITS[(b >> 4) as usize] as char,
                DIGITS[(b & 15) as usize] as char,
            ]
        })
        .collect()
}

fn read_secret(path: &Path) -> Result<Vec<u8>, AuthError> {
    if !runtime_path(path) {
        return Err(AuthError::Unavailable);
    }
    let parent = path.parent().ok_or(AuthError::Unavailable)?;
    let reopen = || open_root(parent, 0).map_err(|_| credential::Error);
    let directory = reopen().map_err(|_| AuthError::Unavailable)?;
    credential::read_at(
        &directory,
        path.file_name().ok_or(AuthError::Unavailable)?,
        0,
        reopen,
    )
    .map_err(|_| AuthError::Unavailable)
}

#[cfg(test)]
mod tests {
    use super::credential::MAX_SECRET;
    use super::*;
    use hyper::http::HeaderValue;
    use std::cell::Cell;
    use std::fs::File;
    use std::fs::{self, DirBuilder};
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt, symlink};
    use std::sync::atomic::{AtomicU64, Ordering};

    const OPERATOR: &[u8] = b"synthetic-operator-credential-for-session-tests";
    const BEARER: &[u8] = b"synthetic-distinct-bearer-for-session-tests";

    fn settings() -> AuthSettings {
        AuthSettings {
            origin: "https://host.example.invalid:8443".into(),
            operator_credential: Some("/run/credentials/api/operator-token".into()),
            bearer_credential: Some("/run/credentials/api/cli-bearer".into()),
        }
    }

    fn current() -> AuthContext<'static> {
        AuthContext {
            principal_id: "operator",
            auth_binding: "policy-fingerprint",
            mutation_epoch: "host-epoch-1",
            enabled: true,
        }
    }

    fn clock() -> Clock {
        Clock {
            monotonic: Instant::now(),
            unix_seconds: 1_800_000_000,
        }
    }

    fn later(clock: Clock, seconds: u64) -> Clock {
        Clock {
            monotonic: clock.monotonic + Duration::from_secs(seconds),
            unix_seconds: clock.unix_seconds + seconds,
        }
    }

    fn headers() -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(
            "host",
            HeaderValue::from_static("host.example.invalid:8443"),
        );
        headers.insert(
            "origin",
            HeaderValue::from_static("https://host.example.invalid:8443"),
        );
        headers.insert("content-type", HeaderValue::from_static("application/json"));
        headers
    }

    fn body(secret: &[u8]) -> Vec<u8> {
        serde_json::to_vec(&serde_json::json!({"method":"operator-token", "secret":std::str::from_utf8(secret).unwrap()})).unwrap()
    }

    fn fixture_secret(path: &Path) -> Result<Vec<u8>, AuthError> {
        match path.file_name().unwrap().to_str().unwrap() {
            "operator-token" => Ok(OPERATOR.to_vec()),
            "cli-bearer" => Ok(BEARER.to_vec()),
            _ => panic!("unexpected credential slot"),
        }
    }

    fn issue(sessions: &mut Sessions, now: Clock) -> NewSession {
        sessions
            .login_with(
                &headers(),
                &body(OPERATOR),
                current(),
                now,
                fixture_secret,
                |_| Ok(()),
            )
            .unwrap()
    }

    fn authenticated_headers(session: &NewSession) -> HeaderMap {
        let mut value = headers();
        value.insert(
            "cookie",
            session
                .set_cookie
                .split(';')
                .next()
                .unwrap()
                .parse()
                .unwrap(),
        );
        value.insert(
            "x-tentaflake-csrf",
            session.auth.csrf_token.parse().unwrap(),
        );
        value
    }

    #[test]
    fn login_commits_only_a_fingerprint_before_returning_protected_session() {
        let mut sessions = Sessions::new(settings()).unwrap();
        let now = clock();
        let committed = Cell::new(false);
        let result = sessions
            .login_with(
                &headers(),
                &body(OPERATOR),
                current(),
                now,
                fixture_secret,
                |fingerprint| {
                    assert!(opaque(fingerprint));
                    assert!(!fingerprint.contains("synthetic"));
                    committed.set(true);
                    Ok(())
                },
            )
            .unwrap();
        assert!(committed.get());
        assert_eq!(sessions.sessions.len(), 1);
        assert_eq!(result.auth.principal_id, "operator");
        assert_eq!(result.auth.expires_at, now.unix_seconds + IDLE.as_secs());
        assert!(opaque(&result.auth.csrf_token));
        assert!(
            result
                .set_cookie
                .ends_with("; Secure; HttpOnly; SameSite=Strict; Path=/")
        );
        assert!(!result.set_cookie.contains("Domain") && !result.set_cookie.contains("synthetic"));
        let auth = sessions
            .authenticate(
                &authenticated_headers(&result),
                RequestKind::Mutation,
                current(),
                now,
            )
            .unwrap();
        assert_eq!(auth.principal_id, "operator");
        sessions.check_active(&auth, current(), now).unwrap();
    }

    #[test]
    fn durable_refusal_never_issues_or_rotates_a_session() {
        let mut sessions = Sessions::new(settings()).unwrap();
        let now = clock();
        let first = issue(&mut sessions, now);
        let result = sessions.login_with(
            &authenticated_headers(&first),
            &body(OPERATOR),
            current(),
            now,
            fixture_secret,
            |_| Err(AuthError::Unavailable),
        );
        assert!(matches!(result, Err(AuthError::Unavailable)));
        assert_eq!(sessions.sessions.len(), 1);
        sessions.check_active(&first.auth, current(), now).unwrap();
        // A restarted process retains no browser sessions and must still ask
        // the durable owner; its prior-consumption refusal cannot be bypassed.
        let mut restarted = Sessions::new(settings()).unwrap();
        assert!(matches!(
            restarted.authenticate(
                &authenticated_headers(&first),
                RequestKind::Read,
                current(),
                now
            ),
            Err(AuthError::Unauthenticated)
        ));
        assert!(matches!(
            restarted.login_with(
                &headers(),
                &body(OPERATOR),
                current(),
                now,
                fixture_secret,
                |_| Err(AuthError::Unauthenticated)
            ),
            Err(AuthError::Unauthenticated)
        ));
        assert!(restarted.sessions.is_empty());
    }

    #[test]
    fn login_rotates_cookie_and_csrf_then_logout_revokes_held_attachment() {
        let mut sessions = Sessions::new(settings()).unwrap();
        let now = clock();
        let first = issue(&mut sessions, now);
        let second = sessions
            .login_with(
                &authenticated_headers(&first),
                &body(OPERATOR),
                current(),
                now,
                fixture_secret,
                |_| Ok(()),
            )
            .unwrap();
        assert_ne!(first.set_cookie, second.set_cookie);
        assert_ne!(first.auth.csrf_token, second.auth.csrf_token);
        assert_eq!(sessions.sessions.len(), 1);
        assert_eq!(
            sessions.check_active(&first.auth, current(), now),
            Err(AuthError::SessionRevoked)
        );
        sessions.logout(&second.auth).unwrap();
        sessions.logout(&second.auth).unwrap();
        assert_eq!(
            sessions.check_active(&second.auth, current(), now),
            Err(AuthError::SessionRevoked)
        );
        assert!(EXPIRED_COOKIE.contains("Max-Age=0"));
    }

    #[test]
    fn exact_origin_host_closed_login_and_distinct_credentials_precede_consumption() {
        for mode in [
            "host",
            "duplicate-host",
            "origin",
            "no-origin",
            "content-type",
            "query-origin",
            "extra",
            "duplicate-secret",
            "wrong",
            "short",
            "bearer",
            "same-secret",
        ] {
            let mut sessions = Sessions::new(settings()).unwrap();
            let mut headers = headers();
            let mut input = body(OPERATOR);
            match mode {
                "host" => {
                    headers.insert("host", HeaderValue::from_static("other.example.invalid"));
                }
                "duplicate-host" => {
                    headers.append(
                        "host",
                        HeaderValue::from_static("host.example.invalid:8443"),
                    );
                }
                "origin" => {
                    headers.insert(
                        "origin",
                        HeaderValue::from_static("https://other.example.invalid"),
                    );
                }
                "no-origin" => {
                    headers.remove("origin");
                }
                "query-origin" => {
                    headers.insert(
                        "origin",
                        HeaderValue::from_static("https://host.example.invalid:8443?x=1"),
                    );
                }
                "content-type" => {
                    headers.insert("content-type", HeaderValue::from_static("text/plain"));
                }
                "extra" => {
                    let mut value: serde_json::Value = serde_json::from_slice(&input).unwrap();
                    value["owner"] = "operator".into();
                    input = serde_json::to_vec(&value).unwrap();
                }
                "duplicate-secret" => {
                    input = br#"{"method":"operator-token","secret":"first","secret":"second"}"#
                        .to_vec();
                }
                "wrong" => input = body(BEARER),
                "short" => input = body(b"short"),
                "bearer" => {
                    headers.insert(
                        "authorization",
                        format!("Bearer {}", std::str::from_utf8(BEARER).unwrap())
                            .parse()
                            .unwrap(),
                    );
                }
                _ => {}
            }
            let result = sessions.login_with(
                &headers,
                &input,
                current(),
                clock(),
                |path| {
                    if mode == "same-secret" {
                        Ok(OPERATOR.to_vec())
                    } else {
                        fixture_secret(path)
                    }
                },
                |_| panic!("refused login reached durable consumption"),
            );
            assert!(result.is_err(), "{mode}");
            assert!(sessions.sessions.is_empty());
        }
    }

    #[test]
    fn cookie_mutations_and_streams_require_origin_but_only_mutations_require_csrf() {
        let mut sessions = Sessions::new(settings()).unwrap();
        let now = clock();
        let session = issue(&mut sessions, now);
        let mut request = authenticated_headers(&session);
        request.remove("x-tentaflake-csrf");
        assert!(matches!(
            sessions.authenticate(&request, RequestKind::Mutation, current(), now),
            Err(AuthError::Csrf)
        ));
        assert!(
            sessions
                .authenticate(&request, RequestKind::Stream, current(), now)
                .is_ok()
        );
        request.remove("origin");
        assert!(matches!(
            sessions.authenticate(&request, RequestKind::Stream, current(), now),
            Err(AuthError::Origin)
        ));
        assert!(
            sessions
                .authenticate(&request, RequestKind::Read, current(), now)
                .is_ok()
        );
        request.insert(
            "origin",
            HeaderValue::from_static("https://host.example.invalid:8443"),
        );
        request.insert("x-tentaflake-csrf", HeaderValue::from_static("incorrect"));
        assert!(matches!(
            sessions.authenticate(&request, RequestKind::Mutation, current(), now),
            Err(AuthError::Csrf)
        ));
        request.append("cookie", HeaderValue::from_static("unrelated=x"));
        assert!(matches!(
            sessions.authenticate(&request, RequestKind::Read, current(), now),
            Err(AuthError::Invalid)
        ));
    }

    #[test]
    fn independent_bearer_needs_no_csrf_and_cannot_mix_with_session() {
        let mut sessions = Sessions::new(settings()).unwrap();
        let now = clock();
        let cookie = issue(&mut sessions, now);
        let mut request = headers();
        request.remove("origin");
        request.insert(
            "authorization",
            format!("Bearer {}", std::str::from_utf8(BEARER).unwrap())
                .parse()
                .unwrap(),
        );
        let auth = sessions
            .authenticate_with(
                &request,
                RequestKind::Mutation,
                current(),
                now,
                fixture_secret,
            )
            .unwrap();
        assert!(opaque(&auth.csrf_token));
        assert_eq!(sessions.logout(&auth), Err(AuthError::Unsupported));
        request.insert("cookie", authenticated_headers(&cookie)["cookie"].clone());
        assert!(matches!(
            sessions.authenticate_with(&request, RequestKind::Read, current(), now, fixture_secret),
            Err(AuthError::Invalid)
        ));
        request.remove("cookie");
        request.insert(
            "authorization",
            format!("Bearer {}", std::str::from_utf8(OPERATOR).unwrap())
                .parse()
                .unwrap(),
        );
        assert!(matches!(
            sessions.authenticate_with(&request, RequestKind::Read, current(), now, fixture_secret),
            Err(AuthError::Unauthenticated)
        ));
        assert!(matches!(
            sessions.authenticate_with(&request, RequestKind::Read, current(), now, |_| Ok(
                OPERATOR.to_vec()
            )),
            Err(AuthError::Unavailable)
        ));
    }

    #[test]
    fn idle_absolute_and_current_authority_revocation_are_independent() {
        let now = clock();
        for mode in ["disabled", "principal", "binding", "epoch", "idle"] {
            let mut sessions = Sessions::new(settings()).unwrap();
            let session = issue(&mut sessions, now);
            let mut current = current();
            match mode {
                "disabled" => current.enabled = false,
                "principal" => current.principal_id = "other",
                "binding" => current.auth_binding = "replacement",
                "epoch" => current.mutation_epoch = "new-epoch",
                _ => {}
            }
            let observed = if mode == "idle" {
                later(now, IDLE.as_secs())
            } else {
                now
            };
            assert!(
                sessions
                    .check_active(&session.auth, current, observed)
                    .is_err()
            );
            assert!(
                sessions
                    .authenticate(
                        &authenticated_headers(&session),
                        RequestKind::Read,
                        current,
                        observed
                    )
                    .is_err()
            );
        }
        let mut sessions = Sessions::new(settings()).unwrap();
        let session = issue(&mut sessions, now);
        let request = authenticated_headers(&session);
        for second in (600..ABSOLUTE.as_secs()).step_by(600) {
            sessions
                .authenticate(&request, RequestKind::Read, current(), later(now, second))
                .unwrap();
        }
        assert!(matches!(
            sessions.authenticate(
                &request,
                RequestKind::Read,
                current(),
                later(now, ABSOLUTE.as_secs())
            ),
            Err(AuthError::SessionExpired)
        ));
    }

    #[test]
    fn login_attempts_and_active_session_capacity_are_bounded() {
        let mut sessions = Sessions::new(settings()).unwrap();
        let now = clock();
        for _ in 0..LOGIN_ATTEMPTS {
            assert!(matches!(
                sessions.login_with(
                    &headers(),
                    &body(BEARER),
                    current(),
                    now,
                    fixture_secret,
                    |_| panic!("wrong token")
                ),
                Err(AuthError::Unauthenticated)
            ));
        }
        assert!(matches!(
            sessions.login_with(
                &headers(),
                &body(OPERATOR),
                current(),
                now,
                |_| panic!("rate limit must precede credential reads"),
                |_| panic!("rate limited")
            ),
            Err(AuthError::RateLimited)
        ));
        let recovered = issue(&mut sessions, later(now, LOGIN_WINDOW.as_secs()));
        assert!(opaque(&recovered.auth.csrf_token));
        sessions.sessions.clear();
        for n in 0..MAX_SESSIONS {
            sessions.sessions.insert(
                sha256(&n.to_le_bytes()),
                Session {
                    principal: "operator".into(),
                    binding: current().auth_binding.into(),
                    epoch: current().mutation_epoch.into(),
                    csrf: "a".repeat(64),
                    issued: now,
                    touched: now.monotonic,
                },
            );
        }
        assert!(matches!(
            sessions.login_with(
                &headers(),
                &body(OPERATOR),
                current(),
                later(now, 61),
                fixture_secret,
                |_| panic!("capacity must precede consumption")
            ),
            Err(AuthError::Capacity)
        ));
        assert_eq!(sessions.sessions.len(), MAX_SESSIONS);
    }

    #[test]
    fn discovery_and_configuration_reject_alternate_authorities_and_store_secrets() {
        let sessions = Sessions::new(settings()).unwrap();
        let mut request = headers();
        request.remove("origin");
        sessions.validate_public_request(&request).unwrap();
        request.remove("host");
        assert_eq!(
            sessions.validate_public_request(&request),
            Err(AuthError::Origin)
        );
        for origin in [
            "http://host.example.invalid",
            "https://host.example.invalid/",
            "https://user@host.example.invalid",
            "https://host.example.invalid?x=1",
            "https://:443",
            "https://host.example.invalid:0",
            "https://host.example.invalid:99999",
            "https://host.example.invalid:invalid",
        ] {
            let mut settings = settings();
            settings.origin = origin.into();
            assert!(matches!(Sessions::new(settings), Err(AuthError::Invalid)));
        }
        for path in [
            "relative",
            "/nix/store/fixture-token",
            "/run/../secret",
            "/run//secret",
        ] {
            let mut settings = settings();
            settings.operator_credential = Some(path.into());
            assert!(matches!(Sessions::new(settings), Err(AuthError::Invalid)));
        }
    }

    struct SecretFixture {
        path: PathBuf,
        directory: File,
    }
    impl SecretFixture {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!(
                "tentaflake-session-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            DirBuilder::new().mode(0o700).create(&path).unwrap();
            let directory = File::open(&path).unwrap();
            fs::write(path.join("secret"), OPERATOR).unwrap();
            fs::set_permissions(path.join("secret"), fs::Permissions::from_mode(0o400)).unwrap();
            Self { path, directory }
        }
        fn read(&self) -> Result<Vec<u8>, AuthError> {
            credential::read_at(
                &self.directory,
                "secret".as_ref(),
                unsafe { libc::geteuid() },
                || File::open(&self.path).map_err(|_| credential::Error),
            )
            .map_err(|_| AuthError::Unavailable)
        }
    }
    impl Drop for SecretFixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    #[test]
    fn credential_reader_checks_real_modes_links_size_and_reopened_identity() {
        for mode in [
            "good",
            "newline",
            "missing",
            "symlink",
            "hardlink",
            "public",
            "short",
            "large",
            "whitespace",
            "directory",
            "swapped",
        ] {
            let f = SecretFixture::new();
            let path = f.path.join("secret");
            match mode {
                "newline" => {
                    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
                    fs::write(&path, [OPERATOR, b"\n"].concat()).unwrap();
                }
                "missing" => fs::remove_file(&path).unwrap(),
                "symlink" => {
                    fs::rename(&path, f.path.join("original")).unwrap();
                    symlink("original", &path).unwrap();
                }
                "hardlink" => fs::hard_link(&path, f.path.join("alias")).unwrap(),
                "public" => fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap(),
                "short" | "large" | "whitespace" => {
                    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
                    fs::write(
                        &path,
                        match mode {
                            "short" => vec![b'x'; 31],
                            "large" => vec![b'x'; MAX_SECRET + 2],
                            _ => vec![b' '; 32],
                        },
                    )
                    .unwrap();
                }
                "directory" => {
                    fs::remove_file(&path).unwrap();
                    fs::create_dir(&path).unwrap();
                }
                "swapped" => {
                    fs::rename(&f.path, f.path.with_extension("old")).unwrap();
                    fs::create_dir(&f.path).unwrap();
                }
                _ => {}
            }
            let result = f.read();
            if matches!(mode, "good" | "newline") {
                assert_eq!(result.unwrap(), OPERATOR);
            } else {
                assert_eq!(result, Err(AuthError::Unavailable), "{mode}");
            }
            if mode == "swapped" {
                fs::remove_dir_all(f.path.with_extension("old")).unwrap();
            }
        }
    }
}
