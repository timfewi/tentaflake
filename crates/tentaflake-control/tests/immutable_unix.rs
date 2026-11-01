//! Isolated source-tier acceptance: the builder is deliberately synthetic, never Nix or activation.
//! Run explicitly with TENTAFLAKE_CONTROL_FIXTURE pointing to tests/fixture.nix's store output.
use serde_json::{Value, json};
use std::fs;
use std::io::{Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tentaflake_control::{
    Config, Engine, Hashes, MinimumProfile, Peer, Role, digest, storage, template, transport,
};

fn http(engine: &Arc<Mutex<Engine>>, method: &str, path: &str, value: Value) -> (u16, Value) {
    let body = if method == "GET" {
        String::new()
    } else {
        value.to_string()
    };
    let (mut client, server) = UnixStream::pair().unwrap();
    let cloned = Arc::clone(engine);
    let task = std::thread::spawn(move || transport::serve_connection(server, &cloned));
    write!(client, "{method} {path} HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
    let mut response = String::new();
    client.read_to_string(&mut response).unwrap();
    task.join().unwrap().unwrap();
    let (headers, body) = response.split_once("\r\n\r\n").unwrap();
    (
        headers.split(' ').nth(1).unwrap().parse().unwrap(),
        serde_json::from_str(body).unwrap(),
    )
}
struct Scratch {
    root: PathBuf,
    _directory: fs::File,
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}
fn config(fixture: &Path, target: &str) -> (Scratch, Config) {
    let parent = std::env::temp_dir();
    storage::secure_ancestry(&parent).expect("fixture requires safe temporary ancestry");
    let root = parent.join(format!("tfc-immutable-{}-{target}", std::process::id()));
    fs::DirBuilder::new().mode(0o700).create(&root).unwrap();
    let directory = storage::open_path(&root, true).unwrap();
    // The raw listener/UI client fixtures need a short address even under a long
    // TMPDIR. Keep its descriptor alive in this process for child clients too.
    // This is not the production serve() socket-directory admission boundary.
    let socket = PathBuf::from(format!(
        "/proc/{}/fd/{}/control.sock",
        std::process::id(),
        directory.as_raw_fd()
    ));
    let state = root.join("state");
    fs::create_dir(&state).unwrap();
    fs::set_permissions(&state, fs::Permissions::from_mode(0o700)).unwrap();
    let source = root.join("tentaflake.toml");
    fs::copy(fixture.join("template/tentaflake.toml"), &source).unwrap();
    fs::set_permissions(&source, fs::Permissions::from_mode(0o600)).unwrap();
    let uid = unsafe { libc::geteuid() };
    let normalizer = std::env::var_os("TENTAFLAKE_REAL_NORMALIZER")
        .map(PathBuf::from)
        .unwrap_or_else(|| fixture.join("normalizer"));
    let config = Config {
        source,
        state,
        socket,
        normalizer: normalizer.clone(),
        schema: fixture.join("schema.json"),
        server_uid: uid,
        peers: vec![Peer {
            uid,
            roles: vec![Role::Read, Role::Propose, Role::Approve, Role::Build],
        }],
        hashes: Hashes {
            policy: "1".repeat(64),
            compiler: digest(&fs::read(&normalizer).unwrap()),
            inputs: digest(&fs::read(fixture.join("template/flake.lock")).unwrap()),
        },
        minimum_profile: MinimumProfile::Balanced,
        builder: Some(template::Builder {
            template: fixture.join("template"),
            template_hash: template::fingerprint(&fixture.join("template")).unwrap(),
            nix: fixture.join("builder"),
            target: target.into(),
            deadline_seconds: 1,
        }),
    };
    (
        Scratch {
            root,
            _directory: directory,
        },
        config,
    )
}
fn approved(engine: &Arc<Mutex<Engine>>) -> (Value, Value) {
    let (status, current) = http(engine, "GET", "/v1/system", Value::Null);
    assert_eq!(status, 200);
    let (status, proposal) = http(
        engine,
        "POST",
        "/v1/system/proposals",
        json!({"request_id":"draft","base_revision":current["revision"],"configuration":current["configuration"]}),
    );
    assert_eq!(status, 200);
    let p = &proposal["proposal"];
    let (status, refused) = http(
        engine,
        "POST",
        "/v1/system/builds",
        json!({"request_id":"unapproved","proposal_id":p["id"],"approval_id":"missing","revision":p["revision"],"hashes":p["hashes"]}),
    );
    assert_eq!(status, 409);
    assert_eq!(refused["error"]["code"], "approval_required");
    assert!(engine.lock().unwrap().claim().unwrap().is_none());
    let (status, approval) = http(
        engine,
        "POST",
        &format!("/v1/system/proposals/{}/approve", p["id"].as_str().unwrap()),
        json!({"request_id":"approval","base_revision":p["base_revision"],"revision":p["revision"],"hashes":p["hashes"]}),
    );
    assert_eq!(status, 200);
    (p.clone(), approval["approval"].clone())
}
fn build_request(p: &Value, a: &Value) -> Value {
    json!({"request_id":"build","proposal_id":p["id"],"approval_id":a["id"],"revision":p["revision"],"hashes":p["hashes"]})
}

#[test]
#[ignore = "requires explicitly built immutable synthetic fixture; see docs/20-system-control.md"]
fn immutable_unix_approval_build_and_recovery_contract() {
    let fixture = PathBuf::from(
        std::env::var_os("TENTAFLAKE_CONTROL_FIXTURE").expect("set immutable fixture output"),
    );
    assert!(template::fingerprint(&fixture.join("linked-template")).is_err());
    {
        let (_scratch, mut invalid) = config(&fixture, "hash_drift");
        invalid.hashes.inputs = "0".repeat(64);
        assert!(Engine::open(invalid.clone()).is_err());
        invalid.hashes.inputs = digest(&fs::read(fixture.join("template/flake.lock")).unwrap());
        invalid.builder.as_mut().unwrap().template_hash = "0".repeat(64);
        assert!(Engine::open(invalid).is_err());
    }
    for target in ["ok", "fail", "badpath", "noisy", "slow"] {
        let (_scratch, config) = config(&fixture, target);
        let source_before = fs::read(&config.source).unwrap();
        let engine = Arc::new(Mutex::new(Engine::open(config.clone()).unwrap()));
        let (proposal, approval) = approved(&engine);
        let body = build_request(&proposal, &approval);
        let (status, queued) = http(&engine, "POST", "/v1/system/builds", body.clone());
        assert_eq!(status, 200);
        assert_eq!(queued["build"]["state"], "queued");
        let mut conflicting = body.clone();
        conflicting["revision"] = json!("0".repeat(64));
        assert_eq!(
            http(&engine, "POST", "/v1/system/builds", conflicting).0,
            409
        );
        let id = queued["build"]["id"].as_str().unwrap();
        let work = engine.lock().unwrap().claim().unwrap().unwrap();
        assert!(engine.lock().unwrap().claim().unwrap().is_none());
        let result = work.execute();
        assert_eq!(result.is_ok(), target == "ok");
        engine.lock().unwrap().finish(id, result).unwrap();
        let (_, receipt) = http(
            &engine,
            "GET",
            &format!("/v1/system/builds/{id}"),
            Value::Null,
        );
        assert_eq!(
            receipt["build"]["state"],
            if target == "ok" { "built" } else { "failed" }
        );
        assert_eq!(http(&engine, "POST", "/v1/system/builds", body).1, receipt);
        assert_eq!(fs::read(&config.source).unwrap(), source_before);
        assert_eq!(
            fs::read(config.state.join(id).join("flake.lock")).unwrap(),
            fs::read(fixture.join("template/flake.lock")).unwrap()
        );
        drop(engine);
        let reopened = Arc::new(Mutex::new(Engine::open(config).unwrap()));
        assert_eq!(
            http(
                &reopened,
                "GET",
                &format!("/v1/system/builds/{id}"),
                Value::Null
            )
            .1,
            receipt
        );
        assert!(reopened.lock().unwrap().claim().unwrap().is_none());
    }
    {
        let (_scratch, mut config) = config(&fixture, "queue");
        config.builder.as_mut().unwrap().target = "ok".into();
        let engine = Arc::new(Mutex::new(Engine::open(config.clone()).unwrap()));
        let (p, _) = approved(&engine);
        for n in 0..5 {
            let (status, a) = http(
                &engine,
                "POST",
                &format!("/v1/system/proposals/{}/approve", p["id"].as_str().unwrap()),
                json!({"request_id":format!("a{n}"),"base_revision":p["base_revision"],"revision":p["revision"],"hashes":p["hashes"]}),
            );
            assert_eq!(status, 200);
            let mut body = build_request(&p, &a["approval"]);
            body["request_id"] = json!(format!("b{n}"));
            assert_eq!(
                http(&engine, "POST", "/v1/system/builds", body).0,
                if n < 4 { 200 } else { 429 }
            );
        }
        // Re-check the live base at dispatch, not only when the intent was admitted.
        fs::write(
            &config.source,
            "schemaVersion=1\nagents=[]\n[sandbox]\nprofile=\"strict\"\n",
        )
        .unwrap();
        assert!(engine.lock().unwrap().claim().unwrap().is_none());
        assert_eq!(
            http(&engine, "GET", "/v1/system/builds/b_b0", Value::Null).1["build"]["state"],
            "failed"
        );
        assert!(!config.state.join("b_b0").exists());
    }
    for (target, claim) in [("recovery_queued", false), ("recovery_building", true)] {
        let (_scratch, mut config) = config(&fixture, target);
        config.builder.as_mut().unwrap().target = "ok".into();
        let engine = Arc::new(Mutex::new(Engine::open(config.clone()).unwrap()));
        let (p, a) = approved(&engine);
        let body = build_request(&p, &a);
        assert_eq!(
            http(&engine, "POST", "/v1/system/builds", body.clone()).0,
            200
        );
        if claim {
            assert!(engine.lock().unwrap().claim().unwrap().is_some());
        }
        drop(engine);
        let reopened = Arc::new(Mutex::new(Engine::open(config.clone()).unwrap()));
        assert!(reopened.lock().unwrap().claim().unwrap().is_none());
        let (_, receipt) = http(&reopened, "POST", "/v1/system/builds", body);
        assert_eq!(receipt["build"]["state"], "unknown");
        let mut fresh_id = build_request(&p, &a);
        fresh_id["request_id"] = json!("different-id-is-not-a-new-grant");
        assert_eq!(
            http(&reopened, "POST", "/v1/system/builds", fresh_id).0,
            409
        );
        assert!(!config.state.join("b_build").exists());
    }
}

#[test]
#[ignore = "requires immutable fixture, real normalizer, pinned Python and explicit UI source"]
fn ui_adapter_speaks_real_closed_transport() {
    use std::os::unix::net::UnixListener;
    use std::process::Command;
    use std::time::{Duration, Instant};
    let fixture =
        PathBuf::from(std::env::var_os("TENTAFLAKE_CONTROL_FIXTURE").expect("set fixture"));
    let python = std::env::var_os("TENTAFLAKE_TEST_PYTHON").expect("set pinned Python");
    let ui = std::env::var_os("TENTAFLAKE_UI_SOURCE").expect("set reviewed UI checkout");
    assert!(std::env::var_os("TENTAFLAKE_REAL_NORMALIZER").is_some());
    let (_scratch, config) = config(&fixture, "ui-contract");
    let listener = UnixListener::bind(&config.socket).unwrap();
    listener.set_nonblocking(true).unwrap();
    let engine = Arc::new(Mutex::new(Engine::open(config.clone()).unwrap()));
    let server = Arc::clone(&engine);
    let task = std::thread::spawn(move || {
        let start = Instant::now();
        let mut requests = 0;
        while requests < 5 && start.elapsed() < Duration::from_secs(20) {
            match listener.accept() {
                Ok((stream, _)) => {
                    transport::serve_connection(stream, &server).unwrap();
                    requests += 1;
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(error) => panic!("test listener failed: {error}"),
            }
        }
        requests
    });
    let output = Command::new(python).env_clear()
        .env("PYTHONPATH", ui).env("PYTHONDONTWRITEBYTECODE", "1")
        .env("CONTROL_SOCKET", &config.socket)
        .env("CONTROL_UID", config.server_uid.to_string())
        .args(["-c", r#"
import os
from types import SimpleNamespace
from bridge.operator import system
cfg = SimpleNamespace(control_socket=os.environ["CONTROL_SOCKET"], control_uid=int(os.environ["CONTROL_UID"]))
def call(method, route, body=None):
    status, value = system(cfg, method, route, body)
    assert status == 200
    return value
current = call("GET", "/api/system")
p = call("POST", "/api/system/proposals", {"request_id":"ui-draft","base_revision":current["revision"],"configuration":current["configuration"]})["proposal"]
a = call("POST", "/api/system/proposals/" + p["id"] + "/approve", {"request_id":"ui-approve","base_revision":p["base_revision"],"revision":p["revision"],"hashes":p["hashes"]})["approval"]
b = call("POST", "/api/system/builds", {"request_id":"ui-build","proposal_id":p["id"],"approval_id":a["id"],"revision":p["revision"],"hashes":p["hashes"]})["build"]
assert b["state"] == "queued"
assert call("GET", "/api/system/builds/" + b["id"])["build"] == b
"#]).output().unwrap();
    assert_eq!(task.join().unwrap(), 5);
    assert!(
        output.status.success(),
        "real UI adapter contract failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    // No worker was started: this contract test neither builds nor activates a host.
    assert!(!config.state.join("b_ui-build").exists());
}
