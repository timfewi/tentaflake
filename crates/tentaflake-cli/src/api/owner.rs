//! Privileged local API owner. Only this process reads credentials and the ledger.
use super::config::Config;
use super::session::{AuthContext, AuthError, Authenticated, Clock, RequestKind, Sessions};
use super::{MAX_CONNECTIONS, Reply, control, error, json_reply, request_id, response};
use bytes::Bytes;
use hyper::http::{HeaderMap, HeaderValue, Method, StatusCode};
use ring::digest;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tentaflake_admission::ledger::{Ledger, LedgerError, PublishedAuthority};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::Semaphore;

pub(super) struct Api {
    config: Config,
    configuration: PathBuf,
    plan: PathBuf,
    sessions: Mutex<Sessions>,
    work: Arc<Semaphore>,
}

struct Current {
    authority: PublishedAuthority,
    policy: String,
    binding: String,
    epoch: String,
    host_policy: tentaflake_admission::HostPolicy,
}

impl Current {
    fn context<'a>(&'a self, config: &'a Config) -> AuthContext<'a> {
        AuthContext {
            principal_id: &config.principal.id,
            auth_binding: &self.binding,
            mutation_epoch: &self.epoch,
            enabled: true,
        }
    }

    fn revisions(&self) -> Value {
        json!({"policy":self.policy,"catalog":revision(crate::RUNTIME_CATALOG.as_bytes()),
            "grants":self.binding,"host":self.epoch})
    }
}

pub(super) fn serve(configuration: &Path, plan: &Path) -> Result<(), String> {
    crate::admission_runtime::require_root()?;
    let config = Config::load(configuration)?;
    let sessions = Sessions::new(config.auth_settings()?)
        .map_err(|_| "invalid private host API authentication configuration")?;
    let api = Arc::new(Api {
        config,
        configuration: configuration.into(),
        plan: plan.into(),
        sessions: Mutex::new(sessions),
        work: Arc::new(Semaphore::new(MAX_CONNECTIONS)),
    });
    api.current()
        .map_err(|_| "host API requires its installed, published admission plan")?;
    let peer_uid = control::frontend_uid()?;
    let listener = control::take_listener()?;
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .max_blocking_threads(4)
        .build()
        .map_err(|_| "cannot initialize host API owner runtime")?
        .block_on(async {
            let listener = UnixListener::from_std(listener)
                .map_err(|_| "cannot initialize host API owner listener")?;
            let capacity = Arc::new(Semaphore::new(MAX_CONNECTIONS));
            loop {
                let (stream, _) = listener
                    .accept()
                    .await
                    .map_err(|_| "host API owner listener failed")?;
                if control::check_peer(&stream, peer_uid).is_err() {
                    continue;
                }
                let Ok(permit) = capacity.clone().try_acquire_owned() else {
                    continue;
                };
                let api = api.clone();
                tokio::spawn(async move {
                    let _permit = permit;
                    connection(stream, api, peer_uid).await;
                });
            }
        })
}

async fn connection(mut stream: UnixStream, api: Arc<Api>, peer_uid: u32) {
    if control::check_peer(&stream, peer_uid).is_err() {
        return;
    }
    let _ = tokio::time::timeout(Duration::from_secs(15), async {
        let Ok((parts, body)) = control::read_request(&mut stream).await else {
            return;
        };
        let id = request_id();
        let failed_id = id.clone();
        let reply = match api.work.clone().try_acquire_owned() {
            Ok(work) => tokio::task::spawn_blocking(move || {
                // Keep work charged even when the Unix peer or HTTP caller leaves.
                let _work = work;
                api.handle(parts, &body, &id)
            })
            .await
            .unwrap_or_else(|_| error(StatusCode::SERVICE_UNAVAILABLE, "unavailable", &failed_id)),
            Err(_) => error(StatusCode::SERVICE_UNAVAILABLE, "unavailable", &failed_id),
        };
        let _ = control::write_reply(&mut stream, reply).await;
    })
    .await;
}

impl Api {
    fn current(&self) -> Result<Current, AuthError> {
        crate::admission_runtime::installed(&self.plan).map_err(|_| AuthError::Unavailable)?;
        let plan =
            crate::admission_host::load_plan(&self.plan).map_err(|_| AuthError::Unavailable)?;
        if plan.api_config.as_ref() != Some(&self.configuration) {
            return Err(AuthError::Unavailable);
        }
        let authority = crate::admission_runtime::authority(&plan, &self.plan);
        let status = Ledger::open(Path::new(crate::admission_runtime::LEDGER))
            .and_then(|ledger| ledger.publication_status())
            .map_err(|_| AuthError::Unavailable)?;
        if status.published != authority {
            return Err(AuthError::Unavailable);
        }
        // Pending maintenance does not disable authentication for recovery.
        // Future resource handlers must independently require stable authority.
        crate::admission_runtime::installed(&self.plan).map_err(|_| AuthError::Unavailable)?;
        let binding = revision(
            serde_json::to_string(&(&authority, &self.configuration))
                .map_err(|_| AuthError::Unavailable)?
                .as_bytes(),
        );
        Ok(Current {
            policy: revision(authority.revision.as_bytes()),
            epoch: revision(authority.generation.as_bytes()),
            authority,
            binding,
            host_policy: plan.policy,
        })
    }

    fn handle(&self, request: hyper::http::request::Parts, body: &[u8], request_id: &str) -> Reply {
        self.dispatch(request, body).unwrap_or_else(|failure| {
            error(
                StatusCode::from_u16(failure.status()).expect("fixed HTTP status"),
                failure.code(),
                request_id,
            )
        })
    }

    fn dispatch(
        &self,
        request: hyper::http::request::Parts,
        body: &[u8],
    ) -> Result<Reply, AuthError> {
        let mut sessions = self.sessions.lock().map_err(|_| AuthError::Unavailable)?;
        sessions.validate_public_request(&request.headers)?;
        // Routes are exact origin-form targets; no proxy absolute-form requests,
        // query aliases or percent-decoding can change authentication semantics.
        if request.uri.scheme().is_some() || request.uri.authority().is_some() {
            return Err(AuthError::Invalid);
        }
        let path = request.uri.path();
        // OIDC negotiation is public in v1, but no provider is implemented.
        // Refusing it must not require an existing authenticated session.
        if request.method == Method::GET
            && matches!(
                path,
                "/api/v1/session/oidc"
                    | "/api/v1/session/oidc/authorize"
                    | "/api/v1/session/oidc/callback"
            )
        {
            return Err(AuthError::Unsupported);
        }
        if request.uri.query().is_some() {
            return Err(AuthError::Invalid);
        }
        if request.method == Method::GET && path == "/api/meta" {
            require_empty(body)?;
            let methods: Vec<&str> = if self.config.operator_login {
                vec!["operator-token"]
            } else {
                vec![]
            };
            return Ok(json_reply(
                StatusCode::OK,
                json!({"service":"tentaflake","apiVersions":[1],
                "host":self.config.host,"auth":{"methods":methods},"backend":"implemented"}),
            ));
        }
        let current = self.current()?;
        let clock = Clock::now()?;
        let context = current.context(&self.config);
        if request.method == Method::POST && path == "/api/v1/session" {
            let login = sessions.login(&request.headers, body, context, clock, |fingerprint| {
                Ledger::open(Path::new(crate::admission_runtime::LEDGER))
                    .and_then(|ledger| {
                        ledger.consume_operator_login(&current.authority, fingerprint)
                    })
                    .map_err(|error| match error {
                        LedgerError::Conflict => AuthError::Unauthenticated,
                        _ => AuthError::Unavailable,
                    })
            })?;
            // An installation/publication change after the durable token claim
            // burns that token but must not return an already stale session.
            let fresh = self.current()?;
            sessions.check_active(&login.auth, fresh.context(&self.config), Clock::now()?)?;
            let mut reply = self.session_reply(&login.auth, &fresh)?;
            reply.headers_mut().insert(
                "set-cookie",
                HeaderValue::from_str(&login.set_cookie).map_err(|_| AuthError::Unavailable)?,
            );
            return Ok(reply);
        }
        let kind = request_kind(&request.method, &request.headers);
        let auth = sessions.authenticate(&request.headers, kind, context, clock)?;
        require_empty(body)?;
        if request.headers.contains_key("upgrade") {
            return Err(AuthError::Unsupported);
        }
        if path == "/api/v1/session" {
            if request.method == Method::GET {
                return self.session_reply(&auth, &current);
            }
            if request.method == Method::DELETE {
                sessions.logout(&auth)?;
                let mut reply = response(StatusCode::NO_CONTENT, Bytes::new());
                reply.headers_mut().insert(
                    "set-cookie",
                    HeaderValue::from_static(super::session::EXPIRED_COOKIE),
                );
                return Ok(reply);
            }
        }
        if path == "/api/v1/capabilities" && request.method == Method::GET {
            return Ok(json_reply(
                StatusCode::OK,
                super::capabilities::project(&current.host_policy, current.revisions())?,
            ));
        }
        // A valid login is not capability, resource or runtime acceptance.
        // Remaining v1 operations stay unavailable until their real owner lands.
        Err(AuthError::Unsupported)
    }

    fn session_reply(&self, auth: &Authenticated, current: &Current) -> Result<Reply, AuthError> {
        Ok(json_reply(
            StatusCode::OK,
            json!({
                "user":{"id":auth.principal_id,"name":self.config.principal.name},
                "csrfToken":auth.csrf_token,"expiresAt":timestamp(auth.expires_at)?,
                "grants":[],"revisions":current.revisions(),"mutationEpoch":current.epoch,
            }),
        ))
    }
}

fn request_kind(method: &Method, headers: &HeaderMap) -> RequestKind {
    if *method == Method::GET && headers.contains_key("upgrade") {
        RequestKind::Stream
    } else if *method == Method::GET || *method == Method::HEAD {
        RequestKind::Read
    } else {
        RequestKind::Mutation
    }
}

fn require_empty(body: &[u8]) -> Result<(), AuthError> {
    if body.is_empty() {
        Ok(())
    } else {
        Err(AuthError::Invalid)
    }
}

fn revision(value: &[u8]) -> String {
    let mut result = String::from("sha256-");
    use std::fmt::Write;
    for byte in digest::digest(&digest::SHA256, value).as_ref() {
        write!(&mut result, "{byte:02x}").expect("string formatting");
    }
    result
}

fn timestamp(seconds: u64) -> Result<String, AuthError> {
    let seconds: libc::time_t = seconds.try_into().map_err(|_| AuthError::Unavailable)?;
    let mut value = std::mem::MaybeUninit::<libc::tm>::uninit();
    if unsafe { libc::gmtime_r(&seconds, value.as_mut_ptr()) }.is_null() {
        return Err(AuthError::Unavailable);
    }
    let value = unsafe { value.assume_init() };
    let year = value
        .tm_year
        .checked_add(1900)
        .filter(|year| (1970..=9999).contains(year))
        .ok_or(AuthError::Unavailable)?;
    Ok(format!(
        "{year:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        value.tm_mon + 1,
        value.tm_mday,
        value.tm_hour,
        value.tm_min,
        value.tm_sec
    ))
}

#[cfg(test)]
pub(super) fn fixture() -> Arc<Api> {
    let config: Config = serde_json::from_value(json!({
        "schemaVersion":1,"host":"host-fixture","origin":"https://host.example.invalid",
        "listen":"127.0.0.1:18181","principal":{"id":"operator","name":"Operator"},
        "operatorLogin":true,"cliBearer":false,
    }))
    .unwrap();
    Arc::new(Api {
        config,
        configuration: PathBuf::from("/uninstalled-api-fixture"),
        plan: PathBuf::from("/uninstalled-plan-fixture"),
        sessions: Mutex::new(
            Sessions::new(super::session::AuthSettings {
                origin: "https://host.example.invalid".into(),
                operator_credential: Some("/run/credentials/api-fixture/operator-token".into()),
                bearer_credential: None,
            })
            .unwrap(),
        ),
        work: Arc::new(Semaphore::new(1)),
    })
}

#[cfg(test)]
pub(super) async fn exchange(
    api: Arc<Api>,
    parts: hyper::http::request::Parts,
    body: Bytes,
) -> Result<Reply, ()> {
    let (client, server) = UnixStream::pair().map_err(|_| ())?;
    let task = tokio::spawn(connection(server, api, unsafe { libc::geteuid() }));
    let reply = control::exchange_for_test(client, parts, body).await;
    task.await.map_err(|_| ())?;
    reply
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cancelled_unix_connection_keeps_blocking_work_charged() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            let api = fixture();
            let held_api = api.clone();
            let (locked, is_locked) = std::sync::mpsc::channel();
            let (release, released) = std::sync::mpsc::channel();
            let holder = std::thread::spawn(move || {
                let _guard = held_api.sessions.lock().unwrap();
                locked.send(()).unwrap();
                released.recv_timeout(Duration::from_secs(5)).unwrap();
            });
            is_locked.recv_timeout(Duration::from_secs(1)).unwrap();
            let request = || {
                hyper::Request::builder()
                    .uri("/api/meta")
                    .header("host", "host.example.invalid")
                    .body(())
                    .unwrap()
                    .into_parts()
                    .0
            };
            let (client, server) = UnixStream::pair().unwrap();
            let caller = tokio::spawn(control::exchange_for_test(client, request(), Bytes::new()));
            let connection =
                tokio::spawn(connection(server, api.clone(), unsafe { libc::geteuid() }));
            tokio::time::timeout(Duration::from_secs(1), async {
                while api.work.available_permits() != 0 {
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
            })
            .await
            .unwrap();
            connection.abort();
            assert!(connection.await.unwrap_err().is_cancelled());
            assert!(caller.await.unwrap().is_err());
            assert_eq!(api.work.available_permits(), 0);
            let refusal = exchange(api.clone(), request(), Bytes::new())
                .await
                .unwrap();
            assert_eq!(refusal.status(), StatusCode::SERVICE_UNAVAILABLE);
            assert_eq!(api.work.available_permits(), 0);
            release.send(()).unwrap();
            holder.join().unwrap();
            tokio::time::timeout(Duration::from_secs(1), async {
                while api.work.available_permits() != 1 {
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
            })
            .await
            .unwrap();
        });
    }

    #[test]
    fn timestamps_and_revisions_obey_public_wire_bounds() {
        assert_eq!(timestamp(0).unwrap(), "1970-01-01T00:00:00Z");
        assert_eq!(timestamp(951782400).unwrap(), "2000-02-29T00:00:00Z");
        assert!(timestamp(u64::MAX).is_err());
        assert!(super::super::config::id(&revision(
            b"sha256:private-plan-revision"
        )));
        assert_ne!(revision(b"a"), revision(b"b"));
    }

    #[test]
    fn upgrade_header_never_weakens_mutation_csrf() {
        let mut headers = HeaderMap::new();
        headers.insert("upgrade", HeaderValue::from_static("websocket"));
        assert!(request_kind(&Method::GET, &headers) == RequestKind::Stream);
        for method in [Method::POST, Method::DELETE, Method::PATCH, Method::PUT] {
            assert!(request_kind(&method, &headers) == RequestKind::Mutation);
        }
    }
}
