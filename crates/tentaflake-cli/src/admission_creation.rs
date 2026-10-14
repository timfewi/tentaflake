//! Once-only stopped storage provisioning. HTTP queueing/recovery remain closed
//! until their collectors and integration tests are connected to this owner.
use crate::admission_host::{HostPlan, load_plan};
use crate::admission_runtime::{self, LEDGER, authority, installed, require_root};
use crate::admission_storage::{CreationTargets, VolumeSpec, check_creation_space};
use crate::admission_systemd::{self, UnitObservation};
use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};
use tentaflake_admission::StorageKind;
use tentaflake_admission::host_fence::{self, HostFence};
use tentaflake_admission::ledger::api::{
    ApiContext, ApiExecution, ApiOperation, CreateClaim, EnvironmentRecord,
};
use tentaflake_admission::ledger::{Ledger, PublishedAuthority, SupervisorIdentity};

fn unit(operation: &str) -> Result<String, String> {
    let name = format!("tentaflake-environment-create@{operation}.service");
    if admission_systemd::creation_operation(&name) != Some(operation) {
        return Err("invalid creation operation identifier".into());
    }
    Ok(name)
}

fn guarded_unit(observed: &UnitObservation, plan: &HostPlan, path: &Path, operation: &str) -> bool {
    let Some(service) = &observed.service else {
        return false;
    };
    let [command] = service.exec_start.as_slice() else {
        return false;
    };
    unit(operation).is_ok_and(|name| observed.id == name)
        && observed.load_state == "loaded"
        && !observed.need_daemon_reload
        && service.service_type == "oneshot"
        && !service.remain_after_exit
        && service.restart == "no"
        && service.kill_mode == "control-group"
        && (1..=300_000_000).contains(&service.timeout_start_usec)
        && (1..=120_000_000).contains(&service.timeout_stop_usec)
        && service.exec_start_pre.is_empty()
        && service.exec_start_post.is_empty()
        && service.exec_stop.is_empty()
        && service.exec_stop_post.is_empty()
        && service.exec_reload.is_empty()
        && service.exec_condition.is_empty()
        && Path::new(&command.path) == plan.tools.cli
        && !command.ignore_failure
        && command.argv
            == [
                plan.tools.cli.to_string_lossy().as_ref(),
                "admission",
                "create-execute",
                path.to_string_lossy().as_ref(),
                operation,
            ]
}

fn valid_invocation(
    observed: &UnitObservation,
    plan: &HostPlan,
    path: &Path,
    operation: &str,
    pid: u32,
) -> bool {
    guarded_unit(observed, plan, path, operation)
        && observed.active_state == "activating"
        && observed.sub_state == "start"
        && observed.invocation_id != [0; 16]
        && pid != 0
        && observed
            .service
            .as_ref()
            .is_some_and(|service| service.main_pid == pid && service.control_pid == 0)
}

fn invocation(plan: &HostPlan, path: &Path, operation: &str) -> Result<ApiExecution, String> {
    let name = unit(operation)?;
    let observed = admission_systemd::inspect_unit(&plan.tools.busctl, &name)?;
    let pid = std::process::id();
    if !valid_invocation(&observed, plan, path, operation, pid) {
        return Err("creation requires the exact owned systemd invocation".into());
    }
    let service = observed.service.as_ref().expect("validated service");
    if service.control_group.is_empty()
        || service.control_group == "/"
        || fs::read_to_string("/proc/self/cgroup")
            .map_err(|_| "cannot inspect creation cgroup")?
            .trim()
            != format!("0::{}", service.control_group)
        || fs::canonicalize(&plan.tools.cli)
            .map_err(|_| "cannot inspect declared creation executable")?
            != fs::canonicalize("/proc/self/exe")
                .map_err(|_| "cannot inspect creation executable")?
    {
        return Err("creation process does not match its owned executable and cgroup".into());
    }
    Ok(ApiExecution {
        boot_id: fs::read_to_string("/proc/sys/kernel/random/boot_id")
            .map_err(|_| "cannot inspect creation boot identity")?
            .trim()
            .into(),
        supervisor: SupervisorIdentity {
            unit: name,
            invocation_id: observed
                .invocation_id
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect(),
            pid,
            start_time_ticks: admission_systemd::process_start(pid)?,
        },
    })
}

fn now() -> Result<u64, String> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|time| time.as_secs())
        .map_err(|_| "creation clock is unavailable".into())
}

fn helper(path: &Path, slot: &str, fence: &HostFence) -> Result<(), String> {
    let mut command = Command::new(path);
    command
        .arg(slot)
        .env_clear()
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    // The exact bounded systemd owner kills its entire cgroup on failure. Every
    // helper inherits the fence, including descendants surviving a parent crash.
    fence.inherit(&mut command)?;
    if !command
        .status()
        .map_err(|_| "cannot execute creation helper")?
        .success()
    {
        return Err("creation helper failed; preserve charged storage for recovery".into());
    }
    Ok(())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Mounts {
    filesystems: Vec<Mount>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Mount {
    target: String,
}

fn unmounted(bytes: &[u8]) -> Result<(), String> {
    let mounts: Mounts =
        serde_json::from_slice(bytes).map_err(|_| "invalid creation mount observation")?;
    if !mounts.filesystems.iter().any(|mount| mount.target == "/")
        || mounts.filesystems.iter().any(|mount| {
            !Path::new(&mount.target).is_absolute()
                || Path::new(&mount.target).starts_with("/var/lib/tentaflake-environments")
        })
    {
        return Err("dynamic storage mounts must be absent during stopped creation".into());
    }
    Ok(())
}

pub(crate) fn stopped_storage(plan: &HostPlan) -> Result<(), String> {
    // Initial support requires an empty Docker inventory and no attached loop
    // devices. This does not claim that unrelated host jobs have been drained.
    admission_runtime::offline_workloads(plan)?;
    unmounted(&admission_runtime::observe(
        &plan.tools.findmnt,
        &["--json", "--list", "--output", "TARGET"],
    )?)
}

pub(crate) fn execute(path: &Path, operation: &str) -> Result<u8, String> {
    require_root()?;
    let plan = load_plan(path)?;
    invocation(&plan, path, operation)?;
    let fence = host_fence::exclusive()?;
    let execution = invocation(&plan, path, operation)?;
    installed(path)?;
    let published = authority(&plan, path);
    let ledger = Ledger::open(Path::new(LEDGER)).map_err(|_| "creation ledger unavailable")?;
    let (environment, original) = ledger
        .api_environments(&published)
        .map_err(|_| "cannot inspect creation ownership")?
        .into_iter()
        .find(|(environment, _)| environment.operation_id == operation)
        .ok_or("unknown creation operation")?;
    let context = ApiContext {
        authority: published.clone(),
        host: environment.host,
        principal: environment.spec.instance.owner_id.clone(),
        // This is a privileged accepted-work handoff, not a fresh HTTP mutation.
        epoch: "supervisor".into(),
        now: now()?,
    };
    let claim = ledger
        .api_claim_create(
            &context,
            operation,
            &execution,
            |current, snapshot, recorded, expected_execution| {
                installed(path)?;
                if current != &original.authority
                    || current != &published
                    || invocation(&plan, path, operation)? != *expected_execution
                {
                    return Err("creation authority or supervisor changed".into());
                }
                let reconstructed = crate::admission_template::verify_recorded(&plan, recorded)?;
                stopped_storage(&plan)?;
                let mut specs: Vec<_> = plan
                    .volumes
                    .iter()
                    .map(|volume| volume.spec.clone())
                    .collect();
                specs.extend(reconstructed.volumes.iter().cloned());
                check_creation_space(
                    &specs,
                    &snapshot.storage,
                    CreationTargets::Reserved(&reconstructed.volumes),
                    plan.physical_space_headroom_bytes,
                )?;
                // Queueing must already have retained this closure. Re-registration
                // here is conservative; it never turns a historical claim into work.
                crate::admission_retention::retain(
                    current,
                    plan.tools
                        .nix_store
                        .as_deref()
                        .ok_or("missing creation retention tool")?,
                    &fence,
                )?;
                stopped_storage(&plan)?;
                installed(path)?;
                if invocation(&plan, path, operation)? != *expected_execution {
                    return Err("creation invocation changed before claim".into());
                }
                Ok(())
            },
        )
        .map_err(|_| "creation claim refused; no helpers authorized")?;
    let CreateClaim::New(mut permit) = claim else {
        return Ok(0);
    };
    let reconstructed = crate::admission_template::verify_recorded(&plan, permit.environment())?;
    let template = &plan.creation_templates[&reconstructed.environment.template_id];
    let slot = &reconstructed.environment.instance.id;
    for volume in &reconstructed.volumes {
        let helpers = match volume.kind {
            StorageKind::State => &template.helpers.state,
            StorageKind::Workspace => &template.helpers.workspace,
        };
        permit
            .provision(volume.kind, |_| {
                installed(path)?;
                if invocation(&plan, path, operation)? != execution {
                    return Err("creation invocation changed before helper".into());
                }
                helper(&helpers.create, slot, &fence)?;
                helper(&helpers.mount, slot, &fence)?;
                helper(&helpers.owner, slot, &fence)?;
                helper(&helpers.unmount, slot, &fence)?;
                helper(&helpers.verify, slot, &fence)
            })
            .map_err(|_| "creation remains uncertain; preserve images and reservations")?;
    }
    stopped_storage(&plan)?;
    installed(path)?;
    if invocation(&plan, path, operation)? != execution {
        return Err("creation invocation changed before completion".into());
    }
    permit
        .complete(now()?)
        .map_err(|_| "cannot confirm stopped creation; preserve reservations")?;
    Ok(0)
}

/// Prepared under the outer host fence before entering any ledger callback.
/// Includes tombstones and revoked owners: historical storage paths never become
/// invisible merely because a template or principal was removed.
pub(crate) struct CreationInventory {
    pub volumes: Vec<VolumeSpec>,
    operations: BTreeSet<String>,
}

impl CreationInventory {
    /// Only for initial bootstrap after proving that the ledger does not exist.
    pub(crate) fn initial() -> Self {
        Self {
            volumes: Vec::new(),
            operations: BTreeSet::new(),
        }
    }
}

pub(crate) fn maintenance_inventory(
    ledger: &Ledger,
    published: &PublishedAuthority,
    _fence: &HostFence,
) -> Result<CreationInventory, String> {
    let records = ledger
        .api_environments(published)
        .map_err(|_| "cannot inspect historical creation ownership")?;
    inventory_records(records, load_plan)
}

fn inventory_records(
    records: Vec<(EnvironmentRecord, ApiOperation)>,
    mut load: impl FnMut(&Path) -> Result<HostPlan, String>,
) -> Result<CreationInventory, String> {
    let mut inventory = CreationInventory::initial();
    let mut plans = BTreeMap::new();
    let mut slots = BTreeSet::new();
    for (environment, operation) in records {
        let name = unit(&environment.operation_id)?;
        if environment.operation_id != operation.mutation.operation_id
            || environment.spec.instance.id != operation.environment_id
            || environment.host != operation.mutation.key.host
            || environment.spec.instance.owner_id != operation.mutation.key.principal
            || operation
                .execution
                .as_ref()
                .is_some_and(|execution| execution.supervisor.unit != name)
            || !inventory.operations.insert(environment.operation_id)
        {
            return Err("historical creation operation binding mismatch".into());
        }
        let path = Path::new(&operation.authority.manifest_path);
        if !plans.contains_key(path) {
            plans.insert(path.to_path_buf(), load(path)?);
        }
        let original = &plans[path];
        if authority(original, path) != operation.authority {
            return Err("historical creation authority mismatch".into());
        }
        let reconstructed =
            crate::admission_template::verify_recorded(original, &environment.spec)?;
        for volume in reconstructed.volumes {
            if !slots.insert(volume.allocation_id.clone()) {
                return Err("duplicate historical creation allocation".into());
            }
            inventory.volumes.push(volume);
        }
    }
    Ok(inventory)
}

/// Observe only. In particular this function never opens the ledger: publication
/// invokes it while holding the ledger lock and passes an already prepared view.
pub(crate) fn offline(
    plan: &HostPlan,
    path: &Path,
    inventory: &CreationInventory,
) -> Result<Vec<String>, String> {
    offline_inventory(
        plan,
        path,
        inventory,
        || admission_systemd::list_creation_units(&plan.tools.busctl),
        |name, required| admission_systemd::load_creation_unit(&plan.tools.busctl, name, required),
    )
}

fn offline_inventory(
    plan: &HostPlan,
    path: &Path,
    inventory: &CreationInventory,
    mut list: impl FnMut() -> Result<admission_systemd::UnitInventory, String>,
    mut load: impl FnMut(&str, bool) -> Result<UnitObservation, String>,
) -> Result<Vec<String>, String> {
    const PROBE: &str = "maintenance-probe";
    let before = list()?;
    let template = before.installed.get(admission_systemd::CREATION_TEMPLATE);
    let enabled = !plan.creation_templates.is_empty();
    let masked =
        template.is_some_and(|state| matches!(state.as_str(), "masked" | "masked-runtime"));
    if (enabled && (template.is_none() || masked)) || (!enabled && template.is_some() && !masked) {
        return Err(
            "creation template must match the installed plan or be masked after removal".into(),
        );
    }
    let mut names = before.names.clone();
    names.remove(admission_systemd::CREATION_TEMPLATE);
    let recorded: BTreeSet<_> = inventory
        .operations
        .iter()
        .map(|operation| unit(operation))
        .collect::<Result<_, _>>()?;
    if enabled {
        names.extend(recorded.iter().cloned());
        // Load metadata only: bare templates cannot be observed as services.
        // This fixed instance authenticates the template even with zero records;
        // it has no dispatch authority without a separately claimed operation.
        names.insert(unit(PROBE)?);
    }
    for name in &names {
        let operation =
            admission_systemd::creation_operation(name).ok_or("invalid creation inventory unit")?;
        let required = enabled || before.installed.contains_key(name);
        let observed = load(name, required)?;
        if observed.id != *name || !admission_runtime::stopped(&observed) {
            return Err("creation supervisors must be drained before maintenance".into());
        }
        if enabled && (recorded.contains(name) || operation == PROBE) {
            if !guarded_unit(&observed, plan, path, operation) {
                return Err("creation supervisor does not match the installed plan".into());
            }
        } else if !matches!(observed.load_state.as_str(), "masked" | "not-found") {
            return Err("removed or unknown creation supervisors must be masked or absent".into());
        }
    }
    let after = list()?;
    let mut expected = before.names.clone();
    expected.extend(names.iter().cloned());
    if before.installed != after.installed || !after.names.is_subset(&expected) {
        return Err("creation unit inventory changed during observation".into());
    }
    names.extend(recorded);
    if template.is_some() {
        names.insert(admission_systemd::CREATION_TEMPLATE.into());
    }
    Ok(names.into_iter().collect())
}

#[cfg(test)]
mod vm_fixture;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::admission_systemd::{ExecCommand, ServiceObservation, UnitJob};

    const OPERATION: &str = "op-0123456789abcdef0123456789abcdef";
    const PLAN: &str = "/nix/store/00000000000000000000000000000000-plan";

    fn fixture() -> (HostPlan, UnitObservation) {
        let plan = crate::admission_oci::tests::template_plan_fixture();
        let exec = ExecCommand {
            path: plan.tools.cli.to_string_lossy().into_owned(),
            argv: vec![
                plan.tools.cli.to_string_lossy().into_owned(),
                "admission".into(),
                "create-execute".into(),
                PLAN.into(),
                OPERATION.into(),
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
            id: unit(OPERATION).unwrap(),
            load_state: "loaded".into(),
            active_state: "activating".into(),
            sub_state: "start".into(),
            job: UnitJob {
                id: 10,
                path: "/org/freedesktop/systemd1/job/10".into(),
            },
            fragment_path: "/etc/systemd/system/tentaflake-environment-create@.service".into(),
            need_daemon_reload: false,
            invocation_id: [1; 16],
            mount: None,
            service: Some(ServiceObservation {
                main_pid: 41,
                control_pid: 0,
                control_group: "/system.slice/creation.service".into(),
                exec_start: vec![exec],
                exec_stop: vec![],
                service_type: "oneshot".into(),
                remain_after_exit: false,
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

    fn configured() -> HostPlan {
        let mut plan = crate::admission_oci::tests::template_plan_fixture();
        let helpers = serde_json::json!({"create":"/nix/store/00000000000000000000000000000000-create",
            "verify":"/nix/store/00000000000000000000000000000000-verify",
            "owner":"/nix/store/00000000000000000000000000000000-owner",
            "mount":"/nix/store/00000000000000000000000000000000-mount",
            "unmount":"/nix/store/00000000000000000000000000000000-unmount"});
        let template = serde_json::json!({"sourceInstance":"agent-fixture",
            "sourceRevision":plan.instances["agent-fixture"].revision,
            "revision":format!("sha256:{}", "b".repeat(64)),
            "reference":format!("docker.io/library/debian@sha256:{}", "a".repeat(64)),
            "image":{"id":"debian-13","distro":"debian","release":"13","label":"Debian","digest":format!("sha256:{}","a".repeat(64))},
            "principals":["operator"],"issuedActions":["environments.read","workspaces.read"],
            "helpers":{"state":helpers,"workspace":helpers}});
        plan.creation_templates
            .insert("base".into(), serde_json::from_value(template).unwrap());
        plan
    }

    fn record(plan: &HostPlan, nonce: u8, operation: &str) -> (EnvironmentRecord, ApiOperation) {
        let spec = crate::admission_template::reconstruct(
            plan,
            "base",
            [nonce; 16],
            "operator",
            "Fixture",
        )
        .unwrap()
        .environment;
        let row = EnvironmentRecord {
            host: "host".into(),
            spec,
            state: tentaflake_admission::ledger::api::EnvironmentState::Aborted,
            operation_id: operation.into(),
            created_at: 1,
            revision: 3,
        };
        let receipt = serde_json::from_value(serde_json::json!({
            "mutation":{"operationId":operation,"key":{"host":"host","principal":"operator","epoch":"epoch","method":"POST",
                "canonicalTarget":"/api/v1/environments","key":"00000000-0000-0000-0000-000000000001"},"requestDigest":"a".repeat(64)},
            "authority":authority(plan,Path::new(PLAN)), "environmentId":row.spec.instance.id,
            "state":"aborted","createdAt":1,"updatedAt":2,"queuedRevision":1,"claimedRevision":null,"updatedRevision":3,
            "execution":null,"storageOperations":[],"result":null
        })).unwrap();
        (row, receipt)
    }

    fn drained(name: &str) -> UnitObservation {
        let (_, mut observed) = fixture();
        observed.id = name.into();
        observed.active_state = "inactive".into();
        observed.sub_state = "dead".into();
        observed.job = UnitJob {
            id: 0,
            path: "/".into(),
        };
        let service = observed.service.as_mut().unwrap();
        service.main_pid = 0;
        service.control_pid = 0;
        service.control_group.clear();
        service.exec_start[0].argv[4] = admission_systemd::creation_operation(name).unwrap().into();
        observed
    }

    fn file_inventory(state: Option<&str>, names: &[String]) -> admission_systemd::UnitInventory {
        let mut inventory = admission_systemd::UnitInventory {
            names: names.iter().cloned().collect(),
            installed: BTreeMap::new(),
        };
        if let Some(state) = state {
            inventory
                .names
                .insert(admission_systemd::CREATION_TEMPLATE.into());
            inventory
                .installed
                .insert(admission_systemd::CREATION_TEMPLATE.into(), state.into());
        }
        inventory
    }

    #[test]
    fn historical_inventory_uses_original_plan_and_keeps_aborted_revoked_owners() {
        let plan = configured();
        let records = vec![record(&plan, 1, OPERATION), record(&plan, 2, "other")];
        let expected: Vec<_> = records
            .iter()
            .flat_map(|(environment, _)| {
                crate::admission_template::verify_recorded(&plan, &environment.spec)
                    .unwrap()
                    .volumes
            })
            .collect();
        let mut loads = 0;
        let inventory = inventory_records(records.clone(), |path| {
            assert_eq!(path, Path::new(PLAN));
            loads += 1;
            Ok(configured())
        })
        .unwrap();
        assert_eq!(loads, 1);
        assert_eq!(inventory.volumes, expected);
        assert_eq!(inventory.operations.len(), 2);
        for case in 0..7 {
            let mut records = records.clone();
            match case {
                0 => records[0].1.authority.revision = "changed".into(),
                1 => records[0].0.spec.instance.owner_id = "other".into(),
                2 => records[0].0.spec.instance.storage[0].binding_revision = "changed".into(),
                3 => records[0].1.environment_id = "other".into(),
                4 => records[0].1.mutation.operation_id = "other".into(),
                5 => records[0].0.spec.template_revision = "changed".into(),
                _ => records.push(records[0].clone()),
            }
            assert!(
                inventory_records(records, |_| Ok(configured())).is_err(),
                "case {case}"
            );
        }
        assert!(
            inventory_records(records.clone(), |_| Err("retained plan unavailable".into()))
                .is_err()
        );
        // Using a replacement plan after template removal must fail; callers
        // must load the original path even though no current owner can create.
        assert!(
            inventory_records(records, |_| Ok(
                crate::admission_oci::tests::template_plan_fixture()
            ))
            .is_err()
        );
    }

    #[test]
    fn zero_record_maintenance_authenticates_template_without_starting_any_unit() {
        let plan = configured();
        for case in 0..5 {
            let mut loaded = Vec::new();
            let result = offline_inventory(
                &plan,
                Path::new(PLAN),
                &CreationInventory::initial(),
                || Ok(file_inventory(Some("static"), &[])),
                |name, required| {
                    assert!(required);
                    assert_ne!(name, admission_systemd::CREATION_TEMPLATE);
                    loaded.push(name.to_owned());
                    let mut observed = drained(name);
                    match case {
                        1 => {
                            observed.service.as_mut().unwrap().exec_start[0].argv[3] =
                                "/old-plan".into()
                        }
                        2 => observed.service.as_mut().unwrap().main_pid = 41,
                        3 => observed.job.id = 1,
                        4 => observed.service.as_mut().unwrap().control_group = "/remaining".into(),
                        _ => (),
                    }
                    Ok(observed)
                },
            );
            assert_eq!(loaded, [unit("maintenance-probe").unwrap()]);
            assert_eq!(result.is_ok(), case == 0);
        }
    }

    #[test]
    fn removed_unknown_creators_and_changed_file_inventory_refuse_maintenance() {
        let plan = configured();
        let name = unit("unrecorded").unwrap();
        for case in 0..5 {
            let mut lists = 0;
            let result = offline_inventory(
                &plan,
                Path::new(PLAN),
                &CreationInventory::initial(),
                || {
                    lists += 1;
                    let mut files = file_inventory(Some("static"), std::slice::from_ref(&name));
                    if case == 3 && lists == 2 {
                        files.installed.insert(name.clone(), "static".into());
                    }
                    if case == 4 && lists == 2 {
                        files.names.insert(unit("new-creator").unwrap());
                    }
                    Ok(files)
                },
                |name, _| {
                    let mut observed = drained(name);
                    if name.ends_with("@unrecorded.service") && case != 1 {
                        observed.load_state = "masked".into();
                    }
                    if case == 2 {
                        observed.service.as_mut().unwrap().control_pid = 4;
                    }
                    Ok(observed)
                },
            );
            assert_eq!(result.is_ok(), case == 0, "case {case}");
        }
        let mut removed = configured();
        removed.creation_templates.clear();
        let mut inventory = CreationInventory::initial();
        inventory.operations.insert(OPERATION.into());
        for state in [None, Some("masked"), Some("static")] {
            let result = offline_inventory(
                &removed,
                Path::new(PLAN),
                &inventory,
                || Ok(file_inventory(state, &[])),
                |_, _| panic!("no concrete unit is present"),
            );
            if state == Some("static") {
                assert!(result.is_err());
            } else {
                assert!(result.unwrap().contains(&unit(OPERATION).unwrap()));
            }
        }
    }

    #[test]
    fn creation_refuses_replaced_owner_commands_unbounded_lifetimes_and_extra_hooks() {
        let (plan, observed) = fixture();
        assert!(valid_invocation(
            &observed,
            &plan,
            Path::new(PLAN),
            OPERATION,
            41
        ));
        assert!(!valid_invocation(
            &observed,
            &plan,
            Path::new(PLAN),
            OPERATION,
            42
        ));
        assert!(!valid_invocation(
            &observed,
            &plan,
            Path::new(PLAN),
            "other",
            41
        ));
        assert!(!valid_invocation(
            &observed,
            &plan,
            Path::new("/changed"),
            OPERATION,
            41
        ));
        for case in 0..22 {
            let mut changed = observed.clone();
            let service = changed.service.as_mut().unwrap();
            let extra = service.exec_start[0].clone();
            match case {
                0 => changed.load_state = "not-found".into(),
                1 => changed.need_daemon_reload = true,
                2 => changed.active_state = "active".into(),
                3 => changed.sub_state = "running".into(),
                4 => changed.invocation_id = [0; 16],
                5 => service.main_pid = 0,
                6 => service.control_pid = 41,
                7 => service.service_type = "simple".into(),
                8 => service.remain_after_exit = true,
                9 => service.restart = "on-failure".into(),
                10 => service.kill_mode = "process".into(),
                11 => service.timeout_start_usec = u64::MAX,
                12 => service.timeout_stop_usec = 0,
                13 => service.exec_start_pre.push(extra),
                14 => service.exec_start_post.push(extra),
                15 => service.exec_stop.push(extra),
                16 => service.exec_stop_post.push(extra),
                17 => service.exec_reload.push(extra),
                18 => service.exec_condition.push(extra),
                19 => service.exec_start[0].ignore_failure = true,
                20 => service.exec_start[0].path = "/changed".into(),
                21 => service.exec_start[0].argv.push("extra".into()),
                _ => unreachable!(),
            }
            assert!(
                !valid_invocation(&changed, &plan, Path::new(PLAN), OPERATION, 41),
                "case {case}"
            );
        }
        for value in ["", "../escape", "@unit", "x%y", "x:y", "x/y", "x y", "-x"] {
            assert!(unit(value).is_err());
        }
        assert!(unit(&"x".repeat(97)).is_err());
    }

    #[test]
    fn stopped_mount_evidence_is_complete_and_refuses_any_dynamic_mount() {
        assert!(unmounted(br#"{"filesystems":[{"target":"/"},{"target":"/var/lib"}]}"#).is_ok());
        for value in [
            r#"{"filesystems":[]}"#,
            r#"{"filesystems":[{"target":"relative"}]}"#,
            r#"{"filesystems":[{"target":"/"},{"target":"/var/lib/tentaflake-environments"}]}"#,
            r#"{"filesystems":[{"target":"/"},{"target":"/var/lib/tentaflake-environments/env-0/state/subdir"}]}"#,
            r#"{"filesystems":[{"target":"/","children":[]}]}"#,
            "null",
        ] {
            assert!(unmounted(value.as_bytes()).is_err());
        }
    }
}
