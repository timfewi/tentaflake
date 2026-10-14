//! Owned offline Docker lifecycle. A recorded intent precedes every create; an
//! exact CID is synced before start. Uncertain outcomes retain the entire claim.
use crate::admission_host::{
    HostPlan, load_plan, open_store_entry, open_store_file, read_store_document, store_path,
};
use crate::admission_model::{self, ModelSpec, ReadyModel, Stage};
use crate::admission_research::{self, ReadyResearch, ResearchSpec};
use crate::admission_runtime::{LEDGER, authority, installed, require_root};
use crate::admission_systemd::{self, ExecCommand, UnitObservation};
use serde::Deserialize;
use serde_json::{Map, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::io::{self, Read, Seek};
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
    AdmissionRequest, AdmissionSnapshot, Capability, EvidenceKind, EvidenceSource, EvidenceStatus,
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
    pub model: Option<ModelSpec>,
    pub research: Option<ResearchSpec>,
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
    let worker_results =
        crate::admission_worker::validate_results_mounts(plan, id, &limits.mounts)?;
    let network_options: Vec<_> = c
        .extra_options
        .iter()
        .filter(|option| {
            option.starts_with("--network")
                || option.starts_with("--dns")
                || option.starts_with("--sysctl")
        })
        .map(String::as_str)
        .collect();
    if let Some(model) = &spec.model {
        admission_model::validate(id, model)?;
        if !instance.requested_capabilities.contains(&Capability::Model)
            || plan.tools.nft.is_none()
            || plan.tools.ip.is_none()
            || c.networks != [model.network.name.clone()]
            || c.environment_files != [model.environment_file.to_string_lossy().into_owned()]
            || network_options
                != [
                    "--dns=127.0.0.1",
                    "--dns-option=attempts:1",
                    "--sysctl=net.ipv6.conf.all.disable_ipv6=1",
                ]
            || c.environment.keys().any(|key| model_environment_key(key))
        {
            return Err("model OCI transport differs from its owned broker declaration".into());
        }
    } else if !c.networks.is_empty()
        || !c.environment_files.is_empty()
        || network_options != ["--network=none"]
    {
        return Err("OCI external transport has no live capability collector".into());
    }
    if let Some(research) = &spec.research {
        admission_research::validate(id, research)?;
        if !instance
            .requested_capabilities
            .contains(&Capability::Research)
            || c.environment
                .get("TENTAFLAKE_RESEARCH_COMMAND")
                .map(String::as_str)
                != research.client.executable.to_str()
            || c.environment
                .get("TENTAFLAKE_RESEARCH_SOCKET")
                .map(String::as_str)
                != research.client.socket_destination.to_str()
        {
            return Err("Research OCI transport differs from its owned declaration".into());
        }
        let mut required: Vec<_> = research
            .client
            .closure_paths
            .iter()
            .map(|path| (path.as_path(), path.as_path()))
            .collect();
        required.push((
            research
                .relay
                .socket_path
                .parent()
                .ok_or("invalid Research relay path")?,
            Path::new("/run/tentaflake-research"),
        ));
        for (source, destination) in required {
            if !limits.mounts.iter().any(|mount| {
                !mount.writable
                    && Path::new(&mount.source) == source
                    && Path::new(&mount.destination) == destination
            }) || limits.mounts.iter().any(|mount| {
                let path = Path::new(&mount.destination);
                path != destination
                    && (path.starts_with(destination) || destination.starts_with(path))
            }) {
                return Err(
                    "Research client or relay mount differs from its owned declaration".into(),
                );
            }
        }
    } else if c.environment.contains_key("TENTAFLAKE_RESEARCH_COMMAND")
        || c.environment.contains_key("TENTAFLAKE_RESEARCH_SOCKET")
    {
        return Err("Research OCI environment has no owned transport declaration".into());
    }
    // Every read-only source needs its concrete owner, regardless of requested
    // capabilities. Worker validation above supplies its one exact result pair.
    if limits.mounts.iter().any(|mount| {
        !mount.writable
            && !store_path(Path::new(&mount.source))
            && spec.research.as_ref().is_none_or(|research| {
                research.relay.socket_path.parent() != Some(Path::new(&mount.source))
                    || mount.destination != "/run/tentaflake-research"
            })
            && worker_results.is_none_or(|expected| {
                expected.source != mount.source || expected.destination != mount.destination
            })
    }) {
        return Err("OCI read-only mount has no immutable source or owned capability".into());
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
    let before = daemon(&plan)?;
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
    if daemon(&plan)? != before {
        return Err("Docker daemon changed during image acquisition".into());
    }
    Ok(0)
}

fn invocation_hex(value: &[u8; 16]) -> String {
    value.iter().map(|byte| format!("{byte:02x}")).collect()
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
        && service.exec_reload.is_empty()
        && service.exec_condition.is_empty()
        && exec_is(&service.exec_start, plan, path, id, "runtime-start")
        && exec_is(&service.exec_stop, plan, path, id, "runtime-stop")
        && exec_is(&service.exec_stop_post, plan, path, id, "runtime-finish")
}
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Lifecycle {
    Start,
    Stop,
    Finish,
}
impl Lifecycle {
    fn accepts_caller(self, unit: &UnitObservation, pid: u32) -> bool {
        let Some(service) = &unit.service else {
            return false;
        };
        if pid == 0 {
            return false;
        }
        match self {
            Self::Start => {
                unit.active_state == "activating"
                    && unit.sub_state == "start"
                    && service.main_pid == pid
            }
            Self::Stop => {
                unit.active_state == "deactivating"
                    && unit.sub_state == "stop"
                    && service.control_pid == pid
            }
            Self::Finish => {
                unit.active_state == "deactivating"
                    && unit.sub_state == "stop-post"
                    && service.control_pid == pid
                    && service.main_pid == 0
            }
        }
    }
}
fn caller(
    plan: &HostPlan,
    path: &Path,
    id: &str,
    spec: &RuntimeSpec,
    action: Lifecycle,
) -> Result<SupervisorIdentity, String> {
    let unit = admission_systemd::inspect_unit(&plan.tools.busctl, &spec.unit)?;
    let service = unit.service.as_ref().ok_or("runtime service unavailable")?;
    let pid = std::process::id();
    if !valid_unit(&unit, plan, path, id, spec)
        || !action.accepts_caller(&unit, pid)
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
    let main_pid = if action == Lifecycle::Start { pid } else { 0 };
    Ok(SupervisorIdentity {
        unit: spec.unit.clone(),
        invocation_id: invocation_hex(&unit.invocation_id),
        pid: main_pid,
        start_time_ticks: if main_pid == 0 {
            0
        } else {
            admission_systemd::process_start(main_pid)?
        },
    })
}
fn classic_image_store(info: &Value) -> bool {
    // Moby's containerd store always reports a driver-type row and identifies
    // images by manifest/index digest. Our immutable archive contract identifies
    // the config digest. Check live status as well as the immutable settings so
    // a daemon-reload cannot make an old running store appear compatible.
    info.get("Driver")
        .and_then(Value::as_str)
        .is_some_and(|driver| !driver.is_empty())
        && info
            .get("DriverStatus")
            .and_then(Value::as_array)
            .is_some_and(|rows| {
                rows.len() <= 128
                    && rows.iter().all(|row| {
                        row.as_array().is_some_and(|fields| {
                            fields.len() == 2
                                && fields.iter().all(|field| {
                                    field.as_str().is_some_and(|value| value.len() <= 4096)
                                })
                                && fields[0].as_str() != Some("driver-type")
                        })
                    })
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
    crate::admission_bootstrap::daemon_config(plan, &unit)?;
    let start_time_ticks = admission_systemd::process_start(service.main_pid)?;
    let info = docker_json(plan, &["info", "--format", "{{json .}}"])?;
    if !classic_image_store(&info) {
        return Err("admission requires the running classic Docker image store; containerd snapshotter or unavailable driver status cannot establish config-digest identity".into());
    }
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
        || admission_systemd::process_start(service.main_pid)? != start_time_ticks
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

fn append_readiness(
    plan: &HostPlan,
    id: &str,
    result: &mut ReadinessSnapshot,
    kind: EvidenceKind,
    generation: String,
    observed_at: u64,
    expires_at: u64,
) {
    result.sources.push(ReadinessGeneration {
        kind,
        generation: generation.clone(),
    });
    result.evidence.push(ReadinessEvidence {
        instance_id: id.into(),
        kind,
        status: EvidenceStatus::Ready,
        source: EvidenceSource::Live,
        policy_revision: plan.policy.revision.clone(),
        instance_revision: plan.instances[id].revision.clone(),
        generation,
        observed_at,
        expires_at,
    });
}

fn collected_readiness(
    plan: &HostPlan,
    id: &str,
    generation: &str,
    now: u64,
    collected: &StartEvidence,
) -> ReadinessSnapshot {
    let mut result = ReadinessSnapshot {
        generation: plan.policy.generation.clone(),
        sources: Vec::new(),
        evidence: Vec::new(),
    };
    let mut add = |kind, source_generation: String, observed_at, expires_at| {
        append_readiness(
            plan,
            id,
            &mut result,
            kind,
            source_generation,
            observed_at,
            expires_at,
        );
    };
    add(
        EvidenceKind::Runtime,
        generation.into(),
        now,
        now.saturating_add(5),
    );
    if let Some(model) = &collected.model {
        add(
            EvidenceKind::Model,
            model.generation.clone(),
            model.observed_at,
            model.valid_until,
        );
    }
    if let Some(research) = &collected.research {
        add(
            EvidenceKind::Research,
            research.generation.clone(),
            research.observed_at,
            research.valid_until,
        );
    }
    result
}

fn readiness(
    plan: &HostPlan,
    path: &Path,
    id: &str,
    generation: &str,
    now: u64,
    collected: &StartEvidence,
) -> Result<ReadinessSnapshot, String> {
    crate::admission_worker::observe_results(plan, id)?;
    let mut result = collected_readiness(plan, id, generation, now, collected);
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
        append_readiness(
            plan,
            id,
            &mut result,
            match volume.spec.kind {
                StorageKind::Workspace => EvidenceKind::WorkspaceQuota,
                StorageKind::State => EvidenceKind::StateQuota,
            },
            source_generation,
            now,
            now.saturating_add(5),
        );
    }
    // Worker still requires an independently generation-bound collector.
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
    model: Option<&ReadyModel>,
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
    if let Some(model) = model {
        args.extend([
            "--network".into(),
            model.network_id.clone(),
            "--env-file=/proc/self/fd/0".into(),
        ]);
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

fn model_environment_key(key: &str) -> bool {
    matches!(
        key,
        "TENTAFLAKE_BROKER_TOKEN"
            | "OPENAI_API_KEY"
            | "OPENAI_BASE_URL"
            | "TENTAFLAKE_LLM_BASE_URL"
    )
}

#[derive(Clone, Copy)]
enum ModelExpectation<'a> {
    Admit(Option<&'a ReadyModel>),
    // Exact owned-CID retirement must remain possible when its broker is down.
    Retire,
}

#[derive(Clone, Copy)]
struct ContainerExpectation<'a> {
    cid: &'a str,
    running: bool,
    model: ModelExpectation<'a>,
}

fn projected_environment(
    spec: &RuntimeSpec,
    image: &Map<String, Value>,
    observed: &Map<String, Value>,
    expectation: ModelExpectation<'_>,
) -> Result<BTreeMap<String, String>, String> {
    let mut environment = env_map(string_array(image.get("Env"))?)?;
    environment.extend(spec.container.environment.clone());
    match (&spec.model, expectation) {
        (Some(_), ModelExpectation::Admit(Some(model))) => {
            environment.extend(model.environment.clone())
        }
        (Some(spec), ModelExpectation::Retire) => {
            let actual = env_map(string_array(observed.get("Env"))?)?;
            let token = actual
                .get("TENTAFLAKE_BROKER_TOKEN")
                .ok_or("owned model environment unavailable")?;
            if token.len() != 64
                || !token
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
                || actual.get("OPENAI_API_KEY") != Some(token)
            {
                return Err("owned model environment changed".into());
            }
            environment.insert("TENTAFLAKE_BROKER_TOKEN".into(), token.clone());
            environment.insert("OPENAI_API_KEY".into(), token.clone());
            let endpoint = format!("http://{}:{}/v1", spec.network.gateway, spec.port);
            environment.insert("OPENAI_BASE_URL".into(), endpoint.clone());
            environment.insert("TENTAFLAKE_LLM_BASE_URL".into(), endpoint);
        }
        (None, ModelExpectation::Admit(None) | ModelExpectation::Retire) => {}
        _ => return Err("model launch has no prepared transport".into()),
    }
    Ok(environment)
}

fn model_attachment(
    spec: &ModelSpec,
    object: &Map<String, Value>,
    host: &Map<String, Value>,
    expected: ContainerExpectation<'_>,
) -> Result<(), String> {
    let mode = host
        .get("NetworkMode")
        .and_then(Value::as_str)
        .ok_or("model network mode unavailable")?;
    if mode.len() != 64
        || !mode
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err("model network is not bound by full ID".into());
    }
    if let ModelExpectation::Admit(prepared) = expected.model
        && prepared.is_none_or(|ready| ready.network_id != mode)
    {
        return Err("model network differs from the collected identity".into());
    }
    let networks = object
        .get("NetworkSettings")
        .and_then(|s| s.get("Networks"))
        .and_then(Value::as_object)
        .ok_or("model network attachment unavailable")?;
    let endpoint = networks
        .get(&spec.network.name)
        .and_then(Value::as_object)
        .ok_or("model endpoint unavailable")?;
    let network_id = endpoint
        .get("NetworkID")
        .and_then(Value::as_str)
        .ok_or("model network ID unavailable")?;
    if networks.len() != 1
        || (network_id != mode && (expected.running || !network_id.is_empty()))
        || string_array(host.get("Dns"))? != ["127.0.0.1"]
        || string_array(host.get("DnsOptions"))? != ["attempts:1"]
        || !string_array(host.get("DnsSearch"))?.is_empty()
        || host.get("Sysctls") != Some(&serde_json::json!({"net.ipv6.conf.all.disable_ipv6":"1"}))
        || endpoint.get("GlobalIPv6Address").and_then(Value::as_str) != Some("")
        || endpoint.get("IPv6Gateway").and_then(Value::as_str) != Some("")
    {
        return Err("model network boundary differs from its declaration".into());
    }
    if expected.running {
        let gateway = spec
            .network
            .gateway
            .parse::<std::net::Ipv4Addr>()
            .map_err(|_| "invalid model gateway")?;
        let address = std::net::Ipv4Addr::from(
            u32::from(gateway)
                .checked_add(1)
                .ok_or("invalid model subnet")?,
        )
        .to_string();
        let endpoint_id = endpoint
            .get("EndpointID")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if endpoint_id.len() != 64
            || !endpoint_id
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            || endpoint.get("IPAddress").and_then(Value::as_str) != Some(address.as_str())
            || endpoint.get("IPPrefixLen").and_then(Value::as_u64) != Some(30)
            || endpoint.get("Gateway").and_then(Value::as_str)
                != Some(spec.network.gateway.as_str())
        {
            return Err("running model endpoint differs from its isolated subnet".into());
        }
    }
    Ok(())
}

fn exact_container(
    plan: &HostPlan,
    id: &str,
    spec: &RuntimeSpec,
    receipt: &Receipt,
    image_config: &Map<String, Value>,
    expected: ContainerExpectation<'_>,
) -> Result<(), String> {
    let observed = docker_json(
        plan,
        &[
            "container",
            "inspect",
            "--format",
            "{{json .}}",
            expected.cid,
        ],
    )?;
    validate_container(plan, id, spec, receipt, image_config, expected, &observed)
}

fn validate_container(
    plan: &HostPlan,
    id: &str,
    spec: &RuntimeSpec,
    receipt: &Receipt,
    image_config: &Map<String, Value>,
    expected: ContainerExpectation<'_>,
    observed: &Value,
) -> Result<(), String> {
    let ContainerExpectation { cid, running, .. } = expected;
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
    let environment = projected_environment(spec, image_config, config, expected.model)?;
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
        || (spec.model.is_none() && host.get("NetworkMode").and_then(Value::as_str) != Some("none"))
        || crate::live_security_options_security(Some(host)) != crate::LiveContainerState::Secure
        || crate::live_capabilities_security(object, Some(host), "docker")
            != crate::LiveContainerState::Secure
    {
        return Err("created container containment differs from the declared boundary".into());
    }
    if let Some(model) = &spec.model {
        model_attachment(model, object, host, expected)?;
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
            spec.model
                .as_ref()
                .map_or("none", |model| &model.network.name),
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

#[derive(PartialEq, Eq)]
struct StoreMountIdentity {
    device: u64,
    inode: u64,
    uid: u32,
    gid: u32,
    mode: u32,
    size: u64,
    modified: (i64, i64),
    changed: (i64, i64),
}

impl StoreMountIdentity {
    fn read(file: &File) -> Result<Self, String> {
        let metadata = file
            .metadata()
            .map_err(|_| "immutable mount metadata unavailable")?;
        Ok(Self {
            device: metadata.dev(),
            inode: metadata.ino(),
            uid: metadata.uid(),
            gid: metadata.gid(),
            mode: metadata.mode(),
            size: metadata.len(),
            modified: (metadata.mtime(), metadata.mtime_nsec()),
            changed: (metadata.ctime(), metadata.ctime_nsec()),
        })
    }
}

struct StoreMount {
    path: PathBuf,
    file: File,
    identity: StoreMountIdentity,
}

// The opener authenticates the selected entry and every path component. Hold
// the original FD while Docker uses the named source; never replace this proof
// with a fresh, unrelated snapshot after create. This does not inspect a mounted
// tree recursively or establish the kernel identity of the container's mount.
fn capture_store_mounts(
    mounts: &[crate::DeclaredMount],
    open: impl Fn(&Path) -> Result<File, String>,
) -> Result<Vec<StoreMount>, String> {
    mounts
        .iter()
        .filter(|mount| !mount.writable && store_path(Path::new(&mount.source)))
        .map(|mount| {
            let path = PathBuf::from(&mount.source);
            let file = open(&path)?;
            let identity = StoreMountIdentity::read(&file)?;
            Ok(StoreMount {
                path,
                file,
                identity,
            })
        })
        .collect()
}

fn recheck_store_mounts(
    mounts: &[StoreMount],
    open: impl Fn(&Path) -> Result<File, String>,
) -> Result<(), String> {
    for mount in mounts {
        let current = open(&mount.path)?;
        if mount.identity != StoreMountIdentity::read(&mount.file)?
            || mount.identity != StoreMountIdentity::read(&current)?
        {
            return Err(
                "immutable mount source changed during dispatch; claim remains charged".into(),
            );
        }
    }
    Ok(())
}

pub(crate) fn lifecycle(path: &Path, id: &str, action: Lifecycle) -> Result<u8, String> {
    require_root()?;
    let plan = load_plan(path)?;
    let spec = plan
        .runtimes
        .get(id)
        .ok_or("no declared runtime for instance")?;
    let caller = caller(&plan, path, id, spec, action)?;
    match action {
        Lifecycle::Start => start_runtime(&plan, path, id, spec, caller),
        Lifecycle::Stop => {
            stop_runtime(&plan, path, id, spec, &caller)?;
            Ok(0)
        }
        Lifecycle::Finish => {
            finish_runtime(&plan, path, id, spec, &caller)?;
            Ok(0)
        }
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
    let mut evidence = StartEvidence::default();
    let outcome = ledger
        .claim(&request, |published| {
            current_authority(plan, path, published)?;
            if caller(plan, path, id, spec, Lifecycle::Start)? != supervisor {
                return Err("runtime supervisor identity changed".into());
            }
            evidence.immutable_mounts =
                capture_store_mounts(&containment(plan, id, spec)?.mounts, open_store_entry)?;
            let (boot_id, daemon) = daemon(plan)?;
            let runtime_generation = format!("{boot_id}:{}", daemon.invocation_id);
            evidence.model = spec
                .model
                .as_ref()
                .map(|model| admission_model::collect(plan, id, model, Stage::BeforeCreate))
                .transpose()?;
            evidence.research = spec
                .research
                .as_ref()
                .map(|research| admission_research::collect(plan, id, research))
                .transpose()?;
            let now = timestamp()?;
            let readiness = readiness(plan, path, id, &runtime_generation, now, &evidence)?;
            evidence.sources = readiness.sources.clone();
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
    let args = create_args(
        id,
        spec,
        &intent,
        &request.operation_id,
        evidence.model.as_ref(),
    );
    let mut create = docker(plan);
    create.args(&args);
    if let Some(model) = &evidence.model {
        let mut file = model
            .environment_file
            .try_clone()
            .map_err(|_| "cannot retain prepared model environment")?;
        file.rewind()
            .map_err(|_| "cannot rewind prepared model environment")?;
        create.stdin(Stdio::from(file));
    }
    refresh_prerequisites(
        plan,
        path,
        id,
        spec,
        &intent,
        &evidence,
        Stage::BeforeCreate,
    )?;
    let output = mutate(&mut create, &fence, Duration::from_secs(60))?;
    let cid = std::str::from_utf8(&output)
        .ok()
        .map(str::trim)
        .filter(|v| {
            v.len() == 64
                && v.bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        })
        .ok_or("create did not return one exact container ID; claim remains charged")?;
    let created = ContainerExpectation {
        cid,
        running: false,
        model: ModelExpectation::Admit(evidence.model.as_ref()),
    };
    exact_container(plan, id, spec, permit.receipt(), &image_config, created)?;
    permit.bind_container(cid).map_err(|e| e.to_string())?;
    refresh_prerequisites(
        plan,
        path,
        id,
        spec,
        &intent,
        &evidence,
        Stage::Created(cid),
    )?;
    mutate(
        docker(plan).args(["container", "start", cid]),
        &fence,
        Duration::from_secs(60),
    )?;
    exact_container(
        plan,
        id,
        spec,
        permit.receipt(),
        &image_config,
        ContainerExpectation {
            running: true,
            ..created
        },
    )?;
    refresh_prerequisites(
        plan,
        path,
        id,
        spec,
        &intent,
        &evidence,
        Stage::Running(cid),
    )?;
    permit
        .complete(Observation::Running)
        .map_err(|e| e.to_string())?;
    drop(evidence);
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

#[derive(Default)]
struct StartEvidence {
    immutable_mounts: Vec<StoreMount>,
    sources: Vec<ReadinessGeneration>,
    model: Option<ReadyModel>,
    research: Option<ReadyResearch>,
}

fn refresh_prerequisites(
    plan: &HostPlan,
    path: &Path,
    id: &str,
    spec: &RuntimeSpec,
    intent: &RuntimeIntent,
    original: &StartEvidence,
    stage: Stage<'_>,
) -> Result<(), String> {
    installed(path)?;
    if caller(plan, path, id, spec, Lifecycle::Start)? != intent.supervisor {
        return Err("runtime supervisor changed during dispatch".into());
    }
    let model = spec
        .model
        .as_ref()
        .map(|model| admission_model::collect(plan, id, model, stage))
        .transpose()?;
    match (&original.model, &model) {
        (Some(original), Some(fresh)) if original.same_source(fresh) => {}
        (None, None) => {}
        _ => return Err("model transport changed during dispatch; claim remains charged".into()),
    }
    let research = spec
        .research
        .as_ref()
        .map(|research| admission_research::collect(plan, id, research))
        .transpose()?;
    match (&original.research, &research) {
        (Some(original), Some(fresh)) if original.same_source(fresh) => {}
        (None, None) => {}
        _ => {
            return Err("Research transport changed during dispatch; claim remains charged".into());
        }
    }
    let collected = StartEvidence {
        model,
        research,
        ..Default::default()
    };
    let fresh = readiness(
        plan,
        path,
        id,
        &intent.runtime_generation,
        timestamp()?,
        &collected,
    )?;
    if fresh.sources != original.sources {
        return Err("readiness generation changed during dispatch; claim remains charged".into());
    }
    unchanged(plan, intent)?;
    // Source reopens can consume a short Research lease, just like daemon
    // observations. The timestamp must be sampled only after those reads.
    finish_prerequisites(
        &fresh,
        || recheck_store_mounts(&original.immutable_mounts, open_store_entry),
        timestamp,
    )
}

fn finish_prerequisites(
    fresh: &ReadinessSnapshot,
    recheck_sources: impl FnOnce() -> Result<(), String>,
    now: impl FnOnce() -> Result<u64, String>,
) -> Result<(), String> {
    recheck_sources()?;
    let now = now()?;
    if fresh
        .evidence
        .iter()
        .any(|evidence| evidence.observed_at > now || evidence.expires_at <= now)
    {
        return Err("readiness expired during dispatch; claim remains charged".into());
    }
    Ok(())
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
                    && admission_systemd::process_start(main_pid)?
                        != dispatch.intent.supervisor.start_time_ticks
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
                exact_container(
                    plan,
                    id,
                    spec,
                    recorded,
                    &config,
                    ContainerExpectation {
                        cid,
                        running: state,
                        model: ModelExpectation::Retire,
                    },
                )?;
                // A successful finite stop is needed even when wait already saw exit.
                mutate(
                    docker(plan).args(["container", "stop", "--time", "30", cid]),
                    &fence,
                    Duration::from_secs(45),
                )?;
                exact_container(
                    plan,
                    id,
                    spec,
                    recorded,
                    &config,
                    ContainerExpectation {
                        cid,
                        running: false,
                        model: ModelExpectation::Retire,
                    },
                )?;
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

// ExecStop is skipped when the main process fails in systemd's running state.
// ExecStopPost can only mark this invocation uncertain. It has no daemon
// mutation/absence authority and must never touch another invocation's claim.
fn finish_observation(
    receipt: &Receipt,
    plan: &HostPlan,
    id: &str,
    spec: &RuntimeSpec,
    supervisor: &SupervisorIdentity,
) -> Result<Option<Observation>, String> {
    let Some(dispatch) = &receipt.runtime else {
        return Ok(None);
    };
    if dispatch.intent.supervisor.invocation_id != supervisor.invocation_id {
        return Ok(None);
    }
    if dispatch.intent.supervisor.unit != supervisor.unit
        || supervisor.unit != spec.unit
        || receipt.request.instance_id != id
        || dispatch.intent.spec_revision != plan.instances[id].revision
        || dispatch.intent.image != spec.image
    {
        return Err("runtime finish does not own this exact invocation".into());
    }
    Ok(match receipt.reservation.state {
        ReservationState::Active | ReservationState::Reserved => Some(Observation::Unknown),
        ReservationState::Unknown | ReservationState::Retained => None,
    })
}

fn finish_runtime(
    plan: &HostPlan,
    path: &Path,
    id: &str,
    spec: &RuntimeSpec,
    supervisor: &SupervisorIdentity,
) -> Result<(), String> {
    let _fence = admission_fence::exclusive()?;
    installed(path)?;
    let ledger = Ledger::open(Path::new(LEDGER)).map_err(|e| e.to_string())?;
    let status = ledger.publication_status().map_err(|e| e.to_string())?;
    current_authority(plan, path, &status.published)?;
    let (revision, receipt) = ledger.runtime_receipt(id).map_err(|e| e.to_string())?;
    let Some(receipt) = receipt else {
        return Ok(());
    };
    if finish_observation(&receipt, plan, id, spec, supervisor)?.is_none() {
        return Ok(());
    }
    ledger
        .reconcile(&receipt.request.operation_id, revision, |recorded| {
            // Recheck the caller and historical identity under the ledger lock.
            // Unknown preserves all charges, bound CID and dispatch history.
            if caller(plan, path, id, spec, Lifecycle::Finish)? != *supervisor {
                return Err("runtime finish supervisor changed".into());
            }
            finish_observation(recorded, plan, id, spec, supervisor)?
                .ok_or_else(|| "runtime finish claim changed".into())
        })
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use serde_json::json;
    use std::cell::Cell;
    use std::ffi::OsStr;
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt, symlink};
    use std::sync::atomic::{AtomicU64, Ordering};
    use tentaflake_admission::ledger::RuntimeDispatch;
    use tentaflake_admission::protected_path::{open_at, open_relative};
    use tentaflake_admission::{Decision, Reservation};

    const STORE_MOUNT: &str = "/nix/store/00000000000000000000000000000000-mount-fixture";

    struct StoreFixture {
        root: PathBuf,
        anchor: File,
    }

    impl StoreFixture {
        fn new(directory: bool) -> Self {
            static SEQUENCE: AtomicU64 = AtomicU64::new(0);
            let root = std::env::temp_dir().join(format!(
                "tentaflake-store-mount-{}-{}",
                std::process::id(),
                SEQUENCE.fetch_add(1, Ordering::Relaxed),
            ));
            fs::DirBuilder::new().mode(0o700).create(&root).unwrap();
            fs::DirBuilder::new()
                .mode(0o700)
                .create(root.join("parent"))
                .unwrap();
            let source = root.join("parent/source");
            if directory {
                fs::create_dir(&source).unwrap();
                // Internal closure links are not mount-source path components.
                symlink("/unavailable-closure-target", source.join("link")).unwrap();
                fs::set_permissions(&source, fs::Permissions::from_mode(0o555)).unwrap();
            } else {
                fs::write(&source, b"fixture").unwrap();
                fs::set_permissions(&source, fs::Permissions::from_mode(0o444)).unwrap();
            }
            let anchor = open_at(
                libc::AT_FDCWD,
                root.as_os_str(),
                libc::O_RDONLY | libc::O_DIRECTORY,
            )
            .unwrap();
            Self { root, anchor }
        }

        fn open(&self, path: &Path) -> Result<File, String> {
            assert_eq!(path, Path::new(STORE_MOUNT));
            // Only this test maps the logical store name to its own protected
            // anchor. Production always uses open_store_entry's full root walk.
            let parent = open_relative(&self.anchor, Path::new("parent"), unsafe {
                libc::geteuid()
            })?;
            open_at(parent.as_raw_fd(), OsStr::new("source"), libc::O_RDONLY)
        }

        fn capture(&self) -> Vec<StoreMount> {
            capture_store_mounts(
                &[crate::DeclaredMount {
                    source: STORE_MOUNT.into(),
                    destination: "/reference".into(),
                    writable: false,
                }],
                |path| self.open(path),
            )
            .unwrap()
        }
    }

    impl Drop for StoreFixture {
        fn drop(&mut self) {
            for relative in [
                "parent/source",
                "parent/previous",
                "previous/source",
                "previous",
            ] {
                let path = self.root.join(relative);
                if fs::symlink_metadata(&path).is_ok_and(|m| m.is_dir()) {
                    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
                }
            }
            fs::remove_dir_all(&self.root).unwrap();
        }
    }

    #[test]
    fn immutable_mount_capture_is_runtime_only_and_selects_readonly_store_entries() {
        let (plan, mut spec, _, _, _) = fixture();
        let fixture = StoreFixture::new(false);
        spec.container
            .volumes
            .push(format!("{STORE_MOUNT}:/reference:ro"));
        // This logical store path does not exist in the host store. Validation
        // remains pure; only the runtime observation opens mount sources.
        validate_runtime(&plan, "agent-fixture", &spec).unwrap();
        let mut mounts = containment(&plan, "agent-fixture", &spec).unwrap().mounts;
        mounts.push(crate::DeclaredMount {
            source: "/run/unrelated-capability".into(),
            destination: "/unrelated".into(),
            writable: false,
        });
        let opens = Cell::new(0);
        let captured = capture_store_mounts(&mounts, |path| {
            opens.set(opens.get() + 1);
            fixture.open(path)
        })
        .unwrap();
        assert_eq!(captured.len(), 1);
        assert_eq!(opens.get(), 1);
        recheck_store_mounts(&captured, |path| fixture.open(path)).unwrap();
        fs::remove_file(fixture.root.join("parent/source")).unwrap();
        assert!(capture_store_mounts(&mounts, |path| fixture.open(path)).is_err());
        assert!(recheck_store_mounts(&captured, |path| fixture.open(path)).is_err());
    }

    #[test]
    fn immutable_mount_rechecks_keep_the_original_file_or_directory_identity() {
        for directory in [false, true] {
            let fixture = StoreFixture::new(directory);
            let captured = fixture.capture();
            for _ in 0..3 {
                recheck_store_mounts(&captured, |path| fixture.open(path)).unwrap();
            }
            fs::rename(
                fixture.root.join("parent/source"),
                fixture.root.join("parent/previous"),
            )
            .unwrap();
            let source = fixture.root.join("parent/source");
            if directory {
                fs::create_dir(&source).unwrap();
                fs::set_permissions(source, fs::Permissions::from_mode(0o555)).unwrap();
            } else {
                fs::write(&source, b"fixture").unwrap();
                fs::set_permissions(source, fs::Permissions::from_mode(0o444)).unwrap();
            }
            assert!(recheck_store_mounts(&captured, |path| fixture.open(path)).is_err());
            // A new snapshot is valid on its own, but cannot replace the one
            // admitted before dispatch.
            recheck_store_mounts(&fixture.capture(), |path| fixture.open(path)).unwrap();
        }
    }

    #[test]
    fn immutable_mount_rechecks_refuse_ancestor_replacement_symlinks_and_in_place_change() {
        for case in ["parent", "symlink", "permissions", "contents"] {
            let fixture = StoreFixture::new(false);
            let source = fixture.root.join("parent/source");
            // Hold a real writer opened before immutable permissions, modeling
            // a stale producer FD without any production permission exception.
            fs::set_permissions(&source, fs::Permissions::from_mode(0o600)).unwrap();
            let writer = fs::OpenOptions::new().write(true).open(&source).unwrap();
            fs::set_permissions(&source, fs::Permissions::from_mode(0o444)).unwrap();
            let captured = fixture.capture();
            match case {
                "parent" => {
                    fs::rename(fixture.root.join("parent"), fixture.root.join("previous")).unwrap();
                    fs::DirBuilder::new()
                        .mode(0o700)
                        .create(fixture.root.join("parent"))
                        .unwrap();
                    fs::write(&source, b"fixture").unwrap();
                    fs::set_permissions(&source, fs::Permissions::from_mode(0o444)).unwrap();
                }
                "symlink" => {
                    fs::rename(&source, fixture.root.join("previous")).unwrap();
                    symlink(fixture.root.join("previous"), &source).unwrap();
                }
                "permissions" => {
                    fs::set_permissions(&source, fs::Permissions::from_mode(0o644)).unwrap()
                }
                "contents" => writer.set_len(64).unwrap(),
                _ => unreachable!(),
            }
            assert!(
                recheck_store_mounts(&captured, |path| fixture.open(path)).is_err(),
                "{case}"
            );
        }
    }

    #[test]
    fn image_store_requires_classic_live_status_even_with_new_declared_settings() {
        let classic = json!({"Driver":"overlay2", "DriverStatus":[["Backing Filesystem","extfs"],["Supports d_type","true"]]});
        assert!(classic_image_store(&classic));
        assert!(classic_image_store(
            &json!({"Driver":"vfs","DriverStatus":[]})
        ));
        for status in [
            json!([["driver-type", "io.containerd.snapshotter.v1"]]),
            json!([["driver-type", "unknown"]]),
            json!([
                ["Backing Filesystem", "extfs"],
                ["driver-type", "io.containerd.snapshotter.v1"]
            ]),
            Value::Null,
            json!({}),
            json!([[]]),
            json!([["key"]]),
            json!([["key", false]]),
        ] {
            let mut observed = classic.clone();
            observed["DriverStatus"] = status;
            assert!(!classic_image_store(&observed));
        }
        assert!(!classic_image_store(&json!({"Driver":"overlay2"})));
        assert!(!classic_image_store(&json!({"DriverStatus":[]})));
    }

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

    fn worker_results_fixture() -> (HostPlan, RuntimeSpec) {
        let (mut plan, mut spec, _, _, _) = fixture();
        plan.instances
            .get_mut("agent-fixture")
            .unwrap()
            .requested_capabilities = vec![tentaflake_admission::Capability::Worker];
        let store = "/nix/store/00000000000000000000000000000000-worker";
        plan.workers.insert(
            "agent-fixture".into(),
            serde_json::from_value(json!({
                "executable": format!("{store}/bin/tentaflake-worker"),
                "config": format!("{store}/worker.json"),
                "unit": "tentaflake-worker-agent-fixture.service",
                "watcher": "tentaflake-worker-agent-fixture.path",
                "inbox": "/var/lib/agent-fixture/work+space/.tentaflake-worker/inbox",
                "stateDir": "/var/lib/tentaflake-worker-agent-fixture"
            }))
            .unwrap(),
        );
        plan.worker_image = Some(
            serde_json::from_value(json!({
                "config": format!("{store}/image.json"),
                "unit": "tentaflake-worker-image.service"
            }))
            .unwrap(),
        );
        crate::admission_worker::validate_plan(&plan).unwrap();
        spec.container.volumes.push(
            "/var/lib/tentaflake-worker-agent-fixture/results:/run/tentaflake-worker/results:ro"
                .into(),
        );
        (plan, spec)
    }

    #[test]
    fn worker_results_binding_requires_the_exact_owned_readonly_mount() {
        let (plan, spec) = worker_results_fixture();
        validate_runtime(&plan, "agent-fixture", &spec).unwrap();
        let mut accepted = Vec::new();
        for case in [
            "missing",
            "foreign-source",
            "wrong-destination",
            "alias",
            "writable",
            "undeclared",
        ] {
            let (mut plan, mut spec) = worker_results_fixture();
            spec.container.volumes.pop();
            match case {
                "missing" => {}
                "foreign-source" => spec.container.volumes.push(
                    "/var/lib/tentaflake-worker-foreign/results:/run/tentaflake-worker/results:ro".into(),
                ),
                "wrong-destination" => spec.container.volumes.push(
                    "/var/lib/tentaflake-worker-agent-fixture/results:/run/other-results:ro".into(),
                ),
                "alias" => spec.container.volumes.extend([
                    "/var/lib/tentaflake-worker-agent-fixture/results:/run/tentaflake-worker/results:ro".into(),
                    "/var/lib/tentaflake-worker-agent-fixture/results:/run/other-results:ro".into(),
                ]),
                // This source has a valid writable quota. That must not let it
                // replace the separate, read-only Worker result capability.
                "writable" => spec.container.volumes.push(
                    "/var/lib/agent-fixture/work+space:/run/tentaflake-worker/results:rw".into(),
                ),
                "undeclared" => {
                    plan.workers.clear();
                    plan.worker_image = None;
                    spec.container.volumes.push(
                        "/var/lib/tentaflake-worker-agent-fixture/results:/run/tentaflake-worker/results:ro".into(),
                    );
                }
                _ => unreachable!(),
            }
            if validate_runtime(&plan, "agent-fixture", &spec).is_ok() {
                accepted.push(case);
            }
        }
        assert!(
            accepted.is_empty(),
            "accepted invalid Worker results bindings: {accepted:?}"
        );
    }

    #[test]
    fn worker_results_cannot_be_attached_under_a_different_capability_request() {
        let (mut plan, spec) = worker_results_fixture();
        plan.instances
            .get_mut("agent-fixture")
            .unwrap()
            .requested_capabilities = vec![tentaflake_admission::Capability::Model];
        assert!(validate_runtime(&plan, "agent-fixture", &spec).is_err());
    }

    #[test]
    fn worker_results_reject_overlapping_sources_and_destinations_alongside_the_valid_pair() {
        for alias in [
            "/var/lib:/all-state:ro",
            "/var/lib/tentaflake-worker-agent-fixture:/private-worker:ro",
            "/var/lib/tentaflake-worker-agent-fixture/results/job:/one-job:ro",
            "/var/lib/tentaflake-worker-foreign/results:/other-worker:ro",
            "/var/lib/agent-fixture:/run/tentaflake-worker:ro",
            "/var/lib/agent-fixture:/run/tentaflake-worker/results/shadow:ro",
        ] {
            let (plan, mut spec) = worker_results_fixture();
            spec.container.volumes.push(alias.into());
            assert!(
                validate_runtime(&plan, "agent-fixture", &spec).is_err(),
                "{alias}"
            );
        }
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

    fn enable_model(plan: &mut HostPlan, spec: &mut RuntimeSpec) -> ReadyModel {
        let model: ModelSpec = serde_json::from_value(json!({
            "schemaVersion":1,"unit":"tentaflake-broker-llm-agent-fixture.service",
            "executable":"/nix/store/00000000000000000000000000000000-broker/bin/tentaflake-broker",
            "configuration":"/nix/store/00000000000000000000000000000000-broker.json",
            "environmentFile":"/run/tentaflake-broker/agent-fixture/agent.env",
            "network":{"name":"tf-agent-fixture","bridge":"tfb-12345678","subnet":"10.203.40.0/30","gateway":"10.203.40.1"},
            "port":7810,"firewallTable":"tentaflake-broker-12345678","firewallPorts":[7810]
        })).unwrap();
        spec.container.networks = vec![model.network.name.clone()];
        spec.container.environment_files = vec![model.environment_file.to_str().unwrap().into()];
        spec.container
            .extra_options
            .retain(|option| option != "--network=none");
        spec.container.extra_options.extend(
            [
                "--dns=127.0.0.1",
                "--dns-option=attempts:1",
                "--sysctl=net.ipv6.conf.all.disable_ipv6=1",
            ]
            .map(str::to_string),
        );
        plan.instances
            .get_mut("agent-fixture")
            .unwrap()
            .requested_capabilities = vec![Capability::Model];
        plan.tools.nft = Some("/nix/store/00000000000000000000000000000000-nft/bin/nft".into());
        plan.tools.ip = Some("/nix/store/00000000000000000000000000000000-ip/bin/ip".into());
        let ready = admission_model::fixture(&model);
        spec.model = Some(model);
        ready
    }

    fn enable_research(plan: &mut HostPlan, runtime: &mut RuntimeSpec) {
        let research = admission_research::tests::spec();
        plan.instances
            .get_mut("agent-fixture")
            .unwrap()
            .requested_capabilities
            .push(Capability::Research);
        runtime.container.environment.insert(
            "TENTAFLAKE_RESEARCH_COMMAND".into(),
            research.client.executable.to_string_lossy().into_owned(),
        );
        runtime.container.environment.insert(
            "TENTAFLAKE_RESEARCH_SOCKET".into(),
            research
                .client
                .socket_destination
                .to_string_lossy()
                .into_owned(),
        );
        for path in &research.client.closure_paths {
            runtime
                .container
                .volumes
                .push(format!("{0}:{0}:ro", path.display()));
        }
        runtime
            .container
            .volumes
            .push("/run/tentaflake-research/agent-fixture:/run/tentaflake-research:ro".into());
        runtime.research = Some(research);
    }

    #[test]
    fn research_projection_requires_the_exact_client_and_relay_mounts() {
        for change in 0..10 {
            let (mut plan, mut runtime, _, _, _) = fixture();
            enable_research(&mut plan, &mut runtime);
            validate_runtime(&plan, "agent-fixture", &runtime).unwrap();
            match change {
                0 => runtime.research = None,
                1 => plan.instances.get_mut("agent-fixture").unwrap().requested_capabilities.clear(),
                2 => { runtime.container.environment.remove("TENTAFLAKE_RESEARCH_COMMAND"); }
                3 => { runtime.container.environment.insert("TENTAFLAKE_RESEARCH_SOCKET".into(), "/run/foreign/socket".into()); }
                4 => { runtime.container.volumes.pop(); }
                5 => { *runtime.container.volumes.last_mut().unwrap() = "/run/tentaflake-research/foreign:/run/tentaflake-research:ro".into(); }
                6 => { runtime.container.volumes.remove(2); }
                7 => runtime.container.volumes.push("/nix/store/00000000000000000000000000000000-shadow:/run/tentaflake-research/socket:ro".into()),
                8 => runtime.container.volumes.push("/run/private:/private:ro".into()),
                _ => {
                    let destination = runtime.research.as_ref().unwrap().client.executable.display().to_string();
                    runtime.container.volumes.push(format!("/nix/store/00000000000000000000000000000000-shadow:{destination}:ro"));
                }
            }
            assert!(
                validate_runtime(&plan, "agent-fixture", &runtime).is_err(),
                "change {change}"
            );
        }
    }

    #[test]
    fn readonly_authority_is_closed_for_every_capability_combination() {
        for requested in 0..8 {
            for extra in [
                "/srv/private:/private:ro",
                "/nix/store:/all-store:ro",
                "/nix/store/00000000000000000000000000000000-source/../other:/alias:ro",
                "/run/tentaflake-research/foreign:/foreign-relay:ro",
                "/run/tentaflake-research/agent-fixture:/relay-alias:ro",
                "/var/lib/tentaflake-worker-agent-fixture/results:/results-alias:ro",
            ] {
                let (mut plan, mut spec) = if requested & 4 != 0 {
                    worker_results_fixture()
                } else {
                    let (plan, spec, _, _, _) = fixture();
                    (plan, spec)
                };
                if requested & 1 != 0 {
                    enable_model(&mut plan, &mut spec);
                    if requested & 4 != 0 {
                        plan.instances
                            .get_mut("agent-fixture")
                            .unwrap()
                            .requested_capabilities
                            .push(Capability::Worker);
                    }
                }
                if requested & 2 != 0 {
                    enable_research(&mut plan, &mut spec);
                }
                spec.container
                    .volumes
                    .push(format!("{STORE_MOUNT}:/reference:ro"));
                validate_runtime(&plan, "agent-fixture", &spec).unwrap();
                // A valid mount declaration creates neither grants nor live
                // Worker evidence; aggregate admission still owns those gates.
                assert!(plan.policy.grants.is_empty());
                spec.container.volumes.push(extra.into());
                assert!(
                    validate_runtime(&plan, "agent-fixture", &spec).is_err(),
                    "capability bits {requested}, source {extra}"
                );
            }
        }
    }

    #[test]
    fn later_collectors_cannot_renew_model_readiness() {
        let (mut plan, mut spec, _, _, _) = fixture();
        let model = enable_model(&mut plan, &mut spec);
        let collected = StartEvidence {
            model: Some(model),
            ..Default::default()
        };
        // Research and filesystem collection can finish long after the Model
        // probe. Snapshot assembly must retain that probe's original window.
        for later in [14, 20] {
            let fresh = collected_readiness(&plan, "agent-fixture", "runtime", later, &collected);
            let model = fresh
                .evidence
                .iter()
                .find(|e| e.kind == EvidenceKind::Model)
                .unwrap();
            assert_eq!((model.observed_at, model.expires_at), (10, 15));
            assert_eq!(
                finish_prerequisites(&fresh, || Ok(()), || Ok(later)).is_ok(),
                later == 14
            );
        }
        let fresh = collected_readiness(&plan, "agent-fixture", "runtime", 14, &collected);
        assert!(finish_prerequisites(&fresh, || Ok(()), || Ok(9)).is_err());
    }

    #[test]
    fn final_lease_sample_follows_held_source_revalidation() {
        let fixture = StoreFixture::new(false);
        let mounts = fixture.capture();
        let mut fresh = ReadinessSnapshot {
            generation: "fixture".into(),
            sources: vec![],
            evidence: vec![ReadinessEvidence {
                instance_id: "agent-fixture".into(),
                kind: EvidenceKind::Research,
                status: EvidenceStatus::Ready,
                source: EvidenceSource::Live,
                policy_revision: "fixture".into(),
                instance_revision: "fixture".into(),
                generation: "lease".into(),
                observed_at: 10,
                expires_at: 11,
            }],
        };
        for kind in [EvidenceKind::Model, EvidenceKind::Research] {
            fresh.evidence[0].kind = kind;
            let clock = Cell::new(10);
            let rechecked = Cell::new(false);
            let result = finish_prerequisites(
                &fresh,
                || {
                    recheck_store_mounts(&mounts, |path| fixture.open(path))?;
                    rechecked.set(true);
                    clock.set(11);
                    Ok(())
                },
                || {
                    assert!(rechecked.get());
                    Ok(clock.get())
                },
            );
            assert_eq!(
                result.unwrap_err(),
                "readiness expired during dispatch; claim remains charged"
            );
            finish_prerequisites(
                &fresh,
                || recheck_store_mounts(&mounts, |path| fixture.open(path)),
                || Ok(10),
            )
            .unwrap();
            assert!(finish_prerequisites(&fresh, || Ok(()), || Ok(9)).is_err());
        }
        fs::rename(
            fixture.root.join("parent/source"),
            fixture.root.join("parent/previous"),
        )
        .unwrap();
        fs::write(fixture.root.join("parent/source"), b"replacement").unwrap();
        assert!(
            finish_prerequisites(
                &fresh,
                || { recheck_store_mounts(&mounts, |path| fixture.open(path)) },
                || panic!("source refusal must precede the clock sample")
            )
            .is_err()
        );
    }

    #[test]
    fn model_projection_requires_request_and_exact_owned_transport() {
        for alter in 0..9 {
            let (mut plan, mut spec, receipt, _, _) = fixture();
            let ready = enable_model(&mut plan, &mut spec);
            validate_runtime(&plan, "agent-fixture", &spec).unwrap();
            let args = create_args(
                "agent-fixture",
                &spec,
                &receipt.runtime.as_ref().unwrap().intent,
                &receipt.request.operation_id,
                Some(&ready),
            );
            assert!(
                args.windows(2)
                    .any(|pair| pair == ["--network", &ready.network_id])
            );
            assert!(args.iter().any(|arg| arg == "--env-file=/proc/self/fd/0"));
            assert!(
                !args
                    .iter()
                    .any(|arg| arg.contains(&ready.environment["TENTAFLAKE_BROKER_TOKEN"]))
            );
            match alter {
                0 => spec.model = None,
                1 => plan
                    .instances
                    .get_mut("agent-fixture")
                    .unwrap()
                    .requested_capabilities
                    .clear(),
                2 => spec.container.networks[0] = "foreign-network".into(),
                3 => {
                    spec.container.environment_files[0] =
                        "/run/tentaflake-broker/foreign/agent.env".into()
                }
                4 => {
                    spec.container
                        .environment
                        .insert("OPENAI_API_KEY".into(), "declarative-secret".into());
                }
                5 => spec
                    .container
                    .extra_options
                    .push("--dns-search=example.test".into()),
                6 => plan.tools.nft = None,
                7 => spec.model.as_mut().unwrap().unit = "foreign.service".into(),
                _ => spec
                    .container
                    .volumes
                    .push("/run/private:/private:ro".into()),
            }
            assert!(
                validate_runtime(&plan, "agent-fixture", &spec).is_err(),
                "alteration {alter}"
            );
        }
    }

    #[test]
    fn model_start_binds_prepared_secret_and_network_but_retirement_needs_no_broker() {
        let (mut plan, mut spec, receipt, image, mut observed) = fixture();
        let ready = enable_model(&mut plan, &mut spec);
        let model = spec.model.as_ref().unwrap();
        let mut env = vec!["BASE=1".to_owned(), "FIXTURE=yes".to_owned()];
        env.extend(
            ready
                .environment
                .iter()
                .map(|(key, value)| format!("{key}={value}")),
        );
        observed["Config"]["Env"] = json!(env);
        observed["AppArmorProfile"] = json!("docker-default");
        observed["HostConfig"]["NetworkMode"] = json!(ready.network_id);
        observed["HostConfig"]["Dns"] = json!(["127.0.0.1"]);
        observed["HostConfig"]["DnsOptions"] = json!(["attempts:1"]);
        observed["HostConfig"]["DnsSearch"] = json!([]);
        observed["HostConfig"]["Sysctls"] = json!({"net.ipv6.conf.all.disable_ipv6":"1"});
        observed["HostConfig"]["PublishAllPorts"] = json!(false);
        observed["HostConfig"]["PortBindings"] = json!({});
        observed["NetworkSettings"] = json!({"Ports":{},"Networks":{model.network.name.clone():{
            "NetworkID":ready.network_id,"EndpointID":"c".repeat(64),"IPAddress":"10.203.40.2","IPPrefixLen":30,
            "Gateway":model.network.gateway,"GlobalIPv6Address":"","IPv6Gateway":""
        }}});
        let cid = "b".repeat(64);
        let created = ContainerExpectation {
            cid: &cid,
            running: false,
            model: ModelExpectation::Admit(Some(&ready)),
        };
        validate_container(
            &plan,
            "agent-fixture",
            &spec,
            &receipt,
            &image,
            created,
            &observed,
        )
        .unwrap();
        let unprepared = ContainerExpectation {
            model: ModelExpectation::Admit(None),
            ..created
        };
        assert!(
            validate_container(
                &plan,
                "agent-fixture",
                &spec,
                &receipt,
                &image,
                unprepared,
                &observed
            )
            .is_err()
        );
        observed["State"]["Running"] = json!(true);
        observed["State"]["Status"] = json!("running");
        let running = ContainerExpectation {
            running: true,
            ..created
        };
        validate_container(
            &plan,
            "agent-fixture",
            &spec,
            &receipt,
            &image,
            running,
            &observed,
        )
        .unwrap();
        let retiring = ContainerExpectation {
            model: ModelExpectation::Retire,
            ..running
        };
        validate_container(
            &plan,
            "agent-fixture",
            &spec,
            &receipt,
            &image,
            retiring,
            &observed,
        )
        .unwrap();
        for (pointer, value) in [
            ("/HostConfig/NetworkMode", json!("d".repeat(64))),
            (
                "/NetworkSettings/Networks/tf-agent-fixture/NetworkID",
                json!("d".repeat(64)),
            ),
            (
                "/NetworkSettings/Networks/tf-agent-fixture/IPAddress",
                json!("10.203.40.1"),
            ),
            (
                "/NetworkSettings/Networks/tf-agent-fixture/Gateway",
                json!("10.203.41.1"),
            ),
            (
                "/HostConfig/Sysctls/net.ipv6.conf.all.disable_ipv6",
                json!("0"),
            ),
            ("/HostConfig/Dns", json!(["10.203.40.1"])),
            ("/Config/Env", json!(["BASE=1", "OPENAI_API_KEY=wrong"])),
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
                    running,
                    &changed
                )
                .is_err(),
                "{pointer}"
            );
            assert!(
                validate_container(
                    &plan,
                    "agent-fixture",
                    &spec,
                    &receipt,
                    &image,
                    retiring,
                    &changed
                )
                .is_err(),
                "{pointer}"
            );
        }
        let mut rotated = observed.clone();
        let old_token = &ready.environment["TENTAFLAKE_BROKER_TOKEN"];
        let replacement = if old_token == &"a".repeat(64) {
            "d".repeat(64)
        } else {
            "a".repeat(64)
        };
        for value in rotated["Config"]["Env"].as_array_mut().unwrap() {
            let entry = value.as_str().unwrap();
            if entry.starts_with("TENTAFLAKE_BROKER_TOKEN=") || entry.starts_with("OPENAI_API_KEY=")
            {
                *value = json!(entry.replace(old_token, &replacement));
            }
        }
        assert!(
            validate_container(
                &plan,
                "agent-fixture",
                &spec,
                &receipt,
                &image,
                running,
                &rotated
            )
            .is_err()
        );
        validate_container(
            &plan,
            "agent-fixture",
            &spec,
            &receipt,
            &image,
            retiring,
            &rotated,
        )
        .unwrap();
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
            None,
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
            ContainerExpectation {
                cid: &cid,
                running: false,
                model: ModelExpectation::Admit(None),
            },
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
                    ContainerExpectation {
                        cid: &cid,
                        running: false,
                        model: ModelExpectation::Admit(None)
                    },
                    &changed
                )
                .is_err(),
                "{pointer}"
            );
        }
    }

    #[test]
    fn finish_only_marks_the_exact_unfinished_invocation_unknown() {
        let (plan, spec, mut receipt, _, _) = fixture();
        let mut supervisor = receipt.runtime.as_ref().unwrap().intent.supervisor.clone();
        supervisor.pid = 0;
        supervisor.start_time_ticks = 0;
        receipt.reservation.state = ReservationState::Active;
        for state in [ReservationState::Active, ReservationState::Reserved] {
            receipt.reservation.state = state;
            assert_eq!(
                finish_observation(&receipt, &plan, "agent-fixture", &spec, &supervisor).unwrap(),
                Some(Observation::Unknown)
            );
        }
        for state in [ReservationState::Unknown, ReservationState::Retained] {
            receipt.reservation.state = state;
            assert_eq!(
                finish_observation(&receipt, &plan, "agent-fixture", &spec, &supervisor).unwrap(),
                None
            );
        }
        receipt.reservation.state = ReservationState::Active;
        let mut later = supervisor.clone();
        later.invocation_id = "3".repeat(32);
        assert_eq!(
            finish_observation(&receipt, &plan, "agent-fixture", &spec, &later).unwrap(),
            None
        );
        for change in 0..4 {
            let mut wrong = receipt.clone();
            match change {
                0 => {
                    wrong.runtime.as_mut().unwrap().intent.supervisor.unit =
                        "docker-foreign.service".into()
                }
                1 => wrong.request.instance_id = "foreign".into(),
                2 => wrong.runtime.as_mut().unwrap().intent.spec_revision = "foreign".into(),
                _ => {
                    wrong.runtime.as_mut().unwrap().intent.image = RuntimeImage::Registry {
                        reference: format!("example@sha256:{}", "b".repeat(64)),
                    }
                }
            }
            assert!(
                finish_observation(&wrong, &plan, "agent-fixture", &spec, &supervisor).is_err()
            );
        }
        receipt.runtime = None;
        assert_eq!(
            finish_observation(&receipt, &plan, "agent-fixture", &spec, &supervisor).unwrap(),
            None
        );
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
                exec_stop_post: vec![exec("runtime-finish")],
                exec_reload: vec![],
                exec_condition: vec![],
            }),
        };
        assert!(valid_unit(&unit, &plan, path, "agent-fixture", &spec));
        assert!(Lifecycle::Start.accepts_caller(&unit, 42));
        assert!(!Lifecycle::Finish.accepts_caller(&unit, 42));
        let mut finishing = unit.clone();
        finishing.active_state = "deactivating".into();
        finishing.sub_state = "stop-post".into();
        finishing.service.as_mut().unwrap().main_pid = 0;
        finishing.service.as_mut().unwrap().control_pid = 52;
        assert!(Lifecycle::Finish.accepts_caller(&finishing, 52));
        assert!(!Lifecycle::Finish.accepts_caller(&finishing, 42));
        assert!(!Lifecycle::Finish.accepts_caller(&finishing, 0));
        assert!(!Lifecycle::Stop.accepts_caller(&finishing, 52));
        finishing.service.as_mut().unwrap().main_pid = 42;
        assert!(!Lifecycle::Finish.accepts_caller(&finishing, 52));
        finishing.service.as_mut().unwrap().main_pid = 0;
        finishing.sub_state = "stop".into();
        assert!(!Lifecycle::Finish.accepts_caller(&finishing, 52));
        assert!(Lifecycle::Stop.accepts_caller(&finishing, 52));
        for change in [0, 1, 2, 3, 4] {
            let mut changed = unit.clone();
            let s = changed.service.as_mut().unwrap();
            match change {
                0 => s.exec_stop_post.push(exec("foreign")),
                1 => s.restart = "always".into(),
                2 => s.timeout_stop_usec = u64::MAX,
                3 => s.kill_mode = "process".into(),
                _ => s.exec_stop_post.clear(),
            }
            assert!(!valid_unit(&changed, &plan, path, "agent-fixture", &spec));
        }
    }
}
