//! Explicit root maintenance for accepted creations. Observation never resumes
//! helpers, repairs storage or grants a second creation permit.
use crate::admission_creation::{CreationInventory, maintenance_inventory};
use crate::admission_host::{HostPlan, load_plan};
use crate::admission_runtime::{self, LEDGER, authority, installed, require_root};
use crate::admission_storage::{VolumeSpec, scan_recovery_inventory};
use crate::admission_template::{ReconstructedCreation, verify_recorded};
use std::collections::BTreeSet;
use std::fs;
use std::path::Path;
use std::process::Command;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tentaflake_admission::host_fence::{self, HostFence};
use tentaflake_admission::ledger::api::{
    ApiExecution, ApiOperation, ApiOperationState, CreateAbortObservation,
    CreateAbortStorageObservation, CreateAbortStorageState, EnvironmentSpec,
};
use tentaflake_admission::ledger::{Ledger, StorageInventory};
use tentaflake_admission::{StorageAllocation, StorageKind};

fn boot() -> Result<String, String> {
    let value = fs::read_to_string("/proc/sys/kernel/random/boot_id")
        .map_err(|_| "cannot inspect recovery boot identity")?;
    let value = value.trim();
    if !canonical_boot(value) {
        return Err("invalid recovery boot identity".into());
    }
    Ok(value.into())
}

fn canonical_boot(value: &str) -> bool {
    value.len() == 36
        && value.bytes().enumerate().all(|(index, byte)| {
            if [8, 13, 18, 23].contains(&index) {
                byte == b'-'
            } else {
                byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)
            }
        })
        && value
            .bytes()
            .any(|byte| byte.is_ascii_hexdigit() && byte != b'0')
}

fn fenced_boot(
    state: ApiOperationState,
    execution: Option<&ApiExecution>,
    current: &str,
) -> Result<(), String> {
    if !canonical_boot(current) {
        return Err("invalid recovery boot identity".into());
    }
    match (state, execution) {
        (ApiOperationState::Queued, None) => Ok(()),
        (ApiOperationState::Unknown, Some(execution))
            if canonical_boot(&execution.boot_id) && execution.boot_id != current =>
        {
            Ok(())
        }
        // The receipt does not record the old cgroup. A changed boot proves its
        // exact process and descendants are gone, without guessing from a PID.
        _ => {
            Err("claimed creation recovery requires a different boot; preserve all charges".into())
        }
    }
}

fn same_charge(left: &StorageAllocation, right: &StorageAllocation) -> bool {
    left.id == right.id && left.kind == right.kind && left.bytes == right.bytes
}

fn matches_volume(row: &StorageAllocation, volume: &VolumeSpec) -> bool {
    row.id == volume.allocation_id
        && row.kind == volume.kind
        && row.bytes == volume.bytes
        && row.binding.as_ref().is_some_and(|binding| {
            binding.instance_id == volume.instance_id
                && binding.owner_id == volume.owner_id
                && binding.revision == volume.binding_revision
        })
}

/// Presence here is only a candidate for read-only filesystem verification.
/// Every observed image must already have an exact retained charge. An unrelated
/// removed static declaration may lack a reconstructed binding; its full charge
/// stays unchanged. Both target bindings are checked separately and exactly.
fn classify(
    inventory: &StorageInventory,
    retained: &[StorageAllocation],
    volumes: &[VolumeSpec; 2],
) -> Result<[CreateAbortStorageState; 2], String> {
    let mut seen = BTreeSet::new();
    if !inventory.complete
        || inventory.allocations.iter().any(|actual| {
            !seen.insert(&actual.id) || !retained.iter().any(|row| same_charge(actual, row))
        })
    {
        return Err("creation recovery inventory differs from retained storage".into());
    }
    let mut states = [CreateAbortStorageState::Absent; 2];
    for (index, volume) in volumes.iter().enumerate() {
        if !retained.iter().any(|row| matches_volume(row, volume)) {
            return Err(
                "creation recovery storage binding differs from its original template".into(),
            );
        }
        if let Some(actual) = inventory
            .allocations
            .iter()
            .find(|row| row.id == volume.allocation_id)
        {
            if !matches_volume(actual, volume) {
                return Err("creation recovery image has a different binding".into());
            }
            states[index] = CreateAbortStorageState::PresentStopped;
        }
    }
    Ok(states)
}

struct Collector<'a> {
    current: &'a HostPlan,
    path: &'a Path,
    original: HostPlan,
    creation: ReconstructedCreation,
    inventory: CreationInventory,
    retained: Vec<StorageAllocation>,
    operation: ApiOperation,
    fence: &'a HostFence,
}

impl Collector<'_> {
    fn stopped(&self, expected_boot: &str) -> Result<(), String> {
        if boot()? != expected_boot {
            return Err("boot changed during creation recovery".into());
        }
        installed(self.path)?;
        admission_runtime::offline_units(self.current, self.path, None, &self.inventory)?;
        // Includes Docker, loop devices and even unrecorded dynamic mounts.
        crate::admission_creation::stopped_storage(self.current)
    }

    fn observe(
        &self,
        operation: &ApiOperation,
        environment: &EnvironmentSpec,
        allocations: &[StorageAllocation],
    ) -> Result<CreateAbortObservation, String> {
        if *operation != self.operation
            || *environment != self.creation.environment
            || allocations.len() != 2
            || allocations.iter().any(|row| !self.retained.contains(row))
        {
            return Err("creation recovery receipt changed".into());
        }
        let current_boot = boot()?;
        fenced_boot(operation.state, operation.execution.as_ref(), &current_boot)?;
        self.stopped(&current_boot)?;
        let mut specs: Vec<_> = self
            .current
            .volumes
            .iter()
            .map(|volume| volume.spec.clone())
            .collect();
        specs.extend(self.inventory.volumes.iter().cloned());
        let before = scan_recovery_inventory(&specs)?;
        let states = classify(&before, &self.retained, &self.creation.volumes)?;
        if operation.state == ApiOperationState::Queued
            && states
                .iter()
                .any(|state| *state != CreateAbortStorageState::Absent)
        {
            return Err("unclaimed creation requires both slots absent".into());
        }
        let template = &self.original.creation_templates[&environment.template_id];
        for (volume, state) in self.creation.volumes.iter().zip(states) {
            if state == CreateAbortStorageState::PresentStopped {
                let helpers = match volume.kind {
                    StorageKind::State => &template.helpers.state,
                    StorageKind::Workspace => &template.helpers.workspace,
                };
                let mut command = Command::new(&helpers.verify);
                command.env_clear().arg(&environment.instance.id);
                self.fence.inherit(&mut command)?;
                // The immutable verifier execs btrfs check --readonly; a timeout
                // refuses recovery and keeps every charge. Never invoke owner,
                // create, mount, unmount, repair or deletion here.
                let output = crate::output_with_timeout(&mut command, Duration::from_secs(10))
                    .map_err(|_| "bounded read-only creation verification failed")?;
                if !output.status.success()
                    || output.stdout.len().saturating_add(output.stderr.len()) > 1024 * 1024
                {
                    return Err("existing creation image failed read-only verification".into());
                }
            }
        }
        if scan_recovery_inventory(&specs)? != before {
            return Err("creation inventory changed during recovery".into());
        }
        self.stopped(&current_boot)?;
        Ok(CreateAbortObservation {
            fenced_execution: operation.execution.clone(),
            allocations: std::array::from_fn(|index| CreateAbortStorageObservation {
                allocation_id: self.creation.volumes[index].allocation_id.clone(),
                state: states[index],
            }),
        })
    }
}

pub(crate) fn run(path: &Path, operation: &str, revision: &str, abort: bool) -> Result<u8, String> {
    let expected = revision
        .parse::<u64>()
        .ok()
        .filter(|value| *value != 0)
        .filter(|_| revision.bytes().all(|byte| byte.is_ascii_digit()))
        .ok_or("creation recovery requires a positive expected ledger revision")?;
    require_root()?;
    let fence = host_fence::exclusive()?;
    installed(path)?;
    let plan = load_plan(path)?;
    let current = authority(&plan, path);
    let ledger = Ledger::open(Path::new(LEDGER)).map_err(|_| "creation ledger unavailable")?;
    let (environment, recorded) = ledger
        .api_environments(&current)
        .map_err(|_| "cannot read current creation maintenance records")?
        .into_iter()
        .find(|(_, op)| op.mutation.operation_id == operation)
        .ok_or("unknown creation operation")?;
    // A terminal retry is history, including after publication. It must not
    // require the old template, filesystem observations or a second boot.
    let historical = if abort {
        recorded.abort.is_some()
    } else {
        recorded.recovery.is_some()
    };
    let collector = if historical {
        None
    } else {
        let snapshot = ledger
            .snapshot()
            .map_err(|_| "cannot inspect retained storage")?;
        if snapshot.revision != expected || !snapshot.complete {
            return Err("creation recovery requires the current complete ledger revision".into());
        }
        let original_path = Path::new(&recorded.authority.manifest_path);
        let original = load_plan(original_path)?;
        if authority(&original, original_path) != recorded.authority {
            return Err("creation recovery original authority mismatch".into());
        }
        let creation = verify_recorded(&original, &environment.spec)?;
        Some(Collector {
            current: &plan,
            path,
            original,
            creation,
            inventory: maintenance_inventory(&ledger, &current, &fence)?,
            retained: snapshot.storage,
            operation: recorded,
            fence: &fence,
        })
    };
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| "creation recovery clock unavailable")?
        .as_secs();
    let result = if abort {
        ledger.api_abort_create(&current, operation, expected, now, |op, env, rows| {
            let allocations: Vec<_> = rows.iter().map(|row| row.allocation.clone()).collect();
            collector
                .as_ref()
                .ok_or("historical abort cannot observe storage")?
                .observe(op, env, &allocations)
        })
    } else {
        ledger.api_recover_create(&current, operation, expected, now, |op, env, rows| {
            let observation = collector
                .as_ref()
                .ok_or("historical recovery cannot observe storage")?
                .observe(op, env, rows)?;
            if observation
                .allocations
                .iter()
                .any(|row| row.state != CreateAbortStorageState::PresentStopped)
            {
                return Err("successful recovery requires both exact existing images".into());
            }
            Ok(())
        })
    };
    result.map_err(|error| {
        format!("creation reconciliation refused; preserve retained storage: {error}")
    })?;
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tentaflake_admission::ledger::SupervisorIdentity;
    use tentaflake_admission::{StorageBinding, StorageState};

    #[test]
    fn claimed_work_requires_another_canonical_boot_but_queued_work_has_no_execution() {
        let original = "11111111-1111-4111-8111-111111111111";
        let next = "22222222-2222-4222-8222-222222222222";
        let execution = ApiExecution {
            boot_id: original.into(),
            supervisor: SupervisorIdentity {
                unit: "fixture.service".into(),
                invocation_id: "1".repeat(32),
                pid: 42,
                start_time_ticks: 10,
            },
        };
        assert!(fenced_boot(ApiOperationState::Unknown, Some(&execution), next).is_ok());
        assert!(fenced_boot(ApiOperationState::Unknown, Some(&execution), original).is_err());
        assert!(fenced_boot(ApiOperationState::Unknown, None, next).is_err());
        assert!(fenced_boot(ApiOperationState::Queued, None, original).is_ok());
        assert!(fenced_boot(ApiOperationState::Queued, Some(&execution), next).is_err());
        for state in [ApiOperationState::Succeeded, ApiOperationState::Aborted] {
            assert!(fenced_boot(state, Some(&execution), next).is_err());
        }
        for value in [
            "",
            "unknown",
            "00000000-0000-0000-0000-000000000000",
            "AAAAAAAA-aaaa-4aaa-8aaa-aaaaaaaaaaaa",
            "22222222_2222-4222-8222-222222222222",
            "22222222-2222-4222-8222-222222222222\n",
        ] {
            assert!(fenced_boot(ApiOperationState::Queued, None, value).is_err());
        }
    }

    fn volumes() -> [VolumeSpec; 2] {
        [StorageKind::State, StorageKind::Workspace].map(|kind| {
            let slot = if kind == StorageKind::State {
                "state"
            } else {
                "workspace"
            };
            VolumeSpec {
                instance_id: "env-fixture".into(),
                owner_id: "operator".into(),
                allocation_id: format!("{slot}:env-fixture"),
                kind,
                image_path: format!("/fixture/{slot}.img").into(),
                mount_path: format!("/fixture/{slot}").into(),
                bytes: 128 * 1024 * 1024,
                owner_uid: 1000,
                owner_gid: 1000,
                binding_revision: "binding-fixture".into(),
            }
        })
    }

    fn allocation(volume: &VolumeSpec) -> StorageAllocation {
        StorageAllocation {
            id: volume.allocation_id.clone(),
            kind: volume.kind,
            bytes: volume.bytes,
            binding: Some(StorageBinding {
                instance_id: volume.instance_id.clone(),
                owner_id: volume.owner_id.clone(),
                revision: volume.binding_revision.clone(),
            }),
            state: StorageState::Unknown,
        }
    }

    #[test]
    fn exact_absence_and_mixed_presence_preserve_charges_without_inventing_readiness() {
        let volumes = volumes();
        let retained = volumes.each_ref().map(allocation);
        for (rows, wanted) in [
            (vec![], [CreateAbortStorageState::Absent; 2]),
            (
                vec![retained[0].clone()],
                [
                    CreateAbortStorageState::PresentStopped,
                    CreateAbortStorageState::Absent,
                ],
            ),
            (
                vec![retained[1].clone()],
                [
                    CreateAbortStorageState::Absent,
                    CreateAbortStorageState::PresentStopped,
                ],
            ),
            (
                retained.to_vec(),
                [CreateAbortStorageState::PresentStopped; 2],
            ),
        ] {
            let inventory = StorageInventory {
                complete: true,
                allocations: rows,
            };
            assert_eq!(classify(&inventory, &retained, &volumes).unwrap(), wanted);
            assert!(
                retained
                    .iter()
                    .all(|row| row.state == StorageState::Unknown)
            );
        }
    }

    #[test]
    fn incomplete_duplicate_drifted_or_unaccounted_inventory_refuses_the_whole_pair() {
        let volumes = volumes();
        let retained = volumes.each_ref().map(allocation);
        let valid = StorageInventory {
            complete: true,
            allocations: retained.to_vec(),
        };
        for change in 0..10 {
            let mut inventory = valid.clone();
            match change {
                0 => inventory.complete = false,
                1 => inventory.allocations.push(retained[0].clone()),
                2 => inventory.allocations[0].id = "state:orphan".into(),
                3 => inventory.allocations[0].kind = StorageKind::Workspace,
                4 => inventory.allocations[0].bytes += 1,
                5 => inventory.allocations[0].binding = None,
                6 => inventory.allocations[0].binding.as_mut().unwrap().owner_id = "other".into(),
                7 => {
                    inventory.allocations[0]
                        .binding
                        .as_mut()
                        .unwrap()
                        .instance_id = "other".into()
                }
                8 => inventory.allocations[0].binding.as_mut().unwrap().revision = "other".into(),
                _ => {
                    let mut orphan = retained[0].clone();
                    orphan.id = "state:orphan".into();
                    inventory.allocations.push(orphan);
                }
            }
            assert!(
                classify(&inventory, &retained, &volumes).is_err(),
                "mutation {change}"
            );
        }
        assert!(classify(&valid, &retained[0..1], &volumes).is_err());
    }

    #[test]
    fn unrelated_removed_static_image_keeps_its_charge_without_a_current_binding() {
        let volumes = volumes();
        let mut retained = volumes.each_ref().map(allocation).to_vec();
        let mut removed = retained[0].clone();
        removed.id = "state:removed-static".into();
        removed.state = StorageState::Present;
        retained.push(removed.clone());
        removed.binding = None;
        removed.state = StorageState::Unknown;
        let mut inventory = StorageInventory {
            complete: true,
            allocations: vec![removed],
        };
        assert_eq!(
            classify(&inventory, &retained, &volumes).unwrap(),
            [CreateAbortStorageState::Absent; 2]
        );
        assert!(retained[2].binding.is_some());
        assert_eq!(retained[2].state, StorageState::Present);
        inventory.allocations[0].bytes += 1;
        assert!(classify(&inventory, &retained, &volumes).is_err());
    }
}
