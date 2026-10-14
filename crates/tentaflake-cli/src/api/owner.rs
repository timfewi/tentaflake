//! Privileged local API owner. Only this process reads credentials and the ledger.
use super::config::Config;
use super::session::{AuthContext, AuthError, Authenticated, Clock, RequestKind, Sessions};
use super::{MAX_CONNECTIONS, Reply, control, error, json_reply, request_id, response};
use bytes::Bytes;
use hyper::http::{HeaderMap, HeaderValue, Method, StatusCode};
use ring::digest;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tentaflake_admission::ledger::{Ledger, LedgerError, PublishedAuthority};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::Semaphore;

#[cfg(test)]
type ConsumeFixture = Arc<dyn Fn(&str) -> Result<(), AuthError> + Send + Sync>;

pub(super) struct Api {
    config: Config,
    configuration: PathBuf,
    plan: PathBuf,
    sessions: Sessions,
    work: Arc<Semaphore>,
    #[cfg(test)]
    fixture_current: Option<Current>,
    #[cfg(test)]
    fixture_consume: Option<ConsumeFixture>,
}

#[derive(Clone)]
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
        sessions,
        work: Arc::new(Semaphore::new(MAX_CONNECTIONS)),
        #[cfg(test)]
        fixture_current: None,
        #[cfg(test)]
        fixture_consume: None,
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
                let (stream, _) = super::accept_retry::accept(|| listener.accept())
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
        #[cfg(test)]
        if let Some(current) = &self.fixture_current {
            return Ok(current.clone());
        }
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
        let sessions = &self.sessions;
        sessions.validate_public_request(&request.headers)?;
        // Routes are exact origin-form targets; no proxy absolute-form requests,
        // query aliases or percent-decoding can change authentication semantics.
        if request.uri.scheme().is_some() || request.uri.authority().is_some() {
            return Err(AuthError::Invalid);
        }
        let handler = ROUTES
            .iter()
            .find(|route| route.method == request.method && route.path == request.uri.path())
            .map_or(Handler::Unsupported, |route| route.handler);
        // OIDC negotiation is public in v1, but no provider is implemented.
        // Refusing it must not require an existing authenticated session.
        if matches!(handler, Handler::Oidc) {
            return Err(AuthError::Unsupported);
        }
        if request.uri.query().is_some() {
            return Err(AuthError::Invalid);
        }
        if matches!(handler, Handler::Meta) {
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
        if matches!(handler, Handler::Login) {
            let consume = |fingerprint: &str| {
                Ledger::open(Path::new(crate::admission_runtime::LEDGER))
                    .and_then(|ledger| {
                        ledger.consume_operator_login(&current.authority, fingerprint)
                    })
                    .map_err(|error| match error {
                        LedgerError::Conflict => AuthError::Unauthenticated,
                        _ => AuthError::Unavailable,
                    })
            };
            #[cfg(test)]
            let login = if let Some(consume) = &self.fixture_consume {
                sessions.fixture_login_request(
                    &request.headers,
                    body,
                    context,
                    clock,
                    |fingerprint| consume(fingerprint),
                )
            } else {
                sessions.login(&request.headers, body, context, clock, consume)
            }?;
            #[cfg(not(test))]
            let login = sessions.login(&request.headers, body, context, clock, consume)?;
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
        if request.headers.contains_key("upgrade") {
            return Err(AuthError::Unsupported);
        }
        match handler {
            Handler::Session => {
                require_empty(body)?;
                self.session_reply(&auth, &current)
            }
            Handler::Logout => {
                require_empty(body)?;
                sessions.logout(&auth)?;
                let mut reply = response(StatusCode::NO_CONTENT, Bytes::new());
                reply.headers_mut().insert(
                    "set-cookie",
                    HeaderValue::from_static(super::session::EXPIRED_COOKIE),
                );
                Ok(reply)
            }
            Handler::Capabilities => {
                require_empty(body)?;
                Ok(json_reply(
                    StatusCode::OK,
                    super::capabilities::project(&current.host_policy, current.revisions())?,
                ))
            }
            // A valid login is not capability, resource or runtime acceptance.
            // Remaining v1 operations stay unavailable until their real owner lands.
            _ => Err(AuthError::Unsupported),
        }
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

#[derive(Clone, Copy)]
enum Handler {
    Meta,
    Oidc,
    Login,
    Session,
    Logout,
    Capabilities,
    Unsupported,
}

struct Route {
    method: Method,
    path: &'static str,
    handler: Handler,
}

// Exact origin-form routes share one authentication boundary above. Unknown
// methods/paths remain authenticated refusals, not aliases for an implemented route.
const ROUTES: &[Route] = &[
    Route {
        method: Method::GET,
        path: "/api/meta",
        handler: Handler::Meta,
    },
    Route {
        method: Method::GET,
        path: "/api/v1/session/oidc",
        handler: Handler::Oidc,
    },
    Route {
        method: Method::GET,
        path: "/api/v1/session/oidc/authorize",
        handler: Handler::Oidc,
    },
    Route {
        method: Method::GET,
        path: "/api/v1/session/oidc/callback",
        handler: Handler::Oidc,
    },
    Route {
        method: Method::POST,
        path: "/api/v1/session",
        handler: Handler::Login,
    },
    Route {
        method: Method::GET,
        path: "/api/v1/session",
        handler: Handler::Session,
    },
    Route {
        method: Method::DELETE,
        path: "/api/v1/session",
        handler: Handler::Logout,
    },
    Route {
        method: Method::GET,
        path: "/api/v1/capabilities",
        handler: Handler::Capabilities,
    },
];

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
    // SAFETY: seconds is a live time_t and value is correctly sized writable
    // storage. gmtime_r retains neither pointer and signals failure with null.
    if unsafe { libc::gmtime_r(&seconds, value.as_mut_ptr()) }.is_null() {
        return Err(AuthError::Unavailable);
    }
    // SAFETY: the successful gmtime_r call above initialized every tm field.
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
        sessions: Sessions::new(super::session::AuthSettings {
            origin: "https://host.example.invalid".into(),
            operator_credential: Some("/run/credentials/api-fixture/operator-token".into()),
            bearer_credential: None,
        })
        .unwrap(),
        work: Arc::new(Semaphore::new(1)),
        fixture_current: None,
        fixture_consume: None,
    })
}

#[cfg(test)]
pub(super) async fn exchange(
    api: Arc<Api>,
    parts: hyper::http::request::Parts,
    body: Bytes,
) -> Result<Reply, ()> {
    let (client, server) = UnixStream::pair().map_err(|_| ())?;
    let task = tokio::spawn(connection(
        server,
        api,
        tentaflake_admission::effective_uid(),
    ));
    let reply = control::exchange_for_test(client, parts, body).await;
    task.await.map_err(|_| ())?;
    reply
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        Mutex,
        atomic::{AtomicBool, Ordering},
    };

    fn authenticated_fixture() -> (Arc<Api>, super::super::session::NewSession) {
        let mut api = fixture();
        let api_mut = Arc::get_mut(&mut api).unwrap();
        let recorded: Value = serde_json::from_str(include_str!(
            "../../../tentaflake-admission/tests/recorded-start.json"
        ))
        .unwrap();
        let current = Current {
            authority: PublishedAuthority {
                generation: "authority-fixture".into(),
                revision: format!("sha256:{}", "1".repeat(64)),
                manifest_path: "/nix/store/00000000000000000000000000000000-fixture".into(),
            },
            policy: "policy-fixture".into(),
            binding: "binding-fixture".into(),
            epoch: "epoch-fixture".into(),
            host_policy: serde_json::from_value(recorded["snapshot"]["policy"].clone()).unwrap(),
        };
        let login = api_mut
            .sessions
            .fixture_login(current.context(&api_mut.config));
        api_mut.fixture_current = Some(current);
        (api, login)
    }

    #[test]
    fn authenticated_unsupported_json_is_not_a_bodyless_route() {
        let (api, login) = authenticated_fixture();
        let cookie = login.set_cookie.split(';').next().unwrap();
        let unknown_cookie = format!("__Host-tentaflake-session={}", "0".repeat(64));
        let fixtures: Value =
            serde_json::from_str(include_str!("../../../../api/v1/fixtures.json")).unwrap();
        let create = fixtures["exchanges"]
            .as_array()
            .unwrap()
            .iter()
            .find(|entry| entry["name"] == "createEnvironment")
            .unwrap();
        let body = serde_json::to_string(&create["request"]["body"]).unwrap();
        tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
            for (method, path, payload, csrf, credential, status) in [
                ("POST", "/api/v1/environments", body.as_str(), login.auth.csrf_token.as_str(), cookie, 501),
                // Unsupported handlers must not parse or dispatch an opaque body.
                ("POST", "/api/v1/environments", "not JSON", login.auth.csrf_token.as_str(), cookie, 501),
                ("POST", "/api/v1/environments", body.as_str(), "wrong", cookie, 403),
                ("POST", "/api/v1/environments", body.as_str(), login.auth.csrf_token.as_str(), unknown_cookie.as_str(), 401),
                ("GET", "/api/v1/session", "{}", login.auth.csrf_token.as_str(), cookie, 400),
                ("DELETE", "/api/v1/session", "{}", login.auth.csrf_token.as_str(), cookie, 400),
                ("GET", "/api/v1/capabilities", "{}", login.auth.csrf_token.as_str(), cookie, 400),
                ("GET", "/api/meta", "{}", login.auth.csrf_token.as_str(), cookie, 400),
                ("GET", "/api/v1/session", "", login.auth.csrf_token.as_str(), cookie, 200),
                ("GET", "/api/v1/session?alias=1", "", login.auth.csrf_token.as_str(), cookie, 400),
                ("GET", "/api/v1/%73ession", "", login.auth.csrf_token.as_str(), cookie, 501),
                ("PUT", "/api/v1/session", "{}", login.auth.csrf_token.as_str(), cookie, 501),
                ("GET", "/api/v1/session/oidc/authorize?state=opaque", "", "", unknown_cookie.as_str(), 501),
            ] {
                let request = format!("{method} {path} HTTP/1.1\r\nHost: host.example.invalid\r\nOrigin: https://host.example.invalid\r\nCookie: {credential}\r\nX-Tentaflake-CSRF: {csrf}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{payload}", payload.len());
                let reply = crate::api::server::tests::exchange(api.clone(), request.into_bytes()).await;
                assert!(reply.starts_with(&format!("HTTP/1.1 {status}")), "{method} {path}: expected HTTP {status}, received {}", reply.lines().next().unwrap_or("missing status"));
                assert!(reply.contains("cache-control: no-store\r\n"));
                assert!(!reply.contains(cookie));
                assert!(!reply.contains(payload) || payload.is_empty() || status == 200);
            }
        });
    }

    fn hold_login(api: &mut Arc<Api>) -> (Arc<AtomicBool>, std::sync::mpsc::Sender<()>) {
        let entered = Arc::new(AtomicBool::new(false));
        let (release, released) = std::sync::mpsc::channel();
        let released = Mutex::new(released);
        let callback_entered = entered.clone();
        let owned = Arc::get_mut(api).unwrap();
        owned.fixture_consume = Some(Arc::new(move |_| {
            callback_entered.store(true, Ordering::Release);
            released
                .lock()
                .unwrap()
                .recv_timeout(Duration::from_secs(5))
                .map_err(|_| AuthError::Unavailable)
        }));
        (entered, release)
    }

    #[test]
    fn slow_login_commit_does_not_block_authenticated_http_session_read() {
        let (mut api, login) = authenticated_fixture();
        Arc::get_mut(&mut api).unwrap().work = Arc::new(Semaphore::new(2));
        let (entered, release) = hold_login(&mut api);
        tokio::runtime::Builder::new_current_thread().enable_all()
            .max_blocking_threads(4).build().unwrap().block_on(async {
                let body = r#"{"method":"operator-token","secret":"synthetic-operation-login-0123456789"}"#;
                let request = format!("POST /api/v1/session HTTP/1.1\r\nHost: host.example.invalid\r\nOrigin: https://host.example.invalid\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}", body.len());
                let slow = tokio::spawn(crate::api::server::tests::exchange(api.clone(), request.into_bytes()));
                tokio::time::timeout(Duration::from_secs(2), async {
                    while !entered.load(Ordering::Acquire) {
                        tokio::time::sleep(Duration::from_millis(5)).await;
                    }
                }).await.unwrap();
                let cookie = login.set_cookie.split(';').next().unwrap();
                let request = format!("GET /api/v1/session HTTP/1.1\r\nHost: host.example.invalid\r\nCookie: {cookie}\r\n\r\n");
                let read = tokio::time::timeout(Duration::from_secs(1),
                    crate::api::server::tests::exchange(api.clone(), request.into_bytes())).await;
                // Always release the observed mutation before asserting the read;
                // a RED result must not leave a blocked worker behind.
                release.send(()).unwrap();
                let committed = slow.await.unwrap();
                assert!(committed.starts_with("HTTP/1.1 200"));
                let read = read.expect("session GET was blocked by the slow login commit");
                assert!(read.starts_with("HTTP/1.1 200"));
            });
    }

    #[test]
    fn cancelled_unix_connection_keeps_blocking_work_charged() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            let (mut api, _) = authenticated_fixture();
            let (entered, release) = hold_login(&mut api);
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
            let login = hyper::Request::builder()
                .method(Method::POST)
                .uri("/api/v1/session")
                .header("host", "host.example.invalid")
                .header("origin", "https://host.example.invalid")
                .header("content-type", "application/json")
                .body(())
                .unwrap()
                .into_parts()
                .0;
            let body = Bytes::from_static(
                br#"{"method":"operator-token","secret":"synthetic-operation-login-0123456789"}"#,
            );
            let caller = tokio::spawn(control::exchange_for_test(client, login, body));
            let connection = tokio::spawn(connection(
                server,
                api.clone(),
                tentaflake_admission::effective_uid(),
            ));
            tokio::time::timeout(Duration::from_secs(2), async {
                while !entered.load(Ordering::Acquire) {
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
    fn poisoned_cookie_state_does_not_permanently_disable_http_owner() {
        let (api, login) = authenticated_fixture();
        api.sessions.fixture_poison();
        tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
            let cookie = login.set_cookie.split(';').next().unwrap();
            for (path, expected) in [
                ("/api/v1/session", 503), // Recovery refuses the triggering request.
                ("/api/meta", 200),
                ("/api/v1/session", 401), // The old cookie is permanently revoked.
            ] {
                let request = format!("GET {path} HTTP/1.1\r\nHost: host.example.invalid\r\nCookie: {cookie}\r\n\r\n");
                let reply = crate::api::server::tests::exchange(api.clone(), request.into_bytes()).await;
                assert!(reply.starts_with(&format!("HTTP/1.1 {expected}")));
            }
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
