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
use tentaflake_admission::ledger::api::{ActionGrant, ApiContext, ApiLookup};
use tentaflake_admission::ledger::{Ledger, LedgerError, PublishedAuthority};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::Semaphore;

pub(super) struct Api {
    config: Config,
    configuration: PathBuf,
    plan: PathBuf,
    sessions: Mutex<Sessions>,
    plans: Mutex<super::planning::Cache>,
    work: Arc<Semaphore>,
    #[cfg(test)]
    fixture: Option<Arc<operation_tests::Fixture>>,
}

#[derive(Clone)]
pub(super) struct Current {
    pub(super) authority: PublishedAuthority,
    policy: String,
    pub(super) binding: String,
    epoch: String,
    host_plan: Arc<crate::admission_host::HostPlan>,
    catalog: super::catalog::Catalog,
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

    fn revisions(&self, grants: &str) -> Value {
        json!({"policy":self.policy,"catalog":self.catalog.revision(),
            "grants":grants,"host":self.epoch})
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
        plans: Mutex::new(super::planning::Cache::default()),
        work: Arc::new(Semaphore::new(MAX_CONNECTIONS)),
        #[cfg(test)]
        fixture: None,
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
        #[cfg(test)]
        if let Some(fixture) = &self.fixture {
            return fixture.current();
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
            catalog: super::catalog::Catalog::project(&plan)?,
            host_plan: Arc::new(plan),
        })
    }

    fn grants(
        &self,
        current: &Current,
        auth: &Authenticated,
    ) -> Result<(Vec<Value>, u64), AuthError> {
        let context = ApiContext {
            authority: current.authority.clone(),
            host: self.config.host.clone(),
            principal: auth.principal_id.clone(),
            epoch: current.epoch.clone(),
            now: Clock::now()?.unix_seconds,
        };
        let (revision, durable) = self.durable_grants(&context)?;
        Ok((self.effective_grants(current, auth, durable)?, revision))
    }

    fn effective_grants(
        &self,
        current: &Current,
        auth: &Authenticated,
        durable: Vec<ActionGrant>,
    ) -> Result<Vec<Value>, AuthError> {
        let create = crate::admission_template::creation_grant(
            &current.host_plan,
            &self.config.host,
            &auth.principal_id,
        )
        .map_err(|_| AuthError::Unavailable)?;
        super::catalog::grants(
            self.config
                .catalog_read_grant
                .clone()
                .into_iter()
                .chain(create),
            durable,
        )
    }

    fn durable_grants(&self, context: &ApiContext) -> Result<(u64, Vec<ActionGrant>), AuthError> {
        #[cfg(test)]
        if let Some(fixture) = &self.fixture {
            return fixture.grants(context);
        }
        let snapshot = Ledger::open(Path::new(crate::admission_runtime::LEDGER))
            .and_then(|ledger| ledger.api_snapshot(context))
            .map_err(|_| AuthError::Unavailable)?;
        Ok((snapshot.grants_revision, snapshot.grants))
    }

    fn public_authority(
        &self,
        current: &Current,
        auth: &Authenticated,
        sessions: &Sessions,
    ) -> Result<(Vec<Value>, Value, u64), AuthError> {
        let (grants, native) = self.grants(current, auth)?;
        let fresh = self.current()?;
        sessions.check_active(auth, fresh.context(&self.config), Clock::now()?)?;
        let revision =
            super::catalog::grants_revision(&self.config.host, &auth.principal_id, &grants);
        Ok((grants, fresh.revisions(&revision), native))
    }

    fn preview(
        &self,
        current: &Current,
        prepared: super::planning::Prepared,
        started: Clock,
        finish: impl FnOnce(super::planning::Prepared) -> Result<Reply, AuthError>,
    ) -> Result<Reply, AuthError> {
        #[cfg(test)]
        if let Some(fixture) = &self.fixture {
            fixture.preview(&prepared)?;
            return finish(prepared);
        }
        let host = super::creation::Host::open()?;
        let context = super::creation::context(
            &current.authority,
            &prepared.host,
            &prepared.principal,
            &current.epoch,
        )?;
        let check = host.check(&current.host_plan, &self.plan, &prepared, started)?;
        host.ledger
            .api_preview_create(
                &context,
                &prepared.operation_id,
                &prepared.creation.environment,
                prepared.native_grants_revision,
                |authority, snapshot, _| check.observe(authority, snapshot),
            )
            .map_err(super::planning::preview_error)?;
        drop(check);
        // The core lock is released; keep the outer fence through final public
        // authority checks and cache admission so publication cannot interleave.
        finish(prepared)
    }

    fn plan(
        &self,
        current: &Current,
        auth: &Authenticated,
        sessions: &Sessions,
        request: &hyper::http::request::Parts,
        body: &[u8],
    ) -> Result<Reply, AuthError> {
        if super::session::single(&request.headers, "content-type")? != Some("application/json") {
            return Err(AuthError::Invalid);
        }
        let started = Clock::now()?;
        let request = super::planning::Request::parse(body)?;
        if crate::admission_template::creation_grant(
            &current.host_plan,
            &self.config.host,
            &auth.principal_id,
        )
        .map_err(|_| AuthError::Unavailable)?
        .is_none()
        {
            return Err(AuthError::Forbidden);
        }
        let (_, revisions, native) = self.public_authority(current, auth, sessions)?;
        if !request.matches_revisions(&revisions) {
            return Err(AuthError::PlanStale);
        }
        self.plans
            .lock()
            .map_err(|_| AuthError::Unavailable)?
            .room(&current.binding, &revisions, native, started)?;
        let final_clock = || {
            let (_, fresh, native_after) = self.public_authority(current, auth, sessions)?;
            if fresh != revisions || native_after != native {
                return Err(AuthError::PlanStale);
            }
            let now = Clock::now()?;
            super::planning::finish_clock(started, now)?;
            Ok(now)
        };
        let Some(prepared) = super::planning::prepare(
            &current.host_plan,
            current,
            &self.config,
            request.clone(),
            native,
            started,
        )?
        else {
            final_clock()?;
            let reply = super::planning::unsupported(request, &auth.principal_id, started)?;
            if reply.to_string().len() > super::MAX_BODY {
                return Err(AuthError::Unavailable);
            }
            return Ok(json_reply(StatusCode::OK, reply));
        };
        self.preview(current, prepared, started, |prepared| {
            let now = final_clock()?;
            let reply = self
                .plans
                .lock()
                .map_err(|_| AuthError::Unavailable)?
                .insert(prepared, now)?;
            Ok(json_reply(StatusCode::OK, reply))
        })
    }

    fn create_receipt(
        &self,
        host: Option<&super::creation::Host>,
        context: &ApiContext,
        request: &super::creation::Request,
    ) -> Result<Option<tentaflake_admission::ledger::api::OwnedCreate>, AuthError> {
        #[cfg(test)]
        if let Some(fixture) = &self.fixture {
            return fixture.create_receipt(context, request);
        }
        host.ok_or(AuthError::Unavailable)?
            .ledger
            .api_create_receipt(context, &request.key, &request.digest)
            .map_err(super::creation::receipt_error)
    }

    fn created_reply(
        &self,
        current: &Current,
        auth: &Authenticated,
        sessions: &Sessions,
        receipt: tentaflake_admission::ledger::api::OwnedCreate,
        started: Clock,
    ) -> Result<Reply, AuthError> {
        #[cfg(test)]
        let original = if let Some(fixture) = &self.fixture {
            fixture.original(&receipt)?
        } else {
            Arc::new(super::creation::original(&receipt)?)
        };
        #[cfg(not(test))]
        let original = super::creation::original(&receipt)?;
        // The exact linked records and effective resource grants share one
        // locked read. No second snapshot can mix a later grants revision into it.
        let grants = self.effective_grants(current, auth, receipt.grants.clone())?;
        let revision =
            super::catalog::grants_revision(&self.config.host, &auth.principal_id, &grants);
        let reply = super::creation::reply(&receipt, &original, current.revisions(&revision))?;
        let fresh = self.current()?;
        let now = Clock::now()?;
        sessions.check_active(auth, fresh.context(&self.config), now)?;
        super::planning::finish_clock(started, now)?;
        Ok(reply)
    }

    fn create(
        &self,
        current: &Current,
        auth: &Authenticated,
        sessions: &Sessions,
        parts: &hyper::http::request::Parts,
        body: &[u8],
    ) -> Result<Reply, AuthError> {
        use tentaflake_admission::ledger::api::ApiQueued;
        let started = Clock::now()?;
        let request = super::creation::Request::parse(
            &parts.headers,
            body,
            &self.config.host,
            &auth.principal_id,
        )?;
        #[cfg(test)]
        let host = if self.fixture.is_some() {
            None
        } else {
            Some(super::creation::Host::open()?)
        };
        #[cfg(not(test))]
        let host = Some(super::creation::Host::open()?);
        let context = super::creation::context(
            &current.authority,
            &self.config.host,
            &auth.principal_id,
            &current.epoch,
        )?;
        // Historic exact-key ownership is independent of current create grants,
        // cache lifetime, epoch or Pending. It can never authorize another job.
        if let Some(receipt) = self.create_receipt(host.as_ref(), &context, &request)? {
            return self.created_reply(current, auth, sessions, receipt, started);
        }
        if request.start {
            return Err(AuthError::Unsupported);
        }
        if request.key.epoch != current.epoch {
            return Err(AuthError::IdempotencyExpired);
        }
        if crate::admission_template::creation_grant(
            &current.host_plan,
            &self.config.host,
            &auth.principal_id,
        )
        .map_err(|_| AuthError::Unavailable)?
        .is_none()
        {
            return Err(AuthError::Forbidden);
        }
        let prepared = self
            .plans
            .lock()
            .map_err(|_| AuthError::Unavailable)?
            .take(&request.plan_id)?;
        let (_, revisions, native) = self.public_authority(current, auth, sessions)?;
        if prepared.host != self.config.host
            || prepared.principal != auth.principal_id
            || prepared.authority != current.authority
            || prepared.binding != current.binding
            || !prepared.live(Clock::now()?)
            || prepared.native_grants_revision != native
            || !prepared.request.matches_revisions(&revisions)
        {
            return Err(AuthError::PlanStale);
        }
        prepared
            .check_source(&current.host_plan)
            .map_err(|_| AuthError::PlanStale)?;
        let mutation = request.mutation(&prepared);
        let authorize =
            || sessions.check_active(auth, current.context(&self.config), Clock::now()?);
        let (outcome, unit) = if let Some(host) = &host {
            let deadline = started.monotonic + super::planning::DEADLINE;
            let unit = crate::admission_creation::prepare_start(
                &current.host_plan,
                &self.plan,
                &prepared.operation_id,
                deadline,
            )
            .map_err(|_| AuthError::Unavailable)?;
            crate::admission_retention::retain(
                &current.authority,
                current
                    .host_plan
                    .tools
                    .nix_store
                    .as_deref()
                    .ok_or(AuthError::Unavailable)?,
                &host.fence,
            )
            .map_err(|_| AuthError::Unavailable)?;
            let check = host.check(&current.host_plan, &self.plan, &prepared, started)?;
            let mut authorization_error = None;
            let outcome = host
                .ledger
                .api_queue_create(
                    &context,
                    &mutation,
                    &prepared.creation.environment,
                    prepared.native_grants_revision,
                    |authority, snapshot, _| {
                        // observe binds the callback authority to the fenced
                        // current plan. Recheck session expiry and the current
                        // bearer after slow reads, before either charge exists.
                        let policy = check.observe(authority, snapshot)?;
                        authorize().map_err(|error| {
                            authorization_error = Some(error);
                            "creation authentication changed".to_owned()
                        })?;
                        Ok(policy)
                    },
                )
                // Matching-key digest conflicts were resolved above under this
                // same fence. A new candidate/native-ID collision is stale.
                .map_err(|error| {
                    authorization_error.unwrap_or_else(|| super::planning::preview_error(error))
                })?;
            (outcome, Some(unit))
        } else {
            #[cfg(test)]
            {
                (
                    self.fixture
                        .as_ref()
                        .ok_or(AuthError::Unavailable)?
                        .queue(&context, &mutation, &prepared, authorize)?,
                    None,
                )
            }
            #[cfg(not(test))]
            {
                return Err(AuthError::Unavailable);
            }
        };
        let receipt = self
            .create_receipt(host.as_ref(), &context, &request)?
            .ok_or(AuthError::Unavailable)?;
        let reply = self.created_reply(current, auth, sessions, receipt, started)?;
        // Finish response construction and authorization under the fence. The
        // supervisor takes the same fence; release it before the no-wait handoff.
        drop(host);
        if matches!(outcome, ApiQueued::New(_)) {
            #[cfg(test)]
            if let Some(fixture) = &self.fixture {
                fixture.start(&mutation.operation_id);
            }
            if let Some(unit) = unit {
                // This single attempt may have reached systemd even on error.
                // Durable Queued is the truthful response either way; a retry
                // only returns history. Explicit operator recovery resolves it.
                let _ = crate::admission_systemd::start_creation_unit(
                    &current.host_plan.tools.busctl,
                    &unit,
                    started.monotonic + super::planning::DEADLINE,
                );
            }
        }
        Ok(reply)
    }

    fn operation(
        &self,
        current: &Current,
        auth: &Authenticated,
        read: &super::operations::Read,
    ) -> Result<ApiLookup, AuthError> {
        let context = ApiContext {
            authority: current.authority.clone(),
            host: self.config.host.clone(),
            principal: auth.principal_id.clone(),
            epoch: current.epoch.clone(),
            now: Clock::now()?.unix_seconds,
        };
        #[cfg(test)]
        if let Some(fixture) = &self.fixture {
            return fixture.lookup(&context, read);
        }
        let ledger = Ledger::open(Path::new(crate::admission_runtime::LEDGER))
            .map_err(|_| AuthError::Unavailable)?;
        read.lookup(&ledger, &context)
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
        let operation = if request.method == Method::GET {
            super::operations::parse(path, request.uri.query())?
        } else {
            None
        };
        let catalog = if request.method == Method::GET {
            super::catalog::parse(path, request.uri.query())?
        } else {
            None
        };
        if request.uri.query().is_some() && operation.is_none() && catalog.is_none() {
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
            let mut reply = self.session_reply(&login.auth, &fresh, &sessions)?;
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
        if path == "/api/v1/plans" && request.method == Method::POST {
            return self.plan(&current, &auth, &sessions, &request, body);
        }
        if path == "/api/v1/environments" && request.method == Method::POST {
            return self.create(&current, &auth, &sessions, &request, body);
        }
        if let Some(operation) = operation {
            require_empty(body)?;
            // Resolve only under the authenticated installed authority. Even a
            // historical receipt must not cross a changed binding/session on
            // the way out; failure is never converted to authoritative absence.
            let outcome = self.operation(&current, &auth, &operation);
            let fresh = self.current()?;
            sessions.check_active(&auth, fresh.context(&self.config), Clock::now()?)?;
            return operation.reply(outcome?);
        }
        if let Some(page) = catalog {
            require_empty(body)?;
            let grant = self
                .config
                .catalog_read_grant
                .as_ref()
                .ok_or(AuthError::Forbidden)?;
            let (_, revisions, _) = self.public_authority(&current, &auth, &sessions)?;
            return page.reply(
                &current.catalog,
                &json!([
                    self.config.host,
                    auth.principal_id,
                    current.binding,
                    grant.target,
                    revisions["grants"]
                ]),
            );
        }
        if path == "/api/v1/session" {
            if request.method == Method::GET {
                require_empty(body)?;
                return self.session_reply(&auth, &current, &sessions);
            }
            if request.method == Method::DELETE {
                require_empty(body)?;
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
            require_empty(body)?;
            let (_, revisions, _) = self.public_authority(&current, &auth, &sessions)?;
            return Ok(json_reply(
                StatusCode::OK,
                super::capabilities::project(&current.host_plan.policy, revisions)?,
            ));
        }
        // A valid login is not capability, resource or runtime acceptance.
        // Remaining v1 operations stay unavailable until their real owner lands.
        Err(AuthError::Unsupported)
    }

    fn session_reply(
        &self,
        auth: &Authenticated,
        current: &Current,
        sessions: &Sessions,
    ) -> Result<Reply, AuthError> {
        let (grants, revisions, _) = self.public_authority(current, auth, sessions)?;
        // The wire session is bounded; do not truncate grants or prevent
        // unrelated receipt reads when this particular response cannot fit.
        if grants.len() > 256 {
            return Err(AuthError::Unavailable);
        }
        let body = json!({
            "user":{"id":auth.principal_id,"name":self.config.principal.name},
            "csrfToken":auth.csrf_token,"expiresAt":timestamp(auth.expires_at)?,
            "grants":grants,"revisions":revisions,"mutationEpoch":current.epoch,
        });
        if body.to_string().len() > super::MAX_BODY {
            return Err(AuthError::Unavailable);
        }
        Ok(json_reply(StatusCode::OK, body))
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

pub(super) fn revision(value: &[u8]) -> String {
    let mut result = String::from("sha256-");
    use std::fmt::Write;
    for byte in digest::digest(&digest::SHA256, value).as_ref() {
        write!(&mut result, "{byte:02x}").expect("string formatting");
    }
    result
}

pub(super) fn timestamp(seconds: u64) -> Result<String, AuthError> {
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
        plans: Mutex::new(super::planning::Cache::default()),
        work: Arc::new(Semaphore::new(1)),
        fixture: None,
    })
}

#[cfg(test)]
#[path = "operation_tests.rs"]
pub(super) mod operation_tests;

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
    fn authenticated_unsupported_json_is_not_a_bodyless_route() {
        let (api, cookie, backend) =
            operation_tests::fixture(tentaflake_admission::ledger::api::ApiOperationState::Queued);
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
            let session = operation_tests::http(api.clone(), &cookie, "/api/v1/session").await;
            assert!(session.starts_with("HTTP/1.1 200"));
            let session: Value = serde_json::from_str(session.split_once("\r\n\r\n").unwrap().1).unwrap();
            let csrf_token = session["csrfToken"].as_str().unwrap();
            let cookie = cookie.as_str();
            for (method, path, payload, csrf, credential, status) in [
                ("POST", "/api/v1/environments/fixture/start", body.as_str(), csrf_token, cookie, 501),
                // Unsupported handlers must not parse or dispatch an opaque body.
                ("POST", "/api/v1/environments/fixture/start", "not JSON", csrf_token, cookie, 501),
                ("POST", "/api/v1/environments/fixture/start", body.as_str(), "wrong", cookie, 403),
                ("POST", "/api/v1/environments/fixture/start", body.as_str(), csrf_token, unknown_cookie.as_str(), 401),
                ("GET", "/api/v1/session", "{}", csrf_token, cookie, 400),
                ("DELETE", "/api/v1/session", "{}", csrf_token, cookie, 400),
                ("GET", "/api/v1/capabilities", "{}", csrf_token, cookie, 400),
                ("GET", "/api/meta", "{}", csrf_token, cookie, 400),
                ("GET", "/api/v1/operations/operation-fixture", "{}", csrf_token, cookie, 400),
                ("GET", "/api/v1/session", "", csrf_token, cookie, 200),
            ] {
                let request = format!("{method} {path} HTTP/1.1\r\nHost: host.example.invalid\r\nOrigin: https://host.example.invalid\r\nCookie: {credential}\r\nX-Tentaflake-CSRF: {csrf}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{payload}", payload.len());
                let reply = crate::api::server::tests::exchange(api.clone(), request.into_bytes()).await;
                assert!(reply.starts_with(&format!("HTTP/1.1 {status}")), "{method} {path}: expected HTTP {status}, received {}", reply.lines().next().unwrap_or("missing status"));
                assert!(reply.contains("cache-control: no-store\r\n"));
                assert!(!reply.contains(cookie));
                assert!(!reply.contains(payload) || payload.is_empty() || status == 200);
                if status == 501 {
                    let refusal: Value = serde_json::from_str(reply.split_once("\r\n\r\n").unwrap().1).unwrap();
                    assert_eq!(refusal["error"]["code"], "unsupported");
                }
            }
            assert_eq!(backend.reads(), 0);
        });
    }

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
