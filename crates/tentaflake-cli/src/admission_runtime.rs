//! Host-owned storage transactions. Only the exact systemd quota invocation may
//! mutate images; an exclusive inherited fence also covers surviving helpers.
use crate::admission_host::{HostPlan, Volume, load_plan, store_path};
use crate::admission_storage::{
    CreationTargets, VolumeSpec, check_creation_space, ensure_backing_roots, scan_inventory,
};
use crate::admission_systemd::{self, ExecCommand, UnitObservation};
use serde::Deserialize;
use std::collections::BTreeSet;
use std::fs;
use std::os::unix::fs::{FileTypeExt, MetadataExt};
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Duration;
use tentaflake_admission::host_fence::{self as admission_fence, HostFence};
use tentaflake_admission::ledger::{Ledger, PublishedAuthority, StorageClaimOutcome};
use tentaflake_admission::{
    AdmissionRequest, ExpectedRevisions, ReservationSnapshot, SCHEMA_VERSION,
    StorageAdmissionSnapshot, StorageAllocation, StorageRequest, StorageState,
};

pub(crate) const LEDGER: &str = "/var/lib/tentaflake-admission";
const INSTALLED: &str = "/etc/tentaflake/admission.json";
const MAX_OBSERVATION: usize = 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Action {
    Start,
    Stop,
}

impl Action {
    fn command(self) -> &'static str {
        match self {
            Self::Start => "volume-start",
            Self::Stop => "volume-stop",
        }
    }
}

pub(crate) fn require_root() -> Result<(), String> {
    if unsafe { libc::geteuid() } != 0 {
        return Err("host admission requires root".into());
    }
    // A mount made inside a private service namespace cannot authorize a host
    // consumer. Namespace inode/device equality avoids trusting unit defaults.
    let ours = fs::metadata("/proc/self/ns/mnt").map_err(|_| "cannot inspect mount namespace")?;
    let host = fs::metadata("/proc/1/ns/mnt").map_err(|_| "cannot inspect host mount namespace")?;
    if (ours.dev(), ours.ino()) != (host.dev(), host.ino()) {
        return Err("storage lifecycle requires the host mount namespace".into());
    }
    Ok(())
}

pub(crate) fn installed(path: &Path) -> Result<(), String> {
    for parent in ["/etc", "/etc/tentaflake"] {
        let metadata =
            fs::symlink_metadata(parent).map_err(|_| "cannot inspect installed plan directory")?;
        if !metadata.is_dir() || metadata.uid() != 0 || metadata.mode() & 0o022 != 0 {
            return Err("unprotected installed plan directory".into());
        }
    }
    let link = fs::symlink_metadata(INSTALLED).map_err(|_| "cannot inspect installed plan link")?;
    let target =
        fs::read_link(INSTALLED).map_err(|_| "installed admission plan must be a store link")?;
    if link.uid() != 0 {
        return Err("installed admission plan link must be root-owned".into());
    }
    if target != path {
        if target != Path::new("/etc/static/tentaflake/admission.json") {
            return Err("installed admission plan uses an unsupported mutable link chain".into());
        }
        let static_link =
            fs::symlink_metadata("/etc/static").map_err(|_| "cannot inspect NixOS etc link")?;
        let static_target =
            fs::read_link("/etc/static").map_err(|_| "NixOS etc must link into the store")?;
        if static_link.uid() != 0 || !store_path(&static_target) {
            return Err("NixOS etc link is not an immutable root-owned store reference".into());
        }
    }
    if fs::canonicalize(INSTALLED).map_err(|_| "host admission plan is not installed")? != path {
        return Err("host admission plan is no longer installed".into());
    }
    Ok(())
}

pub(crate) fn authority(plan: &HostPlan, path: &Path) -> PublishedAuthority {
    PublishedAuthority {
        generation: plan.policy.generation.clone(),
        revision: plan.policy.revision.clone(),
        manifest_path: path.to_string_lossy().into_owned(),
    }
}

fn specs(plan: &HostPlan) -> Vec<VolumeSpec> {
    plan.volumes
        .iter()
        .map(|volume| volume.spec.clone())
        .collect()
}

fn volume<'a>(plan: &'a HostPlan, id: &str) -> Result<&'a Volume, String> {
    plan.volumes
        .iter()
        .find(|volume| volume.spec.allocation_id == id)
        .ok_or_else(|| "unknown declared storage allocation".into())
}

fn exact_command(
    commands: &[ExecCommand],
    plan: &HostPlan,
    path: &Path,
    volume: &Volume,
    action: Action,
) -> bool {
    let [command] = commands else { return false };
    command.path == plan.tools.cli.to_string_lossy()
        && !command.ignore_failure
        && command.argv
            == [
                plan.tools.cli.to_string_lossy().as_ref(),
                "admission",
                action.command(),
                path.to_string_lossy().as_ref(),
                &volume.spec.allocation_id,
            ]
}

fn guarded_unit(observed: &UnitObservation, plan: &HostPlan, path: &Path, volume: &Volume) -> bool {
    observed.id == volume.units.owner
        && observed.load_state == "loaded"
        && observed.service.as_ref().is_some_and(|service| {
            service.service_type == "oneshot"
                && service.remain_after_exit
                && service.kill_mode == "control-group"
                && (1..=300_000_000).contains(&service.timeout_start_usec)
                && (1..=120_000_000).contains(&service.timeout_stop_usec)
                && exact_command(&service.exec_start, plan, path, volume, Action::Start)
                && exact_command(&service.exec_stop, plan, path, volume, Action::Stop)
        })
}

fn invoking_unit(
    plan: &HostPlan,
    path: &Path,
    volume: &Volume,
    action: Action,
) -> Result<UnitObservation, String> {
    let observed = admission_systemd::inspect_unit(&plan.tools.busctl, &volume.units.owner)?;
    if !valid_invocation(&observed, plan, path, volume, action, std::process::id()) {
        return Err("storage mutation requires the exact supervised quota invocation".into());
    }
    let service = observed
        .service
        .as_ref()
        .ok_or("missing quota service observation")?;
    let cgroup =
        fs::read_to_string("/proc/self/cgroup").map_err(|_| "cannot inspect quota cgroup")?;
    if cgroup.trim() != format!("0::{}", service.control_group)
        || service.control_group.is_empty()
        || service.control_group == "/"
        || fs::canonicalize(&plan.tools.cli).map_err(|_| "cannot resolve quota executable")?
            != fs::canonicalize("/proc/self/exe").map_err(|_| "cannot inspect quota executable")?
    {
        return Err("quota executable or cgroup does not match its supervisor".into());
    }
    Ok(observed)
}

fn valid_invocation(
    observed: &UnitObservation,
    plan: &HostPlan,
    path: &Path,
    volume: &Volume,
    action: Action,
    pid: u32,
) -> bool {
    let valid_phase = observed
        .service
        .as_ref()
        .is_some_and(|service| match action {
            Action::Start => {
                observed.active_state == "activating"
                    && observed.sub_state == "start"
                    && service.main_pid == pid
            }
            Action::Stop => {
                observed.active_state == "deactivating"
                    && observed.sub_state == "stop"
                    && service.control_pid == pid
            }
        });
    guarded_unit(observed, plan, path, volume)
        && valid_phase
        && observed.invocation_id != [0; 16]
        && (action == Action::Stop || !observed.need_daemon_reload)
}

fn command(path: &Path) -> Command {
    let mut command = Command::new(path);
    command.env_clear().stdin(Stdio::null());
    command
}

pub(crate) fn observe(path: &Path, args: &[&str]) -> Result<Vec<u8>, String> {
    let output = crate::output_with_timeout(command(path).args(args), Duration::from_secs(10))
        .map_err(|_| "bounded storage observation failed")?;
    if !output.status.success()
        || output.stdout.len().saturating_add(output.stderr.len()) > MAX_OBSERVATION
    {
        return Err("storage observation failed or exceeded its output limit".into());
    }
    Ok(output.stdout)
}

fn mutate(path: &Path, fence: &HostFence) -> Result<(), String> {
    let mut command = command(path);
    // Keep helper diagnostics out of API/CLI errors. The supervisor owns the
    // deadline and kills its entire cgroup, including descendants, on failure.
    command.stdout(Stdio::null()).stderr(Stdio::null());
    fence.inherit(&mut command)?;
    if !command
        .status()
        .map_err(|_| "cannot execute quota helper")?
        .success()
    {
        return Err("quota helper failed; the storage charge is retained".into());
    }
    Ok(())
}

fn request(
    plan: &HostPlan,
    volume: &Volume,
    snapshot: &ReservationSnapshot,
    operation: String,
) -> StorageRequest {
    StorageRequest {
        request: AdmissionRequest {
            schema_version: SCHEMA_VERSION,
            operation_id: operation,
            instance_id: volume.spec.instance_id.clone(),
            principal_id: volume.spec.owner_id.clone(),
            expected: ExpectedRevisions {
                policy: plan.policy.revision.clone(),
                instance: plan.instances[&volume.spec.instance_id].revision.clone(),
                ledger: snapshot.revision,
            },
        },
        allocation_id: volume.spec.allocation_id.clone(),
    }
}

fn context(
    plan: &HostPlan,
    path: &Path,
    volume: &Volume,
    snapshot: &ReservationSnapshot,
    published: &PublishedAuthority,
) -> Result<StorageAdmissionSnapshot, String> {
    installed(path)?;
    if published != &authority(plan, path) {
        return Err("storage authority changed; explicit publication is required".into());
    }
    Ok(StorageAdmissionSnapshot {
        schema_version: SCHEMA_VERSION,
        policy: plan.policy.clone(),
        instance: plan.instances[&volume.spec.instance_id].clone(),
        reservations: snapshot.clone(),
    })
}

pub(crate) fn lifecycle(path: &Path, id: &str, start: bool) -> Result<u8, String> {
    require_root()?;
    let plan = load_plan(path)?;
    let volume = volume(&plan, id)?;
    let action = if start { Action::Start } else { Action::Stop };
    // Authenticate before waiting and repeat under the fence. A direct root CLI
    // invocation is not a lifecycle permit; callers use the generated service.
    invoking_unit(&plan, path, volume, action)?;
    let fence = admission_fence::exclusive()?;
    let observed = invoking_unit(&plan, path, volume, action)?;
    if !start {
        // Exact cleanup remains possible after a declaration changes or the
        // ledger becomes unavailable. It never releases or recreates storage.
        mutate(&volume.helpers.unmount, &fence)?;
        return Ok(0);
    }
    installed(path)?;
    let ledger = Ledger::open(Path::new(LEDGER))
        .map_err(|_| "admission ledger unavailable; explicit bootstrap or recovery required")?;
    let snapshot = ledger
        .snapshot()
        .map_err(|_| "cannot read admission ledger")?;
    let operation = observed
        .invocation_id
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    let request = request(
        &plan,
        volume,
        &snapshot,
        format!("volume-start-{operation}"),
    );
    if snapshot
        .storage
        .iter()
        .any(|allocation| allocation.id == id)
    {
        ledger
            .with_present_storage(
                &authority(&plan, path),
                &request,
                |published| context(&plan, path, volume, &snapshot, published),
                |_| {
                    mutate(&volume.helpers.verify, &fence)?;
                    mutate(&volume.helpers.mount, &fence)?;
                    mutate(&volume.helpers.owner, &fence)
                },
            )
            .map_err(
                |_| "existing storage is unavailable, stale or unverified; recreation is forbidden",
            )?;
    } else {
        let claim = ledger
            .reserve_storage(&request, |published| {
                let context = context(&plan, path, volume, &snapshot, published)?;
                check_creation_space(
                    &specs(&plan),
                    &snapshot.storage,
                    CreationTargets::New(std::slice::from_ref(&volume.spec)),
                    plan.physical_space_headroom_bytes,
                )?;
                Ok(context)
            })
            .map_err(|_| "storage admission failed; no image creation authorized")?;
        let StorageClaimOutcome::New(permit) = claim else {
            return Err(
                "storage creation denied or already attempted; no replay is permitted".into(),
            );
        };
        mutate(&volume.helpers.create, &fence)?;
        mutate(&volume.helpers.mount, &fence)?;
        mutate(&volume.helpers.owner, &fence)?;
        permit
            .complete(StorageState::Present)
            .map_err(|_| "cannot confirm storage; retained charge needs recovery")?;
    }
    Ok(0)
}

pub(crate) fn stopped(observed: &UnitObservation) -> bool {
    matches!(observed.active_state.as_str(), "inactive" | "failed")
        && observed.job.id == 0
        && !observed.need_daemon_reload
        && observed.service.as_ref().is_none_or(|service| {
            service.main_pid == 0 && service.control_pid == 0 && service.control_group.is_empty()
        })
}

fn quota_unit_names(plan: &HostPlan, other: Option<&HostPlan>) -> BTreeSet<String> {
    plan.volumes
        .iter()
        .chain(other.into_iter().flat_map(|plan| &plan.volumes))
        .flat_map(|volume| {
            [
                &volume.units.owner,
                &volume.units.prepare,
                &volume.units.mount,
            ]
        })
        .cloned()
        .collect()
}

fn offline_quota_unit(
    observed: &UnitObservation,
    plan: &HostPlan,
    path: &Path,
) -> Result<(), String> {
    if let Some(volume) = plan
        .volumes
        .iter()
        .find(|volume| volume.units.owner == observed.id)
    {
        // The same owner name can exist in both generations. Only the desired
        // plan's exact commands may remain loaded after installation/reload.
        if !guarded_unit(observed, plan, path, volume) || !stopped(observed) {
            return Err(
                "quota supervisors must be stopped with the exact guarded plan loaded".into(),
            );
        }
    } else if observed.load_state != "masked" || !stopped(observed) {
        return Err("legacy or removed quota units must be masked and drained".into());
    }
    Ok(())
}

/// Observe the desired installed plan and fence remnants of either generation.
/// Callers hold the outer fence; these observations never stop or mask units.
pub(crate) fn offline_units(
    plan: &HostPlan,
    path: &Path,
    other: Option<&HostPlan>,
    creation: &crate::admission_creation::CreationInventory,
) -> Result<Vec<String>, String> {
    let mut inventory: BTreeSet<_> = crate::admission_bootstrap::runtime_units(plan, path, other)?
        .into_iter()
        .collect();
    inventory.extend(crate::admission_worker::offline(plan, other)?);
    inventory.extend(crate::admission_creation::offline(plan, path, creation)?);
    let mut names = quota_unit_names(plan, other);
    names.extend(admission_systemd::list_quota_units(&plan.tools.busctl)?);
    for name in &names {
        let observed = admission_systemd::load_unit(&plan.tools.busctl, name)?;
        offline_quota_unit(&observed, plan, path)?;
    }
    // Removed definitions can leave a native mount job whose prepare dependency
    // already completed. Inspect all available mount units, including unloaded
    // files, rather than inferring their absence from the new plan.
    for observed in admission_systemd::observe_mount_units(&plan.tools.busctl, &names)? {
        maintenance_mount(&observed, plan, other, &creation.volumes)?;
    }
    inventory.extend(names);
    if inventory.len() > 2048 {
        return Err("maintenance unit inventory limit exceeded".into());
    }
    Ok(inventory.into_iter().collect())
}

fn overlaps(left: &Path, right: &Path) -> bool {
    left.starts_with(right) || right.starts_with(left)
}

fn maintenance_mount(
    observed: &UnitObservation,
    plan: &HostPlan,
    other: Option<&HostPlan>,
    dynamic: &[VolumeSpec],
) -> Result<(), String> {
    validate_maintenance_mount(observed, plan, other, dynamic)
        .map_err(|error| format!("{error}: {}", observed.mount_context()))
}

fn validate_maintenance_mount(
    observed: &UnitObservation,
    plan: &HostPlan,
    other: Option<&HostPlan>,
    dynamic: &[VolumeSpec],
) -> Result<(), String> {
    if observed.load_state == "masked" && stopped(observed) {
        return Ok(());
    }
    if observed.load_state != "loaded"
        || observed.job.id != 0
        || observed.need_daemon_reload
        || !matches!(
            observed.active_state.as_str(),
            "active" | "inactive" | "failed"
        )
    {
        return Err("mount inventory is unavailable, queued or transitioning".into());
    }
    let mount = observed.mount.as_ref().ok_or("missing mount observation")?;
    // Overlay's actual lower/upper/work paths live in options, not What. Even
    // an absolute directory in What cannot prove its backing independent.
    if mount.filesystem_type == "overlay" {
        return Err("overlay native mounts are unsupported during maintenance".into());
    }
    let target = Path::new(&mount.where_path);
    let active = observed.active_state == "active";
    // Stable ordinary ancestors (/, /var, /var/lib, the backing filesystem)
    // stay available. No other mount may later cover the protected roots or a
    // declared target; an already mounted declared target is not offline either.
    if (!active
        && (overlaps(target, Path::new("/var/lib/tentaflake-workspace-volumes"))
            || plan
                .volumes
                .iter()
                .chain(other.into_iter().flat_map(|plan| &plan.volumes))
                .map(|volume| &volume.spec)
                .chain(dynamic)
                .any(|volume| overlaps(target, &volume.mount_path))))
        || (active
            && plan
                .volumes
                .iter()
                .chain(other.into_iter().flat_map(|plan| &plan.volumes))
                .map(|volume| &volume.spec)
                .chain(dynamic)
                .any(|volume| target.starts_with(&volume.mount_path)))
    {
        return Err("a native mount can cover managed storage during maintenance".into());
    }
    let source = Path::new(&mount.what);
    if source.is_absolute() {
        let metadata = fs::metadata(source).map_err(|_| "native mount source is unavailable")?;
        if !metadata.is_dir() && !metadata.file_type().is_block_device() {
            return Err("file-backed native mount units must be masked before maintenance".into());
        }
    } else if !(pseudo_filesystem(&mount.filesystem_type)
        || (active && target == Path::new("/") && mount.what == "none"))
    {
        return Err("native mount source cannot be proven independent of backing images".into());
    }
    Ok(())
}

fn pseudo_filesystem(kind: &str) -> bool {
    matches!(
        kind,
        "tmpfs"
            | "devtmpfs"
            | "devpts"
            | "proc"
            | "sysfs"
            | "securityfs"
            | "cgroup"
            | "cgroup2"
            | "debugfs"
            | "tracefs"
            | "fusectl"
            | "mqueue"
            | "hugetlbfs"
            | "pstore"
            | "efivarfs"
            | "configfs"
            | "bpf"
            | "ramfs"
            | "autofs"
            | "9p"
            | "virtiofs"
    )
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Loops {
    loopdevices: Vec<serde::de::IgnoredAny>,
}

fn no_backing_loops(bytes: &[u8]) -> Result<(), String> {
    let loops: Loops =
        serde_json::from_slice(bytes).map_err(|_| "invalid loop-device observation")?;
    // A bind-mount alias can expose the same backing inode under another name.
    // These maintenance operations therefore accept no attached loops at all.
    if !loops.loopdevices.is_empty() {
        return Err("all loop devices must be detached before bootstrap or recovery".into());
    }
    Ok(())
}

pub(crate) fn offline_workloads(plan: &HostPlan) -> Result<(), String> {
    // First bootstrap is a maintenance operation. Do not stop an unrelated
    // workload or infer that an uninspectable daemon means no containers.
    if !observe(
        &plan.tools.docker,
        &[
            "--host",
            "unix:///var/run/docker.sock",
            "ps",
            "--all",
            "--quiet",
            "--no-trunc",
        ],
    )?
    .is_empty()
    {
        return Err("maintenance requires the Docker container inventory to be empty".into());
    }
    offline_loops(plan)
}

pub(crate) fn offline_loops(plan: &HostPlan) -> Result<(), String> {
    no_backing_loops(&observe(
        &plan.tools.losetup,
        &["--list", "--json", "--output", "NAME,BACK-FILE"],
    )?)
}

fn allocation_matches(allocation: &StorageAllocation, volume: &Volume) -> bool {
    allocation.id == volume.spec.allocation_id
        && allocation.kind == volume.spec.kind
        && allocation.bytes == volume.spec.bytes
        && allocation.binding.as_ref().is_some_and(|binding| {
            binding.instance_id == volume.spec.instance_id
                && binding.owner_id == volume.spec.owner_id
                && binding.revision == volume.spec.binding_revision
        })
}

pub(crate) fn bootstrap(path: &Path) -> Result<u8, String> {
    require_root()?;
    let plan = load_plan(path)?;
    let fence = admission_fence::exclusive()?;
    installed(path)?;
    match fs::symlink_metadata(LEDGER) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        _ => return Err("bootstrap requires a genuinely absent ledger; never delete an existing ledger to retry".into()),
    }
    crate::admission_bootstrap::verify(&plan, path)?;
    offline_workloads(&plan)?;
    ensure_backing_roots()?;
    let mut inventory = scan_inventory(&specs(&plan))?;
    for allocation in &mut inventory.allocations {
        if let Some(volume) = plan
            .volumes
            .iter()
            .find(|volume| allocation_matches(allocation, volume))
        {
            // Failed verification stays Unknown and charged, including a full
            // image from an interrupted creation. This never authorizes repair.
            if observe(&volume.helpers.verify, &[]).is_ok() {
                allocation.state = StorageState::Present;
            }
        }
    }
    installed(path)?;
    crate::admission_bootstrap::verify(&plan, path)?;
    offline_workloads(&plan)?;
    // Only this fenced path may assert completeness. Diagnostic inventory never
    // supplies this flag, and callers cannot submit an inventory JSON file.
    crate::admission_retention::retain(
        &authority(&plan, path),
        plan.tools
            .nix_store
            .as_deref()
            .ok_or("bootstrap requires a plan with the Nix retention tool")?,
        &fence,
    )?;
    installed(path)?;
    crate::admission_bootstrap::verify(&plan, path)?;
    offline_workloads(&plan)?;
    inventory.complete = true;
    Ledger::initialize(Path::new(LEDGER), authority(&plan, path), inventory).map_err(
        |_| "cannot initialize admission ledger; preserve any partial state for recovery",
    )?;
    Ok(0)
}

pub(crate) fn recover(path: &Path, id: &str) -> Result<u8, String> {
    require_root()?;
    let plan = load_plan(path)?;
    let volume = volume(&plan, id)?;
    let fence = admission_fence::exclusive()?;
    installed(path)?;
    let ledger = Ledger::open(Path::new(LEDGER)).map_err(|_| "admission ledger unavailable")?;
    let snapshot = ledger
        .snapshot()
        .map_err(|_| "cannot read admission ledger")?;
    let publication = ledger
        .publication_status()
        .map_err(|_| "cannot read published authority")?;
    if publication.published != authority(&plan, path) || publication.pending.is_some() {
        return Err("storage recovery requires stable installed authority".into());
    }
    let creation =
        crate::admission_creation::maintenance_inventory(&ledger, &publication.published, &fence)?;
    offline_units(&plan, path, None, &creation)?;
    offline_workloads(&plan)?;
    ledger
        .reconcile_storage(id, snapshot.revision, |receipt| {
            if !allocation_matches(&receipt.allocation, volume) {
                return Err("storage recovery binding mismatch".into());
            }
            observe(&volume.helpers.verify, &[])?;
            Ok(StorageState::Present)
        })
        .map_err(
            |_| "storage recovery failed; charge retained without recreating or resizing the image",
        )?;
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::admission_systemd::{ServiceObservation, UnitJob};

    fn fixture() -> (HostPlan, UnitObservation) {
        let plan: HostPlan =
            serde_json::from_value(crate::admission_host::tests::plan_fixture()).unwrap();
        let volume = &plan.volumes[0];
        let exec = |action: &str| ExecCommand {
            path: plan.tools.cli.to_string_lossy().into_owned(),
            argv: vec![
                plan.tools.cli.to_string_lossy().into_owned(),
                "admission".into(),
                action.into(),
                "/nix/store/00000000000000000000000000000000-plan".into(),
                volume.spec.allocation_id.clone(),
            ],
            ignore_failure: false,
            start_realtime: 0,
            start_monotonic: 0,
            exit_realtime: 0,
            exit_monotonic: 0,
            pid: 41,
            code: 0,
            status: 0,
        };
        let observed = UnitObservation {
            id: volume.units.owner.clone(),
            load_state: "loaded".into(),
            active_state: "activating".into(),
            sub_state: "start".into(),
            job: UnitJob {
                id: 10,
                path: "/org/freedesktop/systemd1/job/10".into(),
            },
            fragment_path: "/etc/systemd/system/fixture.service".into(),
            need_daemon_reload: false,
            invocation_id: [1; 16],
            mount: None,
            service: Some(ServiceObservation {
                main_pid: 41,
                control_pid: 0,
                control_group: "/system.slice/fixture.service".into(),
                exec_start: vec![exec("volume-start")],
                exec_stop: vec![exec("volume-stop")],
                service_type: "oneshot".into(),
                remain_after_exit: true,
                kill_mode: "control-group".into(),
                timeout_start_usec: 300_000_000,
                timeout_stop_usec: 120_000_000,
                restart: "no".into(),
                notify_access: "none".into(),
                exec_start_pre: vec![],
                exec_start_post: vec![],
                exec_stop_post: vec![],
                exec_reload: vec![],
                exec_condition: vec![],
            }),
        };
        (plan, observed)
    }

    #[test]
    fn invocation_rejects_direct_call_replaced_plan_and_unsupervised_children() {
        let (plan, observed) = fixture();
        let path = Path::new("/nix/store/00000000000000000000000000000000-plan");
        let volume = &plan.volumes[0];
        assert!(valid_invocation(
            &observed,
            &plan,
            path,
            volume,
            Action::Start,
            41
        ));
        assert!(!valid_invocation(
            &observed,
            &plan,
            path,
            volume,
            Action::Start,
            42
        ));
        assert!(!valid_invocation(
            &observed,
            &plan,
            Path::new("/nix/store/00000000000000000000000000000000-replacement"),
            volume,
            Action::Start,
            41
        ));
        for mutate in [
            |unit: &mut UnitObservation| unit.need_daemon_reload = true,
            |unit: &mut UnitObservation| unit.invocation_id = [0; 16],
            |unit: &mut UnitObservation| unit.active_state = "active".into(),
            |unit: &mut UnitObservation| {
                unit.service.as_mut().unwrap().kill_mode = "process".into()
            },
            |unit: &mut UnitObservation| {
                unit.service.as_mut().unwrap().timeout_start_usec = u64::MAX
            },
            |unit: &mut UnitObservation| {
                unit.service.as_mut().unwrap().exec_start[0].ignore_failure = true
            },
            |unit: &mut UnitObservation| {
                unit.service.as_mut().unwrap().exec_stop[0].argv[4] = "workspace:other".into()
            },
        ] {
            let mut changed = observed.clone();
            mutate(&mut changed);
            assert!(!valid_invocation(
                &changed,
                &plan,
                path,
                volume,
                Action::Start,
                41
            ));
        }
    }

    #[test]
    fn stop_binds_control_pid_and_can_clean_up_a_replaced_declaration() {
        let (plan, mut observed) = fixture();
        let path = Path::new("/nix/store/00000000000000000000000000000000-plan");
        observed.active_state = "deactivating".into();
        observed.sub_state = "stop".into();
        observed.need_daemon_reload = true;
        observed.service.as_mut().unwrap().main_pid = 0;
        observed.service.as_mut().unwrap().control_pid = 42;
        assert!(valid_invocation(
            &observed,
            &plan,
            path,
            &plan.volumes[0],
            Action::Stop,
            42
        ));
        assert!(!valid_invocation(
            &observed,
            &plan,
            path,
            &plan.volumes[0],
            Action::Stop,
            41
        ));
        assert!(!valid_invocation(
            &observed,
            &plan,
            path,
            &plan.volumes[0],
            Action::Start,
            42
        ));
    }

    #[test]
    fn idle_pid_alone_is_not_a_bootstrap_fence() {
        let (_, mut unit) = fixture();
        unit.active_state = "inactive".into();
        unit.sub_state = "dead".into();
        let service = unit.service.as_mut().unwrap();
        service.main_pid = 0;
        service.control_group.clear();
        assert!(!stopped(&unit)); // A queued job can still start a legacy creator.
        unit.job = UnitJob {
            id: 0,
            path: "/".into(),
        };
        assert!(stopped(&unit));
        unit.service.as_mut().unwrap().control_group = "/system.slice/lingering.service".into();
        assert!(!stopped(&unit));
    }

    fn stopped_fixture() -> (HostPlan, UnitObservation) {
        let (plan, mut unit) = fixture();
        unit.active_state = "inactive".into();
        unit.sub_state = "dead".into();
        unit.job = UnitJob {
            id: 0,
            path: "/".into(),
        };
        let service = unit.service.as_mut().unwrap();
        service.main_pid = 0;
        service.control_pid = 0;
        service.control_group.clear();
        (plan, unit)
    }

    #[test]
    fn publication_shared_owner_requires_only_the_desired_generation_commands() {
        let (old, mut unit) = stopped_fixture();
        let (desired, _) = fixture();
        let old_path = Path::new("/nix/store/00000000000000000000000000000000-plan");
        let desired_path = Path::new("/nix/store/00000000000000000000000000000000-candidate");
        assert!(offline_quota_unit(&unit, &old, old_path).is_ok());
        assert!(offline_quota_unit(&unit, &desired, desired_path).is_err());
        let service = unit.service.as_mut().unwrap();
        service.exec_start[0].argv[3] = desired_path.to_string_lossy().into_owned();
        // Reloading only one command does not establish the desired lifecycle.
        assert!(offline_quota_unit(&unit, &desired, desired_path).is_err());
        unit.service.as_mut().unwrap().exec_stop[0].argv[3] =
            desired_path.to_string_lossy().into_owned();
        assert!(offline_quota_unit(&unit, &desired, desired_path).is_ok());
        assert!(offline_quota_unit(&unit, &old, old_path).is_err());
        assert_eq!(
            quota_unit_names(&desired, Some(&old)),
            quota_unit_names(&desired, None)
        );
        unit.need_daemon_reload = true;
        assert!(offline_quota_unit(&unit, &desired, desired_path).is_err());
    }

    #[test]
    fn publication_removed_units_require_masks_and_complete_drain() {
        let (old, unit) = stopped_fixture();
        let (mut desired, _) = fixture();
        desired.volumes.remove(0);
        let path = Path::new("/nix/store/00000000000000000000000000000000-plan");
        let names = quota_unit_names(&desired, Some(&old));
        let removed = &old.volumes[0].units;
        for name in [&removed.owner, &removed.prepare, &removed.mount] {
            assert!(names.contains(name));
            let mut removed_unit = unit.clone();
            removed_unit.id = name.clone();
            // An idle legacy definition can be started again until masked.
            assert!(offline_quota_unit(&removed_unit, &desired, path).is_err());
            removed_unit.load_state = "masked".into();
            assert!(offline_quota_unit(&removed_unit, &desired, path).is_ok());
            for mutate in [
                |unit: &mut UnitObservation| unit.load_state = "not-found".into(),
                |unit: &mut UnitObservation| unit.job.id = 9,
                |unit: &mut UnitObservation| unit.active_state = "deactivating".into(),
                |unit: &mut UnitObservation| unit.need_daemon_reload = true,
                |unit: &mut UnitObservation| unit.service.as_mut().unwrap().main_pid = 41,
                |unit: &mut UnitObservation| unit.service.as_mut().unwrap().control_pid = 42,
                |unit: &mut UnitObservation| {
                    unit.service.as_mut().unwrap().control_group =
                        "/system.slice/remaining.service".into()
                },
            ] {
                let mut changed = removed_unit.clone();
                mutate(&mut changed);
                assert!(offline_quota_unit(&changed, &desired, path).is_err());
            }
        }
    }

    #[test]
    fn publication_mount_inventory_covers_removed_and_desired_paths() {
        let (old, mut unit) = stopped_fixture();
        let (mut desired, _) = fixture();
        for volume in &mut desired.volumes {
            volume.spec.mount_path =
                Path::new("/var/lib/replacement").join(&volume.spec.allocation_id);
        }
        unit.id = "fixture.mount".into();
        unit.service = None;
        unit.mount = Some(crate::admission_systemd::MountObservation {
            what: "tmpfs".into(),
            where_path: old.volumes[0]
                .spec
                .mount_path
                .to_string_lossy()
                .into_owned(),
            filesystem_type: "tmpfs".into(),
        });
        for (active_state, sub_state) in [("active", "mounted"), ("inactive", "dead")] {
            unit.active_state = active_state.into();
            unit.sub_state = sub_state.into();
            // The former mount is outside the new plan, but still part of the
            // transition's protected paths even when no old unit file remains.
            assert!(maintenance_mount(&unit, &desired, None, &[]).is_ok());
            assert!(maintenance_mount(&unit, &desired, Some(&old), &[]).is_err());
        }
        unit.mount.as_mut().unwrap().where_path = desired.volumes[0]
            .spec
            .mount_path
            .to_string_lossy()
            .into_owned();
        assert!(maintenance_mount(&unit, &desired, Some(&old), &[]).is_err());
        unit.load_state = "masked".into();
        unit.mount = None;
        assert!(maintenance_mount(&unit, &desired, Some(&old), &[]).is_ok());
        unit.job.id = 9;
        assert!(maintenance_mount(&unit, &desired, Some(&old), &[]).is_err());
    }

    #[test]
    fn historical_dynamic_mounts_remain_protected_after_template_removal() {
        let (plan, mut unit) = stopped_fixture();
        let mut volume = plan.volumes[0].spec.clone();
        volume.mount_path =
            "/var/lib/tentaflake-environments/env-11111111111111111111111111111111/workspace"
                .into();
        unit.id = "dynamic.mount".into();
        unit.service = None;
        unit.mount = Some(crate::admission_systemd::MountObservation {
            what: "tmpfs".into(),
            where_path: volume.mount_path.to_string_lossy().into_owned(),
            filesystem_type: "tmpfs".into(),
        });
        for state in ["active", "inactive"] {
            unit.active_state = state.into();
            assert!(maintenance_mount(&unit, &plan, None, &[]).is_ok());
            assert!(maintenance_mount(&unit, &plan, None, std::slice::from_ref(&volume)).is_err());
        }
        unit.mount.as_mut().unwrap().where_path = "/var/lib/tentaflake-environments".into();
        assert!(maintenance_mount(&unit, &plan, None, std::slice::from_ref(&volume)).is_err());
        unit.active_state = "active".into();
        assert!(
            maintenance_mount(&unit, &plan, None, std::slice::from_ref(&volume)).is_ok(),
            "stable ancestor is retained"
        );
    }

    #[test]
    fn bootstrap_loop_inventory_fails_closed_for_orphans_deleted_images_and_malformed_data() {
        assert!(no_backing_loops(br#"{"loopdevices":[]}"#).is_ok());
        assert!(no_backing_loops(br#"{"loopdevices":[{"name":"/dev/loop12","back-file":"/var/lib/unrelated/image"}]}"#).is_err());
        for backing in [
            "/var/lib/tentaflake-workspace-volumes/orphan.img",
            "/var/lib/tentaflake-workspace-volumes/state/removed.img (deleted)",
            "",
        ] {
            let bytes = serde_json::to_vec(
                &serde_json::json!({"loopdevices":[{"name":"/dev/loop0", "back-file":backing}]}),
            )
            .unwrap();
            assert!(no_backing_loops(&bytes).is_err());
        }
        for bytes in [
            b"{}".as_slice(),
            b"null",
            br#"{"loopdevices":null}"#,
            br#"{"loopdevices":[{"name":"/dev/loop0"}]}"#,
        ] {
            assert!(no_backing_loops(bytes).is_err());
        }
    }

    #[test]
    fn maintenance_refuses_legacy_mount_jobs_alias_files_and_future_overmounts() {
        let (plan, mut unit) = fixture();
        unit.id = "run.mount".into();
        unit.active_state = "active".into();
        unit.sub_state = "mounted".into();
        unit.job = UnitJob {
            id: 0,
            path: "/".into(),
        };
        unit.service = None;
        unit.mount = Some(crate::admission_systemd::MountObservation {
            what: "tmpfs".into(),
            where_path: "/run".into(),
            filesystem_type: "tmpfs".into(),
        });
        assert!(maintenance_mount(&unit, &plan, None, &[]).is_ok());
        let mut pts = unit.clone();
        pts.id = "dev-pts.mount".into();
        let mount = pts.mount.as_mut().unwrap();
        mount.what = "devpts".into();
        mount.where_path = "/dev/pts".into();
        mount.filesystem_type = "devpts".into();
        assert!(maintenance_mount(&pts, &plan, None, &[]).is_ok());
        for source in ["overlay", "/"] {
            let mut overlay = unit.clone();
            let mount = overlay.mount.as_mut().unwrap();
            mount.what = source.into();
            mount.filesystem_type = "overlay".into();
            assert!(
                maintenance_mount(&overlay, &plan, None, &[])
                    .unwrap_err()
                    .starts_with("overlay native mounts are unsupported")
            );
        }
        for kind in ["nfs", "cifs", "fuse.sshfs", "unknown"] {
            let mut unsupported = unit.clone();
            let mount = unsupported.mount.as_mut().unwrap();
            mount.what = "scheme://private-credential@host/path".into();
            mount.filesystem_type = kind.into();
            assert_eq!(
                maintenance_mount(&unsupported, &plan, None, &[]).unwrap_err(),
                format!(
                    "native mount source cannot be proven independent of backing images: unit=run.mount, type={kind}, source=other"
                )
            );
        }
        // Ordinary directory bind sources remain available.
        let mut bind = unit.clone();
        bind.mount.as_mut().unwrap().what = "/".into();
        bind.mount.as_mut().unwrap().filesystem_type = "none".into();
        assert!(maintenance_mount(&bind, &plan, None, &[]).is_ok());
        for mutate in [
            |unit: &mut UnitObservation| unit.job.id = 5,
            |unit: &mut UnitObservation| unit.active_state = "activating".into(),
            |unit: &mut UnitObservation| unit.need_daemon_reload = true,
            // /proc/self/exe is a real regular-file alias, not a managed path.
            |unit: &mut UnitObservation| {
                unit.mount.as_mut().unwrap().what = "/proc/self/exe".into()
            },
            |unit: &mut UnitObservation| {
                unit.mount.as_mut().unwrap().what = "/missing-native-mount-source".into()
            },
            |unit: &mut UnitObservation| {
                unit.active_state = "inactive".into();
                unit.mount.as_mut().unwrap().where_path = "/var/lib".into();
            },
            |unit: &mut UnitObservation| {
                let mount = unit.mount.as_mut().unwrap();
                mount.what = "relative.img".into();
                mount.filesystem_type = "btrfs".into();
            },
        ] {
            let mut changed = unit.clone();
            mutate(&mut changed);
            assert!(maintenance_mount(&changed, &plan, None, &[]).is_err());
        }
        unit.mount.as_mut().unwrap().where_path = plan.volumes[0]
            .spec
            .mount_path
            .to_string_lossy()
            .into_owned();
        assert!(maintenance_mount(&unit, &plan, None, &[]).is_err());
        unit.mount.as_mut().unwrap().where_path = "/".into();
        assert!(maintenance_mount(&unit, &plan, None, &[]).is_ok());
    }
}
