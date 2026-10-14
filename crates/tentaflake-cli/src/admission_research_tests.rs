use super::*;
use serde_json::{Value, json};
use std::io::Write;
use std::os::fd::AsFd;
use std::os::unix::fs::DirBuilderExt;
use std::os::unix::net::UnixListener;
use std::sync::atomic::{AtomicU64, Ordering};

const INSTANCE: &str = "agent-fixture";
const GENERATION: &str = "11111111-2222-4333-8444-555555555555";
const STORE: &str = "/nix/store/00000000000000000000000000000000";

fn descriptor() -> Value {
    let client = format!("{STORE}-research-client");
    let service = format!("{STORE}-research-service");
    let egress = format!("{STORE}-research-egress");
    let socat = format!("{STORE}-socat");
    let dependency = format!("{STORE}-runtime-library");
    let service_config = format!("{STORE}-research-service.json");
    let egress_config = format!("{STORE}-research-egress.json");
    json!({
        "schemaVersion":1,
        "client":{
            "executable":format!("{client}/bin/research-client"),
            "closurePaths":[client,dependency],
            "socketDestination":"/run/tentaflake-research/socket"
        },
        "relay":{
            "socketUnit":"tentaflake-research-agent-fixture.socket",
            "socketPath":"/run/tentaflake-research/agent-fixture/socket",
            "template":{
                "unit":"tentaflake-research-agent-fixture@.service",
                "executable":format!("{socat}/bin/socat"),
                "argv":[format!("{socat}/bin/socat"),"STDIO","UNIX-CONNECT:/run/agent-research/socket"],
                "uid":62107,"rootDirectory":format!("{STORE}-research-container-relay-root"),
                "slice":"agent-research.slice",
                "bindReadOnlyPaths":[socat,dependency,"/run/agent-research"],
                "configuration":null
            }
        },
        "service":{
            "unit":"agent-research.service",
            "executable":format!("{service}/bin/research-service"),
            "argv":[format!("{service}/bin/research-service"),"--listen-fd","3",
                "--temporary-directory","/tmp/research","--public-only","--config","/etc/research/config.json"],
            "uid":4201,"rootDirectory":format!("{STORE}-research-service-root"),
            "slice":"agent-research.slice",
            "bindReadOnlyPaths":[service,dependency,format!("{service_config}:/etc/research/config.json"),
                "/run/agent-research","/run/agent-research-egress"],
            "configuration":{"source":service_config,"destination":"/etc/research/config.json"}
        },
        "egress":{
            "unit":"agent-research-egress.service",
            "executable":format!("{egress}/bin/research-egress"),
            "argv":[format!("{egress}/bin/research-egress"),"--listen-fd","3","--config","/etc/research/config.json"],
            "uid":4202,"rootDirectory":format!("{STORE}-research-egress-root"),
            "slice":"agent-research.slice",
            "bindReadOnlyPaths":[egress,dependency,format!("{egress_config}:/etc/research/config.json"),
                "/run/agent-research-egress"],
            "configuration":{"source":egress_config,"destination":"/etc/research/config.json"}
        },
        "controller":{
            "unit":"agent-research-egress-control.service",
            "executable":format!("{egress}/bin/research-egress-control"),
            "argv":[format!("{egress}/bin/research-egress-control"),"--input","/run/research-vpn/state.json",
                "--output","/run/agent-research-egress/control/state.json","--drain-seconds","30"],
            "uid":0,"rootDirectory":format!("{STORE}-research-control-root"),
            "slice":"agent-research.slice",
            "bindReadOnlyPaths":[egress,dependency,"/run/research-vpn"],
            "configuration":null
        },
        "controlPath":"/run/agent-research-egress/control/state.json",
        "vpnInterface":"fixture-vpn"
    })
}

/// Descriptive fixture matching the owning Nix module's four process roles.
/// Its store paths do not represent acquired artifacts or runtime evidence.
pub(crate) fn spec() -> ResearchSpec {
    serde_json::from_value(descriptor()).unwrap()
}

#[test]
fn descriptor_is_closed_and_accepts_the_declared_process_roles() {
    validate(INSTANCE, &spec()).unwrap();
    assert!(validate("other-agent", &spec()).is_err());
    for field in ["client", "relay", "service", "egress", "controller"] {
        let mut changed = descriptor();
        changed[field]["unexpected"] = json!(true);
        assert!(
            serde_json::from_value::<ResearchSpec>(changed).is_err(),
            "{field}"
        );
    }
    let mut changed = descriptor();
    changed["service"]["configuration"]["unexpected"] = json!(true);
    assert!(serde_json::from_value::<ResearchSpec>(changed).is_err());
    let mut changed = descriptor();
    changed["callerUid"] = json!(62107);
    assert!(serde_json::from_value::<ResearchSpec>(changed).is_err());
}

#[test]
fn declarations_cannot_substitute_identity_command_or_configuration() {
    let mutations: &[fn(&mut ResearchSpec)] = &[
        |s| s.schema_version = 2,
        |s| s.service.uid = 0,
        |s| s.egress.uid = s.service.uid,
        |s| s.relay.template.uid = s.egress.uid,
        |s| s.controller.uid = 4201,
        |s| s.relay.template.uid = u32::MAX,
        |s| std::mem::swap(&mut s.service, &mut s.egress),
        |s| s.relay.template.unit = "tentaflake-research-other@.service".into(),
        |s| s.service.executable = "/usr/bin/research-service".into(),
        |s| s.service.argv[0] = format!("{STORE}-other/bin/research-service"),
        |s| s.service.argv.clear(),
        |s| s.service.argv.retain(|v| v != "--public-only"),
        |s| s.egress.argv[2] = "4".into(),
        |s| s.relay.template.argv[2] = "TCP:localhost:9000".into(),
        |s| s.controller.argv[4] = "/run/another-lease.json".into(),
        |s| s.controller.argv[6] = "030".into(),
        |s| s.controller.argv[6] = "301".into(),
        |s| s.service.root_directory = "/var/lib/research-root".into(),
        |s| s.service.slice = "system.slice".into(),
        |s| s.service.configuration = None,
        |s| s.relay.template.configuration = s.service.configuration.clone(),
        |s| s.service.configuration.as_mut().unwrap().source = "/run/config.json".into(),
        |s| s.service.configuration.as_mut().unwrap().destination = "/etc/other.json".into(),
        |s| s.service.bind_read_only_paths.retain(|v| !v.contains(':')),
        |s| s.control_path = "/run/agent-research-egress/control/../state.json".into(),
        |s| s.relay.socket_path = "/run/tentaflake-research/other/socket".into(),
        |s| s.relay.socket_unit = "tentaflake-research-other.socket".into(),
        |s| s.client.socket_destination = "/run/agent-research/socket".into(),
        |s| s.vpn_interface = "../fixture-vpn".into(),
    ];
    for (index, mutate) in mutations.iter().enumerate() {
        let mut changed = spec();
        mutate(&mut changed);
        assert!(validate(INSTANCE, &changed).is_err(), "mutation {index}");
    }
}

#[test]
fn closure_and_bindings_cannot_add_mutable_or_ambiguous_authority() {
    for binding in [
        "/home/operator".to_owned(),
        "/etc/research/config.json".to_owned(),
        "/run/agent-research/../credentials".to_owned(),
        "/run//agent-research".to_owned(),
        "/run/agent-research\n".to_owned(),
        "-/run/agent-research".to_owned(),
        "/var/lib/config:/etc/research/config.json".to_owned(),
        format!("{STORE}-research-service.json:/etc/research/config.json:norbind"),
    ] {
        let mut changed = spec();
        changed.service.bind_read_only_paths.push(binding.clone());
        assert!(validate(INSTANCE, &changed).is_err(), "binding {binding:?}");
    }
    let mut duplicate = spec();
    duplicate
        .service
        .bind_read_only_paths
        .push(duplicate.service.bind_read_only_paths[0].clone());
    assert!(validate(INSTANCE, &duplicate).is_err());
    for paths in [
        vec![],
        vec![PathBuf::from("/var/lib/research-client")],
        vec![PathBuf::from(format!("{STORE}-research-client/bin"))],
        vec![PathBuf::from(format!("{STORE}-other-client"))],
        vec![spec().client.closure_paths[0].clone(); 2],
        vec![PathBuf::from(format!("{STORE}-research-client/../other"))],
    ] {
        let mut changed = spec();
        changed.client.closure_paths = paths;
        assert!(validate(INSTANCE, &changed).is_err());
    }
}

fn lease_value(expires: u64) -> Value {
    json!({"version":1,"generation":GENERATION,"mode":"ready","valid_until":expires,"region":"DE"})
}

#[test]
fn lease_renewal_preserves_generation_without_reusing_expired_evidence() {
    let first = parse_lease(&serde_json::to_vec(&lease_value(102)).unwrap(), 100).unwrap();
    let renewed = parse_lease(&serde_json::to_vec(&lease_value(104)).unwrap(), 101).unwrap();
    assert_eq!(first.generation, renewed.generation);
    assert!(renewed.valid_until > first.valid_until);
    assert!(parse_lease(&serde_json::to_vec(&lease_value(102)).unwrap(), 102).is_err());
    let mut without_region = lease_value(102);
    without_region.as_object_mut().unwrap().remove("region");
    assert!(parse_lease(&serde_json::to_vec(&without_region).unwrap(), 100).is_ok());
}

#[test]
fn lease_refuses_stale_or_contradictory_controller_evidence() {
    for (field, value) in [
        ("version", json!(2)),
        ("mode", json!("draining")),
        ("mode", json!("offline")),
        ("mode", json!("unknown")),
        ("valid_until", json!(99)),
        ("valid_until", json!(100)),
        ("valid_until", json!(104)),
        ("valid_until", json!(-1)),
        ("generation", json!(null)),
        ("generation", json!("00000000-0000-0000-0000-000000000000")),
        ("generation", json!("AAAAAAAA-2222-4333-8444-555555555555")),
        ("generation", json!("11111111-2222-4333-8444-55555555555g")),
        ("generation", json!("11111111")),
        ("region", json!("de")),
        ("region", json!("DEU")),
        ("region", json!("D")),
        ("region", json!("ÄB")),
        ("extra", json!(true)),
    ] {
        let mut changed = lease_value(102);
        changed[field] = value;
        assert!(
            parse_lease(&serde_json::to_vec(&changed).unwrap(), 100).is_err(),
            "field {field}"
        );
    }
    let valid = serde_json::to_string(&lease_value(102)).unwrap();
    let duplicate = valid.replacen("\"version\":1", "\"version\":1,\"version\":1", 1);
    assert!(parse_lease(duplicate.as_bytes(), 100).is_err());
    for bytes in [b"{".as_slice(), b"null", b"[]", b"\xff", b"{}"] {
        assert!(parse_lease(bytes, 100).is_err());
    }
}

const MOUNTS: &str = "20 1 8:1 / / rw,relatime - ext4 /dev/root rw\n21 20 8:1 /config /etc/research ro,relatime - ext4 /dev/root rw\n";

#[test]
fn read_only_evidence_uses_effective_mount_flags_and_path_components() {
    assert!(readonly_mount(MOUNTS, "/etc/research/config.json"));
    assert!(!readonly_mount(MOUNTS, "/etc/researcher/config.json"));
    assert!(!readonly_mount(
        &MOUNTS.replace("ro,relatime", "rw,relatime"),
        "/etc/research/config.json"
    ));
    // A read-only superblock report does not replace read-only VFS mount flags.
    assert!(!readonly_mount(
        "20 1 8:1 / / rw,relatime - ext4 /dev/root ro\n",
        "/etc/research/config.json"
    ));
    let child = format!("{MOUNTS}22 21 0:44 / /etc/research/child rw,nosuid - tmpfs tmpfs rw\n");
    assert!(!readonly_mount(&child, "/etc/research/child/config.json"));
    assert!(!readonly_mount(&child, "/etc/research"));
}

#[test]
fn read_only_evidence_refuses_stacked_and_malformed_mount_tables() {
    let upper = "22 20 0:44 / /etc/research rw,nosuid - tmpfs tmpfs rw\n";
    for mounts in [format!("{MOUNTS}{upper}"), format!("{upper}{MOUNTS}")] {
        assert!(!readonly_mount(&mounts, "/etc/research/config.json"));
    }
    for mounts in [
        String::new(),
        format!("{MOUNTS}truncated\n"),
        MOUNTS.replace(" - ext4", " ext4"),
        MOUNTS.replace("21 20 8:1", "bad 20 8:1"),
        MOUNTS.replace("ro,relatime", "ro,rw,relatime"),
        MOUNTS.replace("/etc/research", "etc/research"),
        MOUNTS.replace("/etc/research", "/etc/research/../research"),
        MOUNTS.replace("/etc/research", "/etc/research\\999"),
    ] {
        assert!(
            !readonly_mount(&mounts, "/etc/research/config.json"),
            "{mounts:?}"
        );
    }
}

struct Directory(PathBuf);

impl Directory {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        // AF_UNIX pathname limits require a short path even inside Nix builds.
        let path = Path::new("/tmp").join(format!(
            "tf-research-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::DirBuilder::new().mode(0o700).create(&path).unwrap();
        Self(path)
    }
}

impl Drop for Directory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn real_connection_uses_the_held_parent_after_a_path_replacement() {
    let directory = Directory::new();
    let original = directory.0.join("parent");
    fs::create_dir(&original).unwrap();
    let parent = File::open(&original).unwrap();
    let listener = UnixListener::bind(original.join("socket")).unwrap();
    fs::rename(&original, directory.0.join("moved")).unwrap();
    fs::create_dir(&original).unwrap();
    let replacement = UnixListener::bind(original.join("socket")).unwrap();
    replacement.set_nonblocking(true).unwrap();

    let mut client = connect(&parent, Instant::now() + Duration::from_secs(1)).unwrap();
    let (mut accepted, _) = listener.accept().unwrap();
    accepted
        .set_read_timeout(Some(Duration::from_secs(1)))
        .unwrap();
    client
        .set_write_timeout(Some(Duration::from_secs(1)))
        .unwrap();
    client.write_all(b"owned relay").unwrap();
    let mut received = [0; 11];
    accepted.read_exact(&mut received).unwrap();
    assert_eq!(&received, b"owned relay");
    assert_eq!(
        replacement.accept().unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock
    );
}

#[test]
fn real_connection_refuses_missing_non_socket_and_closed_listener_paths() {
    let directory = Directory::new();
    let parent = File::open(&directory.0).unwrap();
    let path = directory.0.join("socket");
    assert!(connect(&parent, Instant::now() + Duration::from_secs(1)).is_err());
    File::create(&path).unwrap();
    assert!(connect(&parent, Instant::now() + Duration::from_secs(1)).is_err());
    fs::remove_file(&path).unwrap();
    let listener = UnixListener::bind(&path).unwrap();
    drop(listener);
    let started = Instant::now();
    assert!(connect(&parent, started + Duration::from_secs(1)).is_err());
    assert!(started.elapsed() < Duration::from_secs(1));
}

#[test]
fn elapsed_connection_deadline_cannot_enqueue_a_new_relay() {
    let directory = Directory::new();
    let parent = File::open(&directory.0).unwrap();
    let listener = UnixListener::bind(directory.0.join("socket")).unwrap();
    listener.set_nonblocking(true).unwrap();
    assert!(connect(&parent, Instant::now()).is_err());
    assert_eq!(
        listener.accept().unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock
    );
}

const UNIX_HEADER: &str = "Num       RefCount Protocol Flags    Type St Inode Path\n";

#[test]
fn listener_table_distinguishes_established_streams_from_listener_ambiguity() {
    let path = "/run/agent-research/socket";
    let listener = format!("0000000000000000: 00000002 00000000 00010000 0001 01 42 {path}\n");
    let established = format!("0000000000000000: 00000003 00000000 00000000 0001 03 43 {path}\n");
    for rows in [
        format!("{listener}{established}"),
        format!("{established}{listener}"),
    ] {
        assert_eq!(
            listening_inode(format!("{UNIX_HEADER}{rows}").as_bytes(), path).unwrap(),
            42
        );
    }
    assert!(listening_inode(format!("{UNIX_HEADER}{established}").as_bytes(), path).is_err());
    for second in [listener.clone(), listener.replace("01 42", "01 44")] {
        assert!(
            listening_inode(format!("{UNIX_HEADER}{listener}{second}").as_bytes(), path).is_err()
        );
    }
}

#[test]
fn listener_table_refuses_malformed_matching_rows() {
    let path = "/run/agent-research/socket";
    let listener = format!("0000000000000000: 00000002 00000000 00010000 0001 01 42 {path}\n");
    for row in [
        listener.replace("00000000 00010000", "00000001 00010000"),
        listener.replace("00010000", "00000000"),
        listener.replace("0001 01", "0002 01"),
        listener.replace("0001 01", "0001 03"),
        listener.replace("01 42", "01 0"),
        listener.replace("01 42", "01 invalid"),
        listener.trim_end().to_owned() + " extra\n",
    ] {
        assert!(listening_inode(format!("{UNIX_HEADER}{listener}{row}").as_bytes(), path).is_err());
    }
    assert!(listening_inode(listener.as_bytes(), path).is_err());
    assert!(listening_inode(b"\xff", path).is_err());
}

#[test]
fn real_listener_inode_survives_an_accepted_stream_with_the_same_pathname() {
    let directory = Directory::new();
    let path = directory.0.join("socket");
    let listener = UnixListener::bind(&path).unwrap();
    let _client = UnixStream::connect(&path).unwrap();
    let (accepted, _) = listener.accept().unwrap();
    // File::metadata uses fstat on a duplicate of the actual socket descriptor;
    // the filesystem socket node belongs to a different inode namespace.
    let listener_inode = File::from(listener.as_fd().try_clone_to_owned().unwrap())
        .metadata()
        .unwrap()
        .ino();
    let accepted_inode = File::from(accepted.as_fd().try_clone_to_owned().unwrap())
        .metadata()
        .unwrap()
        .ino();
    assert_ne!(listener_inode, accepted_inode);
    let path = path.to_str().unwrap();
    let table = bounded(Path::new("/proc/self/net/unix"), 1024 * 1024).unwrap();
    let matching: Vec<_> = std::str::from_utf8(&table)
        .unwrap()
        .lines()
        .map(|line| line.split_whitespace().collect::<Vec<_>>())
        .filter(|fields| fields.get(7).copied() == Some(path))
        .collect();
    assert!(
        matching
            .iter()
            .any(|fields| fields[3..6] == ["00010000", "0001", "01"])
    );
    assert!(
        matching
            .iter()
            .any(|fields| fields[3..6] == ["00000000", "0001", "03"])
    );
    assert_eq!(listening_inode(&table, path).unwrap(), listener_inode);

    drop(listener);
    let table = bounded(Path::new("/proc/self/net/unix"), 1024 * 1024).unwrap();
    assert!(listening_inode(&table, path).is_err());
}
