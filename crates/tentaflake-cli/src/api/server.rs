use super::config::Config;
use super::session::{AuthContext, AuthError, Authenticated, Clock, RequestKind, Sessions};
use bytes::Bytes;
use http_body_util::{BodyExt, Full, Limited};
use hyper::body::Incoming;
use hyper::http::{HeaderMap, HeaderValue, Method, StatusCode};
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Request, Response};
use hyper_util::rt::{TokioIo, TokioTimer};
use ring::{
    digest,
    rand::{SecureRandom, SystemRandom},
};
use serde_json::{Value, json};
use std::convert::Infallible;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tentaflake_admission::ledger::{Ledger, LedgerError, PublishedAuthority};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Semaphore;

const MAX_BODY: usize = 32 * 1024;
const MAX_CONNECTIONS: usize = 32;
type Reply = Response<Full<Bytes>>;

struct Api {
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
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .max_blocking_threads(4)
        .build()
        .map_err(|_| "cannot initialize host API runtime")?
        .block_on(async {
            let listener = TcpListener::bind(api.config.listen)
                .await
                .map_err(|_| "cannot bind host API loopback listener")?;
            let capacity = Arc::new(Semaphore::new(MAX_CONNECTIONS));
            loop {
                let (stream, _) = listener
                    .accept()
                    .await
                    .map_err(|_| "host API listener failed")?;
                let Ok(permit) = capacity.clone().try_acquire_owned() else {
                    continue;
                };
                let api = api.clone();
                tokio::spawn(async move {
                    let _permit = permit;
                    connection(stream, api).await;
                });
            }
        })
}

async fn connection(stream: TcpStream, api: Arc<Api>) {
    let service = service_fn(move |request| receive(api.clone(), request));
    let mut builder = http1::Builder::new();
    builder
        .keep_alive(false)
        .max_headers(64)
        .max_buf_size(32 * 1024)
        .timer(TokioTimer::new())
        .header_read_timeout(Duration::from_secs(5));
    // One bounded request per connection, including headers, body and response.
    // Dropping HTTP work never replays a durable claim made by a blocking task.
    let _ = tokio::time::timeout(
        Duration::from_secs(15),
        builder.serve_connection(TokioIo::new(stream), service),
    )
    .await;
}

async fn receive(api: Arc<Api>, request: Request<Incoming>) -> Result<Reply, Infallible> {
    let request_id = request_id();
    let (parts, body) = request.into_parts();
    let body = match tokio::time::timeout(
        Duration::from_secs(5),
        Limited::new(body, MAX_BODY).collect(),
    )
    .await
    {
        Ok(Ok(body)) => body.to_bytes(),
        Ok(Err(_)) => {
            return Ok(error(
                StatusCode::PAYLOAD_TOO_LARGE,
                "too_large",
                &request_id,
            ));
        }
        Err(_) => return Ok(error(StatusCode::REQUEST_TIMEOUT, "invalid", &request_id)),
    };
    let failed_id = request_id.clone();
    let Ok(work) = api.work.clone().try_acquire_owned() else {
        return Ok(error(
            StatusCode::SERVICE_UNAVAILABLE,
            "unavailable",
            &request_id,
        ));
    };
    let result = tokio::task::spawn_blocking(move || {
        // This permit outlives a timed-out HTTP connection and its JoinHandle.
        // Tokio's blocking-thread limit alone does not bound its pending queue.
        let _work = work;
        api.handle(parts, &body, &request_id)
    })
    .await;
    Ok(
        result
            .unwrap_or_else(|_| error(StatusCode::SERVICE_UNAVAILABLE, "unavailable", &failed_id)),
    )
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

fn response(status: StatusCode, body: Bytes) -> Reply {
    let mut response = Response::new(Full::new(body));
    *response.status_mut() = status;
    response
        .headers_mut()
        .insert("cache-control", HeaderValue::from_static("no-store"));
    response.headers_mut().insert(
        "x-content-type-options",
        HeaderValue::from_static("nosniff"),
    );
    response
}

fn json_reply(status: StatusCode, body: Value) -> Reply {
    let mut response = response(status, Bytes::from(body.to_string()));
    response
        .headers_mut()
        .insert("content-type", HeaderValue::from_static("application/json"));
    response
}

fn error(status: StatusCode, code: &str, request_id: &str) -> Reply {
    json_reply(
        status,
        json!({"error":{"code":code,"message":"The host API could not accept this request."},"requestId":request_id}),
    )
}

fn revision(value: &[u8]) -> String {
    let mut result = String::from("sha256-");
    use std::fmt::Write;
    for byte in digest::digest(&digest::SHA256, value).as_ref() {
        write!(&mut result, "{byte:02x}").expect("string formatting");
    }
    result
}

fn request_id() -> String {
    use std::fmt::Write;
    let mut bytes = [0; 16];
    if SystemRandom::new().fill(&mut bytes).is_err() {
        return "request-unavailable".into();
    }
    let mut id = String::from("request-");
    for byte in bytes {
        write!(&mut id, "{byte:02x}").expect("string formatting");
    }
    id
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
mod tests {
    use super::*;
    use std::io::{Read, Write};

    fn api() -> Arc<Api> {
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
                Sessions::new(super::super::session::AuthSettings {
                    origin: "https://host.example.invalid".into(),
                    operator_credential: Some("/run/credentials/api-fixture/operator-token".into()),
                    bearer_credential: None,
                })
                .unwrap(),
            ),
            work: Arc::new(Semaphore::new(1)),
        })
    }

    fn client(
        address: std::net::SocketAddr,
        request: Vec<u8>,
    ) -> std::thread::JoinHandle<std::io::Result<Vec<u8>>> {
        std::thread::spawn(move || {
            let mut stream =
                std::net::TcpStream::connect_timeout(&address, Duration::from_secs(2))?;
            stream.set_read_timeout(Some(Duration::from_secs(3)))?;
            stream.set_write_timeout(Some(Duration::from_secs(3)))?;
            stream.write_all(&request)?;
            let mut response = Vec::new();
            stream.read_to_end(&mut response)?;
            Ok(response)
        })
    }

    async fn exchange(api: Arc<Api>, request: Vec<u8>) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let client = client(listener.local_addr().unwrap(), request);
        let (stream, _) = listener.accept().await.unwrap();
        connection(stream, api).await;
        String::from_utf8(client.join().unwrap().unwrap()).unwrap()
    }

    #[test]
    fn real_http_discovery_and_refusals_are_bounded_and_never_cacheable() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            let api = api();
            for (target,host,status) in [("/api/meta","host.example.invalid","200"),
                ("/api/meta","other.example.invalid","403"),
                ("/api/meta?ignored=1","host.example.invalid","400"),
                ("/api/v1/session/oidc","host.example.invalid","501"),
                ("/api/v1/session/oidc/authorize","host.example.invalid","501"),
                ("/api/v1/session/oidc/callback?code=synthetic","host.example.invalid","501")] {
                let reply = exchange(api.clone(),format!("GET {target} HTTP/1.1\r\nHost: {host}\r\n\r\n").into_bytes()).await;
                assert!(reply.starts_with(&format!("HTTP/1.1 {status}")),"{reply}");
                assert!(reply.contains("cache-control: no-store\r\n"),"{reply}");
                let body: Value = serde_json::from_str(reply.split_once("\r\n\r\n").unwrap().1).unwrap();
                if status == "200" {
                    assert_eq!(body,json!({"service":"tentaflake","apiVersions":[1],"host":"host-fixture",
                        "auth":{"methods":["operator-token"]},"backend":"implemented"}));
                } else {
                    assert!(body.get("requestId").and_then(Value::as_str).is_some_and(super::super::config::id));
                    assert!(!reply.contains("uninstalled"));
                }
            }
            let mut request = format!("POST /api/v1/session HTTP/1.1\r\nHost: host.example.invalid\r\nContent-Length: {}\r\n\r\n",MAX_BODY+1).into_bytes();
            request.extend(vec![b'x'; MAX_BODY+1]);
            let reply = exchange(api,request).await;
            assert!(reply.starts_with("HTTP/1.1 413"),"{reply}");
            assert!(reply.contains("cache-control: no-store\r\n"));
        });
    }

    #[test]
    fn cancelled_http_keeps_blocking_work_charged_until_it_finishes() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            let api = api();
            let held_api = api.clone();
            let (locked, is_locked) = std::sync::mpsc::channel();
            let (release, released) = std::sync::mpsc::channel();
            let holder = std::thread::spawn(move || {
                let _guard = held_api.sessions.lock().unwrap();
                locked.send(()).unwrap();
                released.recv_timeout(Duration::from_secs(5)).unwrap();
            });
            is_locked.recv_timeout(Duration::from_secs(1)).unwrap();
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let request = b"GET /api/meta HTTP/1.1\r\nHost: host.example.invalid\r\n\r\n".to_vec();
            let client = client(listener.local_addr().unwrap(), request.clone());
            let (stream, _) = listener.accept().await.unwrap();
            let connection = tokio::spawn(connection(stream, api.clone()));
            tokio::time::timeout(Duration::from_secs(1), async {
                while api.work.available_permits() != 0 {
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
            })
            .await
            .unwrap();
            connection.abort();
            assert!(connection.await.unwrap_err().is_cancelled());
            assert_eq!(api.work.available_permits(), 0);
            let refusal = exchange(api.clone(), request).await;
            assert!(refusal.starts_with("HTTP/1.1 503"), "{refusal}");
            assert_eq!(api.work.available_permits(), 0);
            release.send(()).unwrap();
            holder.join().unwrap();
            // This client was intentionally aborted before any response. A
            // reset is valid here; ordinary completed exchanges remain strict.
            match client.join().unwrap() {
                Ok(bytes) => assert!(bytes.is_empty()),
                Err(error) => assert_eq!(error.kind(), std::io::ErrorKind::ConnectionReset),
            }
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
