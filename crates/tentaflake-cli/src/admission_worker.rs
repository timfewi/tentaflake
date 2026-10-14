//! Worker maintenance guards. Fencing is not a Worker capability or resource grant.
use crate::admission_host::{
    HostPlan, load_plan, open_store_file, read_store_document, store_path,
};
use crate::admission_runtime::{LEDGER, authority, installed, stopped};
use crate::admission_systemd::{self, ExecCommand, UnitObservation};
use serde::Deserialize;
use std::collections::BTreeSet;
use std::fs::{self, File};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;
use tentaflake_admission::host_fence;
use tentaflake_admission::ledger::{Ledger, PublicationStatus, WorkerImageLoadOutcome};
use tentaflake_admission::protected_path::{open_relative, open_root};
use tentaflake_admission::{Capability, StorageKind};

const INSTALLED: &str = "/etc/tentaflake/admission.json";
const IMAGE_UNIT: &str = "tentaflake-worker-image.service";
const RESULTS_DESTINATION: &str = "/run/tentaflake-worker/results";

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(crate) struct WorkerSpec {
    executable: PathBuf,
    config: PathBuf,
    unit: String,
    watcher: String,
    inbox: PathBuf,
    state_dir: PathBuf,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(crate) struct ImageSpec {
    config: PathBuf,
    unit: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct ImageConfig {
    schema_version: u32,
    backend: String,
    runtime: PathBuf,
    archive: PathBuf,
    enforced: bool,
}

fn worker_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 48
        && id.as_bytes()[0].is_ascii_alphanumeric()
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        && !matches!(id, "image" | "state-volumes")
}

pub(crate) fn validate_plan(plan: &HostPlan) -> Result<(), String> {
    if plan.workers.len() > 255 || plan.workers.is_empty() != plan.worker_image.is_none() {
        return Err("invalid Worker maintenance inventory".into());
    }
    for (id, spec) in &plan.workers {
        if !worker_id(id)
            || !store_path(&spec.executable)
            || !store_path(&spec.config)
            || spec.executable.file_name() != Some("tentaflake-worker".as_ref())
            || spec.unit != format!("tentaflake-worker-{id}.service")
            || spec.watcher != format!("tentaflake-worker-{id}.path")
            || spec.state_dir != Path::new(&format!("/var/lib/tentaflake-worker-{id}"))
            || !spec.inbox.starts_with("/var/lib")
            || !spec.inbox.ends_with(".tentaflake-worker/inbox")
            || spec
                .inbox
                .components()
                .any(|p| matches!(p, std::path::Component::ParentDir))
        {
            return Err("invalid owned Worker configuration or unit".into());
        }
    }
    if plan
        .worker_image
        .as_ref()
        .is_some_and(|image| image.unit != IMAGE_UNIT || !store_path(&image.config))
    {
        return Err("invalid Worker image loader".into());
    }
    Ok(())
}

fn results_worker<'a>(plan: &'a HostPlan, id: &str) -> Result<Option<&'a WorkerSpec>, String> {
    let instance = plan
        .instances
        .get(id)
        .ok_or("Worker instance is undeclared")?;
    if !instance
        .requested_capabilities
        .contains(&Capability::Worker)
    {
        return Ok(None);
    }
    plan.workers
        .get(id)
        .map(Some)
        .ok_or_else(|| "requested Worker has no owned declaration".into())
}

/// Pure declaration check; neither a capability grant nor live Worker evidence.
pub(crate) fn validate_results_mounts(
    plan: &HostPlan,
    id: &str,
    mounts: &[crate::DeclaredMount],
) -> Result<(), String> {
    let worker = results_worker(plan, id)?;
    let expected = worker.map(|spec| spec.state_dir.join("results"));
    let destination = Path::new(RESULTS_DESTINATION);
    let mut found = false;
    for mount in mounts {
        let source = Path::new(&mount.source);
        let target = Path::new(&mount.destination);
        // Protect all reserved Worker state names, including undeclared ones,
        // and aliases of their ancestors such as /var/lib. This is not a
        // general read-only mount allowlist or a filesystem alias proof.
        let worker_source = Path::new("/var/lib").starts_with(source)
            || source
                .strip_prefix("/var/lib")
                .ok()
                .and_then(|p| p.components().next())
                .is_some_and(|c| {
                    c.as_os_str()
                        .as_encoded_bytes()
                        .starts_with(b"tentaflake-worker-")
                });
        if worker_source || target.starts_with(destination) || destination.starts_with(target) {
            if found
                || expected.as_deref() != Some(source)
                || target != destination
                || mount.writable
            {
                return Err("Worker results require one exact owned read-only mount".into());
            }
            found = true;
        }
    }
    if found != worker.is_some() {
        return Err("requested Worker results mount is missing".into());
    }
    Ok(())
}

fn check_results_config(
    plan: &HostPlan,
    id: &str,
    spec: &WorkerSpec,
    value: &serde_json::Value,
) -> Result<u32, String> {
    let workspace = plan
        .volumes
        .iter()
        .find(|volume| volume.spec.instance_id == id && volume.spec.kind == StorageKind::Workspace)
        .ok_or("Worker results have no owned workspace")?;
    if workspace.spec.owner_uid == 0
        || workspace.spec.owner_gid == 0
        || value.get("container_uid").and_then(|v| v.as_u64())
            != Some(u64::from(workspace.spec.owner_uid))
        || value.get("container_gid").and_then(|v| v.as_u64())
            != Some(u64::from(workspace.spec.owner_gid))
        || spec.inbox != workspace.spec.mount_path.join(".tentaflake-worker/inbox")
    {
        return Err("Worker results ownership differs from the runtime workspace".into());
    }
    Ok(workspace.spec.owner_gid)
}

fn results_directories(
    owner_uid: u32,
    group_gid: u32,
    reopen_state: impl Fn() -> Result<File, String>,
) -> Result<(), String> {
    let identity = |file: &File, results: bool| {
        let metadata = file
            .metadata()
            .map_err(|_| "Worker results metadata unavailable")?;
        let mode = metadata.mode() & 0o7777;
        if !metadata.is_dir()
            || metadata.uid() != owner_uid
            || metadata.gid() != group_gid
            || !(mode == 0o750 || (results && mode == 0o2750))
        {
            return Err("Worker results directory ownership or mode mismatch");
        }
        Ok((
            metadata.dev(),
            metadata.ino(),
            metadata.uid(),
            metadata.gid(),
            metadata.mode(),
        ))
    };
    let state = reopen_state()?;
    let before_state = identity(&state, false)?;
    let results = open_relative(&state, Path::new("results"), owner_uid)?;
    let before_results = identity(&results, true)?;
    let current_state = reopen_state()?;
    let current_results = open_relative(&current_state, Path::new("results"), owner_uid)?;
    if before_state != identity(&state, false)?
        || before_results != identity(&results, true)?
        || before_state != identity(&current_state, false)?
        || before_results != identity(&current_results, true)?
    {
        return Err("Worker results directory changed during observation".into());
    }
    Ok(())
}

/// Read-only startup prerequisite under the host fence. This authenticates the
/// declared directory, not the Worker backend or a generation-bound capability.
pub(crate) fn observe_results(plan: &HostPlan, id: &str) -> Result<(), String> {
    let Some(spec) = results_worker(plan, id)? else {
        return Ok(());
    };
    let value = worker_config(plan, id, spec)?;
    let gid = check_results_config(plan, id, spec, &value)?;
    results_directories(0, gid, || open_root(&spec.state_dir, 0))
}

fn require_root() -> Result<(), String> {
    // Worker services retain their private read-only mount namespace. This guard
    // changes no host mounts; the protected shared lock inodes remain identical.
    if unsafe { libc::geteuid() } != 0 {
        return Err("Worker maintenance guard requires root".into());
    }
    Ok(())
}

fn check_published_authority(
    plan: &HostPlan,
    path: &Path,
    status: &PublicationStatus,
) -> Result<(), String> {
    if status.pending.is_some() || status.published != authority(plan, path) {
        return Err(
            "Worker mutation requires the installed published authority without Pending".into(),
        );
    }
    Ok(())
}

fn check_authority(plan: &HostPlan, path: &Path, status: &PublicationStatus) -> Result<(), String> {
    check_published_authority(plan, path, status)?;
    if status
        .worker_image_load
        .as_ref()
        .is_some_and(|receipt| receipt.outcome == WorkerImageLoadOutcome::Unknown)
    {
        return Err(
            "Worker image load outcome is unknown; explicit stopped-backend recovery is required"
                .into(),
        );
    }
    Ok(())
}

fn context() -> Result<(HostPlan, PathBuf, Ledger), String> {
    let path = fs::canonicalize(INSTALLED).map_err(|_| "Worker admission plan is not installed")?;
    installed(&path)?;
    let plan = load_plan(&path)?;
    let ledger = Ledger::open(Path::new(LEDGER))
        .map_err(|_| "Worker admission authority is missing or unreadable")?;
    let status = ledger
        .publication_status()
        .map_err(|_| "Worker admission authority is missing or unreadable")?;
    check_authority(&plan, &path, &status)?;
    installed(&path)?;
    Ok((plan, path, ledger))
}

fn worker_config(
    plan: &HostPlan,
    id: &str,
    spec: &WorkerSpec,
) -> Result<serde_json::Value, String> {
    let value: serde_json::Value =
        serde_json::from_slice(&read_store_document(&spec.config, 1024 * 1024)?)
            .map_err(|_| "invalid immutable Worker configuration")?;
    check_worker_config(plan, id, spec, &value)?;
    Ok(value)
}

fn check_worker_config(
    plan: &HostPlan,
    id: &str,
    spec: &WorkerSpec,
    value: &serde_json::Value,
) -> Result<(), String> {
    if value.get("agent").and_then(|v| v.as_str()) != Some(id)
        || value.get("admission_cli").and_then(|v| v.as_str()) != plan.tools.cli.to_str()
        || value.get("state_dir").and_then(|v| v.as_str()) != spec.state_dir.to_str()
        || value
            .get("workspace")
            .and_then(|v| v.as_str())
            .map(|p| Path::new(p).join(".tentaflake-worker/inbox"))
            != Some(spec.inbox.clone())
    {
        return Err("Worker configuration differs from its owned declaration".into());
    }
    Ok(())
}

fn valid_action(action: &str, arguments: &[String]) -> bool {
    match (action, arguments) {
        ("drain", []) => true,
        ("approve" | "deny", [job]) => {
            !job.is_empty()
                && job.len() <= 64
                && job
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
        }
        _ => false,
    }
}

pub(crate) fn run(config: &Path, action: &str, arguments: &[String]) -> Result<u8, String> {
    if !valid_action(action, arguments) || !store_path(config) {
        return Err(
            "worker-run requires an immutable configuration and drain|approve|deny [job]".into(),
        );
    }
    require_root()?;
    let fence = host_fence::exclusive()?;
    let (plan, _, _ledger) = context()?;
    let (id, spec) = plan
        .workers
        .iter()
        .find(|(_, spec)| spec.config == config)
        .ok_or("Worker configuration is not declared by the installed plan")?;
    worker_config(&plan, id, spec)?;
    let mut command = Command::new(&spec.executable);
    command
        .env_clear()
        .stdin(Stdio::null())
        .arg("--config")
        .arg(config);
    let fd = fence.inherit(&mut command)?;
    command
        .arg("--maintenance-fence-fd")
        .arg(fd.to_string())
        .arg(action)
        .args(arguments);
    let status = command
        .status()
        .map_err(|_| "cannot supervise guarded Worker")?;
    Ok(status
        .code()
        .and_then(|code| u8::try_from(code).ok())
        .unwrap_or(1))
}

fn image_config(path: &Path) -> Result<ImageConfig, String> {
    let config: ImageConfig = serde_json::from_slice(&read_store_document(path, 1024 * 1024)?)
        .map_err(|_| "invalid Worker image loader configuration")?;
    if config.schema_version != 1
        || !matches!(config.backend.as_str(), "docker" | "podman")
        || !store_path(&config.runtime)
        || !store_path(&config.archive)
    {
        return Err("invalid immutable Worker image source or runtime".into());
    }
    Ok(config)
}

fn check_enforced_image(plan: &HostPlan, path: &Path, config: &ImageConfig) -> Result<(), String> {
    if !config.enforced
        || config.backend != "docker"
        || config.runtime != plan.tools.docker
        || plan
            .worker_image
            .as_ref()
            .is_none_or(|image| image.config != path)
    {
        return Err("Worker image loader differs from the installed Docker plan".into());
    }
    Ok(())
}

pub(crate) fn load_image(path: &Path) -> Result<u8, String> {
    require_root()?;
    let config = image_config(path)?;
    let fence = host_fence::exclusive()?;
    let enforced = if config.enforced {
        let context = context()?;
        check_enforced_image(&context.0, path, &config)?;
        Some(context)
    } else {
        if !matches!(fs::symlink_metadata(LEDGER), Err(error) if error.kind() == std::io::ErrorKind::NotFound)
            || !matches!(fs::symlink_metadata(INSTALLED), Err(error) if error.kind() == std::io::ErrorKind::NotFound)
        {
            return Err(
                "standalone Worker image loading cannot bypass existing admission authority".into(),
            );
        }
        None
    };
    let archive = open_store_file(&config.archive)?;
    let mut command = Command::new(&config.runtime);
    command.env_clear().stdin(Stdio::from(archive));
    if config.backend == "docker" {
        command.args(["--host=unix:///run/docker.sock", "--config=/var/empty"]);
    } else {
        command
            .arg("--remote=false")
            .env("HOME", "/var/empty")
            .env("XDG_CONFIG_HOME", "/var/empty");
    }
    // Prepare immutable input before claiming, but persist uncertainty before
    // any runtime client is spawned. The permit retains the ledger lock until
    // completion; every error/drop preserves the synced Unknown outcome.
    let permit = if let Some((plan, manifest, ledger)) = &enforced {
        installed(manifest)?;
        Some(
            ledger
                .begin_worker_image_load(
                    &authority(plan, manifest),
                    path.to_str()
                        .ok_or("invalid Worker image configuration path")?,
                )
                .map_err(|error| error.to_string())?,
        )
    } else {
        None
    };
    crate::admission_oci::mutate(
        command.args(["load", "--quiet"]),
        &fence,
        Duration::from_secs(180),
    )?;
    if let Some(permit) = permit {
        permit.complete().map_err(|error| error.to_string())?;
    }
    Ok(0)
}

fn revision(value: &str) -> Result<u64, String> {
    value
        .parse::<u64>()
        .ok()
        .filter(|value| *value != 0)
        .ok_or_else(|| "Worker image recovery requires nonzero ledger and claim revisions".into())
}

/// Recovery observes a fully stopped backend; it never restarts a daemon or
/// infers cancellation from a killed image-load client or an empty image list.
pub(crate) fn recover_image(path: &Path, expected: &str, claimed: &str) -> Result<u8, String> {
    let expected = revision(expected)?;
    let claimed = revision(claimed)?;
    crate::admission_runtime::require_root()?;
    let fence = host_fence::exclusive()?;
    installed(path)?;
    let plan = load_plan(path)?;
    let ledger = Ledger::open(Path::new(LEDGER)).map_err(|error| error.to_string())?;
    let status = ledger
        .publication_status()
        .map_err(|error| error.to_string())?;
    check_published_authority(&plan, path, &status)?;
    let dynamic =
        crate::admission_creation::maintenance_inventory(&ledger, &status.published, &fence)?;
    ledger
        .recover_worker_image_load(expected, claimed, |receipt| {
            // The core checks both revisions and Unknown under this same lock. Do
            // not query the ledger recursively while inspecting its exact receipt.
            if receipt.authority != authority(&plan, path) {
                return Err(
                    "Worker image recovery authority differs from the installed plan".into(),
                );
            }
            let configuration = Path::new(&receipt.configuration);
            let config = image_config(configuration)?;
            check_enforced_image(&plan, configuration, &config)?;
            installed(path)?;
            crate::admission_bootstrap::stopped_backend(&plan)?;
            // Unknown blocks publication, so the matching current authority is the
            // recorded plan. Fence every known launcher, including legacy OCI and
            // quota units which could otherwise restart the stopped backend.
            crate::admission_runtime::offline_units(&plan, path, None, &dynamic)?;
            crate::admission_bootstrap::stopped_backend(&plan)?;
            installed(path)
        })
        .map_err(|error| {
            format!("Worker image load recovery refused; preserve the Unknown receipt: {error}")
        })?;
    Ok(0)
}

fn exec_is(commands: &[ExecCommand], executable: &Path, arguments: &[&str]) -> bool {
    let [command] = commands else {
        return false;
    };
    !command.ignore_failure
        && Path::new(&command.path) == executable
        && command
            .argv
            .first()
            .is_some_and(|arg| Path::new(arg) == executable)
        && command.argv[1..]
            .iter()
            .map(String::as_str)
            .eq(arguments.iter().copied())
}

fn owned_service(
    unit: &UnitObservation,
    executable: &Path,
    arguments: &[&str],
    image: bool,
) -> bool {
    unit.load_state == "loaded"
        && unit.service.as_ref().is_some_and(|service| {
            service.service_type == "oneshot"
                && service.remain_after_exit == image
                && service.kill_mode == "control-group"
                && service.exec_start_pre.is_empty()
                && service.exec_start_post.is_empty()
                && service.exec_stop.is_empty()
                && service.exec_stop_post.is_empty()
                && service.exec_reload.is_empty()
                && service.exec_condition.is_empty()
                && exec_is(&service.exec_start, executable, arguments)
        })
}

fn units(plan: &HostPlan) -> impl Iterator<Item = &str> {
    plan.workers
        .values()
        .flat_map(|worker| [worker.unit.as_str(), worker.watcher.as_str()])
        .chain(plan.worker_image.iter().map(|image| image.unit.as_str()))
}

/// Called under the common host fence. Historical state is never guessed away
/// because an empty daemon inventory cannot exclude delayed Worker dispatch.
pub(crate) fn offline(plan: &HostPlan, other: Option<&HostPlan>) -> Result<Vec<String>, String> {
    let available: BTreeSet<_> = admission_systemd::list_worker_units(&plan.tools.busctl)?
        .into_iter()
        .collect();
    let names: BTreeSet<_> = available
        .iter()
        .map(String::as_str)
        .chain(units(plan))
        .chain(other.into_iter().flat_map(units))
        .collect();
    if names.len() > 512 {
        return Err("Worker unit inventory limit exceeded".into());
    }
    for name in &names {
        let desired = units(plan).any(|unit| unit == *name);
        if !desired && !available.contains(*name) {
            continue;
        }
        let unit = admission_systemd::load_unit(&plan.tools.busctl, name)?;
        let owned = if let Some(spec) = plan.workers.values().find(|spec| spec.unit == *name) {
            owned_service(
                &unit,
                &spec.executable,
                &[
                    "--config",
                    spec.config.to_str().ok_or("invalid Worker config")?,
                    "drain",
                ],
                false,
            )
        } else if let Some(spec) = plan.workers.values().find(|spec| spec.watcher == *name) {
            unit.load_state == "loaded"
                && admission_systemd::worker_path_matches(
                    &plan.tools.busctl,
                    &unit,
                    &spec.unit,
                    &spec.inbox,
                )?
        } else if let Some(image) = plan
            .worker_image
            .as_ref()
            .filter(|image| image.unit == *name)
        {
            owned_service(
                &unit,
                &plan.tools.cli,
                &[
                    "admission",
                    "worker-load",
                    image.config.to_str().ok_or("invalid image config")?,
                ],
                true,
            )
        } else {
            unit.load_state == "masked"
        };
        if !stopped(&unit) || !owned {
            return Err("Worker service, watcher and image loader must be stopped; legacy or removed launchers must be masked and drained".into());
        }
    }
    let mut states = BTreeSet::new();
    for (owner, id, spec) in std::iter::once(plan).chain(other).flat_map(|owner| {
        owner
            .workers
            .iter()
            .map(move |(id, spec)| (owner, id, spec))
    }) {
        worker_config(owner, id, spec)?;
        states.insert(spec.state_dir.clone());
        let output = crate::admission_runtime::observe(
            &spec.executable,
            &[
                "--config",
                spec.config.to_str().ok_or("invalid Worker config")?,
                "maintenance",
            ],
        )?;
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields, rename_all = "camelCase")]
        struct Empty {
            schema_version: u32,
            agent: String,
            running_entries: usize,
            jobs_entries: usize,
        }
        let empty: Empty = serde_json::from_slice(&output)
            .map_err(|_| "invalid Worker maintenance observation")?;
        if empty.schema_version != 1
            || empty.agent != *id
            || empty.running_entries != 0
            || empty.jobs_entries != 0
        {
            return Err(
                "Worker maintenance requires no running claims or retained job state".into(),
            );
        }
    }
    // Include orphaned state from removed/legacy declarations, without reading
    // private payloads. Unknown directories require explicit operator recovery.
    for (count, entry) in fs::read_dir("/var/lib")
        .map_err(|_| "cannot enumerate Worker state")?
        .enumerate()
    {
        if count >= 4096 {
            return Err("Worker state inventory limit exceeded".into());
        }
        let entry = entry.map_err(|_| "cannot inspect Worker state entry")?;
        let name = entry.file_name();
        if name.as_encoded_bytes().starts_with(b"tentaflake-worker-")
            && !states.contains(&entry.path())
        {
            return Err(
                "undeclared Worker state remains; preserve it for explicit operator recovery"
                    .into(),
            );
        }
    }
    Ok(names.into_iter().map(str::to_owned).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};
    use std::cell::Cell;
    use std::fs::DirBuilder;
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt, symlink};
    use std::sync::atomic::{AtomicU64, Ordering};
    use tentaflake_admission::ledger::{
        PublicationOutcome, PublicationReceipt, PublicationRequest, WorkerImageLoadReceipt,
    };

    fn fixture() -> (HostPlan, Value) {
        let mut value = crate::admission_host::tests::plan_fixture();
        let store = "/nix/store/00000000000000000000000000000000-worker";
        value["workers"] = json!({"fixture": {
            "executable": format!("{store}/bin/tentaflake-worker"),
            "config": format!("{store}/worker.json"),
            "unit": "tentaflake-worker-fixture.service",
            "watcher": "tentaflake-worker-fixture.path",
            "inbox": "/var/lib/fixture/.tentaflake-worker/inbox",
            "stateDir": "/var/lib/tentaflake-worker-fixture"
        }});
        value["workerImage"] = json!({"config": format!("{store}/image.json"), "unit": IMAGE_UNIT});
        let config = json!({
            "agent": "fixture", "admission_cli": value["tools"]["cli"],
            "state_dir": "/var/lib/tentaflake-worker-fixture", "workspace": "/var/lib/fixture"
        });
        (serde_json::from_value(value).unwrap(), config)
    }

    #[test]
    fn maintenance_state_must_belong_to_its_own_plan_and_configuration() {
        let (mut plan, config) = fixture();
        let check = |plan: &HostPlan, config: &Value| {
            check_worker_config(plan, "fixture", &plan.workers["fixture"], config)
        };
        assert!(check(&plan, &config).is_ok());
        for (field, other) in [
            ("agent", "other"),
            ("state_dir", "/var/lib/empty"),
            ("workspace", "/var/lib/other"),
            ("admission_cli", "/nix/store/other/bin/tentaflake"),
        ] {
            let mut changed = config.clone();
            changed[field] = json!(other);
            assert!(check(&plan, &changed).is_err(), "{field}");
        }
        // A previous plan must authenticate its own CLI, not the candidate's.
        plan.tools.cli = "/nix/store/11111111111111111111111111111111-new/bin/tentaflake".into();
        assert!(check(&plan, &config).is_err());
    }

    #[test]
    fn results_configuration_requires_the_runtime_uid_gid_and_workspace() {
        let (mut plan, mut config) = fixture();
        for volume in &mut plan.volumes {
            volume.spec.instance_id = "fixture".into();
            if volume.spec.kind == StorageKind::Workspace {
                volume.spec.mount_path = "/var/lib/fixture".into();
            }
        }
        config["container_uid"] = json!(1000);
        config["container_gid"] = json!(1000);
        let check = |plan: &HostPlan, value: &Value| {
            check_results_config(plan, "fixture", &plan.workers["fixture"], value)
        };
        assert_eq!(check(&plan, &config).unwrap(), 1000);
        for field in ["container_uid", "container_gid"] {
            for value in [
                Value::Null,
                json!(0),
                json!(1001),
                json!("1000"),
                json!(u64::MAX),
            ] {
                let mut changed = config.clone();
                changed[field] = value;
                assert!(check(&plan, &changed).is_err(), "{field}");
            }
        }
        plan.workers.get_mut("fixture").unwrap().inbox =
            "/var/lib/foreign/.tentaflake-worker/inbox".into();
        config["workspace"] = json!("/var/lib/foreign");
        // The immutable config and WorkerSpec agree, but refer to another
        // workspace. Their agreement alone cannot authorize its results.
        check_worker_config(&plan, "fixture", &plan.workers["fixture"], &config).unwrap();
        assert!(check(&plan, &config).is_err());
        plan.volumes.clear();
        assert!(check(&plan, &config).is_err());
    }

    struct ResultsFixture {
        path: PathBuf,
        anchor: File,
        uid: u32,
        gid: u32,
    }

    impl ResultsFixture {
        fn new() -> Self {
            static SEQUENCE: AtomicU64 = AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!(
                "tentaflake-worker-results-{}-{}",
                std::process::id(),
                SEQUENCE.fetch_add(1, Ordering::Relaxed)
            ));
            DirBuilder::new().mode(0o700).create(&path).unwrap();
            let anchor = tentaflake_admission::protected_path::open_at(
                libc::AT_FDCWD,
                path.as_os_str(),
                libc::O_RDONLY | libc::O_DIRECTORY,
            )
            .unwrap();
            for child in ["state", "state/results"] {
                DirBuilder::new()
                    .mode(0o750)
                    .create(path.join(child))
                    .unwrap();
                fs::set_permissions(path.join(child), fs::Permissions::from_mode(0o750)).unwrap();
            }
            let metadata = fs::metadata(path.join("state")).unwrap();
            Self {
                path,
                anchor,
                uid: metadata.uid(),
                gid: metadata.gid(),
            }
        }

        fn state(&self) -> Result<File, String> {
            // Production anchors at / with owner 0. The same traversal is
            // exercised from an explicit owned fixture FD without root access.
            open_relative(&self.anchor, Path::new("state"), self.uid)
        }

        fn observe(&self) -> Result<(), String> {
            results_directories(self.uid, self.gid, || self.state())
        }
    }

    impl Drop for ResultsFixture {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.path).unwrap();
        }
    }

    #[test]
    fn results_observation_allows_owned_publication_without_changing_directory_identity() {
        let fixture = ResultsFixture::new();
        fixture.observe().unwrap();
        let opens = Cell::new(0);
        results_directories(fixture.uid, fixture.gid, || {
            opens.set(opens.get() + 1);
            if opens.get() == 2 {
                fs::write(fixture.path.join("state/results/new-result"), b"fixture").unwrap();
            }
            fixture.state()
        })
        .unwrap();
        assert_eq!(opens.get(), 2);
        assert_eq!(
            fs::read(fixture.path.join("state/results/new-result")).unwrap(),
            b"fixture"
        );
    }

    #[test]
    fn results_observation_refuses_missing_symlink_foreign_and_writable_directories() {
        for case in [
            "missing",
            "symlink",
            "file",
            "state-mode",
            "results-mode",
            "parent-mode",
            "owner",
            "group",
        ] {
            let fixture = ResultsFixture::new();
            let results = fixture.path.join("state/results");
            match case {
                "missing" | "symlink" | "file" => {
                    fs::remove_dir(&results).unwrap();
                    if case == "symlink" {
                        symlink(&fixture.path, &results).unwrap();
                    } else if case == "file" {
                        fs::write(&results, b"fixture").unwrap();
                    }
                }
                "state-mode" => fs::set_permissions(
                    fixture.path.join("state"),
                    fs::Permissions::from_mode(0o770),
                )
                .unwrap(),
                "results-mode" => {
                    fs::set_permissions(&results, fs::Permissions::from_mode(0o777)).unwrap()
                }
                "parent-mode" => {
                    fs::set_permissions(&fixture.path, fs::Permissions::from_mode(0o777)).unwrap()
                }
                "owner" | "group" => {}
                _ => unreachable!(),
            }
            let result = results_directories(
                if case == "owner" {
                    fixture.uid ^ 1
                } else {
                    fixture.uid
                },
                if case == "group" {
                    fixture.gid ^ 1
                } else {
                    fixture.gid
                },
                || fixture.state(),
            );
            assert!(result.is_err(), "{case}");
            if case == "missing" {
                assert!(
                    !results.exists(),
                    "observation must not repair missing state"
                );
            }
        }
    }

    #[test]
    fn results_observation_refuses_state_or_results_replacement_during_observation() {
        for (target, symlinked) in [
            ("state", false),
            ("state/results", false),
            ("state", true),
            ("state/results", true),
        ] {
            let fixture = ResultsFixture::new();
            let opens = Cell::new(0);
            let result = results_directories(fixture.uid, fixture.gid, || {
                opens.set(opens.get() + 1);
                if opens.get() == 2 {
                    let old = fixture.path.join("previous");
                    fs::rename(fixture.path.join(target), &old).unwrap();
                    if symlinked {
                        symlink(old, fixture.path.join(target)).unwrap();
                    } else {
                        DirBuilder::new()
                            .mode(0o750)
                            .create(fixture.path.join(target))
                            .unwrap();
                        if target == "state" {
                            DirBuilder::new()
                                .mode(0o750)
                                .create(fixture.path.join("state/results"))
                                .unwrap();
                        }
                    }
                }
                fixture.state()
            });
            assert_eq!(opens.get(), 2);
            assert!(result.is_err(), "{target}, symlink={symlinked}");
        }
    }

    #[test]
    fn stale_authority_and_pending_publication_refuse_worker_mutation() {
        let (plan, _) = fixture();
        let path = Path::new("/nix/store/00000000000000000000000000000000-host.json");
        let published = authority(&plan, path);
        let mut status = PublicationStatus {
            ledger_revision: 9,
            published: published.clone(),
            pending: None,
            worker_image_load: None,
        };
        assert!(check_authority(&plan, path, &status).is_ok());
        status.published.generation.push_str("-stale");
        assert!(check_authority(&plan, path, &status).is_err());
        status.published = published.clone();
        status.pending = Some(PublicationReceipt {
            request: PublicationRequest {
                operation_id: "maintenance".into(),
                expected_ledger_revision: 9,
                expected_authority: published.clone(),
                candidate: published,
            },
            outcome: PublicationOutcome::Pending,
            begun_revision: 10,
            updated_revision: 10,
        });
        assert!(check_authority(&plan, path, &status).is_err());
    }

    #[test]
    fn unknown_image_load_refuses_mutations_but_keeps_recovery_authority_observable() {
        let (plan, _) = fixture();
        let path = Path::new("/nix/store/00000000000000000000000000000000-host.json");
        let published = authority(&plan, path);
        let mut status = PublicationStatus {
            ledger_revision: 9,
            published: published.clone(),
            pending: None,
            worker_image_load: Some(WorkerImageLoadReceipt {
                authority: published,
                configuration: plan
                    .worker_image
                    .as_ref()
                    .unwrap()
                    .config
                    .to_str()
                    .unwrap()
                    .into(),
                outcome: WorkerImageLoadOutcome::Unknown,
                claimed_revision: 9,
                updated_revision: 9,
            }),
        };
        assert!(
            check_authority(&plan, path, &status)
                .unwrap_err()
                .contains("unknown")
        );
        assert!(check_published_authority(&plan, path, &status).is_ok());
        for outcome in [
            WorkerImageLoadOutcome::Completed,
            WorkerImageLoadOutcome::Recovered,
        ] {
            status.worker_image_load.as_mut().unwrap().outcome = outcome;
            assert!(check_authority(&plan, path, &status).is_ok());
        }
        status.worker_image_load.as_mut().unwrap().outcome = WorkerImageLoadOutcome::Unknown;
        status.published.revision.push_str("-stale");
        assert!(check_published_authority(&plan, path, &status).is_err());
    }

    #[test]
    fn enforced_image_load_and_recovery_bind_the_exact_declared_docker_runtime() {
        let (plan, _) = fixture();
        let path = &plan.worker_image.as_ref().unwrap().config;
        let mut config = ImageConfig {
            schema_version: 1,
            backend: "docker".into(),
            runtime: plan.tools.docker.clone(),
            archive: "/nix/store/00000000000000000000000000000000-image.tar".into(),
            enforced: true,
        };
        assert!(check_enforced_image(&plan, path, &config).is_ok());
        config.backend = "podman".into();
        assert!(check_enforced_image(&plan, path, &config).is_err());
        config.backend = "docker".into();
        config.runtime = "/nix/store/11111111111111111111111111111111-foreign/bin/docker".into();
        assert!(check_enforced_image(&plan, path, &config).is_err());
        config.runtime = plan.tools.docker.clone();
        config.enforced = false;
        assert!(check_enforced_image(&plan, path, &config).is_err());
        config.enforced = true;
        assert!(
            check_enforced_image(
                &plan,
                Path::new("/nix/store/11111111111111111111111111111111-image.json"),
                &config
            )
            .is_err()
        );
    }

    #[test]
    fn image_recovery_requires_explicit_nonzero_bounded_revisions() {
        assert_eq!(revision("9").unwrap(), 9);
        assert_eq!(revision(&u64::MAX.to_string()).unwrap(), u64::MAX);
        for invalid in ["", "0", "-1", "1.0", "18446744073709551616"] {
            assert!(revision(invalid).is_err());
        }
    }

    #[test]
    fn worker_inventory_rejects_rebound_state_units_and_missing_loader() {
        let (mut plan, _) = fixture();
        assert!(validate_plan(&plan).is_ok());
        plan.workers.get_mut("fixture").unwrap().state_dir = "/var/lib/empty".into();
        assert!(validate_plan(&plan).is_err());
        let (mut plan, _) = fixture();
        plan.workers.get_mut("fixture").unwrap().watcher = "other.path".into();
        assert!(validate_plan(&plan).is_err());
        let (mut plan, _) = fixture();
        plan.worker_image = None;
        assert!(validate_plan(&plan).is_err());
        assert!(!worker_id("image"));
        assert!(!worker_id("state-volumes"));
    }
}
