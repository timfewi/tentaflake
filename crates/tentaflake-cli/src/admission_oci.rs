//! Owned offline Docker lifecycle. A recorded intent precedes every create; an
//! exact CID is synced before start. Uncertain outcomes retain the entire claim.
use crate::admission_host::{
    HostPlan, load_plan, open_store_file, read_store_document, store_path,
};
use crate::admission_runtime::{LEDGER, authority, installed, require_root};
use crate::admission_systemd::{self, ExecCommand, UnitObservation};
use serde::Deserialize;
use serde_json::{Map, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::{self, Read};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{FileTypeExt, MetadataExt};
use std::os::unix::net::UnixDatagram;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tentaflake_admission::host_fence::{self as admission_fence, HostFence};
use tentaflake_admission::ledger::{
    ClaimOutcome, DaemonIdentity, Ledger, Observation, Receipt, RuntimeContext, RuntimeImage,
    RuntimeIntent, SupervisorIdentity,
};
use tentaflake_admission::{
    AdmissionRequest, AdmissionSnapshot, EvidenceKind, EvidenceSource, EvidenceStatus,
    ExpectedRevisions, ReadinessEvidence, ReadinessGeneration, ReadinessSnapshot, ReservationState,
    SCHEMA_VERSION, StorageKind,
};

const MAX_OUTPUT: usize = 1024 * 1024;
const OPERATION_LABEL: &str = "io.tentaflake.operation";
const INSTANCE_LABEL: &str = "io.tentaflake.instance";
const SPEC_LABEL: &str = "io.tentaflake.spec-revision";

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(crate) struct RuntimeSpec {
    pub image: RuntimeImage,
    pub unit: String,
    pub container: ContainerSpec,
    pub provenance: Option<Provenance>,
}

/// Closed projection of the final upstream OCI container, after the shared
/// security owner. Fields unsupported by this dispatcher are refused explicitly.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(crate) struct ContainerSpec {
    service_name: String,
    login: Login,
    cmd: Vec<String>,
    labels: BTreeMap<String, String>,
    entrypoint: Option<String>,
    environment: BTreeMap<String, String>,
    environment_files: Vec<String>,
    #[serde(rename = "log-driver")]
    log_driver: String,
    ports: Vec<String>,
    user: Option<String>,
    volumes: Vec<String>,
    workdir: Option<String>,
    depends_on: Vec<String>,
    hostname: Option<String>,
    pre_run_extra_options: Vec<String>,
    extra_options: Vec<String>,
    auto_start: bool,
    pull: String,
    capabilities: BTreeMap<String, Option<bool>>,
    devices: Vec<String>,
    privileged: bool,
    auto_remove_on_stop: bool,
    networks: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct Login {
    username: Option<String>,
    password_file: Option<String>,
    registry: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(crate) struct Provenance {
    enable: bool,
    mode: String,
    public_key_file: String,
    certificate_identity: String,
    certificate_oidc_issuer: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct LocalDescriptor {
    schema_version: u32,
    format: String,
    archive: PathBuf,
    image_id: String,
    os: String,
    architecture: String,
}

fn digest(value: &str) -> bool {
    value.strip_prefix("sha256:").is_some_and(|hex| {
        hex.len() == 64
            && hex
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    })
}
fn text(value: &str) -> bool {
    value.len() <= 4096 && !value.contains('\0')
}
fn absolute(value: &str) -> bool {
    value.starts_with('/')
        && text(value)
        && !value.bytes().any(|b| b.is_ascii_control())
        && value[1..].split('/').all(|p| !matches!(p, "" | "." | ".."))
}
fn name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && (value.as_bytes()[0].is_ascii_lowercase() || value.as_bytes()[0].is_ascii_digit())
        && value
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}
fn sized(value: &str) -> Result<u64, String> {
    let (number, factor) = match value.as_bytes().last() {
        Some(b'k' | b'K') => (&value[..value.len() - 1], 1024),
        Some(b'm' | b'M') => (&value[..value.len() - 1], 1024 * 1024),
        Some(b'g' | b'G') => (&value[..value.len() - 1], 1024 * 1024 * 1024),
        _ => (value, 1),
    };
    number
        .parse::<u64>()
        .ok()
        .filter(|_| number.bytes().all(|b| b.is_ascii_digit()))
        .and_then(|n| n.checked_mul(factor))
        .filter(|n| *n > 0)
        .ok_or_else(|| "invalid OCI resource quantity".into())
}
fn cpu_nanos(value: &str) -> Result<u64, String> {
    let (whole, fraction) = value.split_once('.').unwrap_or((value, ""));
    if fraction.len() > 9
        || !whole
            .bytes()
            .chain(fraction.bytes())
            .all(|b| b.is_ascii_digit())
    {
        return Err("invalid OCI CPU quantity".into());
    }
    let whole = whole
        .parse::<u64>()
        .map_err(|_| "invalid OCI CPU quantity")?;
    let fraction = if fraction.is_empty() {
        0
    } else {
        fraction
            .parse::<u64>()
            .map_err(|_| "invalid OCI CPU quantity")?
            * 10_u64.pow(9 - fraction.len() as u32)
    };
    whole
        .checked_mul(1_000_000_000)
        .and_then(|n| n.checked_add(fraction))
        .filter(|n| *n > 0)
        .ok_or_else(|| "invalid OCI CPU quantity".into())
}

struct Containment {
    mounts: Vec<crate::DeclaredMount>,
    tmpfs: BTreeMap<String, u64>,
    resources: crate::DeclaredLiveResources,
}
fn containment(plan: &HostPlan, id: &str, spec: &RuntimeSpec) -> Result<Containment, String> {
    let instance = plan
        .instances
        .get(id)
        .ok_or("runtime has no declared instance")?;
    let mut values = BTreeMap::new();
    let mut tmpfs = BTreeMap::new();
    for option in &spec.container.extra_options {
        let (key, value) = option.split_once('=').unwrap_or((option, ""));
        if key == "--tmpfs" {
            let (path, opts) = value.split_once(':').ok_or("invalid OCI tmpfs")?;
            let size = opts
                .strip_prefix("rw,nosuid,nodev,noexec,size=")
                .ok_or("invalid OCI tmpfs policy")?;
            if !matches!(path, "/tmp" | "/run" | "/var/tmp")
                || tmpfs.insert(path.to_owned(), sized(size)?).is_some()
            {
                return Err("invalid or duplicate OCI tmpfs".into());
            }
            continue;
        }
        // Security-opt occurs twice; bind each exact argument separately.
        let key = if key == "--security-opt" || key == "--ulimit" {
            option.as_str()
        } else {
            key
        };
        if values.insert(key, value).is_some() {
            return Err("duplicate OCI launch option".into());
        }
        if !matches!(
            key,
            "--read-only"
                | "--security-opt=no-new-privileges:true"
                | "--security-opt=apparmor=docker-default"
                | "--memory"
                | "--memory-swap"
                | "--cpus"
                | "--pids-limit"
                | "--runtime"
                | "--network"
                | "--dns"
                | "--dns-option"
                | "--sysctl"
        ) && !key.starts_with("--ulimit=")
        {
            return Err("unsupported OCI launch option".into());
        }
    }
    // Ulimits share one option spelling; parse them independently below.
    let required = |key| {
        values
            .get(key)
            .copied()
            .ok_or("missing OCI containment option")
    };
    if !required("--read-only")?.is_empty()
        || required("--runtime")? != "runsc"
        || required("--security-opt=no-new-privileges:true")? != "no-new-privileges:true"
        || required("--security-opt=apparmor=docker-default")? != "apparmor=docker-default"
        || tmpfs.len() != 3
    {
        return Err("unsupported OCI containment policy".into());
    }
    let mut limits = BTreeMap::new();
    for option in &spec.container.extra_options {
        if let Some(value) = option.strip_prefix("--ulimit=") {
            let (kind, bounds) = value.split_once('=').ok_or("invalid OCI ulimit")?;
            let (soft, hard) = bounds.split_once(':').ok_or("invalid OCI ulimit")?;
            if !matches!(kind, "nofile" | "nproc")
                || soft != hard
                || limits.insert(kind, sized(soft)?).is_some()
            {
                return Err("invalid OCI ulimit".into());
            }
        }
    }
    let resources = crate::DeclaredLiveResources {
        memory_bytes: sized(required("--memory")?)?,
        memory_swap_bytes: sized(required("--memory-swap")?)?,
        nano_cpus: cpu_nanos(required("--cpus")?)?,
        pids_limit: sized(required("--pids-limit")?)?,
        nofile: *limits.get("nofile").ok_or("missing nofile limit")?,
        nproc: *limits.get("nproc").ok_or("missing nproc limit")?,
    };
    if resources.memory_bytes != instance.resources.memory_bytes
        || resources.memory_swap_bytes != instance.resources.memory_swap_bytes
        || resources.nano_cpus != instance.resources.nano_cpus
        || resources.pids_limit != instance.resources.pids
        || resources.nproc != resources.pids_limit
    {
        return Err("OCI resources do not match admission resources".into());
    }
    let mut mounts = Vec::new();
    let mut destinations = BTreeSet::new();
    for volume in &spec.container.volumes {
        let parts: Vec<_> = volume.split(':').collect();
        if !(2..=3).contains(&parts.len())
            || !absolute(parts[0])
            || !absolute(parts[1])
            || !destinations.insert(parts[1])
        {
            return Err("invalid OCI bind mount".into());
        }
        let writable = match parts.get(2).copied() {
            None | Some("rw") => true,
            Some("ro") => false,
            _ => return Err("unsupported OCI mount options".into()),
        };
        if writable
            && !plan
                .volumes
                .iter()
                .any(|v| v.spec.instance_id == id && v.spec.mount_path == Path::new(parts[0]))
        {
            return Err("OCI writable mount has no quota".into());
        }
        mounts.push(crate::DeclaredMount {
            source: parts[0].into(),
            destination: parts[1].into(),
            writable,
        });
    }
    Ok(Containment {
        mounts,
        tmpfs,
        resources,
    })
}

pub(crate) fn validate_runtime(
    plan: &HostPlan,
    id: &str,
    spec: &RuntimeSpec,
) -> Result<(), String> {
    let c = &spec.container;
    let instance = plan
        .instances
        .get(id)
        .ok_or("runtime has no declared instance")?;
    if !name(id)
        || spec.unit != format!("{}.service", c.service_name)
        || c.service_name != format!("docker-{id}")
        || c.login.username.is_some()
        || c.login.password_file.is_some()
        || c.login.registry.is_some()
        || c.privileged
        || c.auto_remove_on_stop
        || c.pull != "never"
        || !c.ports.is_empty()
        || !c.devices.is_empty()
        || !c.pre_run_extra_options.is_empty()
        || c.capabilities.get("ALL") != Some(&Some(false))
        || c.capabilities.values().any(|v| *v == Some(true))
        || c.extra_options.len() > 64
        || c.volumes.len() > 64
        || c.cmd.len() > 128
        || c.labels.len() > 128
        || c.environment.len() > 128
        || !matches!(c.log_driver.as_str(), "journald" | "json-file" | "local")
        || c.labels
            .keys()
            .any(|k| matches!(k.as_str(), OPERATION_LABEL | INSTANCE_LABEL | SPEC_LABEL))
        || c.cmd
            .iter()
            .chain(c.environment.keys())
            .chain(c.environment.values())
            .chain(c.labels.keys())
            .chain(c.labels.values())
            .any(|v| !text(v))
        || c.entrypoint
            .as_deref()
            .is_some_and(|v| !text(v) || v.is_empty())
        || c.workdir.as_deref().is_some_and(|v| !absolute(v))
        || c.hostname.as_deref().is_some_and(|v| !name(v))
        || c.depends_on.iter().any(|v| !name(v))
        || c.networks.len() > 1
        || c.networks.iter().any(|v| !name(v))
        || c.environment_files.iter().any(|v| {
            !absolute(v) || !v.starts_with("/run/tentaflake-broker/") || !v.ends_with(".env")
        })
    {
        return Err("unsupported or unsafe final OCI configuration".into());
    }
    let owners: BTreeSet<_> = plan
        .volumes
        .iter()
        .filter(|v| v.spec.instance_id == id)
        .map(|v| format!("{}:{}", v.spec.owner_uid, v.spec.owner_gid))
        .collect();
    if owners.len() != 1
        || c.user.as_ref() != owners.first()
        || c.user
            .as_deref()
            .is_none_or(|v| v.starts_with("0:") || v.ends_with(":0"))
    {
        return Err("OCI user does not match quota ownership".into());
    }
    spec.image
        .validate()
        .map_err(|_| "invalid runtime image source")?;
    if let RuntimeImage::LocalArtifact { descriptor } = &spec.image
        && (Path::new(descriptor).parent() != Some(Path::new("/nix/store"))
            || spec.provenance.is_some())
    {
        return Err(
            "local descriptor must be a direct store file without registry provenance".into(),
        );
    }
    if let Some(p) = &spec.provenance
        && (!p.enable
            || !matches!(p.mode.as_str(), "key" | "keyless")
            || [
                &p.public_key_file,
                &p.certificate_identity,
                &p.certificate_oidc_issuer,
            ]
            .iter()
            .any(|v| !text(v)))
    {
        return Err("invalid image provenance policy".into());
    }
    let limits = containment(plan, id, spec)?;
    if instance.requested_capabilities.is_empty()
        && (!c.networks.is_empty()
            || !c.environment_files.is_empty()
            || !c.extra_options.iter().any(|v| v == "--network=none")
            || c.extra_options
                .iter()
                .any(|v| v.starts_with("--dns") || v.starts_with("--sysctl"))
            || limits
                .mounts
                .iter()
                .any(|m| !m.writable && !store_path(Path::new(&m.source))))
    {
        return Err("offline OCI definition has an external transport".into());
    }
    // autoStart is systemd policy, but remains revision-bound with every field.
    let _ = c.auto_start;
    Ok(())
}

/// Restrict an installed source to the exact offline configuration supported by
/// stopped creation. Identity/path rebinding happens separately; no dependencies
/// or additional source mounts may be inherited by a new environment.
pub(crate) fn validate_template_source(plan: &HostPlan, id: &str) -> Result<(), String> {
    let spec = plan
        .runtimes
        .get(id)
        .ok_or("creation source has no runtime")?;
    validate_runtime(plan, id, spec)?;
    let instance = &plan.instances[id];
    let container = &spec.container;
    let volumes: Vec<_> = plan
        .volumes
        .iter()
        .filter(|v| v.spec.instance_id == id)
        .collect();
    if !instance.requested_capabilities.is_empty()
        || spec.provenance.is_some()
        || container.auto_start
        || container.hostname.is_some()
        || !container.depends_on.is_empty()
        || !container.environment_files.is_empty()
        || !container.networks.is_empty()
        || volumes.len() != 2
        || container.volumes.len() != 2
        || !volumes.iter().all(|volume| {
            let destination = match volume.spec.kind {
                tentaflake_admission::StorageKind::State => "/state",
                tentaflake_admission::StorageKind::Workspace => {
                    container.workdir.as_deref().unwrap_or("")
                }
            };
            container.volumes.contains(&format!(
                "{}:{destination}:rw",
                volume.spec.mount_path.display()
            ))
        })
    {
        return Err("unsupported creation source configuration".into());
    }
    Ok(())
}

fn docker(plan: &HostPlan) -> Command {
    let mut command = Command::new(&plan.tools.docker);
    command
        .env_clear()
        .stdin(Stdio::null())
        .args(["--host=unix:///run/docker.sock", "--config=/var/empty"]);
    command
}
fn read_output(command: &mut Command) -> Result<Vec<u8>, String> {
    let output = crate::output_with_timeout(command, Duration::from_secs(10))
        .map_err(|_| "runtime observation unavailable")?;
    if !output.status.success() || output.stdout.len() > MAX_OUTPUT {
        return Err("runtime observation failed or exceeded its bound".into());
    }
    Ok(output.stdout)
}
fn docker_json(plan: &HostPlan, args: &[&str]) -> Result<Value, String> {
    let bytes = read_output(docker(plan).args(args))?;
    serde_json::from_slice(&bytes).map_err(|_| "invalid runtime observation".into())
}

/// Unlike the read-only runner, a mutator inherits the outer fence. Killing its
/// client never proves the daemon cancelled the request; callers retain Unknown.
pub(crate) fn mutate(
    command: &mut Command,
    fence: &HostFence,
    timeout: Duration,
) -> Result<Vec<u8>, String> {
    command.stdout(Stdio::piped()).stderr(Stdio::null());
    fence.inherit(command)?;
    let mut child = command
        .spawn()
        .map_err(|_| "cannot start owned runtime mutation")?;
    let mut stdout = child
        .stdout
        .take()
        .ok_or("runtime mutation has no output pipe")?;
    let flags = unsafe { libc::fcntl(stdout.as_raw_fd(), libc::F_GETFL) };
    if flags < 0
        || unsafe { libc::fcntl(stdout.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0
    {
        let _ = child.kill();
        let _ = child.wait();
        return Err("cannot bound runtime mutation output".into());
    }
    let deadline = Instant::now() + timeout;
    let mut bytes = Vec::new();
    let result = loop {
        let mut chunk = [0_u8; 4096];
        match stdout.read(&mut chunk) {
            Ok(n) if n > 0 => {
                bytes.extend_from_slice(&chunk[..n]);
                if bytes.len() > MAX_OUTPUT {
                    break Err("runtime mutation output limit exceeded");
                }
            }
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
            Err(_) => break Err("cannot read runtime mutation outcome"),
        }
        match child.try_wait() {
            Ok(Some(status)) => {
                // The fixed Docker client has exited; drain the bounded pipe.
                loop {
                    match stdout.read(&mut chunk) {
                        Ok(0) => break,
                        Ok(n) => {
                            bytes.extend_from_slice(&chunk[..n]);
                            if bytes.len() > MAX_OUTPUT {
                                break;
                            }
                        }
                        Err(_) => break,
                    }
                }
                break if status.success() && bytes.len() <= MAX_OUTPUT {
                    Ok(())
                } else {
                    Err("runtime mutation did not confirm success")
                };
            }
            Ok(None) => {}
            Err(_) => break Err("runtime mutation completion is unknown"),
        }
        if Instant::now() >= deadline {
            break Err("runtime mutation timed out; outcome remains unknown");
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    if result.is_err() {
        let _ = child.kill();
        let _ = child.wait();
    }
    result.map(|()| bytes).map_err(str::to_owned)
}

fn local_descriptor(path: &str) -> Result<LocalDescriptor, String> {
    let descriptor: LocalDescriptor =
        serde_json::from_slice(&read_store_document(Path::new(path), 16 * 1024)?)
            .map_err(|_| "invalid local image descriptor")?;
    if descriptor.schema_version != 1
        || descriptor.format != "docker-archive"
        || !digest(&descriptor.image_id)
        || descriptor.os != "linux"
        || !matches!(descriptor.architecture.as_str(), "amd64" | "arm64")
        || !store_path(&descriptor.archive)
    {
        return Err("unsupported local image descriptor".into());
    }
    Ok(descriptor)
}
fn image(plan: &HostPlan, spec: &RuntimeSpec) -> Result<(String, Map<String, Value>), String> {
    let (reference, local) = match &spec.image {
        RuntimeImage::Registry { reference } => (reference.clone(), None),
        RuntimeImage::LocalArtifact { descriptor } => {
            let d = local_descriptor(descriptor)?;
            (d.image_id.clone(), Some(d))
        }
    };
    let observed = docker_json(
        plan,
        &["image", "inspect", "--format", "{{json .}}", &reference],
    )?;
    let id = observed
        .get("Id")
        .and_then(Value::as_str)
        .filter(|id| digest(id))
        .ok_or("image has no immutable config identity")?;
    if observed.get("Os").and_then(Value::as_str) != Some("linux") {
        return Err("unsupported runtime image OS".into());
    }
    let architecture = observed
        .get("Architecture")
        .and_then(Value::as_str)
        .ok_or("image architecture unavailable")?;
    let expected_arch = match std::env::consts::ARCH {
        "x86_64" => "amd64",
        "aarch64" => "arm64",
        _ => return Err("unsupported host image architecture".into()),
    };
    if architecture != expected_arch {
        return Err("image architecture does not match this host".into());
    }
    if let Some(local) = local {
        if id != local.image_id || architecture != local.architecture {
            return Err("loaded image does not match its immutable descriptor".into());
        }
    } else if observed
        .get("RepoDigests")
        .and_then(Value::as_array)
        .is_none_or(|values| !values.iter().any(|v| v.as_str() == Some(&reference)))
    {
        return Err("loaded image does not retain the declared registry digest".into());
    }
    let config = observed
        .get("Config")
        .and_then(Value::as_object)
        .ok_or("image config unavailable")?
        .clone();
    if config
        .get("Volumes")
        .is_some_and(|v| !v.is_null() && v.as_object().is_none_or(|o| !o.is_empty()))
    {
        return Err("image-declared anonymous volumes are unsupported".into());
    }
    if config
        .get("Healthcheck")
        .is_some_and(|v| !v.is_null() && v.as_object().is_none_or(|o| !o.is_empty()))
    {
        return Err("image healthcheck requires a separately owned lifecycle".into());
    }
    Ok((id.into(), config))
}

pub(crate) fn acquire(path: &Path, id: &str) -> Result<u8, String> {
    require_root()?;
    let plan = load_plan(path)?;
    let spec = plan
        .runtimes
        .get(id)
        .ok_or("no declared runtime for instance")?;
    let RuntimeImage::LocalArtifact { descriptor } = &spec.image else {
        return Err("registry acquisition is not implemented by host admission".into());
    };
    let fence = admission_fence::exclusive()?;
    installed(path)?;
    let d = local_descriptor(descriptor)?;
    // Descriptors are generated from this immutable regular archive. Hold its FD
    // through load; no mutable path lookup or extraction is involved.
    let file = open_store_file(&d.archive)?;
    let metadata = file
        .metadata()
        .map_err(|_| "cannot inspect local archive")?;
    if !metadata.is_file()
        || metadata.uid() != 0
        || metadata.mode() & 0o222 != 0
        || metadata.len() == 0
    {
        return Err("local archive is not an immutable root-owned file".into());
    }
    mutate(
        docker(&plan)
            .args(["image", "load", "--quiet"])
            .stdin(Stdio::from(file)),
        &fence,
        Duration::from_secs(180),
    )?;
    image(&plan, spec)?;
    Ok(0)
}

fn invocation_hex(value: &[u8; 16]) -> String {
    value.iter().map(|byte| format!("{byte:02x}")).collect()
}
fn process_start(pid: u32) -> Result<u64, String> {
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
fn exec_is(exec: &[ExecCommand], plan: &HostPlan, path: &Path, id: &str, action: &str) -> bool {
    exec.len() == 1
        && !exec[0].ignore_failure
        && Path::new(&exec[0].path) == plan.tools.cli
        && exec[0].argv
            == [
                plan.tools.cli.to_string_lossy().into_owned(),
                "admission".into(),
                action.into(),
                path.to_string_lossy().into_owned(),
                id.into(),
            ]
}
pub(crate) fn valid_unit(
    observed: &UnitObservation,
    plan: &HostPlan,
    path: &Path,
    id: &str,
    spec: &RuntimeSpec,
) -> bool {
    let Some(service) = &observed.service else {
        return false;
    };
    observed.id == spec.unit
        && observed.load_state == "loaded"
        && !observed.need_daemon_reload
        && service.service_type == "notify"
        && service.notify_access == "main"
        && !service.remain_after_exit
        && service.restart == "no"
        && service.kill_mode == "control-group"
        && (1..=300_000_000).contains(&service.timeout_start_usec)
        && (1..=120_000_000).contains(&service.timeout_stop_usec)
        && service.exec_start_pre.is_empty()
        && service.exec_start_post.is_empty()
        && service.exec_stop_post.is_empty()
        && service.exec_reload.is_empty()
        && service.exec_condition.is_empty()
        && exec_is(&service.exec_start, plan, path, id, "runtime-start")
        && exec_is(&service.exec_stop, plan, path, id, "runtime-stop")
}
fn caller(
    plan: &HostPlan,
    path: &Path,
    id: &str,
    spec: &RuntimeSpec,
    start: bool,
) -> Result<SupervisorIdentity, String> {
    let unit = admission_systemd::inspect_unit(&plan.tools.busctl, &spec.unit)?;
    let service = unit.service.as_ref().ok_or("runtime service unavailable")?;
    let pid = std::process::id();
    let correct_phase = if start {
        unit.active_state == "activating" && unit.sub_state == "start" && service.main_pid == pid
    } else {
        unit.active_state == "deactivating"
            && unit.sub_state == "stop"
            && service.control_pid == pid
    };
    if !valid_unit(&unit, plan, path, id, spec)
        || !correct_phase
        || unit.invocation_id == [0; 16]
        || service.control_group.is_empty()
        || service.control_group == "/"
        || fs::read_to_string("/proc/self/cgroup")
            .map_err(|_| "cannot inspect runtime cgroup")?
            .trim()
            != format!("0::{}", service.control_group)
        || fs::canonicalize(&plan.tools.cli).map_err(|_| "cannot inspect declared supervisor")?
            != fs::canonicalize("/proc/self/exe")
                .map_err(|_| "cannot inspect running supervisor")?
    {
        return Err("runtime command is not the exact owned systemd invocation".into());
    }
    let main_pid = if start { pid } else { 0 };
    Ok(SupervisorIdentity {
        unit: spec.unit.clone(),
        invocation_id: invocation_hex(&unit.invocation_id),
        pid: main_pid,
        start_time_ticks: if main_pid == 0 {
            0
        } else {
            process_start(main_pid)?
        },
    })
}
pub(crate) fn daemon(plan: &HostPlan) -> Result<(String, DaemonIdentity), String> {
    let socket =
        fs::symlink_metadata("/run/docker.sock").map_err(|_| "local Docker socket unavailable")?;
    if !socket.file_type().is_socket() || socket.uid() != 0 || socket.mode() & 0o002 != 0 {
        return Err("unsafe local Docker socket".into());
    }
    let unit = admission_systemd::inspect_unit(&plan.tools.busctl, "docker.service")?;
    let service = unit.service.as_ref().ok_or("Docker service unavailable")?;
    if unit.active_state != "active"
        || unit.sub_state != "running"
        || unit.need_daemon_reload
        || unit.invocation_id == [0; 16]
        || service.main_pid == 0
    {
        return Err("Docker launch prerequisites unavailable".into());
    }
    let start_time_ticks = process_start(service.main_pid)?;
    let info = docker_json(plan, &["info", "--format", "{{json .}}"])?;
    if info
        .get("Runtimes")
        .and_then(Value::as_object)
        .is_none_or(|r| !r.contains_key("runsc"))
    {
        return Err("required OCI runtime unavailable".into());
    }
    let id = info
        .get("ID")
        .and_then(Value::as_str)
        .filter(|v| tentaflake_admission::identifier(v))
        .ok_or("Docker daemon identity unavailable")?
        .to_owned();
    if admission_systemd::inspect_unit(&plan.tools.busctl, "docker.service")? != unit
        || process_start(service.main_pid)? != start_time_ticks
    {
        return Err("Docker daemon changed during observation".into());
    }
    let boot = fs::read_to_string("/proc/sys/kernel/random/boot_id")
        .map_err(|_| "boot identity unavailable")?
        .trim()
        .to_owned();
    if boot.len() != 36 || !boot.bytes().all(|b| b.is_ascii_hexdigit() || b == b'-') {
        return Err("invalid boot identity".into());
    }
    Ok((
        boot,
        DaemonIdentity {
            id,
            invocation_id: invocation_hex(&unit.invocation_id),
            pid: service.main_pid,
            start_time_ticks,
        },
    ))
}

fn readiness(
    plan: &HostPlan,
    path: &Path,
    id: &str,
    generation: &str,
    now: u64,
) -> Result<ReadinessSnapshot, String> {
    let instance = &plan.instances[id];
    let mut result = ReadinessSnapshot {
        generation: plan.policy.generation.clone(),
        sources: Vec::new(),
        evidence: Vec::new(),
    };
    let mut add = |kind, source_generation: String| {
        result.sources.push(ReadinessGeneration {
            kind,
            generation: source_generation.clone(),
        });
        result.evidence.push(ReadinessEvidence {
            instance_id: id.into(),
            kind,
            status: EvidenceStatus::Ready,
            source: EvidenceSource::Live,
            policy_revision: plan.policy.revision.clone(),
            instance_revision: instance.revision.clone(),
            generation: source_generation,
            observed_at: now,
            expires_at: now.saturating_add(5),
        });
    };
    add(EvidenceKind::Runtime, generation.into());
    for volume in plan.volumes.iter().filter(|v| v.spec.instance_id == id) {
        let unit = admission_systemd::inspect_unit(&plan.tools.busctl, &volume.units.owner)?;
        let service = unit.service.as_ref().ok_or("quota owner unavailable")?;
        if unit.active_state != "active"
            || unit.sub_state != "exited"
            || unit.need_daemon_reload
            || unit.job.id != 0
            || unit.invocation_id == [0; 16]
            || service.service_type != "oneshot"
            || !service.remain_after_exit
            || service.main_pid != 0
            || service.control_pid != 0
            || !exec_is(
                &service.exec_start,
                plan,
                path,
                &volume.spec.allocation_id,
                "volume-start",
            )
            || !exec_is(
                &service.exec_stop,
                plan,
                path,
                &volume.spec.allocation_id,
                "volume-stop",
            )
        {
            return Err("quota owner is not the exact active guarded service".into());
        }
        let mut verify = Command::new(&volume.helpers.verify);
        verify.env_clear().stdin(Stdio::null()).arg("--mounted");
        read_output(&mut verify)?;
        let mount = read_output(
            Command::new(&plan.tools.findmnt)
                .env_clear()
                .stdin(Stdio::null())
                .args(["--noheadings", "--raw", "--output", "ID", "--mountpoint"])
                .arg(&volume.spec.mount_path),
        )?;
        let mount_id = std::str::from_utf8(&mount)
            .ok()
            .map(str::trim)
            .filter(|s| !s.is_empty() && s.len() <= 20 && s.bytes().all(|b| b.is_ascii_digit()))
            .ok_or("quota mount identity unavailable")?;
        let backing = fs::symlink_metadata(&volume.spec.image_path)
            .map_err(|_| "quota backing identity unavailable")?;
        if !backing.is_file() || backing.uid() != 0 || backing.len() != volume.spec.bytes {
            return Err("quota backing identity mismatch".into());
        }
        if admission_systemd::inspect_unit(&plan.tools.busctl, &volume.units.owner)? != unit {
            return Err("quota owner changed during observation".into());
        }
        let source_generation = format!(
            "{}:{}:{}:{}",
            invocation_hex(&unit.invocation_id),
            mount_id,
            backing.dev(),
            backing.ino()
        );
        add(
            match volume.spec.kind {
                StorageKind::Workspace => EvidenceKind::WorkspaceQuota,
                StorageKind::State => EvidenceKind::StateQuota,
            },
            source_generation,
        );
    }
    // No synthetic Model/Research/Worker evidence: the core denies requested
    // capabilities until their independently generation-bound collectors exist.
    Ok(result)
}
fn current_authority(
    plan: &HostPlan,
    path: &Path,
    published: &tentaflake_admission::ledger::PublishedAuthority,
) -> Result<(), String> {
    installed(path)?;
    if authority(plan, path) != *published {
        return Err("installed runtime plan differs from published authority".into());
    }
    Ok(())
}
fn timestamp() -> Result<u64, String> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .map_err(|_| "host clock unavailable".into())
}

fn create_args(
    id: &str,
    spec: &RuntimeSpec,
    intent: &RuntimeIntent,
    operation: &str,
) -> Vec<String> {
    let c = &spec.container;
    let mut args: Vec<String> = [
        "create",
        "--pull=never",
        "--restart=no",
        "--name",
        id,
        "--log-driver",
        &c.log_driver,
    ]
    .into_iter()
    .map(str::to_owned)
    .collect();
    for (key, value) in c
        .labels
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .chain([
            (OPERATION_LABEL, operation),
            (INSTANCE_LABEL, id),
            (SPEC_LABEL, &intent.spec_revision),
        ])
    {
        args.extend(["--label".into(), format!("{key}={value}")]);
    }
    for (key, value) in &c.environment {
        args.extend(["--env".into(), format!("{key}={value}")]);
    }
    for (flag, value) in [
        ("--user", &c.user),
        ("--entrypoint", &c.entrypoint),
        ("--workdir", &c.workdir),
        ("--hostname", &c.hostname),
    ] {
        if let Some(value) = value {
            args.extend([flag.into(), value.clone()]);
        }
    }
    for volume in &c.volumes {
        args.extend(["--volume".into(), volume.clone()]);
    }
    for (capability, enabled) in &c.capabilities {
        if *enabled == Some(false) {
            args.extend(["--cap-drop".into(), capability.clone()]);
        }
    }
    args.extend(c.extra_options.iter().cloned());
    args.push(intent.image_id.clone());
    args.extend(c.cmd.iter().cloned());
    args
}
fn string_array(value: Option<&Value>) -> Result<Vec<String>, String> {
    match value {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(Value::Array(values)) if values.len() <= 4096 => values
            .iter()
            .map(|v| {
                v.as_str()
                    .filter(|s| text(s))
                    .map(str::to_owned)
                    .ok_or_else(|| "invalid container string array".into())
            })
            .collect(),
        _ => Err("invalid container string array".into()),
    }
}
fn env_map(values: Vec<String>) -> Result<BTreeMap<String, String>, String> {
    let mut map = BTreeMap::new();
    for value in values {
        let (key, value) = value
            .split_once('=')
            .ok_or("invalid container environment")?;
        if key.is_empty() || map.insert(key.into(), value.into()).is_some() {
            return Err("ambiguous container environment".into());
        }
    }
    Ok(map)
}
fn exact_container(
    plan: &HostPlan,
    id: &str,
    spec: &RuntimeSpec,
    receipt: &Receipt,
    image_config: &Map<String, Value>,
    cid: &str,
    running: bool,
) -> Result<(), String> {
    let observed = docker_json(
        plan,
        &["container", "inspect", "--format", "{{json .}}", cid],
    )?;
    validate_container(
        plan,
        id,
        spec,
        receipt,
        image_config,
        (cid, running),
        &observed,
    )
}

fn validate_container(
    plan: &HostPlan,
    id: &str,
    spec: &RuntimeSpec,
    receipt: &Receipt,
    image_config: &Map<String, Value>,
    expected: (&str, bool),
    observed: &Value,
) -> Result<(), String> {
    let (cid, running) = expected;
    let object = observed
        .as_object()
        .ok_or("invalid container observation")?;
    let dispatch = receipt
        .runtime
        .as_ref()
        .ok_or("receipt has no runtime identity")?;
    let c = &spec.container;
    let config = object
        .get("Config")
        .and_then(Value::as_object)
        .ok_or("container config unavailable")?;
    let host = object
        .get("HostConfig")
        .and_then(Value::as_object)
        .ok_or("container host config unavailable")?;
    let state = object
        .get("State")
        .and_then(Value::as_object)
        .ok_or("container state unavailable")?;
    if cid.len() != 64
        || !cid
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        || object.get("Id").and_then(Value::as_str) != Some(cid)
        || object.get("Name").and_then(Value::as_str) != Some(&format!("/{id}"))
        || object.get("Image").and_then(Value::as_str) != Some(&dispatch.intent.image_id)
        || config.get("Image").and_then(Value::as_str) != Some(&dispatch.intent.image_id)
        || state.get("Running").and_then(Value::as_bool) != Some(running)
        || state.get("Paused").and_then(Value::as_bool) != Some(false)
        || state.get("Restarting").and_then(Value::as_bool) != Some(false)
        || state.get("Dead").and_then(Value::as_bool) != Some(false)
        || !matches!(
            state.get("Status").and_then(Value::as_str),
            Some("running" | "created" | "exited")
        )
    {
        return Err("container identity or lifecycle state differs from its owned receipt".into());
    }
    let mut labels = image_config
        .get("Labels")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    for (key, value) in &c.labels {
        labels.insert(key.clone(), Value::String(value.clone()));
    }
    for (key, value) in [
        (OPERATION_LABEL, receipt.request.operation_id.as_str()),
        (INSTANCE_LABEL, id),
        (SPEC_LABEL, dispatch.intent.spec_revision.as_str()),
    ] {
        labels.insert(key.into(), Value::String(value.into()));
    }
    let mut environment = env_map(string_array(image_config.get("Env"))?)?;
    environment.extend(c.environment.clone());
    let expected_cmd = if c.cmd.is_empty() {
        if c.entrypoint.is_some() {
            vec![]
        } else {
            string_array(image_config.get("Cmd"))?
        }
    } else {
        c.cmd.clone()
    };
    let expected_entrypoint = if let Some(value) = &c.entrypoint {
        vec![value.clone()]
    } else {
        string_array(image_config.get("Entrypoint"))?
    };
    let expected_workdir = c.workdir.as_deref().unwrap_or_else(|| {
        image_config
            .get("WorkingDir")
            .and_then(Value::as_str)
            .unwrap_or("")
    });
    if config.get("Labels") != Some(&Value::Object(labels))
        || env_map(string_array(config.get("Env"))?)? != environment
        || string_array(config.get("Cmd"))? != expected_cmd
        || string_array(config.get("Entrypoint"))? != expected_entrypoint
        || config.get("User").and_then(Value::as_str) != c.user.as_deref()
        || config.get("WorkingDir").and_then(Value::as_str) != Some(expected_workdir)
        || config.get("Hostname").and_then(Value::as_str)
            != Some(c.hostname.as_deref().unwrap_or(&cid[..12]))
        || host.get("AutoRemove").and_then(Value::as_bool) != Some(false)
        || host
            .get("RestartPolicy")
            .and_then(Value::as_object)
            .and_then(|p| p.get("Name"))
            .and_then(Value::as_str)
            != Some("no")
        || host
            .get("LogConfig")
            .and_then(Value::as_object)
            .and_then(|p| p.get("Type"))
            .and_then(Value::as_str)
            != Some(&c.log_driver)
    {
        return Err(
            "container configuration differs from the immutable launch specification".into(),
        );
    }
    let mut binds = string_array(host.get("Binds"))?;
    binds.sort();
    let mut declared_binds = c.volumes.clone();
    declared_binds.sort();
    let declared = containment(plan, id, spec)?;
    if binds != declared_binds
        || crate::live_resource_limits_security(host, "docker", &declared.resources)
            != crate::LiveContainerState::Secure
        || host.get("ReadonlyRootfs").and_then(Value::as_bool) != Some(true)
        || host.get("Privileged").and_then(Value::as_bool) != Some(false)
        || host.get("Runtime").and_then(Value::as_str) != Some("runsc")
        || host.get("NetworkMode").and_then(Value::as_str) != Some("none")
        || crate::live_security_options_security(Some(host)) != crate::LiveContainerState::Secure
        || crate::live_capabilities_security(object, Some(host), "docker")
            != crate::LiveContainerState::Secure
    {
        return Err("created container containment differs from the declared boundary".into());
    }
    if host
        .get("Tmpfs")
        .and_then(Value::as_object)
        .is_none_or(|tmpfs| {
            crate::live_tmpfs_set_security(tmpfs, &declared.tmpfs, "docker")
                != crate::LiveContainerState::Secure
        })
        || crate::live_mounts_security(object.get("Mounts"), &declared.mounts)
            != crate::LiveContainerState::Secure
    {
        return Err("created container mounts differ from the declared boundary".into());
    }
    if running
        && crate::live_observed_security_drift(
            object,
            "docker",
            "none",
            &declared.mounts,
            &declared.tmpfs,
            &declared.resources,
        ) != crate::LiveContainerState::Secure
    {
        return Err("running containment could not be verified".into());
    }
    Ok(())
}

fn unchanged(plan: &HostPlan, expected: &RuntimeIntent) -> Result<(), String> {
    let (boot, observed) = daemon(plan)?;
    if boot != expected.boot_id || observed != expected.daemon {
        return Err("runtime daemon generation changed; claim remains charged".into());
    }
    Ok(())
}
fn notify_ready() -> Result<(), String> {
    // NixOS systemd's host manager owns this fixed socket. Do not send a notice
    // to an inherited arbitrary address, even after authenticating the caller.
    if std::env::var_os("NOTIFY_SOCKET").as_deref()
        != Some(std::ffi::OsStr::new("/run/systemd/notify"))
    {
        return Err("unexpected systemd notification socket".into());
    }
    let socket =
        UnixDatagram::unbound().map_err(|_| "cannot create systemd notification socket")?;
    socket
        .send_to(b"READY=1", "/run/systemd/notify")
        .map_err(|_| "cannot notify systemd readiness")?;
    Ok(())
}

pub(crate) fn lifecycle(path: &Path, id: &str, start: bool) -> Result<u8, String> {
    require_root()?;
    let plan = load_plan(path)?;
    let spec = plan
        .runtimes
        .get(id)
        .ok_or("no declared runtime for instance")?;
    let caller = caller(&plan, path, id, spec, start)?;
    if start {
        start_runtime(&plan, path, id, spec, caller)
    } else {
        stop_runtime(&plan, path, id, spec, &caller)?;
        Ok(0)
    }
}
fn start_runtime(
    plan: &HostPlan,
    path: &Path,
    id: &str,
    spec: &RuntimeSpec,
    supervisor: SupervisorIdentity,
) -> Result<u8, String> {
    let fence = admission_fence::exclusive()?;
    installed(path)?;
    let ledger = Ledger::open(Path::new(LEDGER)).map_err(|e| e.to_string())?;
    let (revision, current) = ledger.runtime_receipt(id).map_err(|e| e.to_string())?;
    let operation = format!("runtime:{}", supervisor.invocation_id);
    let instance = &plan.instances[id];
    let request = current
        .as_ref()
        .filter(|r| r.request.operation_id == operation)
        .map(|r| r.request.clone())
        .unwrap_or(AdmissionRequest {
            schema_version: SCHEMA_VERSION,
            operation_id: operation,
            instance_id: id.into(),
            principal_id: instance.owner_id.clone(),
            expected: ExpectedRevisions {
                policy: plan.policy.revision.clone(),
                instance: instance.revision.clone(),
                ledger: revision,
            },
        });
    let mut image_config = Map::new();
    let mut sources = Vec::new();
    let outcome = ledger
        .claim(&request, |published| {
            current_authority(plan, path, published)?;
            if caller(plan, path, id, spec, true)? != supervisor {
                return Err("runtime supervisor identity changed".into());
            }
            let now = timestamp()?;
            let (boot_id, daemon) = daemon(plan)?;
            let runtime_generation = format!("{boot_id}:{}", daemon.invocation_id);
            let readiness = readiness(plan, path, id, &runtime_generation, now)?;
            sources = readiness.sources.clone();
            let (image_id, config) = image(plan, spec)?;
            image_config = config;
            if spec.provenance.is_some() {
                return Err("runtime provenance readiness collector is not implemented".into());
            }
            let intent = RuntimeIntent {
                boot_id,
                daemon,
                supervisor: supervisor.clone(),
                runtime_generation,
                spec_revision: instance.revision.clone(),
                image: spec.image.clone(),
                image_id,
            };
            Ok(RuntimeContext {
                snapshot: AdmissionSnapshot {
                    schema_version: SCHEMA_VERSION,
                    policy: plan.policy.clone(),
                    instance: instance.clone(),
                    reservations: ledger_snapshot_placeholder(plan),
                    readiness,
                },
                now: timestamp()?,
                intent: Some(intent),
            })
        })
        .map_err(|e| e.to_string())?;
    let mut permit = match outcome {
        ClaimOutcome::New(permit) => permit,
        ClaimOutcome::Existing(_) => {
            return Err("runtime operation already recorded; dispatch is never replayed".into());
        }
        ClaimOutcome::Denied(decision) => {
            return Err(format!(
                "runtime admission denied: {}",
                serde_json::to_string(&decision.reasons)
                    .map_err(|_| "cannot encode admission refusal")?
            ));
        }
    };
    let intent = permit
        .receipt()
        .runtime
        .as_ref()
        .ok_or("new runtime permit has no intent")?
        .intent
        .clone();
    unchanged(plan, &intent)?;
    let args = create_args(id, spec, &intent, &request.operation_id);
    let output = mutate(docker(plan).args(&args), &fence, Duration::from_secs(60))?;
    let cid = std::str::from_utf8(&output)
        .ok()
        .map(str::trim)
        .filter(|v| {
            v.len() == 64
                && v.bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        })
        .ok_or("create did not return one exact container ID; claim remains charged")?;
    exact_container(plan, id, spec, permit.receipt(), &image_config, cid, false)?;
    permit.bind_container(cid).map_err(|e| e.to_string())?;
    refresh_prerequisites(plan, path, id, spec, &intent, &sources)?;
    mutate(
        docker(plan).args(["container", "start", cid]),
        &fence,
        Duration::from_secs(60),
    )?;
    exact_container(plan, id, spec, permit.receipt(), &image_config, cid, true)?;
    refresh_prerequisites(plan, path, id, spec, &intent, &sources)?;
    permit
        .complete(Observation::Running)
        .map_err(|e| e.to_string())?;
    drop(fence);
    notify_ready()?;
    // Readers inherit no admission locks. Journal forwarding has ordinary child
    // lifetime under KillMode=control-group, independent of ledger transactions.
    let mut logs = docker(plan)
        .args(["container", "logs", "--follow", cid])
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn()
        .map_err(|_| "cannot forward container logs")?;
    let mut waiter = docker(plan)
        .args(["container", "wait", cid])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| "cannot wait for owned container")?;
    let mut exit = String::new();
    let read = waiter
        .stdout
        .take()
        .ok_or("container wait pipe unavailable")?
        .take(64)
        .read_to_string(&mut exit);
    if read.is_err() || exit.len() >= 64 {
        let _ = waiter.kill();
    }
    let status = waiter.wait();
    let _ = logs.kill();
    let _ = logs.wait();
    if read.is_err() || !status.is_ok_and(|status| status.success()) {
        return Err("container wait outcome unknown; claim remains charged".into());
    }
    let exit = parse_wait_exit(&exit)?;
    let unit = admission_systemd::inspect_unit(&plan.tools.busctl, &spec.unit)?;
    if unit.active_state == "deactivating"
        && unit.sub_state == "stop"
        && invocation_hex(&unit.invocation_id) == supervisor.invocation_id
        && valid_unit(&unit, plan, path, id, spec)
        && unit
            .service
            .as_ref()
            .is_some_and(|service| service.control_pid != 0)
    {
        // ExecStop owns the transaction. The main waiter must not compete for
        // its fence while systemd is synchronously waiting for that control PID.
        return Ok(0);
    }
    stop_runtime(plan, path, id, spec, &supervisor)?;
    Ok(exit)
}
fn parse_wait_exit(value: &str) -> Result<u8, String> {
    let value = value.strip_suffix('\n').unwrap_or(value);
    if value.is_empty() || value.len() > 3 || !value.bytes().all(|b| b.is_ascii_digit()) {
        return Err("invalid container wait exit status; claim remains charged".into());
    }
    value
        .parse::<u8>()
        .map_err(|_| "invalid container wait exit status; claim remains charged".into())
}

fn refresh_prerequisites(
    plan: &HostPlan,
    path: &Path,
    id: &str,
    spec: &RuntimeSpec,
    intent: &RuntimeIntent,
    original: &[ReadinessGeneration],
) -> Result<(), String> {
    installed(path)?;
    if caller(plan, path, id, spec, true)? != intent.supervisor {
        return Err("runtime supervisor changed during dispatch".into());
    }
    let fresh = readiness(plan, path, id, &intent.runtime_generation, timestamp()?)?;
    if fresh.sources != original {
        return Err("quota generation changed during dispatch; claim remains charged".into());
    }
    unchanged(plan, intent)
}

fn ledger_snapshot_placeholder(plan: &HostPlan) -> tentaflake_admission::ReservationSnapshot {
    // claim replaces this field from the locked durable ledger before decide.
    tentaflake_admission::ReservationSnapshot {
        generation: plan.policy.generation.clone(),
        revision: 0,
        complete: false,
        entries: vec![],
        storage: vec![],
    }
}
fn stop_runtime(
    plan: &HostPlan,
    path: &Path,
    id: &str,
    spec: &RuntimeSpec,
    supervisor: &SupervisorIdentity,
) -> Result<(), String> {
    let fence = admission_fence::exclusive()?;
    installed(path)?;
    let ledger = Ledger::open(Path::new(LEDGER)).map_err(|e| e.to_string())?;
    let status = ledger.publication_status().map_err(|e| e.to_string())?;
    current_authority(plan, path, &status.published)?;
    let (revision, receipt) = ledger.runtime_receipt(id).map_err(|e| e.to_string())?;
    let receipt = receipt.ok_or("no owned runtime receipt")?;
    let dispatch = receipt
        .runtime
        .as_ref()
        .ok_or("runtime receipt has no owned identity")?;
    if dispatch.intent.supervisor.unit != supervisor.unit
        || dispatch.intent.supervisor.invocation_id != supervisor.invocation_id
        || (supervisor.pid != 0 && dispatch.intent.supervisor != *supervisor)
        || dispatch.intent.spec_revision != plan.instances[id].revision
        || dispatch.intent.image != spec.image
    {
        return Err("runtime stop does not own this exact invocation".into());
    }
    if receipt.reservation.state == ReservationState::Retained {
        return Ok(());
    }
    if receipt.reservation.state != ReservationState::Active {
        return Err(
            "uncertain runtime requires explicit fenced recovery; stop cannot free it".into(),
        );
    }
    let cid = dispatch
        .container_id
        .as_deref()
        .ok_or("runtime receipt has no bound CID")?;
    let mut error = None;
    ledger
        .reconcile(&receipt.request.operation_id, revision, |recorded| {
            let result = (|| {
                unchanged(plan, &dispatch.intent)?;
                let (image_id, config) = image(plan, spec)?;
                if image_id != dispatch.intent.image_id {
                    return Err("runtime image changed before stop".into());
                }
                let unit = admission_systemd::inspect_unit(&plan.tools.busctl, &spec.unit)?;
                if !valid_unit(&unit, plan, path, id, spec)
                    || invocation_hex(&unit.invocation_id)
                        != dispatch.intent.supervisor.invocation_id
                {
                    return Err("runtime unit changed before stop".into());
                }
                let main_pid = unit
                    .service
                    .as_ref()
                    .ok_or("runtime service unavailable")?
                    .main_pid;
                if main_pid != 0 && main_pid != dispatch.intent.supervisor.pid {
                    return Err("runtime main process changed before stop".into());
                }
                if main_pid != 0
                    && fs::metadata(format!("/proc/{main_pid}")).is_ok()
                    && process_start(main_pid)? != dispatch.intent.supervisor.start_time_ticks
                {
                    return Err("runtime process generation changed before stop".into());
                }
                let state = docker_json(
                    plan,
                    &[
                        "container",
                        "inspect",
                        "--format",
                        "{{json .State.Running}}",
                        cid,
                    ],
                )?
                .as_bool()
                .ok_or("container running state unavailable")?;
                exact_container(plan, id, spec, recorded, &config, cid, state)?;
                // A successful finite stop is needed even when wait already saw exit.
                mutate(
                    docker(plan).args(["container", "stop", "--time", "30", cid]),
                    &fence,
                    Duration::from_secs(45),
                )?;
                exact_container(plan, id, spec, recorded, &config, cid, false)?;
                unchanged(plan, &dispatch.intent)?;
                mutate(
                    docker(plan).args(["container", "rm", cid]),
                    &fence,
                    Duration::from_secs(30),
                )?;
                unchanged(plan, &dispatch.intent)?;
                Ok::<(), String>(())
            })();
            if let Err(reason) = result {
                error = Some(reason);
                Ok(Observation::Unknown)
            } else {
                Ok(Observation::Stopped)
            }
        })
        .map_err(|e| e.to_string())?;
    if let Some(error) = error {
        return Err(error);
    }
    Ok(())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use serde_json::json;
    use tentaflake_admission::ledger::RuntimeDispatch;
    use tentaflake_admission::{Decision, Reservation};

    fn fixture() -> (HostPlan, RuntimeSpec, Receipt, Map<String, Value>, Value) {
        let plan: HostPlan =
            serde_json::from_value(crate::admission_host::tests::plan_fixture()).unwrap();
        let id = "agent-fixture";
        let instance = &plan.instances[id];
        let r = instance.resources;
        let spec: RuntimeSpec = serde_json::from_value(json!({
            "image": {"kind":"local-artifact", "descriptor":"/nix/store/00000000000000000000000000000000-image.json"},
            "unit":"docker-agent-fixture.service", "provenance":null,
            "container": {
                "serviceName":"docker-agent-fixture", "login":{"username":null,"passwordFile":null,"registry":null},
                "cmd":["sleep","3600"],"labels":{"io.tentaflake.agent":"true"}, "entrypoint":null,
                "environment":{"FIXTURE":"yes"},"environmentFiles":[],"log-driver":"journald", "ports":[],
                "user":"1000:1000", "volumes":["/var/lib/agent-fixture/work+space:/workspace", "/var/lib/agent-fixture:/state"],
                "workdir":"/workspace","dependsOn":[],"hostname":null,"preRunExtraOptions":[],
                "extraOptions":["--read-only","--security-opt=no-new-privileges:true","--security-opt=apparmor=docker-default",
                    "--tmpfs=/tmp:rw,nosuid,nodev,noexec,size=16m", "--tmpfs=/run:rw,nosuid,nodev,noexec,size=8m", "--tmpfs=/var/tmp:rw,nosuid,nodev,noexec,size=16m",
                    format!("--memory={}",r.memory_bytes),format!("--memory-swap={}",r.memory_swap_bytes),
                    format!("--cpus={}.{:09}",r.nano_cpus / 1_000_000_000, r.nano_cpus % 1_000_000_000), format!("--pids-limit={}",r.pids),
                    "--ulimit=nofile=1024:1024",format!("--ulimit=nproc={}:{}",r.pids,r.pids),"--network=none","--runtime=runsc"],
                "autoStart":false,"pull":"never","capabilities":{"ALL":false},"devices":[],"privileged":false,"autoRemoveOnStop":false,"networks":[]
            }
        })).unwrap();
        let reservation = Reservation {
            instance_id: id.into(),
            owner_id: instance.owner_id.clone(),
            instance_revision: instance.revision.clone(),
            state: ReservationState::Unknown,
            resources: r,
        };
        let intent = RuntimeIntent {
            boot_id: "11111111-1111-4111-8111-111111111111".into(),
            daemon: DaemonIdentity {
                id: "fixture-daemon".into(),
                invocation_id: "1".repeat(32),
                pid: 10,
                start_time_ticks: 10,
            },
            supervisor: SupervisorIdentity {
                unit: spec.unit.clone(),
                invocation_id: "2".repeat(32),
                pid: 11,
                start_time_ticks: 11,
            },
            runtime_generation: "fixture-generation".into(),
            spec_revision: instance.revision.clone(),
            image: spec.image.clone(),
            image_id: format!("sha256:{}", "a".repeat(64)),
        };
        let receipt = Receipt {
            request: AdmissionRequest {
                schema_version: SCHEMA_VERSION,
                operation_id: "fixture-operation".into(),
                instance_id: id.into(),
                principal_id: instance.owner_id.clone(),
                expected: ExpectedRevisions {
                    policy: plan.policy.revision.clone(),
                    instance: instance.revision.clone(),
                    ledger: 1,
                },
            },
            decision: Decision {
                allowed: true,
                reasons: vec![],
                reservation: Some(reservation.clone()),
            },
            reservation,
            claimed_revision: 2,
            updated_revision: 2,
            runtime: Some(RuntimeDispatch {
                intent,
                container_id: Some("b".repeat(64)),
            }),
        };
        let image_config = json!({"Cmd":["default"],"Entrypoint":null,"Env":["BASE=1"],"Labels":{"base":"yes"},"WorkingDir":"/"}).as_object().unwrap().clone();
        let observed = json!({
            "Id":"b".repeat(64),"Name":"/agent-fixture","Image":format!("sha256:{}","a".repeat(64)),
            "Config":{"Image":format!("sha256:{}","a".repeat(64)),"Cmd":["sleep","3600"],"Entrypoint":null,"Env":["BASE=1","FIXTURE=yes"],
                "Labels":{"base":"yes","io.tentaflake.agent":"true",OPERATION_LABEL:"fixture-operation",INSTANCE_LABEL:id,SPEC_LABEL:instance.revision},
                "User":"1000:1000","WorkingDir":"/workspace","Hostname":"b".repeat(12)},
            "HostConfig":{"AutoRemove":false,"RestartPolicy":{"Name":"no"},"LogConfig":{"Type":"journald"},"Binds":spec.container.volumes,
                "Memory":r.memory_bytes,"MemorySwap":r.memory_swap_bytes,"NanoCpus":r.nano_cpus,"PidsLimit":r.pids,
                "Ulimits":[{"Name":"nofile","Soft":1024,"Hard":1024},{"Name":"nproc","Soft":r.pids,"Hard":r.pids}],
                "ReadonlyRootfs":true,"Privileged":false,"Runtime":"runsc","NetworkMode":"none","SecurityOpt":["no-new-privileges:true","apparmor=docker-default"],"CapAdd":null,"CapDrop":["ALL"],
                "Tmpfs":{"/tmp":"rw,nosuid,nodev,noexec,size=16m","/run":"rw,nosuid,nodev,noexec,size=8m","/var/tmp":"rw,nosuid,nodev,noexec,size=16m"}},
            "Mounts":[{"Type":"bind","Source":"/var/lib/agent-fixture/work+space","Destination":"/workspace","RW":true},{"Type":"bind","Source":"/var/lib/agent-fixture","Destination":"/state","RW":true}],
            "State":{"Running":false,"Paused":false,"Restarting":false,"Dead":false,"Status":"created"}
        });
        (plan, spec, receipt, image_config, observed)
    }

    pub(crate) fn template_plan_fixture() -> HostPlan {
        let (mut plan, mut spec, _, _, _) = fixture();
        plan.instances
            .get_mut("agent-fixture")
            .unwrap()
            .resources
            .nano_cpus = 1_000_000_000;
        let cpus = spec
            .container
            .extra_options
            .iter_mut()
            .find(|option| option.starts_with("--cpus="))
            .unwrap();
        *cpus = "--cpus=1".into();
        spec.image = RuntimeImage::Registry {
            reference: format!("docker.io/library/debian@sha256:{}", "a".repeat(64)),
        };
        spec.container
            .volumes
            .iter_mut()
            .for_each(|mount| mount.push_str(":rw"));
        plan.runtimes.insert("agent-fixture".into(), spec);
        plan
    }

    #[test]
    fn templates_refuse_source_dependencies_and_extra_mounts() {
        let plan = template_plan_fixture();
        validate_template_source(&plan, "agent-fixture").unwrap();
        for case in 0..5 {
            let mut plan = template_plan_fixture();
            let spec = plan.runtimes.get_mut("agent-fixture").unwrap();
            match case {
                0 => spec.container.depends_on.push("source-seed".into()),
                1 => spec.container.auto_start = true,
                2 => spec.container.hostname = Some("private-source".into()),
                3 => spec
                    .container
                    .volumes
                    .push("/nix/store/private:/private:ro".into()),
                _ => spec.container.volumes[0] = "/var/lib/foreign:/workspace:rw".into(),
            }
            assert!(validate_template_source(&plan, "agent-fixture").is_err());
        }
    }

    #[test]
    fn wait_preserves_workload_failure_and_refuses_ambiguous_output() {
        assert_eq!(parse_wait_exit("0\n").unwrap(), 0);
        assert_eq!(parse_wait_exit("137\n").unwrap(), 137);
        for value in ["", "256", "0\n1\n", " 0", "-1", "private-error"] {
            assert!(parse_wait_exit(value).is_err());
        }
    }

    #[test]
    fn final_projection_binds_resources_and_never_adds_pull_or_remove() {
        let (plan, spec, receipt, _, _) = fixture();
        validate_runtime(&plan, "agent-fixture", &spec).unwrap();
        let args = create_args(
            "agent-fixture",
            &spec,
            &receipt.runtime.as_ref().unwrap().intent,
            &receipt.request.operation_id,
        );
        assert!(args.contains(&"--pull=never".into()));
        assert!(args.contains(&"--restart=no".into()));
        assert!(
            !args
                .iter()
                .any(|v| v == "--rm" || v.starts_with("--env-file"))
        );
        assert!(args.contains(&format!("{OPERATION_LABEL}=fixture-operation")));
        assert!(args.contains(&format!("sha256:{}", "a".repeat(64))));
        let (plan, mut spec, _, _, _) = fixture();
        spec.container.extra_options.push("--memory=1".into());
        assert!(validate_runtime(&plan, "agent-fixture", &spec).is_err());
        let (mut plan, spec, _, _, _) = fixture();
        plan.instances
            .get_mut("agent-fixture")
            .unwrap()
            .resources
            .memory_bytes += 1;
        assert!(validate_runtime(&plan, "agent-fixture", &spec).is_err());
    }

    #[test]
    fn rejects_alternate_authority_and_offline_transport() {
        for alter in [0, 1, 2, 3, 4, 5] {
            let (plan, mut spec, _, _, _) = fixture();
            match alter {
                0 => spec.container.extra_options.push("--privileged".into()),
                1 => spec.container.login.username = Some("root".into()),
                2 => spec
                    .container
                    .labels
                    .insert(OPERATION_LABEL.into(), "forged".into())
                    .map(|_| ())
                    .unwrap_or(()),
                3 => spec
                    .container
                    .volumes
                    .push("/run/docker.sock:/socket:ro".into()),
                4 => spec.container.auto_remove_on_stop = true,
                _ => {
                    spec.image = RuntimeImage::LocalArtifact {
                        descriptor:
                            "/nix/store/00000000000000000000000000000000-directory/descriptor.json"
                                .into(),
                    }
                }
            }
            assert!(validate_runtime(&plan, "agent-fixture", &spec).is_err());
        }
        assert!(
            serde_json::from_value::<RuntimeSpec>(
                json!({"image":null,"unit":"x","container":{},"provenance":null,"unknown":true})
            )
            .is_err()
        );
    }

    #[test]
    fn created_identity_configuration_and_mounts_must_match_before_start() {
        let (plan, spec, receipt, image, observed) = fixture();
        let cid = "b".repeat(64);
        validate_container(
            &plan,
            "agent-fixture",
            &spec,
            &receipt,
            &image,
            (&cid, false),
            &observed,
        )
        .unwrap();
        for (pointer, value) in [
            ("/Id", json!("c".repeat(64))),
            ("/Image", json!(format!("sha256:{}", "c".repeat(64)))),
            (
                "/Config/Labels/io.tentaflake.operation",
                json!("another-operation"),
            ),
            ("/Config/Cmd", json!(["unexpected"])),
            ("/Config/Env", json!(["BASE=1", "FIXTURE=yes", "EXTRA=1"])),
            ("/Config/User", json!("0:0")),
            ("/HostConfig/Memory", json!(1)),
            ("/Mounts/0/Source", json!("/tmp/foreign")),
            ("/HostConfig/Tmpfs/~1run", json!("rw,size=8m")),
        ] {
            let mut changed = observed.clone();
            *changed.pointer_mut(pointer).unwrap() = value;
            assert!(
                validate_container(
                    &plan,
                    "agent-fixture",
                    &spec,
                    &receipt,
                    &image,
                    (&cid, false),
                    &changed
                )
                .is_err(),
                "{pointer}"
            );
        }
    }

    #[test]
    fn whole_unit_rejects_additional_mutators_restart_and_unbounded_cleanup() {
        use crate::admission_systemd::{ServiceObservation, UnitJob};
        let (plan, spec, _, _, _) = fixture();
        let path = Path::new("/nix/store/00000000000000000000000000000000-plan");
        let exec = |action: &str| ExecCommand {
            path: plan.tools.cli.to_string_lossy().into_owned(),
            argv: vec![
                plan.tools.cli.to_string_lossy().into_owned(),
                "admission".into(),
                action.into(),
                path.to_string_lossy().into_owned(),
                "agent-fixture".into(),
            ],
            ignore_failure: false,
            start_realtime: 0,
            start_monotonic: 0,
            exit_realtime: 0,
            exit_monotonic: 0,
            pid: 42,
            code: 0,
            status: 0,
        };
        let unit = UnitObservation {
            id: spec.unit.clone(),
            load_state: "loaded".into(),
            active_state: "activating".into(),
            sub_state: "start".into(),
            job: UnitJob {
                id: 1,
                path: "/org/freedesktop/systemd1/job/1".into(),
            },
            fragment_path: "/etc/systemd/system/docker-agent-fixture.service".into(),
            need_daemon_reload: false,
            invocation_id: [1; 16],
            mount: None,
            service: Some(ServiceObservation {
                main_pid: 42,
                control_pid: 0,
                control_group: "/system.slice/docker-agent-fixture.service".into(),
                exec_start: vec![exec("runtime-start")],
                exec_stop: vec![exec("runtime-stop")],
                service_type: "notify".into(),
                remain_after_exit: false,
                kill_mode: "control-group".into(),
                timeout_start_usec: 300_000_000,
                timeout_stop_usec: 120_000_000,
                restart: "no".into(),
                notify_access: "main".into(),
                exec_start_pre: vec![],
                exec_start_post: vec![],
                exec_stop_post: vec![],
                exec_reload: vec![],
                exec_condition: vec![],
            }),
        };
        assert!(valid_unit(&unit, &plan, path, "agent-fixture", &spec));
        for change in [0, 1, 2, 3] {
            let mut changed = unit.clone();
            let s = changed.service.as_mut().unwrap();
            match change {
                0 => s.exec_stop_post.push(exec("foreign")),
                1 => s.restart = "always".into(),
                2 => s.timeout_stop_usec = u64::MAX,
                _ => s.kill_mode = "process".into(),
            }
            assert!(!valid_unit(&changed, &plan, path, "agent-fixture", &spec));
        }
    }
}
