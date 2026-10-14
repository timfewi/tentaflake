//! Bounded, selected systemd observations and explicit metadata loading. These
//! calls never start/stop units or establish a fence; callers own synchronization.

use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

pub(crate) fn process_start(pid: u32) -> Result<u64, String> {
    let metadata =
        fs::metadata(format!("/proc/{pid}")).map_err(|_| "runtime process disappeared")?;
    if pid == 0 || metadata.uid() != 0 {
        return Err("runtime process is not root-owned".into());
    }
    let stat = fs::read_to_string(format!("/proc/{pid}/stat"))
        .map_err(|_| "runtime process identity unavailable")?;
    if stat.len() > 4096 {
        return Err("runtime process identity exceeds its bound".into());
    }
    // comm may itself contain ')' and spaces; fields after its last ')' are fixed.
    stat.rsplit_once(')')
        .and_then(|(_, tail)| tail.split_whitespace().nth(19))
        .and_then(|s| s.parse::<u64>().ok())
        .filter(|n| *n > 0)
        .ok_or_else(|| "invalid runtime process identity".into())
}

const MAX_RESPONSE_BYTES: usize = 64 * 1024;
const MAX_LIST_BYTES: usize = 512 * 1024;
const OBSERVATION_TIMEOUT: Duration = Duration::from_secs(6);
const CALL_TIMEOUT: Duration = Duration::from_secs(2);
const DESTINATION: &str = "org.freedesktop.systemd1";
const UNIT_INTERFACE: &str = "org.freedesktop.systemd1.Unit";
const UNIT_PROPERTIES: [&str; 8] = [
    "Id",
    "LoadState",
    "ActiveState",
    "SubState",
    "Job",
    "FragmentPath",
    "NeedDaemonReload",
    "InvocationID",
];
const SERVICE_PROPERTIES: [&str; 17] = [
    "MainPID",
    "ControlGroup",
    "ExecStart",
    "ControlPID",
    "ExecStop",
    "Type",
    "RemainAfterExit",
    "KillMode",
    "TimeoutStartUSec",
    "TimeoutStopUSec",
    "Restart",
    "NotifyAccess",
    "ExecStartPre",
    "ExecStartPost",
    "ExecStopPost",
    "ExecReload",
    "ExecCondition",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct UnitObservation {
    pub id: String,
    pub load_state: String,
    pub active_state: String,
    pub sub_state: String,
    pub job: UnitJob,
    pub fragment_path: String,
    pub need_daemon_reload: bool,
    pub invocation_id: [u8; 16],
    pub service: Option<ServiceObservation>,
    pub mount: Option<MountObservation>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MountObservation {
    pub what: String,
    pub where_path: String,
    pub filesystem_type: String,
}

impl UnitObservation {
    /// Bounded diagnostic context. What may contain credentials, so never echo
    /// it or mount options; unit identity and filesystem type have closed shapes.
    pub(crate) fn mount_context(&self) -> String {
        let id = if mount_name(&self.id) {
            &self.id
        } else {
            "invalid"
        };
        let (kind, source) = self
            .mount
            .as_ref()
            .map_or(("unobserved", "unobserved"), |mount| {
                let kind = if !valid_filesystem_type(&mount.filesystem_type) {
                    "invalid"
                } else if mount.filesystem_type.is_empty() {
                    "empty"
                } else {
                    &mount.filesystem_type
                };
                let source = if mount.what.is_empty() {
                    "empty"
                } else if mount.what == "none" {
                    "none"
                } else if Path::new(&mount.what).is_absolute() {
                    "absolute"
                } else {
                    "other"
                };
                (kind, source)
            });
        format!("unit={id}, type={kind}, source={source}")
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct UnitJob {
    pub id: u32,
    pub path: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ServiceObservation {
    pub main_pid: u32,
    pub control_pid: u32,
    pub control_group: String,
    pub exec_start: Vec<ExecCommand>,
    pub exec_stop: Vec<ExecCommand>,
    pub service_type: String,
    pub remain_after_exit: bool,
    pub kill_mode: String,
    pub timeout_start_usec: u64,
    pub timeout_stop_usec: u64,
    pub restart: String,
    pub notify_access: String,
    pub exec_start_pre: Vec<ExecCommand>,
    pub exec_start_post: Vec<ExecCommand>,
    pub exec_stop_post: Vec<ExecCommand>,
    pub exec_reload: Vec<ExecCommand>,
    pub exec_condition: Vec<ExecCommand>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ExecCommand {
    pub path: String,
    pub argv: Vec<String>,
    pub ignore_failure: bool,
    pub start_realtime: u64,
    pub start_monotonic: u64,
    pub exit_realtime: u64,
    pub exit_monotonic: u64,
    pub pid: u32,
    pub code: i32,
    pub status: i32,
}

/// Observe an exact loaded unit. Aliases, unavailable units and bus failures are
/// errors, never evidence of absence. Unit properties bracket type-specific reads to
/// reject observed transitions, but D-Bus property reads are not atomic.
pub(crate) fn inspect_unit(busctl: &Path, name: &str) -> Result<UnitObservation, String> {
    observe_unit(busctl, name, "GetUnit", MetadataScope::Required)
}

/// Explicitly load a named unit's metadata, without starting it. A missing unit
/// is still an error; a loaded mask remains visible for legacy-creator fencing.
pub(crate) fn load_unit(busctl: &Path, name: &str) -> Result<UnitObservation, String> {
    observe_unit(busctl, name, "LoadUnit", MetadataScope::Required)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum MetadataScope {
    Required,
    IncidentalMount,
    IncidentalCreation,
}

fn valid_busctl(busctl: &Path) -> bool {
    crate::admission_host::store_path(busctl)
        && busctl.file_name() == Some("busctl".as_ref())
        && busctl.parent().and_then(Path::file_name) == Some("bin".as_ref())
}

fn observe_unit(
    busctl: &Path,
    name: &str,
    method: &str,
    scope: MetadataScope,
) -> Result<UnitObservation, String> {
    if !valid_busctl(busctl) || !unit_name(name) {
        return Err("invalid systemd observation tool or unit".into());
    }
    let deadline = Instant::now() + OBSERVATION_TIMEOUT;
    let object = parse_object(&read_bus(
        busctl,
        &[
            "call",
            DESTINATION,
            "/org/freedesktop/systemd1",
            "org.freedesktop.systemd1.Manager",
            method,
            "s",
            name,
        ],
        deadline,
        MAX_RESPONSE_BYTES,
    )?)?;
    let mut arguments = vec!["get-property", DESTINATION, &object, UNIT_INTERFACE];
    arguments.extend(UNIT_PROPERTIES);
    let mut observed = parse_metadata(
        &read_bus(busctl, &arguments, deadline, MAX_RESPONSE_BYTES)?,
        name,
        scope,
    )?;
    let service = if name.ends_with(".service") {
        let mut service_arguments = vec![
            "get-property",
            DESTINATION,
            &object,
            "org.freedesktop.systemd1.Service",
        ];
        service_arguments.extend(SERVICE_PROPERTIES);
        Some(parse_service(&read_bus(
            busctl,
            &service_arguments,
            deadline,
            MAX_RESPONSE_BYTES,
        )?)?)
    } else {
        None
    };
    let mount = if name.ends_with(".mount") {
        Some(parse_mount(&read_bus(
            busctl,
            &[
                "get-property",
                DESTINATION,
                &object,
                "org.freedesktop.systemd1.Mount",
                "What",
                "Where",
                "Type",
            ],
            deadline,
            MAX_RESPONSE_BYTES,
        )?)?)
    } else {
        None
    };
    let after = parse_metadata(
        &read_bus(busctl, &arguments, deadline, MAX_RESPONSE_BYTES)?,
        name,
        scope,
    )?;
    if observed != after {
        return Err("systemd unit changed during observation".into());
    }
    observed.service = service;
    observed.mount = mount;
    Ok(observed)
}

/// List loaded and installed quota service names, including obsolete creators.
/// Enumeration is not a fence and never establishes that no units can appear.
pub(crate) fn list_quota_units(busctl: &Path) -> Result<Vec<String>, String> {
    list_units(
        busctl,
        &[
            "tentaflake-workspace-quota-*.service",
            "tentaflake-state-quota-*.service",
        ],
        quota_name,
    )
    .map(|inventory| inventory.names.into_iter().collect())
}

/// Observe native mounts, including installed but currently unloaded files.
/// Dependency-only mount objects (observed as home.mount in the NixOS VM) may
/// explicitly report not-found. Only undeclared, inactive objects absent from
/// successful file inventories before and after observation are inert here.
/// Required units and all query failures remain errors. Callers hold the host
/// maintenance fence; this read-only sweep does not fence privileged writers.
pub(crate) fn observe_mount_units(
    busctl: &Path,
    required: &BTreeSet<String>,
) -> Result<Vec<UnitObservation>, String> {
    observe_mount_inventory(
        required,
        || list_units(busctl, &["*.mount"], mount_name),
        |name, scope| observe_unit(busctl, name, "LoadUnit", scope),
    )
}

fn observe_mount_inventory(
    required: &BTreeSet<String>,
    mut list: impl FnMut() -> Result<UnitInventory, String>,
    mut observe: impl FnMut(&str, MetadataScope) -> Result<UnitObservation, String>,
) -> Result<Vec<UnitObservation>, String> {
    let inventory = list()?;
    let mut observed = Vec::new();
    let mut inert = BTreeSet::new();
    for name in inventory.names {
        let scope = if required.contains(&name) || inventory.installed.contains_key(&name) {
            MetadataScope::Required
        } else {
            MetadataScope::IncidentalMount
        };
        let unit = observe(&name, scope)?;
        if unit.load_state == "not-found" {
            validate_inert_mount(&unit)?;
            inert.insert(name);
        } else {
            observed.push(unit);
        }
    }
    if !inert.is_empty() {
        let after = list()?;
        ensure_uninstalled(&inert, &after.installed)?;
    }
    Ok(observed)
}

/// Include old OCI launchers even when absent from the currently desired plan.
pub(crate) fn list_runtime_units(busctl: &Path) -> Result<Vec<String>, String> {
    list_units(
        busctl,
        &["docker-*.service", "podman-*.service"],
        runtime_name,
    )
    .map(|inventory| inventory.names.into_iter().collect())
}

pub(crate) fn list_worker_units(busctl: &Path) -> Result<Vec<String>, String> {
    list_units(
        busctl,
        &["tentaflake-worker-*.service", "tentaflake-worker-*.path"],
        worker_name,
    )
    .map(|inventory| inventory.names.into_iter().collect())
}

pub(crate) const CREATION_TEMPLATE: &str = "tentaflake-environment-create@.service";

/// Bare templates are file metadata, not loadable service instances. Preserve
/// file states so callers can require removed creators to be absent or masked.
pub(crate) fn list_creation_units(busctl: &Path) -> Result<UnitInventory, String> {
    list_units(
        busctl,
        &["tentaflake-environment-create@*.service"],
        creation_name,
    )
}

pub(crate) fn creation_operation(name: &str) -> Option<&str> {
    let operation = name
        .strip_prefix("tentaflake-environment-create@")?
        .strip_suffix(".service")?;
    (operation.len() <= 96
        && operation
            .as_bytes()
            .first()
            .is_some_and(u8::is_ascii_alphanumeric)
        && operation
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b)))
    .then_some(operation)
}

fn creation_name(name: &str) -> bool {
    name == CREATION_TEMPLATE || creation_operation(name).is_some()
}

/// An incidental unavailable object is accepted only after exact inert metadata
/// checks. The caller must bracket this observation with successful file lists.
pub(crate) fn load_creation_unit(
    busctl: &Path,
    name: &str,
    required: bool,
) -> Result<UnitObservation, String> {
    if creation_operation(name).is_none() {
        return Err("invalid concrete creation unit".into());
    }
    let observed = observe_unit(
        busctl,
        name,
        "LoadUnit",
        if required {
            MetadataScope::Required
        } else {
            MetadataScope::IncidentalCreation
        },
    )?;
    if observed.load_state == "not-found" {
        validate_inert_creation(&observed)?;
    }
    Ok(observed)
}

fn validate_inert_creation(unit: &UnitObservation) -> Result<(), String> {
    let service = unit
        .service
        .as_ref()
        .ok_or("missing creation service metadata")?;
    if creation_operation(&unit.id).is_none()
        || !inert_metadata(unit)
        || unit.mount.is_some()
        || service.main_pid != 0
        || service.control_pid != 0
        || !service.control_group.is_empty()
        || !service.exec_start.is_empty()
        || !service.exec_stop.is_empty()
        || !service.exec_start_pre.is_empty()
        || !service.exec_start_post.is_empty()
        || !service.exec_stop_post.is_empty()
        || !service.exec_reload.is_empty()
        || !service.exec_condition.is_empty()
    {
        return Err("unavailable creation unit has live or configured metadata".into());
    }
    Ok(())
}

/// Path triggers are an independent launcher: a stopped service alone is not
/// quiescence. Authenticate the exact target and inbox around unit metadata.
pub(crate) fn worker_path_matches(
    busctl: &Path,
    before: &UnitObservation,
    service: &str,
    inbox: &Path,
) -> Result<bool, String> {
    if !valid_busctl(busctl) || !worker_name(&before.id) || !before.id.ends_with(".path") {
        return Err("invalid Worker path observation".into());
    }
    let deadline = Instant::now() + OBSERVATION_TIMEOUT;
    let object = parse_object(&read_bus(
        busctl,
        &[
            "call",
            DESTINATION,
            "/org/freedesktop/systemd1",
            "org.freedesktop.systemd1.Manager",
            "GetUnit",
            "s",
            &before.id,
        ],
        deadline,
        MAX_RESPONSE_BYTES,
    )?)?;
    let bytes = read_bus(
        busctl,
        &[
            "get-property",
            DESTINATION,
            &object,
            "org.freedesktop.systemd1.Path",
            "Unit",
            "Paths",
            "MakeDirectory",
        ],
        deadline,
        MAX_RESPONSE_BYTES,
    )?;
    let matches = worker_path_properties(&bytes, service, inbox)?;
    if load_unit(busctl, &before.id)? != *before {
        return Err("Worker path changed during observation".into());
    }
    Ok(matches)
}

fn worker_path_properties(bytes: &[u8], service: &str, inbox: &Path) -> Result<bool, String> {
    let mut rows = records(bytes, 3, MAX_RESPONSE_BYTES)?;
    let target: String = property(&mut rows, "s")?;
    let paths: Vec<(String, String)> = property(&mut rows, "a(ss)")?;
    let make_directory: bool = property(&mut rows, "b")?;
    Ok(target == service
        && !make_directory
        && paths.len() == 1
        && paths[0].0 == "DirectoryNotEmpty"
        && Path::new(&paths[0].1) == inbox)
}

pub(crate) struct UnitInventory {
    pub names: BTreeSet<String>,
    pub installed: BTreeMap<String, String>,
}

fn list_units(
    busctl: &Path,
    patterns: &[&str],
    valid_name: fn(&str) -> bool,
) -> Result<UnitInventory, String> {
    if !valid_busctl(busctl) {
        return Err("invalid systemd observation tool".into());
    }
    let deadline = Instant::now() + OBSERVATION_TIMEOUT;
    let pattern_count = patterns.len().to_string();
    let list = |method| {
        let mut arguments = vec![
            "call",
            DESTINATION,
            "/org/freedesktop/systemd1",
            "org.freedesktop.systemd1.Manager",
            method,
            "asas",
            "0",
            &pattern_count,
        ];
        arguments.extend_from_slice(patterns);
        read_bus(busctl, &arguments, deadline, MAX_LIST_BYTES)
    };
    let mut names: BTreeSet<_> = parse_listed_units(&list("ListUnitsByPatterns")?, valid_name)?
        .into_iter()
        .collect();
    let installed = parse_listed_files(&list("ListUnitFilesByPatterns")?, valid_name)?;
    names.extend(installed.keys().cloned());
    if names.len() > 512 {
        return Err("systemd unit listing limit exceeded".into());
    }
    Ok(UnitInventory { names, installed })
}

fn inert_metadata(unit: &UnitObservation) -> bool {
    unit.load_state == "not-found"
        && unit.active_state == "inactive"
        && unit.sub_state == "dead"
        && unit.job.id == 0
        && !unit.need_daemon_reload
        && unit.fragment_path.is_empty()
        && unit.invocation_id == [0; 16]
}

fn validate_inert_mount(unit: &UnitObservation) -> Result<(), String> {
    if !mount_name(&unit.id)
        || !inert_metadata(unit)
        || unit.service.is_some()
        || unit
            .mount
            .as_ref()
            .is_none_or(|mount| !mount.what.is_empty() || !mount.filesystem_type.is_empty())
    {
        return Err("unavailable mount has live or configured metadata".into());
    }
    Ok(())
}

fn ensure_uninstalled(
    inert: &BTreeSet<String>,
    installed: &BTreeMap<String, String>,
) -> Result<(), String> {
    if inert.iter().any(|name| installed.contains_key(name)) {
        return Err("mount unit files changed during observation".into());
    }
    Ok(())
}

fn read_bus(
    busctl: &Path,
    arguments: &[&str],
    deadline: Instant,
    max_bytes: usize,
) -> Result<Vec<u8>, String> {
    let remaining = deadline
        .checked_duration_since(Instant::now())
        .ok_or("systemd observation deadline exceeded")?;
    let mut command = Command::new(busctl);
    command.env_clear().args([
        "--system",
        "--no-pager",
        "--auto-start=no",
        "--allow-interactive-authorization=no",
        "--timeout=1",
        "--json=short",
        "--",
    ]);
    command.args(arguments);
    // The shared reader caps captured stdout+stderr and kills only this read-only
    // client on timeout. Never use this path for start/stop or other mutations.
    let output = crate::output_with_timeout(&mut command, remaining.min(CALL_TIMEOUT))
        .map_err(|_| "systemd observation command failed")?;
    if !output.status.success()
        || output.stdout.len().saturating_add(output.stderr.len()) > max_bytes
        || !output.stderr.is_empty()
    {
        return Err("systemd observation response unavailable or oversized".into());
    }
    Ok(output.stdout)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WireValue {
    #[serde(rename = "type")]
    signature: String,
    data: Value,
}

fn records(
    bytes: &[u8],
    count: usize,
    max_bytes: usize,
) -> Result<std::vec::IntoIter<WireValue>, String> {
    if bytes.len() > max_bytes {
        return Err("systemd observation response oversized".into());
    }
    let values: Vec<_> = serde_json::Deserializer::from_slice(bytes)
        .into_iter::<WireValue>()
        .take(count + 1)
        .collect::<Result<_, _>>()
        .map_err(|_| "invalid systemd observation JSON")?;
    if values.len() != count {
        return Err("systemd observation property count mismatch".into());
    }
    Ok(values.into_iter())
}

fn property<T: DeserializeOwned>(
    values: &mut std::vec::IntoIter<WireValue>,
    signature: &str,
) -> Result<T, String> {
    let value = values.next().ok_or("missing systemd property")?;
    if value.signature != signature {
        return Err("systemd property signature mismatch".into());
    }
    serde_json::from_value(value.data).map_err(|_| "invalid systemd property value".into())
}

fn parse_object(bytes: &[u8]) -> Result<String, String> {
    // Method replies wrap arguments in an array, unlike get-property replies.
    let [path]: [String; 1] = property(&mut records(bytes, 1, MAX_RESPONSE_BYTES)?, "o")?;
    if !valid_unit_object(&path) {
        return Err("invalid systemd unit object path".into());
    }
    Ok(path)
}

fn valid_unit_object(path: &str) -> bool {
    path.strip_prefix("/org/freedesktop/systemd1/unit/")
        .is_some_and(|suffix| {
            !suffix.is_empty()
                && suffix.len() <= 1024
                && suffix
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
        })
}

fn valid_states(load: &str, active: &str, sub: &str) -> bool {
    matches!(
        load,
        "stub" | "loaded" | "not-found" | "bad-setting" | "error" | "merged" | "masked"
    ) && matches!(
        active,
        "active"
            | "reloading"
            | "inactive"
            | "failed"
            | "activating"
            | "deactivating"
            | "maintenance"
            | "refreshing"
    ) && !sub.is_empty()
        && sub.len() <= 64
        && sub
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte == b'-')
}

fn parse_unit(bytes: &[u8], name: &str) -> Result<UnitObservation, String> {
    let mut values = records(bytes, UNIT_PROPERTIES.len(), MAX_RESPONSE_BYTES)?;
    let id: String = property(&mut values, "s")?;
    let load_state: String = property(&mut values, "s")?;
    let active_state: String = property(&mut values, "s")?;
    let sub_state: String = property(&mut values, "s")?;
    let (job_id, job_path): (u32, String) = property(&mut values, "(uo)")?;
    let fragment_path: String = property(&mut values, "s")?;
    let need_daemon_reload = property(&mut values, "b")?;
    let invocation_bytes: Vec<u8> = property(&mut values, "ay")?;
    // A loaded but never invoked unit (including a mask) reports an empty ay.
    // Preserve the existing zero-ID representation of absent invocation proof.
    let invocation_id = if invocation_bytes.is_empty() {
        [0; 16]
    } else {
        invocation_bytes
            .try_into()
            .map_err(|_| "invalid systemd invocation identity")?
    };
    if id != name
        || !unit_name(&id)
        || !valid_states(&load_state, &active_state, &sub_state)
        || !(fragment_path.is_empty() || absolute_path(&fragment_path))
        || (job_id == 0 && job_path != "/")
        || (job_id != 0 && job_path != format!("/org/freedesktop/systemd1/job/{job_id}"))
    {
        return Err("invalid or mismatched systemd unit properties".into());
    }
    Ok(UnitObservation {
        id,
        load_state,
        active_state,
        sub_state,
        job: UnitJob {
            id: job_id,
            path: job_path,
        },
        fragment_path,
        need_daemon_reload,
        invocation_id,
        service: None,
        mount: None,
    })
}

fn parse_metadata(
    bytes: &[u8],
    name: &str,
    scope: MetadataScope,
) -> Result<UnitObservation, String> {
    let unit = parse_unit(bytes, name)?;
    if !(matches!(unit.load_state.as_str(), "loaded" | "masked")
        || scope == MetadataScope::IncidentalMount && mount_name(name) && inert_metadata(&unit)
        || scope == MetadataScope::IncidentalCreation
            && creation_operation(name).is_some()
            && inert_metadata(&unit))
    {
        return Err(format!(
            "systemd unit metadata is unavailable: {} has LoadState={}",
            unit.id, unit.load_state
        ));
    }
    Ok(unit)
}

fn parse_mount(bytes: &[u8]) -> Result<MountObservation, String> {
    let mut values = records(bytes, 3, MAX_RESPONSE_BYTES)?;
    let what: String = property(&mut values, "s")?;
    let where_path: String = property(&mut values, "s")?;
    let filesystem_type: String = property(&mut values, "s")?;
    // What may be a block device, directory, image, network source, or a
    // filesystem name such as tmpfs. Classification belongs to the caller.
    if what.len() > 4096
        || what.bytes().any(|byte| byte.is_ascii_control())
        || !(where_path.is_empty() || absolute_path(&where_path))
        || !valid_filesystem_type(&filesystem_type)
    {
        return Err("invalid systemd mount properties".into());
    }
    Ok(MountObservation {
        what,
        where_path,
        filesystem_type,
    })
}

fn valid_filesystem_type(value: &str) -> bool {
    value.len() <= 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

fn parse_service(bytes: &[u8]) -> Result<ServiceObservation, String> {
    let mut values = records(bytes, SERVICE_PROPERTIES.len(), MAX_RESPONSE_BYTES)?;
    let main_pid = property(&mut values, "u")?;
    let control_group: String = property(&mut values, "s")?;
    let exec_start = parse_exec(property(&mut values, "a(sasbttttuii)")?)?;
    let control_pid = property(&mut values, "u")?;
    let exec_stop = parse_exec(property(&mut values, "a(sasbttttuii)")?)?;
    let service_type: String = property(&mut values, "s")?;
    let remain_after_exit = property(&mut values, "b")?;
    let kill_mode: String = property(&mut values, "s")?;
    let timeout_start_usec = property(&mut values, "t")?;
    let timeout_stop_usec = property(&mut values, "t")?;
    let restart: String = property(&mut values, "s")?;
    let notify_access: String = property(&mut values, "s")?;
    let exec_start_pre = parse_exec(property(&mut values, "a(sasbttttuii)")?)?;
    let exec_start_post = parse_exec(property(&mut values, "a(sasbttttuii)")?)?;
    let exec_stop_post = parse_exec(property(&mut values, "a(sasbttttuii)")?)?;
    let exec_reload = parse_exec(property(&mut values, "a(sasbttttuii)")?)?;
    let exec_condition = parse_exec(property(&mut values, "a(sasbttttuii)")?)?;
    if !matches!(
        restart.as_str(),
        "no" | "on-success" | "on-failure" | "on-abnormal" | "on-watchdog" | "on-abort" | "always"
    ) || !matches!(notify_access.as_str(), "none" | "main" | "exec" | "all")
    {
        return Err("invalid systemd service policy".into());
    }
    if !(control_group.is_empty() || absolute_path(&control_group))
        || !(matches!(
            service_type.as_str(),
            "simple"
                | "exec"
                | "forking"
                | "oneshot"
                | "dbus"
                | "notify"
                | "notify-reload"
                | "idle"
        ) || (service_type.is_empty()
            && main_pid == 0
            && control_pid == 0
            && control_group.is_empty()
            && exec_start.is_empty()
            && exec_stop.is_empty()))
        || !matches!(
            kill_mode.as_str(),
            "control-group" | "mixed" | "process" | "none"
        )
    {
        return Err("invalid systemd service properties".into());
    }
    Ok(ServiceObservation {
        main_pid,
        control_pid,
        control_group,
        exec_start,
        exec_stop,
        service_type,
        remain_after_exit,
        kill_mode,
        timeout_start_usec,
        timeout_stop_usec,
        restart,
        notify_access,
        exec_start_pre,
        exec_start_post,
        exec_stop_post,
        exec_reload,
        exec_condition,
    })
}

fn parse_exec(data: Value) -> Result<Vec<ExecCommand>, String> {
    type ExecRecord = (String, Vec<String>, bool, u64, u64, u64, u64, u32, i32, i32);
    let commands: Vec<ExecRecord> =
        serde_json::from_value(data).map_err(|_| "invalid systemd command record")?;
    if commands.len() > 16 {
        return Err("excessive systemd command records".into());
    }
    let mut parsed = Vec::new();
    for (
        path,
        argv,
        ignore_failure,
        start_realtime,
        start_monotonic,
        exit_realtime,
        exit_monotonic,
        pid,
        code,
        status,
    ) in commands
    {
        if !absolute_path(&path)
            || argv.is_empty()
            || argv.len() > 64
            || argv
                .iter()
                .any(|arg| arg.len() > 4096 || arg.contains('\0'))
        {
            return Err("invalid systemd command properties".into());
        }
        parsed.push(ExecCommand {
            path,
            argv,
            ignore_failure,
            start_realtime,
            start_monotonic,
            exit_realtime,
            exit_monotonic,
            pid,
            code,
            status,
        });
    }
    Ok(parsed)
}

fn parse_listed_units(bytes: &[u8], valid_name: fn(&str) -> bool) -> Result<Vec<String>, String> {
    type UnitRecord = (
        String,
        String,
        String,
        String,
        String,
        String,
        String,
        u32,
        String,
        String,
    );
    // A method's single array return is wrapped once more than a property array.
    let [units]: [Vec<UnitRecord>; 1] =
        property(&mut records(bytes, 1, MAX_LIST_BYTES)?, "a(ssssssouso)")?;
    if units.len() > 512 {
        return Err("systemd unit listing limit exceeded".into());
    }
    let mut names = BTreeSet::new();
    let mut objects = BTreeSet::new();
    for (name, description, load, active, sub, following, object, job, job_type, job_path) in units
    {
        if !valid_name(&name)
            || description.len() > 1024
            || description.contains('\0')
            || !valid_states(&load, &active, &sub)
            || !following.is_empty()
            || !valid_unit_object(&object)
            || (job == 0 && (job_path != "/" || !job_type.is_empty()))
            || (job != 0
                && (job_path != format!("/org/freedesktop/systemd1/job/{job}")
                    || job_type.is_empty()
                    || job_type.len() > 64
                    || !job_type
                        .bytes()
                        .all(|byte| byte.is_ascii_lowercase() || byte == b'-')))
            || !names.insert(name)
            || !objects.insert(object)
        {
            return Err("invalid or aliased systemd unit listing".into());
        }
    }
    Ok(names.into_iter().collect())
}

fn quota_name(name: &str) -> bool {
    unit_name(name)
        && name.ends_with(".service")
        && (name.starts_with("tentaflake-workspace-quota-")
            || name.starts_with("tentaflake-state-quota-"))
}

fn mount_name(name: &str) -> bool {
    unit_name(name) && name.ends_with(".mount")
}

fn runtime_name(name: &str) -> bool {
    unit_name(name)
        && name.ends_with(".service")
        && (name.starts_with("docker-") || name.starts_with("podman-"))
}

fn worker_name(name: &str) -> bool {
    unit_name(name)
        && name.starts_with("tentaflake-worker-")
        && (name.ends_with(".service") || name.ends_with(".path"))
}

fn parse_listed_files(
    bytes: &[u8],
    valid_name: fn(&str) -> bool,
) -> Result<BTreeMap<String, String>, String> {
    let [files]: [Vec<(String, String)>; 1] =
        property(&mut records(bytes, 1, MAX_LIST_BYTES)?, "a(ss)")?;
    if files.len() > 512 {
        return Err("systemd unit listing limit exceeded".into());
    }
    let mut names = BTreeMap::new();
    for (path, state) in files {
        let name = path
            .rsplit('/')
            .next()
            .ok_or("invalid systemd unit file path")?;
        if !absolute_path(&path)
            || !valid_name(name)
            || !matches!(
                state.as_str(),
                "enabled"
                    | "enabled-runtime"
                    | "linked"
                    | "linked-runtime"
                    | "masked"
                    | "masked-runtime"
                    | "static"
                    | "disabled"
                    | "indirect"
                    | "generated"
                    | "transient"
                    | "bad"
            )
            || names.insert(name.to_owned(), state).is_some()
        {
            return Err("invalid, duplicate or aliased systemd unit files".into());
        }
    }
    Ok(names)
}

fn absolute_path(value: &str) -> bool {
    value.starts_with('/')
        && value.len() <= 4096
        && !value.bytes().any(|byte| byte.is_ascii_control())
        && (value == "/"
            || value[1..]
                .split('/')
                .all(|part| !matches!(part, "" | "." | "..")))
}

fn unit_name(value: &str) -> bool {
    if value == "-.mount" {
        return true;
    }
    let Some((stem, suffix)) = value.rsplit_once('.') else {
        return false;
    };
    if stem.is_empty()
        || value.len() > 255
        || value.starts_with('-')
        || !matches!(
            suffix,
            "service"
                | "mount"
                | "target"
                | "socket"
                | "path"
                | "slice"
                | "timer"
                | "automount"
                | "swap"
                | "device"
                | "scope"
        )
    {
        return false;
    }
    let mut bytes = value.bytes();
    while let Some(byte) = bytes.next() {
        if byte == b'\\' {
            if bytes.next() != Some(b'x')
                || !bytes.next().is_some_and(|byte| byte.is_ascii_hexdigit())
                || !bytes.next().is_some_and(|byte| byte.is_ascii_hexdigit())
            {
                return false;
            }
        } else if !byte.is_ascii_alphanumeric() && !b":_.@-".contains(&byte) {
            return false;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn worker_path_requires_exact_scalar_properties_and_one_owned_trigger() {
        // get-property returns values directly, unlike wrapped method arguments.
        let wire = r#"{"type":"s","data":"tentaflake-worker-fixture.service"}
{"type":"a(ss)","data":[["DirectoryNotEmpty","/var/lib/fixture/.tentaflake-worker/inbox"]]}
{"type":"b","data":false}"#;
        let check = |bytes: &str| {
            worker_path_properties(
                bytes.as_bytes(),
                "tentaflake-worker-fixture.service",
                Path::new("/var/lib/fixture/.tentaflake-worker/inbox"),
            )
        };
        assert_eq!(check(wire), Ok(true));
        for changed in [
            wire.replace("fixture.service", "other.service"),
            wire.replace("DirectoryNotEmpty", "PathExists"),
            wire.replace("/var/lib/fixture", "/var/lib/other"),
            wire.replace("false", "true"),
            wire.replace("\"]]", "\"],[\"PathExists\",\"/tmp/extra\"]]"),
        ] {
            assert_eq!(check(&changed), Ok(false));
        }
        assert!(check(&wire.replace("\"data\":false", "\"data\":[false]")).is_err());
        assert!(check(&format!("{wire}\n{{\"type\":\"b\",\"data\":false}}")).is_err());
    }

    // Wire shapes observed with systemd 261.3 busctl --json=short on the local
    // dbus.service alias. Invocation/PID values are synthetic fixture values.
    const UNIT: &str = r#"{"type":"s","data":"dbus-broker.service"}
{"type":"s","data":"loaded"}
{"type":"s","data":"active"}
{"type":"s","data":"running"}
{"type":"(uo)","data":[0,"/"]}
{"type":"s","data":"/etc/systemd/system/dbus-broker.service"}
{"type":"b","data":false}
{"type":"ay","data":[1,2,3,4,5,6,7,8,9,10,11,12,13,14,15,16]}"#;
    const SERVICE: &str = r#"{"type":"u","data":42}
{"type":"s","data":"/system.slice/dbus-broker.service"}
{"type":"a(sasbttttuii)","data":[["/nix/store/00000000000000000000000000000000-dbus-broker/bin/dbus-broker-launch",["/nix/store/00000000000000000000000000000000-dbus-broker/bin/dbus-broker-launch","--scope","system","--audit"],false,0,0,0,0,0,0,0]]}
{"type":"u","data":0}
{"type":"a(sasbttttuii)","data":[]}
{"type":"s","data":"notify-reload"}
{"type":"b","data":false}
{"type":"s","data":"control-group"}
{"type":"t","data":90000000}
{"type":"t","data":90000000}
{"type":"s","data":"no"}
{"type":"s","data":"main"}
{"type":"a(sasbttttuii)","data":[]}
{"type":"a(sasbttttuii)","data":[]}
{"type":"a(sasbttttuii)","data":[]}
{"type":"a(sasbttttuii)","data":[]}
{"type":"a(sasbttttuii)","data":[]}"#;

    #[test]
    fn parses_observed_selected_properties_without_aliasing() {
        let path = parse_object(
            br#"{"type":"o","data":["/org/freedesktop/systemd1/unit/dbus_2dbroker_2eservice"]}"#,
        )
        .unwrap();
        assert!(path.ends_with("dbus_2dbroker_2eservice"));
        let unit = parse_unit(UNIT.as_bytes(), "dbus-broker.service").unwrap();
        assert_eq!(unit.id, "dbus-broker.service");
        assert_eq!(
            unit.job,
            UnitJob {
                id: 0,
                path: "/".into()
            }
        );
        assert_eq!(unit.invocation_id[15], 16);
        assert!(!unit.need_daemon_reload);
        assert!(unit.service.is_none());
        let service = parse_service(SERVICE.as_bytes()).unwrap();
        assert_eq!(service.main_pid, 42);
        assert_eq!(service.control_pid, 0);
        assert_eq!(service.service_type, "notify-reload");
        assert_eq!(service.kill_mode, "control-group");
        assert_eq!(service.timeout_stop_usec, 90_000_000);
        assert_eq!(
            service.exec_start[0].argv[1..],
            ["--scope", "system", "--audit"]
        );
        assert!(parse_unit(UNIT.as_bytes(), "dbus.service").is_err());
    }

    #[test]
    fn job_and_reload_remain_observations_not_readiness() {
        let changed = UNIT
            .replace("[0,\"/\"]", "[7,\"/org/freedesktop/systemd1/job/7\"]")
            .replace("\"data\":false", "\"data\":true")
            .replace("\"active\"", "\"deactivating\"");
        let unit = parse_unit(changed.as_bytes(), "dbus-broker.service").unwrap();
        assert_eq!(unit.job.id, 7);
        assert!(unit.need_daemon_reload);
        assert_eq!(unit.active_state, "deactivating");
        assert!(
            parse_unit(
                changed.replace("job/7", "job/8").as_bytes(),
                "dbus-broker.service"
            )
            .is_err()
        );
    }

    #[test]
    fn rejects_missing_extra_duplicate_or_wrongly_typed_properties() {
        for changed in [
            UNIT.lines().take(7).collect::<Vec<_>>().join("\n"),
            format!("{UNIT}\n{{\"type\":\"s\",\"data\":\"extra\"}}"),
            UNIT.replacen("\"type\":\"s\"", "\"type\":\"s\",\"type\":\"s\"", 1),
            UNIT.replacen("\"data\":\"loaded\"", "\"data\":\"loaded\",\"extra\":0", 1),
            UNIT.replacen("\"data\":false", "\"data\":\"false\"", 1),
            UNIT.replacen("\"data\":\"loaded\"", "\"data\":\"future-load-state\"", 1),
            UNIT.replacen("\"data\":\"active\"", "\"data\":\"future-active-state\"", 1),
            UNIT.replacen("\"type\":\"ay\"", "\"type\":\"s\"", 1),
            UNIT.replace("14,15,16", "14,15,256"),
            UNIT.replace("14,15,16", "14,15"),
            format!("{UNIT} trailing"),
        ] {
            assert!(parse_unit(changed.as_bytes(), "dbus-broker.service").is_err());
        }
    }

    #[test]
    fn unavailable_unit_diagnostic_preserves_refusal_and_validates_identity() {
        for state in ["stub", "not-found", "bad-setting", "error", "merged"] {
            let response = UNIT.replace("\"loaded\"", &format!("\"{state}\""));
            assert_eq!(
                parse_metadata(
                    response.as_bytes(),
                    "dbus-broker.service",
                    MetadataScope::Required
                )
                .unwrap_err(),
                format!(
                    "systemd unit metadata is unavailable: dbus-broker.service has LoadState={state}"
                )
            );
            // Do not echo an alias or unvalidated response content in diagnostics.
            assert_eq!(
                parse_metadata(
                    response.as_bytes(),
                    "other.service",
                    MetadataScope::Required
                )
                .unwrap_err(),
                "invalid or mismatched systemd unit properties"
            );
        }
        let invalid = UNIT.replace("\"loaded\"", "\"PRIVATE RESPONSE DATA\"");
        assert_eq!(
            parse_metadata(
                invalid.as_bytes(),
                "dbus-broker.service",
                MetadataScope::Required
            )
            .unwrap_err(),
            "invalid or mismatched systemd unit properties"
        );
    }

    #[test]
    fn rejects_wrong_object_and_method_reply_shapes() {
        for bytes in [
            br#"{"type":"o","data":"/org/freedesktop/systemd1/unit/example"}"#.as_slice(),
            br#"{"type":"o","data":["/org/freedesktop/systemd1/job/1"]}"#,
            br#"{"type":"o","data":["/org/freedesktop/systemd1/unit/"]}"#,
            br#"{"type":"o","data":["/org/freedesktop/systemd1/unit/a/b"]}"#,
            br#"{"type":"o","data":["/org/freedesktop/systemd1/unit/a","extra"]}"#,
        ] {
            assert!(parse_object(bytes).is_err());
        }
    }

    #[test]
    fn exec_start_preserves_argv_and_rejects_unbounded_or_malformed_records() {
        for changed in [
            SERVICE.replace("\"data\":42", "\"data\":-1"),
            SERVICE.replace("false,0,0,0,0,0,0,0", "false,0,0,0,0,0,0,0,0"),
            SERVICE.replace("false,0,0,0,0,0,0,0", "false,0,0,0,0,4294967296,0,0"),
            SERVICE.replace("false,0,0,0,0,0,0,0", "false,0,0,0,0,0,2147483648,0"),
            SERVICE.replace("\"--audit\"", "null"),
            SERVICE.replace("\"--audit\"", &format!("\"{}\"", "a".repeat(4097))),
            SERVICE.replace("/system.slice/", "/system.slice/../"),
        ] {
            assert!(parse_service(changed.as_bytes()).is_err());
        }
        let empty = format!(
            "{{\"type\":\"u\",\"data\":0}}\n{{\"type\":\"s\",\"data\":\"\"}}\n{{\"type\":\"a(sasbttttuii)\",\"data\":[]}}\n{}",
            SERVICE.lines().skip(3).collect::<Vec<_>>().join("\n")
        );
        assert!(
            parse_service(empty.as_bytes())
                .unwrap()
                .exec_start
                .is_empty()
        );
    }

    #[test]
    fn preserves_stop_process_and_bounded_cleanup_facts_without_accepting_them() {
        let stop = r#"{"type":"a(sasbttttuii)","data":[["/nix/store/00000000000000000000000000000000-cli/bin/tentaflake",["tentaflake","admission","volume-stop","/nix/store/00000000000000000000000000000000-plan","workspace:example"],false,0,0,0,0,0,0,0]]}"#;
        let guarded = SERVICE
            .replace("\"notify-reload\"", "\"oneshot\"")
            .replace("\"data\":false", "\"data\":true")
            .replace(
                "{\"type\":\"u\",\"data\":0}",
                "{\"type\":\"u\",\"data\":43}",
            )
            .replace("{\"type\":\"a(sasbttttuii)\",\"data\":[]}", stop);
        let service = parse_service(guarded.as_bytes()).unwrap();
        assert_eq!(service.control_pid, 43);
        assert_eq!(service.service_type, "oneshot");
        assert!(service.remain_after_exit);
        assert_eq!(service.exec_stop[0].argv[2], "volume-stop");
        assert_eq!(service.timeout_start_usec, 90_000_000);
        assert!(parse_service(guarded.replace("90000000", "-1").as_bytes()).is_err());
        // Infinity is an observed u64 value; callers must refuse unbounded units.
        let unlimited = parse_service(
            guarded
                .replace("90000000", "18446744073709551615")
                .as_bytes(),
        )
        .unwrap();
        assert_eq!(unlimited.timeout_stop_usec, u64::MAX);
    }

    #[test]
    fn observed_masked_never_started_unit_has_no_invocation_or_service_type() {
        // Observed after LoadUnit(console-getty.service), already masked on the
        // host: no starting operation, and the same selected property signature.
        let masked = UNIT
            .replace("dbus-broker", "console-getty")
            .replace("\"loaded\"", "\"masked\"")
            .replace("\"active\"", "\"inactive\"")
            .replace("\"running\"", "\"dead\"")
            .replace("[1,2,3,4,5,6,7,8,9,10,11,12,13,14,15,16]", "[]");
        let unit = parse_unit(masked.as_bytes(), "console-getty.service").unwrap();
        assert_eq!(unit.load_state, "masked");
        assert_eq!(unit.invocation_id, [0; 16]);
        let mut records: Vec<Value> = SERVICE
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        records[0]["data"] = serde_json::json!(0);
        records[1]["data"] = serde_json::json!("");
        records[2]["data"] = serde_json::json!([]);
        records[5]["data"] = serde_json::json!("");
        let encode = |records: &[Value]| {
            records
                .iter()
                .map(Value::to_string)
                .collect::<Vec<_>>()
                .join("\n")
        };
        let service = parse_service(encode(&records).as_bytes()).unwrap();
        assert!(service.service_type.is_empty());
        assert!(service.exec_start.is_empty());
        records[0]["data"] = serde_json::json!(42);
        assert!(parse_service(encode(&records).as_bytes()).is_err());
    }

    fn listing() -> Value {
        // ListUnitsByPatterns has one array-valued return argument. The observed
        // dbus.service response used this additional outer array exactly.
        serde_json::json!({"type":"a(ssssssouso)","data":[[[
            "tentaflake-state-quota-example.service", "Quota fixture", "loaded", "inactive", "dead", "",
            "/org/freedesktop/systemd1/unit/tentaflake_2dstate_2dquota_2dexample_2eservice", 0, "", "/"
        ]]]})
    }

    #[test]
    fn runtime_inventory_covers_both_backends_and_rejects_aliases() {
        for name in ["docker-removed.service", "podman-removed.service"] {
            let mut value = listing();
            value["data"][0][0][0] = Value::String(name.into());
            assert_eq!(
                parse_listed_units(&serde_json::to_vec(&value).unwrap(), runtime_name).unwrap(),
                [name]
            );
            value["data"][0][0][5] = Value::String("alias.service".into());
            assert!(
                parse_listed_units(&serde_json::to_vec(&value).unwrap(), runtime_name).is_err()
            );
        }
        assert!(
            parse_listed_units(&serde_json::to_vec(&listing()).unwrap(), runtime_name).is_err()
        );
        let files = br#"{"type":"a(ss)","data":[[["/etc/systemd/system/docker-removed.service","masked"],["/etc/systemd/system/podman-removed.service","disabled"]]]}"#;
        assert_eq!(
            parse_listed_files(files, runtime_name)
                .unwrap()
                .into_keys()
                .collect::<Vec<_>>(),
            ["docker-removed.service", "podman-removed.service"]
        );
    }

    #[test]
    fn lists_only_bounded_unique_canonical_quota_names() {
        assert_eq!(
            parse_listed_units(&serde_json::to_vec(&listing()).unwrap(), quota_name).unwrap(),
            ["tentaflake-state-quota-example.service"]
        );
        assert!(
            parse_listed_units(br#"{"type":"a(ssssssouso)","data":[[]]}"#, quota_name)
                .unwrap()
                .is_empty()
        );
        for mode in 0..7 {
            let mut value = listing();
            let original = value["data"][0][0].clone();
            match mode {
                0 => value["data"][0].as_array_mut().unwrap().push(original),
                1 => value["data"][0][0][0] = Value::String("unrelated.service".into()),
                2 => value["data"][0][0][5] = Value::String("alias.service".into()),
                3 => value["data"][0][0][7] = Value::String("0".into()),
                4 => value["data"][0] = Value::Array(vec![original; 513]),
                5 => value["data"][0][0]
                    .as_array_mut()
                    .unwrap()
                    .push(Value::Null),
                _ => value["extra"] = Value::Bool(true),
            }
            assert!(parse_listed_units(&serde_json::to_vec(&value).unwrap(), quota_name).is_err());
        }
        assert!(parse_listed_units(&vec![b' '; MAX_LIST_BYTES + 1], quota_name).is_err());
    }

    #[test]
    fn installed_quota_files_include_unloaded_masks_and_refuse_aliases() {
        let file = serde_json::json!([
            "/etc/systemd/system/tentaflake-workspace-quota-old.service",
            "masked"
        ]);
        let fixture = serde_json::json!({"type":"a(ss)","data":[[file.clone()]]});
        assert_eq!(
            parse_listed_files(&serde_json::to_vec(&fixture).unwrap(), quota_name)
                .unwrap()
                .into_keys()
                .collect::<Vec<_>>(),
            ["tentaflake-workspace-quota-old.service"]
        );
        for mode in 0..6 {
            let mut value = fixture.clone();
            match mode {
                0 => value["data"][0][0][1] = serde_json::json!("alias"),
                1 => value["data"][0].as_array_mut().unwrap().push(file.clone()),
                2 => {
                    value["data"][0][0][0] = serde_json::json!(
                        "/etc/systemd/system/../tentaflake-workspace-quota-old.service"
                    )
                }
                3 => {
                    value["data"][0][0][0] =
                        serde_json::json!("/etc/systemd/system/unrelated.service")
                }
                4 => value["data"][0] = Value::Array(vec![file.clone(); 513]),
                _ => value["data"][0][0][1] = serde_json::json!(null),
            }
            assert!(parse_listed_files(&serde_json::to_vec(&value).unwrap(), quota_name).is_err());
        }
    }

    #[test]
    fn selected_mount_properties_preserve_source_without_classifying_it() {
        // Exact scalar wire shape observed on -.mount with systemd 261.3.
        let root = parse_mount(b"{\"type\":\"s\",\"data\":\"none\"}\n{\"type\":\"s\",\"data\":\"/\"}\n{\"type\":\"s\",\"data\":\"tmpfs\"}").unwrap();
        assert_eq!(
            root,
            MountObservation {
                what: "none".into(),
                where_path: "/".into(),
                filesystem_type: "tmpfs".into()
            }
        );
        let encode = |values: &[Value]| {
            values
                .iter()
                .map(Value::to_string)
                .collect::<Vec<_>>()
                .join("\n")
        };
        for (source, filesystem_type) in [
            ("tmpfs", "tmpfs"),
            ("/dev/mapper/volume", "btrfs"),
            ("/var/lib/old-image.img", "ext4"),
            ("/var/lib/source", "none"),
            ("root", "9p"),
            ("root", "virtiofs"),
        ] {
            let bytes = encode(&[
                serde_json::json!({"type":"s","data":source}),
                serde_json::json!({"type":"s","data":"/var/lib/target"}),
                serde_json::json!({"type":"s","data":filesystem_type}),
            ]);
            let observed = parse_mount(bytes.as_bytes()).unwrap();
            assert_eq!(observed.what, source);
            assert_eq!(observed.filesystem_type, filesystem_type);
        }
        let inert = b"{\"type\":\"s\",\"data\":\"\"}\n{\"type\":\"s\",\"data\":\"\"}\n{\"type\":\"s\",\"data\":\"\"}";
        let observed = parse_mount(inert).unwrap();
        assert!(observed.where_path.is_empty() && observed.filesystem_type.is_empty());
        for mode in 0..8 {
            let mut values = vec![
                serde_json::json!({"type":"s","data":"source"}),
                serde_json::json!({"type":"s","data":"/target"}),
                serde_json::json!({"type":"s","data":"btrfs"}),
            ];
            match mode {
                0 => {
                    values.pop();
                }
                1 => values[1]["data"] = serde_json::json!("/a/../b"),
                2 => values[0]["data"] = serde_json::json!("source\0"),
                3 => values[1]["data"] = serde_json::json!(false),
                4 => values[2]["data"] = serde_json::json!(false),
                5 => values[2]["data"] = serde_json::json!("tmpfs\n"),
                6 => values[2]["data"] = serde_json::json!("x".repeat(65)),
                _ => values.push(serde_json::json!({"type":"s","data":"extra"})),
            }
            assert!(parse_mount(encode(&values).as_bytes()).is_err());
        }
    }

    #[test]
    fn mount_listing_reuses_alias_bounds_and_accepts_the_root_unit() {
        let mut value = listing();
        value["data"][0][0][0] = serde_json::json!("-.mount");
        value["data"][0][0][6] = serde_json::json!("/org/freedesktop/systemd1/unit/_2d_2emount");
        let bytes = serde_json::to_vec(&value).unwrap();
        assert_eq!(parse_listed_units(&bytes, mount_name).unwrap(), ["-.mount"]);
        assert!(parse_listed_units(&bytes, quota_name).is_err());
        assert!(parse_listed_units(&serde_json::to_vec(&listing()).unwrap(), mount_name).is_err());
        value["data"][0][0][5] = serde_json::json!("alias.mount");
        assert!(parse_listed_units(&serde_json::to_vec(&value).unwrap(), mount_name).is_err());
        let files = br#"{"type":"a(ss)","data":[[["/etc/systemd/system/var-lib-old.mount","disabled"],["/run/systemd/generator/-.mount","generated"]]]}"#;
        assert_eq!(
            parse_listed_files(files, mount_name)
                .unwrap()
                .into_keys()
                .collect::<Vec<_>>(),
            ["-.mount", "var-lib-old.mount"]
        );
        assert!(parse_listed_files(files, quota_name).is_err());
    }

    fn missing_mount_wire() -> String {
        // The VM reported home.mount/not-found in its loaded inventory. Other
        // properties model the inert case; they are independently required.
        UNIT.replace("dbus-broker.service", "home.mount")
            .replace("\"loaded\"", "\"not-found\"")
            .replace("\"active\"", "\"inactive\"")
            .replace("\"running\"", "\"dead\"")
            .replace("/etc/systemd/system/home.mount", "")
            .replace("[1,2,3,4,5,6,7,8,9,10,11,12,13,14,15,16]", "[]")
    }

    #[test]
    fn mount_diagnostics_validate_metadata_and_never_echo_source() {
        let mut unit = missing_mount("home.mount", MetadataScope::IncidentalMount).unwrap();
        for (what, class) in [
            ("", "empty"),
            ("none", "none"),
            ("/private/path", "absolute"),
            ("scheme://secret@host/path", "other"),
        ] {
            unit.mount.as_mut().unwrap().what = what.into();
            assert_eq!(
                unit.mount_context(),
                format!("unit=home.mount, type=empty, source={class}")
            );
        }
        unit.id = "private\ninvalid.mount".into();
        unit.mount.as_mut().unwrap().filesystem_type = "private\ninvalid".into();
        assert_eq!(
            unit.mount_context(),
            "unit=invalid, type=invalid, source=other"
        );
        unit.mount.as_mut().unwrap().filesystem_type = "x".repeat(65);
        assert_eq!(
            unit.mount_context(),
            "unit=invalid, type=invalid, source=other"
        );
    }

    fn missing_mount(name: &str, scope: MetadataScope) -> Result<UnitObservation, String> {
        let mut unit = parse_metadata(missing_mount_wire().as_bytes(), name, scope)?;
        unit.mount = Some(parse_mount(
            br#"{"type":"s","data":""}
{"type":"s","data":"/home"}
{"type":"s","data":""}"#,
        )?);
        Ok(unit)
    }

    #[test]
    fn incidental_missing_mount_requires_every_inert_property() {
        let wire = missing_mount_wire();
        let unit = missing_mount("home.mount", MetadataScope::IncidentalMount).unwrap();
        validate_inert_mount(&unit).unwrap();
        assert_eq!(unit.mount.as_ref().unwrap().where_path, "/home");
        assert!(missing_mount("home.mount", MetadataScope::Required).is_err());
        assert!(missing_mount("alias.mount", MetadataScope::IncidentalMount).is_err());
        for (before, after) in [
            ("home.mount", "docker.service"),
            ("\"not-found\"", "\"stub\""),
            ("\"not-found\"", "\"error\""),
            ("\"inactive\"", "\"active\""),
            ("\"inactive\"", "\"failed\""),
            ("\"dead\"", "\"mounting\""),
            ("[0,\"/\"]", "[7,\"/org/freedesktop/systemd1/job/7\"]"),
            ("\"data\":false", "\"data\":true"),
            (
                "\"data\":\"\"",
                "\"data\":\"/etc/systemd/system/home.mount\"",
            ),
            (
                "\"data\":[]",
                "\"data\":[1,2,3,4,5,6,7,8,9,10,11,12,13,14,15,16]",
            ),
            ("\"type\":\"ay\"", "\"type\":\"s\""),
        ] {
            let changed = wire.replace(before, after);
            let name = if before == "home.mount" {
                "docker.service"
            } else {
                "home.mount"
            };
            assert!(
                parse_metadata(changed.as_bytes(), name, MetadataScope::IncidentalMount).is_err(),
                "{before} -> {after}"
            );
        }
        for mode in 0..4 {
            let mut changed = unit.clone();
            match mode {
                0 => changed.mount = None,
                1 => changed.service = Some(parse_service(SERVICE.as_bytes()).unwrap()),
                2 => changed.mount.as_mut().unwrap().what = "/var/lib/old.img".into(),
                _ => changed.mount.as_mut().unwrap().filesystem_type = "btrfs".into(),
            }
            assert!(validate_inert_mount(&changed).is_err());
        }
    }

    fn mount_inventory(installed: bool) -> UnitInventory {
        UnitInventory {
            names: BTreeSet::from(["home.mount".into()]),
            installed: if installed {
                BTreeMap::from([("home.mount".into(), "static".into())])
            } else {
                BTreeMap::new()
            },
        }
    }

    #[test]
    fn incidental_mount_inventory_requires_bracketing_file_absence() {
        for mode in 0..6 {
            let mut calls = 0;
            let required = if mode == 1 {
                BTreeSet::from(["home.mount".into()])
            } else {
                BTreeSet::new()
            };
            let result = observe_mount_inventory(
                &required,
                || {
                    calls += 1;
                    if (mode == 4 && calls == 1) || (mode == 5 && calls == 2) {
                        return Err("file inventory unavailable".into());
                    }
                    Ok(mount_inventory(mode == 2 || (mode == 3 && calls == 2)))
                },
                missing_mount,
            );
            if mode == 0 {
                assert!(result.unwrap().is_empty());
                assert_eq!(calls, 2);
            } else {
                assert!(result.is_err(), "mode {mode}");
                assert_eq!(calls, if mode <= 2 || mode == 4 { 1 } else { 2 });
            }
        }
        // Loaded or masked mounts stay visible to the caller's maintenance
        // policy; the incidental exception must not hide normal observations.
        for state in ["loaded", "masked"] {
            let result = observe_mount_inventory(
                &BTreeSet::new(),
                || Ok(mount_inventory(false)),
                |name, scope| {
                    parse_metadata(
                        missing_mount_wire().replace("not-found", state).as_bytes(),
                        name,
                        scope,
                    )
                },
            )
            .unwrap();
            assert_eq!(result.len(), 1);
            assert_eq!(result[0].load_state, state);
        }
    }

    #[test]
    fn creation_inventory_preserves_template_mask_and_exact_operation_names() {
        let files = serde_json::json!({"type":"a(ss)","data":[[
            ["/etc/systemd/system/tentaflake-environment-create@.service","masked"],
            ["/etc/systemd/system/tentaflake-environment-create@op-1.service","static"]]]});
        let parsed =
            parse_listed_files(&serde_json::to_vec(&files).unwrap(), creation_name).unwrap();
        assert_eq!(parsed[CREATION_TEMPLATE], "masked");
        assert_eq!(
            parsed["tentaflake-environment-create@op-1.service"],
            "static"
        );
        assert!(creation_operation(CREATION_TEMPLATE).is_none());
        for operation in ["op-1", "maintenance-probe", "A.b_2"] {
            assert_eq!(
                creation_operation(&format!(
                    "tentaflake-environment-create@{operation}.service"
                )),
                Some(operation)
            );
        }
        for operation in ["", ".hidden", "../escape", "x@y", "x*", "space name"] {
            assert!(
                creation_operation(&format!(
                    "tentaflake-environment-create@{operation}.service"
                ))
                .is_none()
            );
        }
        assert!(
            creation_operation(&format!(
                "tentaflake-environment-create@{}.service",
                "a".repeat(97)
            ))
            .is_none()
        );
        let mut listed = listing();
        listed["data"][0][0][0] = serde_json::json!("tentaflake-environment-create@op-1.service");
        assert_eq!(
            parse_listed_units(&serde_json::to_vec(&listed).unwrap(), creation_name).unwrap(),
            ["tentaflake-environment-create@op-1.service"]
        );
        listed["data"][0][0][5] = serde_json::json!("other.service");
        assert!(parse_listed_units(&serde_json::to_vec(&listed).unwrap(), creation_name).is_err());
    }

    #[test]
    fn incidental_creation_absence_requires_empty_bracketed_service_metadata() {
        let name = "tentaflake-environment-create@old.service";
        let wire = missing_mount_wire().replace("home.mount", name);
        assert!(parse_metadata(wire.as_bytes(), name, MetadataScope::Required).is_err());
        assert!(parse_metadata(wire.as_bytes(), name, MetadataScope::IncidentalMount).is_err());
        let mut unit =
            parse_metadata(wire.as_bytes(), name, MetadataScope::IncidentalCreation).unwrap();
        let mut service = parse_service(SERVICE.as_bytes()).unwrap();
        service.main_pid = 0;
        service.control_group.clear();
        service.exec_start.clear();
        unit.service = Some(service);
        validate_inert_creation(&unit).unwrap();
        for case in 0..10 {
            let mut changed = unit.clone();
            match case {
                0 => changed.job.id = 1,
                1 => changed.invocation_id = [1; 16],
                2 => changed.fragment_path = "/etc/systemd/system/old.service".into(),
                3 => changed.need_daemon_reload = true,
                4 => changed.active_state = "failed".into(),
                5 => changed.service.as_mut().unwrap().control_pid = 1,
                6 => changed.service.as_mut().unwrap().control_group = "/remaining".into(),
                7 => {
                    changed.service.as_mut().unwrap().exec_start =
                        parse_service(SERVICE.as_bytes()).unwrap().exec_start
                }
                8 => {
                    changed.service.as_mut().unwrap().exec_condition =
                        parse_service(SERVICE.as_bytes()).unwrap().exec_start
                }
                _ => changed.service = None,
            }
            assert!(validate_inert_creation(&changed).is_err(), "case {case}");
        }
    }

    #[test]
    fn rejects_oversize_and_never_echoes_wire_data() {
        let payload = vec![b'x'; MAX_RESPONSE_BYTES + 1];
        assert_eq!(
            parse_service(&payload).unwrap_err(),
            "systemd observation response oversized"
        );
        let error = parse_service(b"PRIVATE RESPONSE DATA").unwrap_err();
        assert_eq!(error, "invalid systemd observation JSON");
    }

    #[test]
    fn names_accept_generated_mount_escaping_without_globs_or_options() {
        for name in [
            "tentaflake-quota.service",
            r"var-lib-agent\x2dstate.mount",
            "docker.service",
            "-.mount",
        ] {
            assert!(unit_name(name));
        }
        for name in [
            "",
            ".service",
            "--host=remote.service",
            "a*.service",
            "a/b.mount",
            "a\n.service",
            r"a\xqq.mount",
            "example",
        ] {
            assert!(!unit_name(name));
        }
    }
}
