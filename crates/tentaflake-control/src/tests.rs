use super::*;
use std::io::{Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{DirBuilderExt, PermissionsExt, symlink};
use std::os::unix::net::{UnixListener, UnixStream};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicU64, Ordering},
};

static SEQUENCE: AtomicU64 = AtomicU64::new(0);
struct Fixture {
    root: PathBuf,
    config: Config,
    _directory: fs::File,
}
impl Fixture {
    fn new() -> Self {
        Self::new_in(&std::env::temp_dir())
    }
    fn new_in(parent: &Path) -> Self {
        // Use the launcher's build temp root, not a sandbox's unrelated /tmp.
        storage::secure_ancestry(parent).expect("fixture requires safe temporary ancestry");
        let root = parent.join(format!(
            "tfc-unit-{}-{}",
            std::process::id(),
            SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::DirBuilder::new().mode(0o700).create(&root).unwrap();
        let directory = storage::open_path(&root, true).unwrap();
        // Only the raw transport fixtures use this descriptor address. State and
        // executable paths retain real ancestry checks; this does not test serve() admission.
        // Keep the descriptor alive, and use our PID so child clients resolve it too.
        let socket = PathBuf::from(format!(
            "/proc/{}/fd/{}/control.sock",
            std::process::id(),
            directory.as_raw_fd()
        ));
        let state = root.join("state");
        fs::create_dir(&state).unwrap();
        fs::set_permissions(&state, fs::Permissions::from_mode(0o700)).unwrap();
        let source = root.join("tentaflake.toml");
        fs::write(&source, "schemaVersion = 1\nagents = []\n").unwrap();
        let normalizer = root.join("mock-normalizer");
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into());
        // A fixed synthetic schema compiler, not a replacement for the shared real-schema corpus.
        fs::write(&normalizer, format!("#!{shell}\n[ \"$1\" = config ] && [ \"$2\" = normalize ] && [ \"$4\" = --json ] || exit 9\ntext=\nwhile IFS= read -r line || [ -n \"$line\" ]; do text=\"$text$line\"; done < \"$3\"\ncase \"$text\" in\n *invalid*) exit 2;;\n *strict*) printf '%s' '{{\"schemaVersion\":1,\"agents\":[],\"sandbox\":{{\"profile\":\"strict\"}}}}';;\n *balanced*) printf '%s' '{{\"schemaVersion\":1,\"agents\":[],\"sandbox\":{{\"profile\":\"balanced\"}}}}';;\n *) printf '%s' '{{\"schemaVersion\":1,\"agents\":[]}}';;\nesac\n")).unwrap();
        fs::set_permissions(&normalizer, fs::Permissions::from_mode(0o700)).unwrap();
        let schema = root.join("schema.json");
        fs::write(&schema, b"{}").unwrap();
        let uid = unsafe { libc::geteuid() };
        let config = Config {
            source,
            normalizer: normalizer.clone(),
            schema,
            state,
            socket,
            server_uid: uid,
            peers: vec![Peer {
                uid,
                roles: vec![Role::Read, Role::Propose, Role::Approve, Role::Build],
            }],
            hashes: Hashes {
                policy: "1".repeat(64),
                compiler: digest(&fs::read(&normalizer).unwrap()),
                inputs: "2".repeat(64),
            },
            minimum_profile: MinimumProfile::Balanced,
            builder: None,
        };
        Self {
            root,
            config,
            _directory: directory,
        }
    }
    fn open(&self) -> Engine {
        // Exercise the real descriptor locks, persistence, hash checks, authorization and restart recovery.
        // Only deployment admission is separate: production open() requires immutable /nix/store executables.
        Engine::open_state(self.config.clone()).unwrap()
    }
    fn script(&self, name: &str, body: &str) -> PathBuf {
        let path = self.root.join(name);
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into());
        fs::write(&path, format!("#!{shell}\n{body}\n")).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
        path
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}
fn call(engine: &mut Engine, method: &str, route: &str, body: Value) -> ApiResult<Value> {
    let bytes = if method == "GET" {
        Vec::new()
    } else {
        serde_json::to_vec(&body).unwrap()
    };
    engine.handle(unsafe { libc::geteuid() }, method, route, &bytes)
}
fn draft(engine: &mut Engine, id: &str) -> Value {
    let system = call(engine, "GET", "/v1/system", Value::Null).unwrap();
    call(engine, "POST", "/v1/system/proposals", json!({"request_id":id,"base_revision":system["revision"],"configuration":system["configuration"]})).unwrap()["proposal"].clone()
}
fn approval(engine: &mut Engine, proposal: &Value, id: &str) -> Value {
    call(engine, "POST", &format!("/v1/system/proposals/{}/approve", proposal["id"].as_str().unwrap()), json!({"request_id":id,"base_revision":proposal["base_revision"],"revision":proposal["revision"],"hashes":proposal["hashes"]})).unwrap()["approval"].clone()
}
fn http(engine: Arc<Mutex<Engine>>, request: &[u8]) -> (u16, Value) {
    let (mut client, server) = UnixStream::pair().unwrap();
    let handler = std::thread::spawn(move || transport::serve_connection(server, &engine));
    let _ = client.write_all(request);
    client.shutdown(std::net::Shutdown::Write).unwrap();
    let mut response = String::new();
    let received = client.read_to_string(&mut response);
    assert!(
        received.is_ok() || !response.is_empty(),
        "missing denial response"
    );
    handler.join().unwrap().unwrap();
    let (headers, body) = response.split_once("\r\n\r\n").unwrap();
    let status = headers.split(' ').nth(1).unwrap().parse().unwrap();
    (status, serde_json::from_str(body).unwrap())
}
fn request(method: &str, route: &str, body: &str) -> Vec<u8> {
    format!("{method} {route} HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).into_bytes()
}

#[test]
fn fixture_uses_private_launcher_root_with_short_socket_address() {
    let parent = Fixture::new();
    let long_temp = parent.root.join("long-launcher-temp-".repeat(8));
    fs::DirBuilder::new()
        .mode(0o700)
        .create(&long_temp)
        .unwrap();
    let f = Fixture::new_in(&long_temp);
    assert_eq!(f.root.parent(), Some(long_temp.as_path()));
    assert_eq!(
        fs::metadata(&f.root).unwrap().permissions().mode() & 0o777,
        0o700
    );
    assert!(f.root.join("control.sock").as_os_str().len() > 108);
    assert!(f.config.socket.as_os_str().len() < 108);
    let _engine = f.open();
    let listener = UnixListener::bind(&f.config.socket).unwrap();
    let mut client = UnixStream::connect(&f.config.socket).unwrap();
    let (mut server, _) = listener.accept().unwrap();
    client.write_all(b"x").unwrap();
    let mut byte = [0];
    server.read_exact(&mut byte).unwrap();
    assert_eq!(byte, *b"x");
    assert!(f.root.join("control.sock").exists());
}

#[test]
fn private_state_refuses_writable_ancestors_even_with_private_leaf() {
    let f = Fixture::new();
    let parent = f.root.join("unsafe");
    let state = parent.join("state");
    fs::DirBuilder::new().mode(0o700).create(&parent).unwrap();
    fs::DirBuilder::new().mode(0o700).create(&state).unwrap();
    for mode in [0o770, 0o702] {
        // No sticky bit: deterministic for both ordinary contributors and root.
        fs::set_permissions(&parent, fs::Permissions::from_mode(mode)).unwrap();
        assert!(storage::secure_ancestry(&state).is_err());
        assert!(PrivateDir::lock(&state).is_err());
        assert!(!state.join("writer.lock").exists());
    }
    fs::set_permissions(&parent, fs::Permissions::from_mode(0o700)).unwrap();
    assert!(PrivateDir::lock(&state).is_ok());
}

#[test]
fn unix_peer_and_role_are_exact_no_root_bypass() {
    let f = Fixture::new();
    let mut e = f.open();
    e.config.peers[0].uid = unsafe { libc::geteuid() }.wrapping_add(1);
    let e = Arc::new(Mutex::new(e));
    assert_eq!(http(e.clone(), &request("GET", "/v1/system", "")).0, 403);
    {
        let mut e = e.lock().unwrap();
        e.config.peers[0].uid = unsafe { libc::geteuid() };
        e.config.peers[0].roles = vec![Role::Read];
    }
    assert_eq!(
        http(e.clone(), &request("POST", "/v1/system/proposals", "{}")).0,
        403
    );
    assert!(e.lock().unwrap().state.requests.is_empty());
}
#[test]
fn unix_malformed_closed_fields_and_nested_duplicates() {
    let f = Fixture::new();
    let e = Arc::new(Mutex::new(f.open()));
    for body in [
        "{",
        "{\"request_id\":\"x\",\"base_revision\":\"x\",\"configuration\":{},\"command\":\"nix\"}",
        "{\"request_id\":\"x\",\"base_revision\":\"x\",\"configuration\":{\"sandbox\":{\"profile\":\"strict\",\"profile\":\"dev\"}}}",
    ] {
        assert_eq!(
            http(e.clone(), &request("POST", "/v1/system/proposals", body)).0,
            400
        );
    }
    assert_eq!(
        http(
            e.clone(),
            b"POST /v1/system/proposals HTTP/1.1\r\nContent-Length: 0\r\nContent-Length: 0\r\n\r\n"
        )
        .0,
        400
    );
    assert_eq!(http(e.clone(), b"POST /v1/system/proposals HTTP/1.1\r\nTransfer-Encoding: chunked\r\nContent-Length: 0\r\n\r\n").0, 400);
    assert!(e.lock().unwrap().state.requests.is_empty());
}
#[test]
fn draft_replay_and_conflicting_request_ids_are_durable() {
    let f = Fixture::new();
    let mut e = f.open();
    let proposal = draft(&mut e, "draft1");
    assert_eq!(draft(&mut e, "draft1"), proposal);
    let bad = call(
        &mut e,
        "POST",
        "/v1/system/proposals",
        json!({"request_id":"draft1","base_revision":"wrong","configuration":{}}),
    )
    .unwrap_err();
    assert_eq!(bad.status, 409);
    drop(e);
    let mut reopened = f.open();
    assert_eq!(draft(&mut reopened, "draft1"), proposal);
}
#[test]
fn base_and_policy_drift_invalidate_approval() {
    let f = Fixture::new();
    let mut e = f.open();
    let proposal = draft(&mut e, "draft1");
    let route = format!(
        "/v1/system/proposals/{}/approve",
        proposal["id"].as_str().unwrap()
    );
    let body = json!({"request_id":"approve1","base_revision":proposal["base_revision"],"revision":proposal["revision"],"hashes":proposal["hashes"]});
    e.config.hashes.policy = "3".repeat(64);
    assert_eq!(
        call(&mut e, "POST", &route, body.clone())
            .unwrap_err()
            .status,
        409
    );
    e.config.hashes.policy = f.config.hashes.policy.clone();
    fs::write(
        &f.config.source,
        "schemaVersion=1\nagents=[]\n[sandbox]\nprofile=\"strict\"\n",
    )
    .unwrap();
    assert!(call(&mut e, "POST", &route, body).is_err());
    assert!(e.state.approvals.is_empty());
    assert!(e.state.builds.is_empty());
}
#[test]
fn compiler_and_schema_drift_fail_closed() {
    let f = Fixture::new();
    let mut e = f.open();
    let proposal = draft(&mut e, "draft1");
    fs::write(&f.config.schema, b"{\"changed\":true}").unwrap();
    assert_ne!(
        e.current_hashes().unwrap().policy,
        proposal["hashes"]["policy"]
    );
    fs::write(&f.config.normalizer, b"replaced executable").unwrap();
    assert_eq!(
        call(&mut e, "GET", "/v1/system", Value::Null)
            .unwrap_err()
            .status,
        503
    );
    assert!(e.state.builds.is_empty());
}
#[test]
fn minimum_sandbox_and_stricter_base_cannot_be_downgraded() {
    let f = Fixture::new();
    let mut e = f.open();
    let system = call(&mut e, "GET", "/v1/system", Value::Null).unwrap();
    let body = json!({"request_id":"unsafe","base_revision":system["revision"],"configuration":{"schemaVersion":1,"agents":[],"sandbox":{"profile":"dev"}}});
    assert_eq!(
        call(&mut e, "POST", "/v1/system/proposals", body)
            .unwrap_err()
            .code,
        "sandbox_policy_denied"
    );
    let strict = json!({"sandbox":{"profile":"strict"}});
    assert!(e.enforce_profile(&json!({}), &strict).is_err());
    assert!(e.enforce_profile(&strict, &strict).is_ok());
    e.config.minimum_profile = MinimumProfile::Strict;
    assert!(e.enforce_profile(&json!({}), &json!({})).is_err());
    assert!(e.state.proposals.is_empty());
    assert!(e.claim().unwrap().is_none());
}
#[test]
fn approvals_are_exact_and_builds_disabled_without_reviewed_inputs() {
    let f = Fixture::new();
    let mut e = f.open();
    let p = draft(&mut e, "draft1");
    let unapproved = call(&mut e, "POST", "/v1/system/builds", json!({"request_id":"unapproved","proposal_id":p["id"],"approval_id":"missing","revision":p["revision"],"hashes":p["hashes"]})).unwrap_err();
    assert_eq!(unapproved.code, "approval_required");
    assert!(e.claim().unwrap().is_none());
    let a = approval(&mut e, &p, "approve1");
    assert_eq!(a["revision"], p["revision"]);
    let result = call(&mut e, "POST", "/v1/system/builds", json!({"request_id":"build1","proposal_id":p["id"],"approval_id":a["id"],"revision":p["revision"],"hashes":p["hashes"]})).unwrap_err();
    assert_eq!(result.code, "build_disabled");
    assert!(e.state.builds.is_empty());
}
#[test]
fn abandoned_work_becomes_unknown_without_redispatch_and_lookup_is_stable() {
    let f = Fixture::new();
    let mut e = f.open();
    let p = draft(&mut e, "draft1");
    let a = approval(&mut e, &p, "approve1");
    let body = json!({"request_id":"build1","proposal_id":p["id"],"approval_id":a["id"],"revision":p["revision"],"hashes":p["hashes"]});
    let parsed: BuildRequest = serde_json::from_value(body.clone()).unwrap();
    let mut next = e.state.clone();
    for (id, state) in [("b_build1", "building"), ("b_build2", "queued")] {
        next.builds.insert(
            id.into(),
            Intent {
                build: Build {
                    id: id.into(),
                    proposal_id: p["id"].as_str().unwrap().into(),
                    revision: p["revision"].as_str().unwrap().into(),
                    state: state.into(),
                    output_path: None,
                    error: None,
                },
                approval_id: a["id"].as_str().unwrap().into(),
                hashes: e.current_hashes().unwrap(),
            },
        );
    }
    next.requests.insert(
        "build1".into(),
        RequestRecord {
            fingerprint: revision(&json!(["/v1/system/builds", parsed])),
            kind: "build".into(),
            id: "b_build1".into(),
        },
    );
    e.state = next;
    e.persist().unwrap();
    drop(e);
    let mut e = f.open();
    assert!(e.claim().unwrap().is_none());
    for id in ["b_build1", "b_build2"] {
        assert_eq!(e.state.builds[id].build.state, "unknown");
    }
    let first = call(&mut e, "GET", "/v1/system/builds/b_build1", Value::Null).unwrap();
    assert_eq!(
        call(&mut e, "POST", "/v1/system/builds", body).unwrap(),
        first
    );
    assert_eq!(
        call(&mut e, "GET", "/v1/system/builds/b_build1", Value::Null).unwrap(),
        first
    );
    assert_eq!(
        fs::read_to_string(&f.config.source).unwrap(),
        "schemaVersion = 1\nagents = []\n"
    );
}
#[test]
fn storage_lock_symlinks_and_immutable_template_are_enforced() {
    let f = Fixture::new();
    let e = f.open();
    assert!(PrivateDir::lock(&f.config.state).is_err());
    symlink(&f.config.source, f.root.join("linked-source")).unwrap();
    assert!(storage::read_file(&f.root.join("linked-source"), MAX_DOCUMENT).is_err());
    symlink(&f.root, f.root.join("linked-parent")).unwrap();
    assert!(
        storage::read_file(&f.root.join("linked-parent/tentaflake.toml"), MAX_DOCUMENT).is_err()
    );
    assert!(template::fingerprint(&f.root).is_err());
    assert!(Engine::open(f.config.clone()).is_err());
    drop(e);
    fs::remove_file(f.config.state.join("receipts.json")).unwrap();
    symlink(&f.config.source, f.config.state.join("receipts.json")).unwrap();
    assert!(Engine::open_state(f.config.clone()).is_err());
}
#[test]
fn fixed_mock_process_success_failure_output_and_deadline_are_bounded() {
    let f = Fixture::new();
    let success = f.script("success", "[ -z \"${HOSTILE_ENV+x}\" ] || exit 8\nprintf '%s' '[{\"outputs\":{\"out\":\"/nix/store/00000000000000000000000000000000-system\"}}]' ");
    let mut command = Command::new(success);
    command.env("HOSTILE_ENV", "must-not-leak");
    let output = process::run(&mut command, Duration::from_secs(2), 1024).unwrap();
    assert!(serde_json::from_slice::<Value>(&output).unwrap().is_array());
    let failure = f.script("failure", "printf 'private diagnostic' >&2; exit 1");
    assert_eq!(
        process::run(&mut Command::new(failure), Duration::from_secs(2), 1024).unwrap_err(),
        "fixed executable failed"
    );
    let overflow = f.script(
        "overflow",
        "while :; do printf '0123456789012345678901234567890123456789'; done",
    );
    assert!(
        process::run(&mut Command::new(overflow), Duration::from_secs(2), 100)
            .unwrap_err()
            .contains("output")
    );
    let timeout = f.script("timeout", "while :; do :; done");
    assert!(
        process::run(&mut Command::new(timeout), Duration::from_millis(20), 100)
            .unwrap_err()
            .contains("deadline")
    );
}
#[test]
fn output_paths_and_diff_never_accept_executable_or_secret_excerpts() {
    assert!(template::output_path(
        "/nix/store/00000000000000000000000000000000-system"
    ));
    for path in [
        "/etc/passwd",
        "/nix/store/../../etc/passwd",
        "/nix/store/00000000000000000000000000000000-system/bin/switch-to-configuration",
        "/nix/store/eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee-system",
        "/nix/store/00000000000000000000000000000000-system\n",
    ] {
        assert!(!template::output_path(path));
    }
    let mut changes = Vec::new();
    diff(
        &json!({"a":["old-private"]}),
        &json!({"a":["new-private"]}),
        "",
        &mut changes,
    );
    assert_eq!(changes, vec![json!({"path":"/a/0","kind":"changed"})]);
}
#[test]
fn unix_client_rejects_wrong_server_uid_before_sending() {
    let f = Fixture::new();
    let listener = UnixListener::bind(&f.config.socket).unwrap();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut bytes = Vec::new();
        stream.read_to_end(&mut bytes).unwrap();
        bytes
    });
    let mut config = f.config.clone();
    config.server_uid = unsafe { libc::geteuid() }.wrapping_add(1);
    assert!(
        transport::client(&config, "GET", "/v1/system", b"")
            .unwrap_err()
            .contains("UID")
    );
    assert!(server.join().unwrap().is_empty());
}

#[test]
fn failed_receipt_publication_closes_writer_before_dispatch() {
    let f = Fixture::new();
    let mut e = f.open();
    fs::remove_file(f.config.state.join("receipts.json")).unwrap();
    fs::create_dir(f.config.state.join("receipts.json")).unwrap();
    assert!(e.persist().is_err());
    assert!(e.claim().is_err());
    assert_eq!(
        call(&mut e, "GET", "/v1/system", Value::Null)
            .unwrap_err()
            .status,
        503
    );
}

#[test]
fn corrupted_candidate_receipt_is_not_reinterpreted_on_restart() {
    let f = Fixture::new();
    let mut e = f.open();
    let p = draft(&mut e, "draft1");
    e.state
        .proposals
        .get_mut(p["id"].as_str().unwrap())
        .unwrap()
        .toml = "schemaVersion=2\nagents=[]\n".into();
    e.persist().unwrap();
    drop(e);
    assert!(Engine::open_state(f.config.clone()).is_err());
}
