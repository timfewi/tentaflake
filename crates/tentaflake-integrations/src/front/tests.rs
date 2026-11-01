use super::*;
fn config() -> Config {
    serde_json::from_value(json!({"socket":"/run/tentaflake-connectors/front.sock","caller_uid":1001,"reviewer_uid":1002,"agent":"agent-a","tenant":"tenant-a","account":"account-a","provider":"drive","resources":["file-a"],"methods":["drive_metadata"],"generation":1,"expires_unix":now().unwrap()+3600,"addresses":["8.8.8.8"],"max_body_bytes":4096,"deadline_seconds":1})).unwrap()
}
#[test]
fn runtime_credentials_are_bound_expiring_rotatable_and_not_replayed() {
    let config = config();
    let source = RuntimeCredentials::new(&config);
    assert!(
        source
            .load("runtime", Provider::Drive, "tenant-a", "account-a")
            .is_err()
    );
    assert!(
        source
            .rotate("synthetic-only".into(), 2, now().unwrap() + 60, true)
            .is_err()
    );
    assert!(
        source
            .rotate("synthetic-only".into(), 1, now().unwrap() - 1, true)
            .is_err()
    );
    assert!(
        source
            .rotate("synthetic-only".into(), 1, now().unwrap() + 60, false)
            .is_err()
    );
    source
        .rotate("synthetic-only".into(), 1, now().unwrap() + 60, true)
        .unwrap();
    assert!(
        source
            .load("runtime", Provider::Drive, "wrong-tenant", "account-a")
            .is_err()
    );
    assert!(
        source
            .load("other", Provider::Drive, "tenant-a", "account-a")
            .is_err()
    );
    let first = source
        .load("runtime", Provider::Drive, "tenant-a", "account-a")
        .unwrap();
    source
        .rotate("synthetic-rotated".into(), 1, now().unwrap() + 60, true)
        .unwrap();
    assert_eq!(source.validate(&first), Err(Error::Revoked));
    let second = source
        .load("runtime", Provider::Drive, "tenant-a", "account-a")
        .unwrap();
    source.revoke().unwrap();
    assert_eq!(source.validate(&second), Err(Error::Revoked));
    let restarted = RuntimeCredentials::new(&config);
    assert!(
        restarted
            .load("runtime", Provider::Drive, "tenant-a", "account-a")
            .is_err()
    );
}
#[test]
fn exact_uid_and_fixed_tool_binding() {
    let server = Server::new(config(), false).unwrap();
    assert!(!server.allowed(999));
    assert!(server.dispatch(999, "connections/list", json!({})).is_err());
    assert!(
        server
            .dispatch(1001, "connections/list", json!({}))
            .is_err()
    );
    assert!(
        server
            .dispatch(1002, "credentials/revoke", json!({}))
            .is_err()
    );
    assert!(server.dispatch(1002,"tools/call",json!({"name":"connector_read","arguments":{"resource":"file-a","method":"drive_metadata"}})).is_err());
    assert!(server.dispatch(1001,"tools/call",json!({"name":"unknown","arguments":{"resource":"file-a","method":"drive_metadata"}})).is_err());
    assert!(server.dispatch(1001,"tools/call",json!({"name":"connector_read","arguments":{"resource":"file-a","method":"drive_metadata","tenant":"other"}})).is_err());
    assert!(server.dispatch(1001,"tools/call",json!({"name":"connector_read","arguments":{"resource":"wrong","method":"drive_metadata"}})).is_err());
    assert!(server.dispatch(1001,"tools/call",json!({"name":"connector_read","arguments":{"resource":"file-a","method":"drive_metadata","url":"https://evil.invalid"}})).is_err());
    let connection = server
        .dispatch(1002, "connections/list", json!({}))
        .unwrap();
    assert_eq!(connection["tenant"], "tenant-a");
    assert_eq!(connection["credential_available"], false);
    let text = connection.to_string();
    assert!(!text.contains("account-a"));
    assert!(!text.contains("file-a"));
    assert!(!text.contains("token"));
}
#[test]
fn framing_and_lifecycle_reject_malformed_and_uninitialized() {
    assert!(serde_json::from_str::<Rpc>("[]").is_err());
    assert!(
        serde_json::from_str::<Rpc>(r#"{"jsonrpc":"2.0","id":1,"method":"initialize","extra":1}"#)
            .is_err()
    );
    let server = Server::new(config(), false).unwrap();
    let (mut client, socket) = UnixStream::pair().unwrap();
    client
        .write_all(b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"tools/list\"}\n")
        .unwrap();
    client.shutdown(std::net::Shutdown::Write).unwrap();
    assert!(server.connection(socket, 1001).is_err());
    let mut output = String::new();
    client.read_to_string(&mut output).unwrap();
    assert!(output.contains("not initialized"));
    assert!(!output.contains("connector_read"));
    let (mut client, socket) = UnixStream::pair().unwrap();
    client.write_all(b"{bad}\n").unwrap();
    assert!(server.connection(socket, 1001).is_err());
}
#[test]
fn proposal_decision_has_no_approval_or_execution_variant() {
    for invalid in ["approved", "execute", "activate", "remediate"] {
        assert!(serde_json::from_value::<review::Decision>(json!(invalid)).is_err());
    }
    assert!(
        decode::<Review>(json!({"id":"proposal-1","decision":"acknowledged","action":"activate"}))
            .is_err()
    );
    let server = Server::new(config(), false).unwrap();
    assert!(
        server
            .dispatch(
                1001,
                "proposals/review",
                json!({"id":"proposal-1","decision":"acknowledged"})
            )
            .is_err()
    );
    assert!(server.dispatch(1002, "proposals/list", json!({})).is_err());
}
#[test]
fn durable_review_first_decision_wins_and_restart_retains_it() {
    let directory = std::env::temp_dir().join(format!(
        "tentaflake-review-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir(&directory).unwrap();
    std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700)).unwrap();
    let context =
        json!({"tenant":"tenant-a","system":"system","agent":"agent-a","reviewer_uid":1002});
    let mut store = review::Store::open(&directory, context.clone()).unwrap();
    assert!(review::Store::open(&directory, context.clone()).is_err());
    let policy = advisory::Policy {
        system: "system".into(),
        observer: "observer".into(),
        admitted: true,
        resources: BTreeSet::from(["agent-a".into()]),
        max_evidence: 4,
        max_content_bytes: 4096,
    };
    let proposal = advisory::Advisory::new(policy)
        .unwrap()
        .propose(
            "observer",
            &[advisory::Telemetry {
                resource: "agent-a".into(),
                evidence_reference: "event-1".into(),
                summary: "bounded evidence".into(),
            }],
            Some(advisory::Finding {
                title: "Review".into(),
                explanation: "Evidence only".into(),
                evidence_references: vec!["event-1".into()],
                category: advisory::Category::Security,
            }),
        )
        .unwrap();
    store.submit(proposal).unwrap();
    store
        .review("proposal-1", review::Decision::Acknowledged)
        .unwrap();
    assert!(
        store
            .review("proposal-1", review::Decision::Dismissed)
            .is_err()
    );
    drop(store);
    // An interrupted next-file publication is not replayed on recovery.
    std::fs::write(directory.join("proposals.next"), b"incomplete").unwrap();
    let original = std::fs::read(directory.join("proposals.json")).unwrap();
    for (field, value) in [
        ("tenant", json!("tenant-b")),
        ("system", json!("other-system")),
        ("agent", json!("agent-b")),
        ("reviewer_uid", json!(1003)),
    ] {
        let mut changed = context.clone();
        changed[field] = value;
        assert!(review::Store::open(&directory, changed).is_err());
        assert_eq!(
            std::fs::read(directory.join("proposals.json")).unwrap(),
            original
        );
    }
    let store = review::Store::open(&directory, context.clone()).unwrap();
    assert_eq!(
        store.list().unwrap()["proposals"][0]["review"],
        "acknowledged"
    );
    drop(store);
    // Legacy unbound data cannot be adopted by whichever tenant starts first.
    std::fs::write(directory.join("proposals.json"), b"[]").unwrap();
    assert!(review::Store::open(&directory, context).is_err());
    assert_eq!(
        std::fs::read(directory.join("proposals.json")).unwrap(),
        b"[]"
    );
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn revoked_inflight_provider_output_is_discarded_without_replay() {
    struct RevokeDuringRead {
        source: RuntimeCredentials,
        calls: AtomicUsize,
    }
    impl Transport for RevokeDuringRead {
        fn get(&self, _: &Url, _: &Credential, _: usize, _: Duration) -> Result<Response, Error> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.source.revoke()?;
            Ok(Response {
                status: 200,
                body: br#"{"id":"file-a","name":"private"}"#.to_vec(),
            })
        }
    }
    struct Sink;
    impl AuditSink for Sink {
        fn record(&self, event: Audit) {
            assert!(!serde_json::to_string(&event).unwrap().contains("private"));
        }
    }
    let config = config();
    let source = RuntimeCredentials::new(&config);
    source
        .rotate("synthetic-only".into(), 1, now().unwrap() + 60, true)
        .unwrap();
    let gateway = Gateway::new(
        Grant {
            agent: config.agent,
            tenant: config.tenant,
            account: config.account,
            provider: config.provider,
            resources: config.resources,
            methods: config.methods,
            credential_reference: "runtime".into(),
            generation: 1,
            expires_unix: config.expires_unix,
            max_body_bytes: 4096,
            max_concurrency: 1,
            deadline: Duration::from_secs(1),
        },
        RevokeDuringRead {
            source: source.clone(),
            calls: AtomicUsize::new(0),
        },
        source,
        Sink,
    )
    .unwrap();
    let result = gateway.read(&ReadRequest {
        tenant: "tenant-a".into(),
        account: "account-a".into(),
        resource: "file-a".into(),
        method: ReadMethod::DriveMetadata,
        generation: 1,
    });
    assert!(matches!(result, Err(Error::Revoked)));
    assert_eq!(gateway.transport.calls.load(Ordering::SeqCst), 1);
}

mod audit_regressions;
