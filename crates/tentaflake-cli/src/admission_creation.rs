//! Once-only stopped storage provisioning. HTTP queueing/recovery remain closed
//! until their collectors and integration tests are connected to this owner.
use crate::admission_host::{HostPlan, load_plan};
use crate::admission_runtime::{self, LEDGER, authority, installed, require_root};
use crate::admission_storage::{CreationTargets, check_creation_space};
use crate::admission_systemd::{self, UnitObservation};
use serde::Deserialize;
use std::fs;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};
use tentaflake_admission::StorageKind;
use tentaflake_admission::host_fence::{self, HostFence};
use tentaflake_admission::ledger::api::{ApiContext, ApiExecution, CreateClaim};
use tentaflake_admission::ledger::{Ledger, SupervisorIdentity};

fn unit(operation: &str) -> Result<String, String> {
    if operation.is_empty()
        || operation.len() > 96
        || !operation.as_bytes()[0].is_ascii_alphanumeric()
        || !operation
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b))
    {
        return Err("invalid creation operation identifier".into());
    }
    Ok(format!("tentaflake-environment-create@{operation}.service"))
}

fn valid_invocation(
    observed: &UnitObservation,
    plan: &HostPlan,
    path: &Path,
    operation: &str,
    pid: u32,
) -> bool {
    let Some(service) = &observed.service else {
        return false;
    };
    let [command] = service.exec_start.as_slice() else {
        return false;
    };
    unit(operation).is_ok_and(|name| observed.id == name)
        && observed.load_state == "loaded"
        && !observed.need_daemon_reload
        && observed.active_state == "activating"
        && observed.sub_state == "start"
        && observed.invocation_id != [0; 16]
        && pid != 0
        && service.main_pid == pid
        && service.control_pid == 0
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

fn stopped_storage(plan: &HostPlan) -> Result<(), String> {
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
                if current != &original
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
