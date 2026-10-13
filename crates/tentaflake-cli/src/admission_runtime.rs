//! Host-owned storage transactions. Only the exact systemd quota invocation may
//! mutate images; an exclusive inherited fence also covers surviving helpers.
use crate::admission_fence::{self, HostFence};
use crate::admission_host::{HostPlan, Volume, load_plan, store_path};
use crate::admission_storage::{
    VolumeSpec, check_creation_space, ensure_backing_roots, scan_inventory,
};
use crate::admission_systemd::{self, ExecCommand, UnitObservation};
use serde::Deserialize;
use std::collections::BTreeSet;
use std::fs;
use std::os::unix::fs::{FileTypeExt, MetadataExt};
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Duration;
use tentaflake_admission::ledger::{Ledger, PublishedAuthority, StorageClaimOutcome};
use tentaflake_admission::{
    AdmissionRequest, ExpectedRevisions, ReservationSnapshot, SCHEMA_VERSION,
    StorageAdmissionSnapshot, StorageAllocation, StorageRequest, StorageState,
};

const LEDGER: &str = "/var/lib/tentaflake-admission";
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

fn require_root() -> Result<(), String> {
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

fn installed(path: &Path) -> Result<(), String> {
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

fn authority(plan: &HostPlan, path: &Path) -> PublishedAuthority {
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

fn observe(path: &Path, args: &[&str]) -> Result<Vec<u8>, String> {
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
                    &volume.spec,
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

fn stopped(observed: &UnitObservation) -> bool {
    matches!(observed.active_state.as_str(), "inactive" | "failed")
        && observed.job.id == 0
        && !observed.need_daemon_reload
        && observed.service.as_ref().is_none_or(|service| {
            service.main_pid == 0 && service.control_pid == 0 && service.control_group.is_empty()
        })
}

fn offline_units(plan: &HostPlan, path: &Path) -> Result<(), String> {
    let mut expected = BTreeSet::new();
    for volume in &plan.volumes {
        let owner = admission_systemd::load_unit(&plan.tools.busctl, &volume.units.owner)?;
        if !guarded_unit(&owner, plan, path, volume) || !stopped(&owner) {
            return Err(
                "quota supervisors must be stopped with the exact guarded plan loaded".into(),
            );
        }
        expected.insert(volume.units.owner.as_str());
        expected.insert(volume.units.prepare.as_str());
        for name in [&volume.units.prepare, &volume.units.mount] {
            let observed = admission_systemd::load_unit(&plan.tools.busctl, name)?;
            if observed.load_state != "masked" || !stopped(&observed) {
                return Err(
                    "legacy quota prepare and mount units must be masked and drained".into(),
                );
            }
        }
    }
    for name in admission_systemd::list_quota_units(&plan.tools.busctl)? {
        if !expected.contains(name.as_str()) {
            let observed = admission_systemd::load_unit(&plan.tools.busctl, &name)?;
            if observed.load_state != "masked" || !stopped(&observed) {
                return Err("an undeclared legacy quota unit remains available; explicit migration is required".into());
            }
        }
    }
    // Removed definitions can leave a native mount job whose prepare dependency
    // already completed. Inspect all available mount units, including unloaded
    // files, rather than inferring their absence from the new plan.
    for name in admission_systemd::list_mount_units(&plan.tools.busctl)? {
        let observed = admission_systemd::load_unit(&plan.tools.busctl, &name)?;
        maintenance_mount(&observed, plan)?;
    }
    Ok(())
}

fn overlaps(left: &Path, right: &Path) -> bool {
    left.starts_with(right) || right.starts_with(left)
}

fn maintenance_mount(observed: &UnitObservation, plan: &HostPlan) -> Result<(), String> {
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
                .any(|volume| overlaps(target, &volume.spec.mount_path))))
        || (active
            && plan
                .volumes
                .iter()
                .any(|volume| target.starts_with(&volume.spec.mount_path)))
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

fn offline_workloads(plan: &HostPlan) -> Result<(), String> {
    // First bootstrap is a maintenance operation. Do not stop an unrelated
    // workload or infer that an uninspectable daemon means no containers.
    if !observe(
        &plan.tools.docker,
        &[
            "--host",
            "unix:///var/run/docker.sock",
            "ps",
            "--quiet",
            "--no-trunc",
        ],
    )?
    .is_empty()
    {
        return Err("bootstrap or recovery requires all Docker workloads stopped".into());
    }
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
    let _fence = admission_fence::exclusive()?;
    installed(path)?;
    match fs::symlink_metadata(LEDGER) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        _ => return Err("bootstrap requires a genuinely absent ledger; never delete an existing ledger to retry".into()),
    }
    offline_units(&plan, path)?;
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
    offline_units(&plan, path)?;
    offline_workloads(&plan)?;
    // Only this fenced path may assert completeness. Diagnostic inventory never
    // supplies this flag, and callers cannot submit an inventory JSON file.
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
    let _fence = admission_fence::exclusive()?;
    installed(path)?;
    offline_units(&plan, path)?;
    offline_workloads(&plan)?;
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
        assert!(maintenance_mount(&unit, &plan).is_ok());
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
            assert!(maintenance_mount(&changed, &plan).is_err());
        }
        unit.mount.as_mut().unwrap().where_path = plan.volumes[0]
            .spec
            .mount_path
            .to_string_lossy()
            .into_owned();
        assert!(maintenance_mount(&unit, &plan).is_err());
        unit.mount.as_mut().unwrap().where_path = "/".into();
        assert!(maintenance_mount(&unit, &plan).is_ok());
    }
}
