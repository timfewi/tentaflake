use super::*;
use std::os::unix::fs::DirBuilderExt;

struct SocketDirectory(std::path::PathBuf);
impl SocketDirectory {
    fn new() -> Self {
        // AF_UNIX has a fixed pathname limit; Nix/profile TMPDIR prefixes can
        // exceed it before the fixture name is appended. Keep this Linux-only
        // socket fixture in an atomically created private short directory.
        let path = std::path::Path::new("/tmp").join(format!(
            "tf-audit-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(&path)
            .unwrap();
        Self(path)
    }
    fn activity(&self) -> ActivityConfig {
        ActivityConfig {
            socket: self.0.join("s").to_str().unwrap().into(),
            server_uid: unsafe { libc::geteuid() },
            producer_id: "connector".into(),
            run_id: "run".into(),
            workload_id: "workload".into(),
            policy_revision: "policy".into(),
        }
    }
}
impl Drop for SocketDirectory {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).unwrap();
    }
}

#[test]
fn stalled_reply_and_full_backlog_obey_absolute_publication_deadline() {
    let directory = SocketDirectory::new();
    let config = directory.activity();
    let listener = UnixListener::bind(&config.socket).unwrap();
    // The listener accepts neither data nor connections: publication must time out.
    let start = Instant::now();
    assert!(activity::publish(&config, &json!({"version":1}), Duration::from_millis(80)).is_err());
    assert!(start.elapsed() < Duration::from_secs(1));
    drop(listener);
    std::fs::remove_file(&config.socket).unwrap();
    let listener = UnixListener::bind(&config.socket).unwrap();
    assert_eq!(unsafe { libc::listen(listener.as_raw_fd(), 0) }, 0);
    let _queued = UnixStream::connect(&config.socket).unwrap();
    // Linux AF_UNIX backlog exhaustion must return EAGAIN on a nonblocking socket.
    let start = Instant::now();
    assert!(activity::publish(&config, &json!({"version":1}), Duration::from_millis(80)).is_err());
    assert!(start.elapsed() < Duration::from_secs(1));
}

#[test]
fn front_door_early_refusals_each_emit_exactly_one_minimized_event() {
    let directory = SocketDirectory::new();
    let mut cfg = config();
    cfg.activity = Some(directory.activity());
    let mut server = Server::new(cfg, false).unwrap();
    let call = |resource: &str| json!({"name":"connector_read","arguments":{"resource":resource,"method":"drive_metadata"}});
    assert!(
        server
            .dispatch(1001, "tools/call", call("ungranted"))
            .is_err()
    );
    assert_eq!(server.gateway.audit.sequence.load(Ordering::SeqCst), 1);
    assert!(server.dispatch(1001, "tools/call", call("file-a")).is_err()); // no credential
    assert_eq!(server.gateway.audit.sequence.load(Ordering::SeqCst), 2);
    server
        .credentials
        .rotate("synthetic-only".into(), 1, now().unwrap() + 60, true)
        .unwrap();
    server.credentials.revoke().unwrap();
    assert!(server.dispatch(1001, "tools/call", call("file-a")).is_err());
    assert_eq!(server.gateway.audit.sequence.load(Ordering::SeqCst), 3);
    server.gateway.grant.expires_unix = now().unwrap() - 1;
    assert!(server.dispatch(1001, "tools/call", call("file-a")).is_err());
    assert_eq!(server.gateway.audit.sequence.load(Ordering::SeqCst), 4);
}

#[test]
fn stalled_projection_preserves_provider_result_and_never_replays() {
    struct ProviderStub(AtomicUsize);
    impl Transport for ProviderStub {
        fn get(&self, _: &Url, _: &Credential, _: usize, _: Duration) -> Result<Response, Error> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(Response {
                status: 200,
                body: br#"{"id":"file-a","name":"private"}"#.to_vec(),
            })
        }
    }
    let directory = SocketDirectory::new();
    let activity = directory.activity();
    let _listener = UnixListener::bind(&activity.socket).unwrap();
    let cfg = config();
    let source = RuntimeCredentials::new(&cfg);
    source
        .rotate("synthetic-only".into(), 1, now().unwrap() + 60, true)
        .unwrap();
    let journal = Journal {
        activity: Some(activity),
        tenant: cfg.tenant.clone(),
        agent: cfg.agent.clone(),
        sequence: AtomicU64::new(0),
        session: "synthetic-session".into(),
    };
    let gateway = Gateway::new(
        Grant {
            agent: cfg.agent,
            tenant: cfg.tenant,
            account: cfg.account,
            provider: cfg.provider,
            resources: cfg.resources,
            methods: cfg.methods,
            credential_reference: "runtime".into(),
            generation: 1,
            expires_unix: cfg.expires_unix,
            max_body_bytes: 4096,
            max_concurrency: 1,
            deadline: Duration::from_secs(1),
        },
        ProviderStub(AtomicUsize::new(0)),
        source,
        journal,
    )
    .unwrap();
    let mut request = ReadRequest {
        tenant: "tenant-a".into(),
        account: "account-a".into(),
        resource: "file-a".into(),
        method: ReadMethod::DriveMetadata,
        generation: 1,
    };
    let start = Instant::now();
    assert!(matches!(gateway.read(&request), Ok(Content::Json(_))));
    assert!(start.elapsed() < Duration::from_secs(2));
    assert_eq!(gateway.transport.0.load(Ordering::SeqCst), 1);
    assert_eq!(gateway.audit.sequence.load(Ordering::SeqCst), 1);
    request.resource = "ungranted".into();
    let start = Instant::now();
    assert!(matches!(gateway.read(&request), Err(Error::Denied)));
    assert!(start.elapsed() < Duration::from_secs(2));
    assert_eq!(gateway.transport.0.load(Ordering::SeqCst), 1); // zero additional provider calls
    assert_eq!(gateway.audit.sequence.load(Ordering::SeqCst), 2);
}
