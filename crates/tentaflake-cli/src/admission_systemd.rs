//! Bounded, selected systemd observations and explicit metadata loading. These
//! calls never start/stop units or establish a fence; callers own synchronization.

use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::Value;
use std::collections::BTreeSet;
use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

const MAX_RESPONSE_BYTES: usize = 64 * 1024;
const MAX_LIST_BYTES: usize = 512 * 1024;
const OBSERVATION_TIMEOUT: Duration = Duration::from_secs(6);
const CALL_TIMEOUT: Duration = Duration::from_secs(2);
const RELAY_STATES: [&str; 5] = [
    "active",
    "activating",
    "reloading",
    "deactivating",
    "refreshing",
];
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
pub(crate) struct SocketObservation {
    pub unit: UnitObservation,
    pub listen: Vec<(String, String)>,
    pub accept: bool,
    // Socket.Service is not exposed by systemd. Triggers are observed unit
    // associations, not proof of the process handling a particular connection.
    pub triggers: Vec<String>,
    pub max_connections: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MountObservation {
    pub what: String,
    pub where_path: String,
    pub filesystem_type: String,
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
    inspect_unit_until(busctl, name, Instant::now() + OBSERVATION_TIMEOUT)
}

/// Preserve the caller's total deadline across all selected property reads.
pub(crate) fn inspect_unit_until(
    busctl: &Path,
    name: &str,
    deadline: Instant,
) -> Result<UnitObservation, String> {
    observe_unit(busctl, name, "GetUnit", deadline)
}

/// Explicitly load a named unit's metadata, without starting it. A missing unit
/// is still an error; a loaded mask remains visible for legacy-creator fencing.
pub(crate) fn load_unit(busctl: &Path, name: &str) -> Result<UnitObservation, String> {
    observe_unit(
        busctl,
        name,
        "LoadUnit",
        Instant::now() + OBSERVATION_TIMEOUT,
    )
}

/// Observe a loaded pathname stream socket. Unit identity and trigger metadata
/// bracket the Socket properties; callers must still authenticate the selected
/// listener and the process serving their actual connection.
pub(crate) fn inspect_socket_until(
    busctl: &Path,
    name: &str,
    deadline: Instant,
) -> Result<SocketObservation, String> {
    if !valid_busctl(busctl) || !unit_name(name) || !name.ends_with(".socket") {
        return Err("invalid systemd observation tool or socket unit".into());
    }
    let deadline = observation_deadline(deadline, Instant::now());
    let object = unit_object(busctl, name, "GetUnit", deadline)?;
    let mut arguments = vec!["get-property", DESTINATION, &object, UNIT_INTERFACE];
    arguments.extend(UNIT_PROPERTIES);
    arguments.push("Triggers");
    let before = read_bus(busctl, &arguments, deadline, MAX_RESPONSE_BYTES)?;
    let properties = read_bus(
        busctl,
        &[
            "get-property",
            DESTINATION,
            &object,
            "org.freedesktop.systemd1.Socket",
            "Listen",
            "Accept",
            "MaxConnections",
        ],
        deadline,
        MAX_RESPONSE_BYTES,
    )?;
    let after = read_bus(busctl, &arguments, deadline, MAX_RESPONSE_BYTES)?;
    parse_socket(&before, &properties, &after, name)
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
    deadline: Instant,
) -> Result<UnitObservation, String> {
    if !valid_busctl(busctl) || !unit_name(name) {
        return Err("invalid systemd observation tool or unit".into());
    }
    let deadline = observation_deadline(deadline, Instant::now());
    let object = unit_object(busctl, name, method, deadline)?;
    let mut arguments = vec!["get-property", DESTINATION, &object, UNIT_INTERFACE];
    arguments.extend(UNIT_PROPERTIES);
    let mut observed = parse_unit(
        &read_bus(busctl, &arguments, deadline, MAX_RESPONSE_BYTES)?,
        name,
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
    let after = parse_unit(
        &read_bus(busctl, &arguments, deadline, MAX_RESPONSE_BYTES)?,
        name,
    )?;
    if observed != after {
        return Err("systemd unit changed during observation".into());
    }
    observed.service = service;
    observed.mount = mount;
    Ok(observed)
}

fn unit_object(
    busctl: &Path,
    name: &str,
    method: &str,
    deadline: Instant,
) -> Result<String, String> {
    parse_object(&read_bus(
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
    )?)
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
}

/// Include native mount files that are installed but not currently loaded.
pub(crate) fn list_mount_units(busctl: &Path) -> Result<Vec<String>, String> {
    list_units(busctl, &["*.mount"], mount_name)
}

/// Include old OCI launchers even when absent from the currently desired plan.
pub(crate) fn list_runtime_units(busctl: &Path) -> Result<Vec<String>, String> {
    list_units(
        busctl,
        &["docker-*.service", "podman-*.service"],
        runtime_name,
    )
}

/// Discover at most 16 loaded instances of one Research relay template. Include
/// transitions so callers can refuse them; this list does not authenticate the
/// process handling a connection or prove that no new instance can appear.
pub(crate) fn list_relay_units_until(
    busctl: &Path,
    instance_id: &str,
    deadline: Instant,
) -> Result<Vec<String>, String> {
    if !valid_busctl(busctl) {
        return Err("invalid systemd observation tool".into());
    }
    let prefix = relay_prefix(instance_id)?;
    let pattern = format!("{prefix}*.service");
    let state_count = RELAY_STATES.len().to_string();
    let mut arguments = vec![
        "call",
        DESTINATION,
        "/org/freedesktop/systemd1",
        "org.freedesktop.systemd1.Manager",
        "ListUnitsByPatterns",
        "asas",
        &state_count,
    ];
    arguments.extend(RELAY_STATES);
    arguments.extend(["1", &pattern]);
    let bytes = read_bus(
        busctl,
        &arguments,
        observation_deadline(deadline, Instant::now()),
        MAX_RESPONSE_BYTES,
    )?;
    parse_relay_units(&bytes, &prefix)
}

fn relay_prefix(instance_id: &str) -> Result<String, String> {
    if instance_id.is_empty()
        || instance_id.len() > 63
        || !instance_id.bytes().enumerate().all(|(index, byte)| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || (index > 0 && byte == b'-')
        })
    {
        return Err("invalid Research relay instance ID".into());
    }
    Ok(format!("tentaflake-research-{instance_id}@"))
}

fn parse_relay_units(bytes: &[u8], prefix: &str) -> Result<Vec<String>, String> {
    parse_listed_units_bounded(
        bytes,
        |name| {
            unit_name(name)
                && name
                    .strip_prefix(prefix)
                    .and_then(|suffix| suffix.strip_suffix(".service"))
                    .is_some_and(|instance| !instance.is_empty() && !instance.contains('@'))
        },
        &RELAY_STATES,
        16,
        MAX_RESPONSE_BYTES,
    )
}

fn list_units(
    busctl: &Path,
    patterns: &[&str],
    valid_name: fn(&str) -> bool,
) -> Result<Vec<String>, String> {
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
    names.extend(parse_listed_files(
        &list("ListUnitFilesByPatterns")?,
        valid_name,
    )?);
    if names.len() > 512 {
        return Err("systemd unit listing limit exceeded".into());
    }
    Ok(names.into_iter().collect())
}

fn observation_deadline(deadline: Instant, now: Instant) -> Instant {
    deadline.min(now + OBSERVATION_TIMEOUT)
}

fn read_bus(
    busctl: &Path,
    arguments: &[&str],
    deadline: Instant,
    max_bytes: usize,
) -> Result<Vec<u8>, String> {
    let remaining = deadline
        .checked_duration_since(Instant::now())
        .filter(|remaining| !remaining.is_zero())
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
    if Instant::now() >= deadline {
        return Err("systemd observation deadline exceeded".into());
    }
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
    parse_unit_values(&mut values, name)
}

fn parse_unit_values(
    values: &mut std::vec::IntoIter<WireValue>,
    name: &str,
) -> Result<UnitObservation, String> {
    let id: String = property(values, "s")?;
    let load_state: String = property(values, "s")?;
    let active_state: String = property(values, "s")?;
    let sub_state: String = property(values, "s")?;
    let (job_id, job_path): (u32, String) = property(values, "(uo)")?;
    let fragment_path: String = property(values, "s")?;
    let need_daemon_reload = property(values, "b")?;
    let invocation_bytes: Vec<u8> = property(values, "ay")?;
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
    if !matches!(load_state.as_str(), "loaded" | "masked") {
        return Err(format!(
            "systemd unit metadata is unavailable: {id} has LoadState={load_state}"
        ));
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

fn parse_socket_unit(bytes: &[u8], name: &str) -> Result<(UnitObservation, Vec<String>), String> {
    let mut values = records(bytes, UNIT_PROPERTIES.len() + 1, MAX_RESPONSE_BYTES)?;
    let unit = parse_unit_values(&mut values, name)?;
    let mut triggers: Vec<String> = property(&mut values, "as")?;
    if !name.ends_with(".socket")
        || triggers.len() > 16
        || triggers
            .iter()
            .any(|trigger| !unit_name(trigger) || !trigger.ends_with(".service"))
    {
        return Err("invalid systemd socket trigger properties".into());
    }
    triggers.sort();
    if triggers.windows(2).any(|pair| pair[0] == pair[1]) {
        return Err("duplicate systemd socket trigger properties".into());
    }
    Ok((unit, triggers))
}

fn parse_socket(
    before: &[u8],
    properties: &[u8],
    after: &[u8],
    name: &str,
) -> Result<SocketObservation, String> {
    let (unit, triggers) = parse_socket_unit(before, name)?;
    let mut values = records(properties, 3, MAX_RESPONSE_BYTES)?;
    let listen: Vec<(String, String)> = property(&mut values, "a(ss)")?;
    let accept = property(&mut values, "b")?;
    let max_connections = property(&mut values, "u")?;
    if listen.len() > 16
        || listen
            .iter()
            .any(|(kind, address)| kind != "Stream" || !absolute_path(address))
        || listen.iter().collect::<BTreeSet<_>>().len() != listen.len()
    {
        return Err("invalid or unsupported systemd socket listener properties".into());
    }
    let (after_unit, after_triggers) = parse_socket_unit(after, name)?;
    if unit != after_unit || triggers != after_triggers {
        return Err("systemd socket changed during observation".into());
    }
    Ok(SocketObservation {
        unit,
        listen,
        accept,
        triggers,
        max_connections,
    })
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
        || filesystem_type.len() > 64
        || !filesystem_type
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
    {
        return Err("invalid systemd mount properties".into());
    }
    Ok(MountObservation {
        what,
        where_path,
        filesystem_type,
    })
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
    parse_listed_units_bounded(bytes, valid_name, &[], 512, MAX_LIST_BYTES)
}

fn parse_listed_units_bounded(
    bytes: &[u8],
    valid_name: impl Fn(&str) -> bool,
    states: &[&str],
    limit: usize,
    max_bytes: usize,
) -> Result<Vec<String>, String> {
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
        property(&mut records(bytes, 1, max_bytes)?, "a(ssssssouso)")?;
    if units.len() > limit {
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
            || (!states.is_empty() && !states.contains(&active.as_str()))
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

fn parse_listed_files(bytes: &[u8], valid_name: fn(&str) -> bool) -> Result<Vec<String>, String> {
    let [files]: [Vec<(String, String)>; 1] =
        property(&mut records(bytes, 1, MAX_LIST_BYTES)?, "a(ss)")?;
    if files.len() > 512 {
        return Err("systemd unit listing limit exceeded".into());
    }
    let mut names = BTreeSet::new();
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
            || !names.insert(name.to_owned())
        {
            return Err("invalid, duplicate or aliased systemd unit files".into());
        }
    }
    Ok(names.into_iter().collect())
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
    fn nested_observations_never_extend_the_callers_deadline() {
        let now = Instant::now();
        let outer = now + Duration::from_millis(20);
        assert_eq!(observation_deadline(outer, now), outer);
        assert_eq!(
            observation_deadline(outer, now + Duration::from_millis(10)),
            outer
        );
        assert_eq!(
            observation_deadline(now + Duration::from_secs(60), now),
            now + OBSERVATION_TIMEOUT
        );
        assert_eq!(
            observation_deadline(now, now + Duration::from_millis(1)),
            now
        );
    }

    #[test]
    fn expired_observations_refuse_before_attempting_any_bus_command() {
        // A syntactically valid but nonexistent tool distinguishes the deadline
        // refusal from spawning a client with a freshly reset timeout.
        let busctl = Path::new("/nix/store/00000000000000000000000000000000-systemd/bin/busctl");
        let expired = Instant::now() - Duration::from_secs(1);
        for error in [
            inspect_unit_until(busctl, "example.service", expired).unwrap_err(),
            inspect_socket_until(busctl, "example.socket", expired).unwrap_err(),
            list_relay_units_until(busctl, "example", expired).unwrap_err(),
        ] {
            assert_eq!(error, "systemd observation deadline exceeded");
        }
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

    // Selected properties observed on dbus.socket with systemd 261.3. Service
    // is not a Socket property; Unit.Triggers separately returned the service.
    const SOCKET: &str = r#"{"type":"a(ss)","data":[["Stream","/run/dbus/system_bus_socket"]]}
{"type":"b","data":false}
{"type":"u","data":64}"#;

    fn socket_unit() -> String {
        format!(
            "{}\n{}",
            UNIT.replace("dbus-broker.service", "dbus.socket"),
            r#"{"type":"as","data":["dbus-broker.service"]}"#
        )
    }

    fn encode_properties(values: &[Value]) -> Vec<u8> {
        values
            .iter()
            .map(Value::to_string)
            .collect::<Vec<_>>()
            .join("\n")
            .into_bytes()
    }

    #[test]
    fn socket_observation_preserves_actual_triggers_without_synthesizing_a_service() {
        let unit = socket_unit();
        let observed = parse_socket(
            unit.as_bytes(),
            SOCKET.as_bytes(),
            unit.as_bytes(),
            "dbus.socket",
        )
        .unwrap();
        assert_eq!(observed.unit.id, "dbus.socket");
        assert_eq!(
            observed.listen,
            [(
                "Stream".to_owned(),
                "/run/dbus/system_bus_socket".to_owned()
            )]
        );
        assert!(!observed.accept);
        assert_eq!(observed.max_connections, 64);
        assert_eq!(observed.triggers, ["dbus-broker.service"]);
        // Accept does not imply that a template or a live process appears in
        // Triggers. The caller proves the servicing process independently.
        let empty = unit.replace("[\"dbus-broker.service\"]", "[]");
        let observed = parse_socket(
            empty.as_bytes(),
            SOCKET.replace("false", "true").as_bytes(),
            empty.as_bytes(),
            "dbus.socket",
        )
        .unwrap();
        assert!(observed.accept);
        assert!(observed.triggers.is_empty());
    }

    #[test]
    fn socket_observation_refuses_malformed_or_unbounded_selected_properties() {
        let unit = socket_unit();
        for mode in 0..11 {
            let mut values: Vec<Value> = SOCKET
                .lines()
                .map(|line| serde_json::from_str(line).unwrap())
                .collect();
            match mode {
                0 => {
                    values.pop();
                }
                1 => values.push(serde_json::json!({"type":"s","data":"extra"})),
                2 => values[0]["type"] = serde_json::json!("as"),
                3 => values[0]["data"][0] = serde_json::json!(["Stream", "/one", "/two"]),
                4 => values[0]["data"][0][1] = serde_json::json!("/run/../socket"),
                5 => values[0]["data"][0][1] = serde_json::json!("/run/socket\0"),
                6 => values[0]["data"][0][0] = serde_json::json!("FutureTransport"),
                7 => {
                    values[0]["data"] = serde_json::json!([["Stream", "/one"], ["Stream", "/one"]])
                }
                8 => {
                    values[0]["data"] = serde_json::json!(
                        (0..17)
                            .map(|index| ("Stream", format!("/run/socket{index}")))
                            .collect::<Vec<_>>()
                    )
                }
                9 => values[1]["data"] = serde_json::json!("true"),
                _ => values[2]["data"] = serde_json::json!(4294967296_u64),
            }
            assert!(
                parse_socket(
                    unit.as_bytes(),
                    &encode_properties(&values),
                    unit.as_bytes(),
                    "dbus.socket"
                )
                .is_err()
            );
        }
        for triggers in [
            serde_json::json!(["dbus-broker.service", "dbus-broker.service"]),
            serde_json::json!(["other.socket"]),
            serde_json::json!(["wild*.service"]),
            serde_json::json!([false]),
            serde_json::json!(
                (0..17)
                    .map(|index| format!("relay@{index}.service"))
                    .collect::<Vec<_>>()
            ),
        ] {
            let mut values: Vec<Value> = unit
                .lines()
                .map(|line| serde_json::from_str(line).unwrap())
                .collect();
            values[8]["data"] = triggers;
            assert!(parse_socket_unit(&encode_properties(&values), "dbus.socket").is_err());
        }
        assert!(
            parse_socket(
                unit.as_bytes(),
                &vec![b' '; MAX_RESPONSE_BYTES + 1],
                unit.as_bytes(),
                "dbus.socket"
            )
            .is_err()
        );
        assert!(parse_socket_unit(UNIT.as_bytes(), "dbus-broker.service").is_err());
    }

    #[test]
    fn socket_observation_refuses_identity_trigger_and_reload_races() {
        let before = socket_unit();
        for (index, replacement) in [
            (0, serde_json::json!("other.socket")),
            (1, serde_json::json!("masked")),
            (2, serde_json::json!("deactivating")),
            (3, serde_json::json!("stop-pre")),
            (4, serde_json::json!([7, "/org/freedesktop/systemd1/job/7"])),
            (5, serde_json::json!("/etc/systemd/system/changed.socket")),
            (6, serde_json::json!(true)),
            (7, serde_json::json!(vec![2; 16])),
            (8, serde_json::json!(["other.service"])),
        ] {
            let mut after: Vec<Value> = before
                .lines()
                .map(|line| serde_json::from_str(line).unwrap())
                .collect();
            after[index]["data"] = replacement;
            assert!(
                parse_socket(
                    before.as_bytes(),
                    SOCKET.as_bytes(),
                    &encode_properties(&after),
                    "dbus.socket"
                )
                .is_err()
            );
        }
    }

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
                parse_unit(response.as_bytes(), "dbus-broker.service").unwrap_err(),
                format!(
                    "systemd unit metadata is unavailable: dbus-broker.service has LoadState={state}"
                )
            );
            // Do not echo an alias or unvalidated response content in diagnostics.
            assert_eq!(
                parse_unit(response.as_bytes(), "other.service").unwrap_err(),
                "invalid or mismatched systemd unit properties"
            );
        }
        let invalid = UNIT.replace("\"loaded\"", "\"PRIVATE RESPONSE DATA\"");
        assert_eq!(
            parse_unit(invalid.as_bytes(), "dbus-broker.service").unwrap_err(),
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
    fn relay_listing_is_exact_bounded_and_includes_transitions() {
        let prefix = relay_prefix("example-1").unwrap();
        let relay = "tentaflake-research-example-1@0-42-0.service";
        let mut value = listing();
        value["data"][0][0][0] = serde_json::json!(relay);
        for state in RELAY_STATES {
            value["data"][0][0][3] = serde_json::json!(state);
            assert_eq!(
                parse_relay_units(&serde_json::to_vec(&value).unwrap(), &prefix).unwrap(),
                [relay]
            );
        }
        assert!(
            parse_relay_units(br#"{"type":"a(ssssssouso)","data":[[]]}"#, &prefix)
                .unwrap()
                .is_empty()
        );
        for id in [
            "",
            "-example",
            "example*",
            "example@other",
            "Example",
            "a/b",
            &"x".repeat(64),
        ] {
            assert!(relay_prefix(id).is_err());
        }
        for mode in 0..11 {
            let mut changed = value.clone();
            match mode {
                0 => {
                    changed["data"][0][0][0] =
                        serde_json::json!("tentaflake-research-other@0.service")
                }
                1 => {
                    changed["data"][0][0][0] =
                        serde_json::json!("tentaflake-research-example-1@.service")
                }
                2 => {
                    changed["data"][0][0][0] =
                        serde_json::json!("tentaflake-research-example-1@a@b.service")
                }
                3 => {
                    changed["data"][0][0][0] =
                        serde_json::json!("tentaflake-research-example-1@*.service")
                }
                4 => changed["data"][0][0][3] = serde_json::json!("inactive"),
                5 => changed["data"][0][0][5] = serde_json::json!("alias.service"),
                6 => changed["data"][0]
                    .as_array_mut()
                    .unwrap()
                    .push(value["data"][0][0].clone()),
                7 => changed["data"][0][0][7] = serde_json::json!("0"),
                8 => {
                    changed["data"][0][0][6] = serde_json::json!("/org/freedesktop/systemd1/job/1")
                }
                9 => changed["extra"] = serde_json::json!(true),
                _ => {
                    // Unique valid entries still exceed the per-template cap.
                    changed["data"][0] = serde_json::json!(
                        (0..17)
                            .map(|index| {
                                let mut record = value["data"][0][0].clone();
                                record[0] = serde_json::json!(format!("{prefix}{index}.service"));
                                record[6] = serde_json::json!(format!(
                                    "/org/freedesktop/systemd1/unit/relay{index}"
                                ));
                                record
                            })
                            .collect::<Vec<_>>()
                    );
                }
            }
            assert!(parse_relay_units(&serde_json::to_vec(&changed).unwrap(), &prefix).is_err());
        }
        assert!(parse_relay_units(&vec![b' '; MAX_RESPONSE_BYTES + 1], &prefix).is_err());
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
            parse_listed_files(files, runtime_name).unwrap(),
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
            parse_listed_files(&serde_json::to_vec(&fixture).unwrap(), quota_name).unwrap(),
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
            parse_listed_files(files, mount_name).unwrap(),
            ["-.mount", "var-lib-old.mount"]
        );
        assert!(parse_listed_files(files, quota_name).is_err());
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
