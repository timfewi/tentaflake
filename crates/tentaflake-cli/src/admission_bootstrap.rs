//! Explicit initial-maintenance barrier, never an authority or inventory.
//!
//! The operator stops all old creators and Docker/socket, records this barrier,
//! then starts Docker and bootstraps without activating legacy configuration in
//! between. Other root operators must not mutate this maintenance window. We do
//! not stop services, infer absence from daemon failure, or reset a ledger.
//! Discovery covers conventional docker-/podman- service names and both plans'
//! declared names. Operators must separately drain historical custom launchers.
use crate::admission_host::{HostPlan, load_plan, read_store_document};
use crate::admission_oci;
use crate::admission_runtime::{self, LEDGER, authority, installed, require_root, stopped};
use crate::admission_storage::open_root;
use crate::admission_systemd::{self, UnitObservation};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeSet;
use std::ffi::CStr;
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::fs::{FileTypeExt, MetadataExt};
use std::path::{Path, PathBuf};
use tentaflake_admission::host_fence as admission_fence;
use tentaflake_admission::ledger::PublishedAuthority;

const DIRECTORY: &CStr = c"tentaflake-admission-bootstrap";
const RECEIPT: &CStr = c"receipt.json";
const STAGING: &CStr = c"receipt.new";
const MAX_RECEIPT: u64 = 256 * 1024;

#[derive(Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct Barrier {
    schema_version: u32,
    authority: PublishedAuthority,
    boot_id: String,
    stopped_ticks: u64,
    daemon_config: PathBuf,
    units: Vec<String>,
}

/// Both generations and installed/loaded legacy launchers are accounted for.
/// The outer fence blocks current launchers; masking/draining old ones is an
/// explicit operator action, not something this observation performs.
pub(crate) fn runtime_units(
    plan: &HostPlan,
    path: &Path,
    other: Option<&HostPlan>,
) -> Result<Vec<String>, String> {
    let available: BTreeSet<_> = admission_systemd::list_runtime_units(&plan.tools.busctl)?
        .into_iter()
        .collect();
    let mut names = available.clone();
    names.extend(
        plan.runtimes
            .values()
            .chain(other.into_iter().flat_map(|p| p.runtimes.values()))
            .map(|runtime| runtime.unit.clone()),
    );
    if names.len() > 512 {
        return Err("runtime unit inventory limit exceeded".into());
    }
    for name in &names {
        // Removing an owned declaration normally removes its unit file. Exact
        // absence from both manager and file inventories needs no synthetic
        // mask. Never turn a failed query into absence or skip a desired unit.
        if !needs_runtime_observation(name, &available, plan) {
            continue;
        }
        let observed = admission_systemd::load_unit(&plan.tools.busctl, name)?;
        if !offline_runtime(&observed, plan, path) {
            return Err("runtime supervisors must be stopped with the exact owned plan; legacy or removed launchers must be masked and drained".into());
        }
    }
    Ok(names.into_iter().collect())
}

fn needs_runtime_observation(name: &str, available: &BTreeSet<String>, plan: &HostPlan) -> bool {
    available.contains(name) || plan.runtimes.values().any(|runtime| runtime.unit == name)
}

fn offline_runtime(unit: &UnitObservation, plan: &HostPlan, path: &Path) -> bool {
    stopped(unit)
        && match plan.runtimes.iter().find(|(_, spec)| spec.unit == unit.id) {
            Some((id, spec)) => admission_oci::valid_unit(unit, plan, path, id, spec),
            None => unit.load_state == "masked",
        }
}

fn unit_inventory(plan: &HostPlan, path: &Path) -> Result<Vec<String>, String> {
    // Only first bootstrap reaches this helper after proving no ledger exists.
    admission_runtime::offline_units(
        plan,
        path,
        None,
        &crate::admission_creation::CreationInventory::initial(),
    )
}

fn boot_id() -> Result<String, String> {
    let value = fs::read_to_string("/proc/sys/kernel/random/boot_id")
        .map_err(|_| "boot identity unavailable")?
        .trim()
        .to_owned();
    if value.len() != 36
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() || byte == b'-')
    {
        return Err("invalid boot identity".into());
    }
    Ok(value)
}

fn boot_ticks() -> Result<u64, String> {
    let mut time = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    let ticks = unsafe { libc::sysconf(libc::_SC_CLK_TCK) };
    if ticks <= 0
        || unsafe { libc::clock_gettime(libc::CLOCK_BOOTTIME, &mut time) } != 0
        || time.tv_sec < 0
        || !(0..1_000_000_000).contains(&time.tv_nsec)
    {
        return Err("monotonic bootstrap clock unavailable".into());
    }
    u64::try_from(
        (time.tv_sec as u128 * 1_000_000_000 + time.tv_nsec as u128) * ticks as u128
            / 1_000_000_000,
    )
    .map_err(|_| "bootstrap clock overflow".into())
}

fn supported_settings(settings: &Value) -> bool {
    let Some(settings) = settings.as_object() else {
        return false;
    };
    settings.get("live-restore") == Some(&Value::Bool(false))
        && settings
            .get("features")
            .and_then(Value::as_object)
            .and_then(|features| features.get("containerd-snapshotter"))
            == Some(&Value::Bool(false))
        && [
            "containerd",
            "containerd-namespace",
            "containerd-plugins-namespace",
            "exec-root",
            "data-root",
        ]
        .iter()
        .all(|key| !settings.contains_key(*key))
}

/// Support the current NixOS Docker module's internal-containerd contract only.
/// No extra flags may select another daemon state tree or external containerd.
pub(crate) fn daemon_config(plan: &HostPlan, unit: &UnitObservation) -> Result<PathBuf, String> {
    let service = unit
        .service
        .as_ref()
        .ok_or("Docker service observation unavailable")?;
    let [command] = service.exec_start.as_slice() else {
        return Err("unsupported Docker service command".into());
    };
    let expected = plan.tools.docker.with_file_name("dockerd");
    if command.path != expected.to_string_lossy()
        || command.ignore_failure
        || command.argv.len() != 2
        || command.argv[0] != command.path
    {
        return Err(
            "bootstrap requires the declared NixOS dockerd and no extra daemon flags".into(),
        );
    }
    let path = command.argv[1]
        .strip_prefix("--config-file=")
        .map(PathBuf::from)
        .ok_or("Docker needs one immutable config-file argument")?;
    let settings: Value = serde_json::from_slice(&read_store_document(&path, 1024 * 1024)?)
        .map_err(|_| "invalid immutable Docker configuration")?;
    if !supported_settings(&settings) {
        return Err("admission requires live-restore=false, features.containerd-snapshotter=false, internal containerd and default Docker state paths/namespaces".into());
    }
    Ok(path)
}

fn backend_process(name: &str) -> bool {
    let name = name
        .strip_suffix(" (deleted)")
        .unwrap_or(name)
        .trim_start_matches('.');
    [
        "docker",
        "dockerd",
        "podman",
        "containerd",
        "runc",
        "crun",
        "runsc",
    ]
    .iter()
    .any(|prefix| {
        name == *prefix
            || name
                .strip_prefix(prefix)
                .is_some_and(|rest| rest.starts_with('-'))
    })
}

fn no_backend_processes() -> Result<(), String> {
    // Executable names only: never read process argv, environment or secrets.
    // Reject unrelated containerd/runtime users conservatively rather than
    // claiming that stopping dockerd reaps shims in other service cgroups.
    for (index, entry) in fs::read_dir("/proc")
        .map_err(|_| "process inventory unavailable")?
        .enumerate()
    {
        if index >= 65536 {
            return Err("process inventory limit exceeded".into());
        }
        let entry = entry.map_err(|_| "process inventory changed")?;
        if !entry
            .file_name()
            .as_encoded_bytes()
            .iter()
            .all(u8::is_ascii_digit)
        {
            continue;
        }
        match fs::read_link(entry.path().join("exe")) {
            Ok(exe) => {
                let name = exe
                    .file_name()
                    .and_then(|name| name.to_str())
                    .ok_or("unclassifiable process executable")?;
                if backend_process(name) {
                    return Err("bootstrap-fence requires all Docker/Podman clients, daemons, containerd, shims and OCI runtime processes stopped; unrelated runtimes must also be quiescent".into());
                }
            }
            // Kernel threads, zombies and processes which exited have no exe.
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(_) => return Err("cannot inspect complete process executable inventory".into()),
        }
    }
    Ok(())
}

pub(crate) fn stopped_backend(plan: &HostPlan) -> Result<PathBuf, String> {
    let daemon = admission_systemd::load_unit(&plan.tools.busctl, "docker.service")?;
    let socket = admission_systemd::load_unit(&plan.tools.busctl, "docker.socket")?;
    if !stopped(&daemon) || !stopped(&socket) {
        return Err("stop docker.service and docker.socket before maintenance fencing".into());
    }
    let config = daemon_config(plan, &daemon)?;
    stopped_socket_node(Path::new("/run/docker.sock"), 0)?;
    no_backend_processes()?;
    if admission_systemd::load_unit(&plan.tools.busctl, "docker.service")? != daemon
        || admission_systemd::load_unit(&plan.tools.busctl, "docker.socket")? != socket
    {
        return Err("Docker changed during stopped observation".into());
    }
    Ok(config)
}

fn stopped_socket_node(path: &Path, owner_uid: u32) -> Result<(), String> {
    // systemd.socket defaults RemoveOnStop=false. A leftover filesystem node
    // is not a listener: stopped unit/backend observations remain mandatory.
    // The production caller fixes both the host path and its root owner.
    match fs::symlink_metadata(path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Ok(metadata)
            if metadata.file_type().is_socket()
                && metadata.uid() == owner_uid
                && metadata.mode() & 0o002 == 0
                && metadata.nlink() == 1 =>
        {
            Ok(())
        }
        _ => Err("unsafe leftover Docker socket node during bootstrap-fence".into()),
    }
}

fn open_at(parent: &File, name: &CStr, flags: i32) -> io::Result<File> {
    let fd = unsafe {
        libc::openat(
            parent.as_raw_fd(),
            name.as_ptr(),
            flags | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC,
            0o600,
        )
    };
    if fd < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(unsafe { File::from_raw_fd(fd) })
    }
}

fn receipt_directory(create: bool) -> Result<File, String> {
    let run = open_root(Path::new("/run"), 0)?;
    if create
        && unsafe { libc::mkdirat(run.as_raw_fd(), DIRECTORY.as_ptr(), 0o700) } != 0
        && io::Error::last_os_error().kind() != io::ErrorKind::AlreadyExists
    {
        return Err("cannot create private bootstrap receipt directory".into());
    }
    let directory = open_at(&run, DIRECTORY, libc::O_RDONLY | libc::O_DIRECTORY).map_err(
        |_| "bootstrap-fence receipt unavailable; perform explicit stop/fence/start procedure",
    )?;
    let metadata = directory
        .metadata()
        .map_err(|_| "cannot inspect bootstrap directory")?;
    if metadata.uid() != 0 || metadata.mode() & 0o7777 != 0o700 {
        return Err("unsafe bootstrap receipt directory".into());
    }
    if create {
        run.sync_all()
            .map_err(|_| "cannot sync bootstrap directory")?;
    }
    Ok(directory)
}

fn read_receipt(directory: &File) -> Result<Barrier, String> {
    let before = open_at(directory, RECEIPT, libc::O_PATH)
        .map_err(|_| "bootstrap-fence receipt unavailable")?;
    let metadata = before
        .metadata()
        .map_err(|_| "cannot inspect bootstrap receipt")?;
    if !metadata.is_file()
        || metadata.uid() != unsafe { libc::geteuid() }
        || metadata.mode() & 0o7777 != 0o600
        || metadata.nlink() != 1
        || metadata.len() > MAX_RECEIPT
    {
        return Err("unsafe bootstrap receipt".into());
    }
    let file =
        open_at(directory, RECEIPT, libc::O_RDONLY).map_err(|_| "cannot open bootstrap receipt")?;
    let after = file
        .metadata()
        .map_err(|_| "cannot inspect opened bootstrap receipt")?;
    if (metadata.dev(), metadata.ino()) != (after.dev(), after.ino()) {
        return Err("bootstrap receipt changed while opening".into());
    }
    let mut bytes = Vec::new();
    file.take(MAX_RECEIPT + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "cannot read bootstrap receipt")?;
    if bytes.len() as u64 > MAX_RECEIPT {
        return Err("oversized bootstrap receipt".into());
    }
    serde_json::from_slice(&bytes).map_err(|_| "invalid bootstrap receipt; review volatile proof or reboot before repeating maintenance; never remove the ledger".into())
}

fn write_receipt(directory: &File, receipt: &Barrier) -> Result<(), String> {
    match open_at(directory, RECEIPT, libc::O_PATH) {
        Ok(_) => {
            read_receipt(directory)?;
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(_) => return Err("cannot inspect existing bootstrap receipt".into()),
    }
    let bytes = serde_json::to_vec(receipt).map_err(|_| "cannot encode bootstrap receipt")?;
    if bytes.len() as u64 > MAX_RECEIPT {
        return Err("oversized bootstrap receipt".into());
    }
    let mut file = open_at(directory, STAGING, libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL)
        .map_err(|_| "cannot create bootstrap receipt; review volatile staging proof or reboot before repeating maintenance; never remove the ledger")?;
    // Do not repair existing inodes. Only our newly created private inode gets
    // its exact mode, even when the caller supplied a more restrictive umask.
    if unsafe { libc::fchmod(file.as_raw_fd(), 0o600) } != 0 {
        return Err("cannot protect bootstrap receipt".into());
    }
    file.write_all(&bytes)
        .and_then(|()| file.sync_all())
        .map_err(|_| "cannot sync bootstrap receipt")?;
    if unsafe {
        libc::renameat(
            directory.as_raw_fd(),
            STAGING.as_ptr(),
            directory.as_raw_fd(),
            RECEIPT.as_ptr(),
        )
    } != 0
    {
        return Err("cannot publish bootstrap receipt".into());
    }
    directory
        .sync_all()
        .map_err(|_| "cannot sync bootstrap receipt directory".into())
}

pub(crate) fn fence(path: &Path) -> Result<u8, String> {
    require_root()?;
    let plan = load_plan(path)?;
    let _fence = admission_fence::exclusive()?;
    installed(path)?;
    match fs::symlink_metadata(LEDGER) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        _ => return Err(
            "bootstrap-fence requires a genuinely absent ledger; never remove an existing ledger"
                .into(),
        ),
    }
    let units = unit_inventory(&plan, path)?;
    admission_runtime::offline_loops(&plan)?;
    let config = stopped_backend(&plan)?;
    let receipt = Barrier {
        schema_version: 1,
        authority: authority(&plan, path),
        boot_id: boot_id()?,
        stopped_ticks: boot_ticks()?,
        daemon_config: config,
        units,
    };
    installed(path)?;
    if unit_inventory(&plan, path)? != receipt.units
        || stopped_backend(&plan)? != receipt.daemon_config
    {
        return Err("bootstrap maintenance observations changed".into());
    }
    admission_runtime::offline_loops(&plan)?;
    write_receipt(&receipt_directory(true)?, &receipt)?;
    Ok(0)
}

fn matches_barrier(
    receipt: &Barrier,
    published: &PublishedAuthority,
    boot: &str,
    start_ticks: u64,
    config: &Path,
    units: &[String],
) -> bool {
    receipt.schema_version == 1
        && &receipt.authority == published
        && receipt.boot_id == boot
        && receipt.stopped_ticks > 0
        && start_ticks > receipt.stopped_ticks
        && receipt.daemon_config == config
        && receipt.units == units
}

/// Caller holds the outer fence. Require a later real daemon generation; this
/// receipt neither grants a start nor substitutes for complete storage scans.
pub(crate) fn verify(plan: &HostPlan, path: &Path) -> Result<(), String> {
    let receipt = read_receipt(&receipt_directory(false)?)?;
    let units = unit_inventory(plan, path)?;
    let daemon = admission_systemd::inspect_unit(&plan.tools.busctl, "docker.service")?;
    let config = daemon_config(plan, &daemon)?;
    let (boot, identity) = admission_oci::daemon(plan)?;
    if !matches_barrier(
        &receipt,
        &authority(plan, path),
        &boot,
        identity.start_time_ticks,
        &config,
        &units,
    ) {
        return Err("missing or stale stopped-Docker bootstrap barrier; repeat the explicit stop/fence/start/bootstrap procedure".into());
    }
    // nixpkgs' Docker package links bin/dockerd to the engine wrapper, which
    // execs that same output's libexec/docker/dockerd. Authenticate the running
    // process, not merely the currently loaded unit's declared command.
    let wrapper = fs::canonicalize(plan.tools.docker.with_file_name("dockerd"))
        .map_err(|_| "cannot inspect declared dockerd wrapper")?;
    let binary = wrapper
        .parent()
        .and_then(Path::parent)
        .ok_or("invalid declared dockerd wrapper")?
        .join("libexec/docker/dockerd");
    let service = daemon
        .service
        .as_ref()
        .ok_or("Docker service unavailable")?;
    if fs::canonicalize(format!("/proc/{}/exe", identity.pid))
        .map_err(|_| "cannot inspect running dockerd")?
        != fs::canonicalize(binary).map_err(|_| "cannot resolve declared dockerd binary")?
        || service.control_group.is_empty()
        || fs::read_to_string(format!("/proc/{}/cgroup", identity.pid))
            .map_err(|_| "cannot inspect dockerd cgroup")?
            .trim()
            != format!("0::{}", service.control_group)
    {
        return Err("running daemon does not match the supported NixOS Docker service".into());
    }
    let info: Value = serde_json::from_slice(&admission_runtime::observe(
        &plan.tools.docker,
        &[
            "--host=unix:///run/docker.sock",
            "--config=/var/empty",
            "info",
            "--format",
            "{{json .LiveRestoreEnabled}}",
        ],
    )?)
    .map_err(|_| "Docker live-restore observation unavailable")?;
    if info != Value::Bool(false) {
        return Err("live-restore must remain disabled for bootstrap".into());
    }
    if admission_systemd::inspect_unit(&plan.tools.busctl, "docker.service")? != daemon
        || admission_oci::daemon(plan)? != (boot, identity)
    {
        return Err("Docker changed during bootstrap verification".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::{PermissionsExt, symlink};
    use std::os::unix::net::UnixListener;
    use std::sync::atomic::{AtomicU64, Ordering};

    fn barrier() -> Barrier {
        Barrier {
            schema_version: 1,
            authority: PublishedAuthority {
                generation: "host-epoch".into(),
                revision: "manifest-revision".into(),
                manifest_path: "/nix/store/00000000000000000000000000000000-plan".into(),
            },
            boot_id: "11111111-1111-1111-1111-111111111111".into(),
            stopped_ticks: 30,
            daemon_config: "/nix/store/00000000000000000000000000000000-daemon.json".into(),
            units: vec!["docker-agent.service".into()],
        }
    }

    #[test]
    fn barrier_requires_same_boot_authority_inventory_and_strictly_later_daemon() {
        let receipt = barrier();
        let matches =
            |authority: &PublishedAuthority, boot: &str, ticks, config: &Path, units: &[String]| {
                matches_barrier(&receipt, authority, boot, ticks, config, units)
            };
        assert!(matches(
            &receipt.authority,
            &receipt.boot_id,
            31,
            &receipt.daemon_config,
            &receipt.units
        ));
        assert!(!matches(
            &receipt.authority,
            &receipt.boot_id,
            30,
            &receipt.daemon_config,
            &receipt.units
        ));
        assert!(!matches(
            &receipt.authority,
            &receipt.boot_id,
            29,
            &receipt.daemon_config,
            &receipt.units
        ));
        assert!(!matches(
            &receipt.authority,
            "another-boot",
            31,
            &receipt.daemon_config,
            &receipt.units
        ));
        for field in 0..3 {
            let mut authority = receipt.authority.clone();
            match field {
                0 => authority.generation.push('x'),
                1 => authority.revision.push('x'),
                _ => authority.manifest_path.push('x'),
            }
            assert!(!matches(
                &authority,
                &receipt.boot_id,
                31,
                &receipt.daemon_config,
                &receipt.units
            ));
        }
        assert!(!matches(
            &receipt.authority,
            &receipt.boot_id,
            31,
            Path::new("/different-config"),
            &receipt.units
        ));
        assert!(!matches(
            &receipt.authority,
            &receipt.boot_id,
            31,
            &receipt.daemon_config,
            &["docker-old.service".into()]
        ));
    }

    #[test]
    fn removed_runtime_is_absent_only_when_both_inventories_omit_it() {
        let plan: HostPlan =
            serde_json::from_value(crate::admission_host::tests::plan_fixture()).unwrap();
        let mut available = BTreeSet::new();
        let removed = "docker-removed.service";
        assert!(!needs_runtime_observation(removed, &available, &plan));
        available.insert(removed.to_owned());
        assert!(needs_runtime_observation(removed, &available, &plan));
    }

    #[test]
    fn backend_contract_rejects_live_restore_external_state_and_surviving_clients_or_shims() {
        let settings = serde_json::json!({"live-restore": false, "features": {"containerd-snapshotter": false}, "hosts": ["fd://"], "runtimes": {"runsc": {"path": "/nix/store/runsc"}}});
        assert!(supported_settings(&settings));
        for value in [
            serde_json::json!({}),
            serde_json::json!({"live-restore": true}),
            Value::Null,
        ] {
            assert!(!supported_settings(&value));
        }
        for value in [
            Value::Null,
            serde_json::json!({}),
            serde_json::json!({"containerd-snapshotter": true}),
            serde_json::json!({"containerd-snapshotter": "false"}),
        ] {
            let mut changed = settings.clone();
            changed["features"] = value;
            assert!(!supported_settings(&changed));
        }
        let mut missing = settings.clone();
        missing.as_object_mut().unwrap().remove("features");
        assert!(!supported_settings(&missing));
        for key in [
            "containerd",
            "containerd-namespace",
            "containerd-plugins-namespace",
            "exec-root",
            "data-root",
        ] {
            let mut changed = settings.clone();
            changed[key] = Value::String("external".into());
            assert!(!supported_settings(&changed));
        }
        for name in [
            "docker",
            "dockerd (deleted)",
            ".docker-wrapped",
            "podman",
            "containerd-shim-runc-v2",
            "containerd",
            "runsc-sandbox",
            "runc",
            "crun",
            "docker-proxy",
        ] {
            assert!(backend_process(name), "{name}");
        }
        for name in ["tentaflake", "systemd", "bash", "containerdnotes"] {
            assert!(!backend_process(name));
        }
    }

    struct Fixture {
        path: PathBuf,
        directory: File,
    }
    impl Fixture {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!(
                "tentaflake-bootstrap-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path).unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
            let directory = File::open(&path).unwrap();
            Self { path, directory }
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.path).unwrap();
        }
    }

    #[test]
    fn stopped_socket_accepts_inert_node_but_refuses_substituted_or_unprotected_paths() {
        let fixture = Fixture::new();
        let path = fixture.path.join("docker.sock");
        let owner = unsafe { libc::geteuid() };
        assert!(stopped_socket_node(&path, owner).is_ok());
        let listener = UnixListener::bind(&path).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o660)).unwrap();
        drop(listener);
        assert!(path.exists());
        assert!(stopped_socket_node(&path, owner).is_ok());
        assert!(stopped_socket_node(&path, owner.wrapping_add(1)).is_err());
        fs::set_permissions(&path, fs::Permissions::from_mode(0o662)).unwrap();
        assert!(stopped_socket_node(&path, owner).is_err());
        fs::remove_file(&path).unwrap();
        fs::write(&path, b"not a socket").unwrap();
        assert!(stopped_socket_node(&path, owner).is_err());
        fs::remove_file(&path).unwrap();
        symlink("missing", &path).unwrap();
        assert!(stopped_socket_node(&path, owner).is_err());
    }

    #[test]
    fn receipt_persists_exact_binding_and_interrupted_replacement_preserves_old_record() {
        let fixture = Fixture::new();
        let receipt = barrier();
        write_receipt(&fixture.directory, &receipt).unwrap();
        assert_eq!(read_receipt(&fixture.directory).unwrap(), receipt);
        let before = fs::read(fixture.path.join("receipt.json")).unwrap();
        let mut changed = receipt;
        changed.stopped_ticks += 1;
        fs::write(fixture.path.join("receipt.new"), b"interrupted").unwrap();
        assert!(write_receipt(&fixture.directory, &changed).is_err());
        assert_eq!(fs::read(fixture.path.join("receipt.json")).unwrap(), before);
        assert_eq!(
            fs::read(fixture.path.join("receipt.new")).unwrap(),
            b"interrupted"
        );
        fs::remove_file(fixture.path.join("receipt.new")).unwrap();
        write_receipt(&fixture.directory, &changed).unwrap();
        assert_eq!(read_receipt(&fixture.directory).unwrap(), changed);
        assert!(!fixture.path.join("receipt.new").exists());
        let mode = fs::metadata(fixture.path.join("receipt.json"))
            .unwrap()
            .mode()
            & 0o7777;
        assert_eq!(mode, 0o600);
    }

    #[test]
    fn corrupt_linked_or_permissive_receipt_never_gets_replaced_or_trusted() {
        for kind in 0..4 {
            let fixture = Fixture::new();
            write_receipt(&fixture.directory, &barrier()).unwrap();
            let path = fixture.path.join("receipt.json");
            match kind {
                0 => fs::write(&path, b"corrupt").unwrap(),
                1 => fs::hard_link(&path, fixture.path.join("other")).unwrap(),
                2 => fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap(),
                _ => {
                    fs::rename(&path, fixture.path.join("other")).unwrap();
                    symlink("other", &path).unwrap();
                }
            }
            assert!(read_receipt(&fixture.directory).is_err());
            assert!(write_receipt(&fixture.directory, &barrier()).is_err());
            assert!(!fixture.path.join("receipt.new").exists());
        }
    }
}
