//! Explicit stopped-host publication. This records authority, never installs a
//! system configuration, drains services or retries an uncertain host action.
use crate::OutputMode;
use crate::admission_creation::{CreationInventory, maintenance_inventory};
use crate::admission_host::{HostPlan, load_plan};
use crate::admission_runtime::{
    LEDGER, authority, installed, offline_units, offline_workloads, require_root,
};
use std::path::Path;
use tentaflake_admission::host_fence as admission_fence;
use tentaflake_admission::ledger::{
    Ledger, PublicationBegin, PublicationObservation, PublicationOutcome, PublicationReceipt,
    PublicationRequest, PublishedAuthority,
};
use tentaflake_admission::{ReservationSnapshot, ReservationState, StorageState, identifier};

pub(crate) fn run(args: &[String], mode: OutputMode) -> Result<u8, String> {
    require_root()?;
    let ledger = Ledger::open(Path::new(LEDGER))
        .map_err(|_| "admission ledger unavailable; never initialize over existing state")?;
    match args {
        [action] if action == "status" => {
            let mut status = ledger.publication_status().map_err(|error| error.to_string())?;
            if mode.hide {
                redact_authority(&mut status.published);
                if let Some(receipt) = &mut status.pending {
                    redact_receipt(receipt);
                }
                if let Some(load) = &mut status.worker_image_load {
                    redact_authority(&mut load.authority);
                    load.configuration = "redacted".into();
                }
            }
            emit(&status)
        }
        [action, path, operation, revision] if action == "begin" => {
            let revision = expected_revision(revision)?;
            let mut receipt = begin(&ledger, Path::new(path), operation, revision)?;
            if mode.hide {
                redact_receipt(&mut receipt);
            }
            emit(&receipt)
        }
        [action, operation, revision] if action == "finish" || action == "cancel" => {
            let revision = expected_revision(revision)?;
            let mut receipt = finalize(&ledger, operation, revision, action == "finish")?;
            if mode.hide {
                redact_receipt(&mut receipt);
            }
            emit(&receipt)
        }
        _ => Err("publication requires status, begin <candidate-store-plan> <operation-id> <expected-ledger-revision>, or finish|cancel <operation-id> <expected-ledger-revision>".into()),
    }
}

fn expected_revision(value: &str) -> Result<u64, String> {
    let revision = value.parse::<u64>().ok().filter(|revision| *revision != 0);
    if !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err("expected ledger revision must be a positive integer".into());
    }
    revision.ok_or_else(|| "expected ledger revision must be a positive integer".into())
}

fn recorded_plan(recorded: &PublishedAuthority) -> Result<HostPlan, String> {
    let path = Path::new(&recorded.manifest_path);
    let plan = load_plan(path)?;
    if authority(&plan, path) != *recorded {
        return Err("recorded publication authority does not match its immutable plan".into());
    }
    Ok(plan)
}

fn quiescent(snapshot: &ReservationSnapshot, expected: u64) -> Result<(), String> {
    if snapshot.revision != expected {
        return Err("admission ledger revision changed".into());
    }
    if !snapshot.complete
        || snapshot
            .entries
            .iter()
            .any(|entry| entry.state != ReservationState::Retained)
        || snapshot
            .storage
            .iter()
            .any(|allocation| allocation.state != StorageState::Present)
    {
        return Err("publication requires retained runtime claims and Present storage; recover uncertain state before beginning".into());
    }
    Ok(())
}

fn matching_retry(
    receipt: &PublicationReceipt,
    candidate_path: &Path,
    expected: u64,
) -> Result<(), String> {
    if candidate_path.as_os_str() != std::ffi::OsStr::new(&receipt.request.candidate.manifest_path)
        || receipt.request.expected_ledger_revision != expected
    {
        return Err("publication operation conflicts with its original request".into());
    }
    Ok(())
}

fn begin(
    ledger: &Ledger,
    candidate_path: &Path,
    operation: &str,
    expected: u64,
) -> Result<PublicationReceipt, String> {
    if !identifier(operation) {
        return Err("invalid publication operation identifier".into());
    }
    let fence = admission_fence::exclusive()?;
    if let Some(receipt) = ledger
        .publication_receipt(operation)
        .map_err(|error| error.to_string())?
    {
        matching_retry(&receipt, candidate_path, expected)?;
        // Historical visibility, including after publication. Never infer a new
        // request from today's published authority or repeat host actions.
        return Ok(receipt);
    }
    let candidate_plan = load_plan(candidate_path)?;
    let candidate = authority(&candidate_plan, candidate_path);
    let status = ledger
        .publication_status()
        .map_err(|error| error.to_string())?;
    if status.pending.is_some() {
        return Err("another publication is pending; explicitly finish or cancel it".into());
    }
    if status.published.generation != candidate.generation || status.published == candidate {
        return Err(
            "publication requires a different plan in the same durable authority epoch".into(),
        );
    }
    let current = recorded_plan(&status.published)?;
    let current_path = Path::new(&status.published.manifest_path);
    let snapshot = ledger.snapshot().map_err(|error| error.to_string())?;
    // Once Pending exists, normal storage recovery is intentionally blocked.
    // Refuse uncertain claims now, rather than creating an unfinishable change.
    quiescent(&snapshot, expected)?;
    let dynamic = maintenance_inventory(ledger, &status.published, &fence)?;
    observe_offline(&current, current_path, None, &dynamic)?;
    let nix_store = current
        .tools
        .nix_store
        .as_deref()
        .or(candidate_plan.tools.nix_store.as_deref())
        .ok_or("publication requires a plan with the Nix retention tool")?;
    crate::admission_retention::retain(&status.published, nix_store, &fence)?;
    crate::admission_retention::retain(&candidate, nix_store, &fence)?;
    observe_offline(&current, current_path, None, &dynamic)?;
    let request = PublicationRequest {
        operation_id: operation.into(),
        expected_ledger_revision: expected,
        expected_authority: status.published,
        candidate,
    };
    let receipt = ledger
        .begin_publication(&request)
        .map_err(|error| error.to_string())?;
    // Dropping the fence allows guarded ExecStop cleanup and the separately
    // authorized operator installation. Durable Pending still refuses starts.
    Ok(match receipt {
        PublicationBegin::New(receipt) | PublicationBegin::Existing(receipt) => receipt,
    })
}

fn observe_offline(
    desired: &HostPlan,
    desired_path: &Path,
    other: Option<&HostPlan>,
    dynamic: &CreationInventory,
) -> Result<(), String> {
    installed(desired_path)?;
    offline_units(desired, desired_path, other, dynamic)?;
    offline_workloads(desired)?;
    // Reobserve after the complete inventory. A reload, new queued job or
    // changed installation cannot be mistaken for the first observation.
    offline_units(desired, desired_path, other, dynamic)?;
    offline_workloads(desired)?;
    installed(desired_path)
}

fn finalize(
    ledger: &Ledger,
    operation: &str,
    expected: u64,
    publish: bool,
) -> Result<PublicationReceipt, String> {
    if !identifier(operation) {
        return Err("invalid publication operation identifier".into());
    }
    let fence = admission_fence::exclusive()?;
    // Prepare immutable historical paths before entering the ledger callback.
    // Terminal retries must not reload old manifests or repeat host observations.
    let dynamic = ledger
        .publication_receipt(operation)
        .map_err(|error| error.to_string())?
        .filter(|receipt| receipt.outcome == PublicationOutcome::Pending)
        .map(|receipt| maintenance_inventory(ledger, &receipt.request.expected_authority, &fence))
        .transpose()?;
    let observe = |receipt: &PublicationReceipt, _snapshot: &ReservationSnapshot| {
        let previous = recorded_plan(&receipt.request.expected_authority)?;
        let candidate = recorded_plan(&receipt.request.candidate)?;
        let (desired, other, reference) = if publish {
            (&candidate, &previous, &receipt.request.candidate)
        } else {
            (&previous, &candidate, &receipt.request.expected_authority)
        };
        observe_offline(
            desired,
            Path::new(&reference.manifest_path),
            Some(other),
            dynamic
                .as_ref()
                .ok_or("publication maintenance inventory unavailable")?,
        )?;
        Ok(PublicationObservation {
            installed: reference.clone(),
            mutators_fenced: true,
        })
    };
    // The ledger owns terminal retry semantics and does not invoke the observer
    // again for an identical completed request. Opposite outcomes conflict.
    if publish {
        ledger.finish_publication(operation, expected, observe)
    } else {
        ledger.cancel_publication(operation, expected, observe)
    }
    .map_err(|error| {
        format!("publication not completed: {error}; inspect status and preserve pending state")
    })
}

fn redact_authority(authority: &mut PublishedAuthority) {
    authority.generation = "redacted".into();
    authority.revision = "redacted".into();
    authority.manifest_path = "redacted".into();
}

fn redact_receipt(receipt: &mut PublicationReceipt) {
    receipt.request.operation_id = "redacted".into();
    redact_authority(&mut receipt.request.expected_authority);
    redact_authority(&mut receipt.request.candidate);
}

fn emit(value: &impl serde::Serialize) -> Result<u8, String> {
    println!(
        "{}",
        serde_json::to_string(value).map_err(|_| "cannot encode publication status")?
    );
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tentaflake_admission::ledger::PublicationOutcome;
    use tentaflake_admission::{
        MIN_STORAGE_BYTES, Reservation, Resources, StorageAllocation, StorageKind,
    };

    fn authority(revision: &str) -> PublishedAuthority {
        PublishedAuthority {
            generation: "private-host".into(),
            revision: revision.into(),
            manifest_path: format!("/nix/store/{}-private-{revision}", "0".repeat(32)),
        }
    }

    #[test]
    fn uncertain_storage_and_stale_revisions_cannot_enter_pending() {
        let mut snapshot = ReservationSnapshot {
            revision: 7,
            generation: "host".into(),
            complete: true,
            entries: vec![],
            storage: vec![StorageAllocation {
                id: "workspace-private".into(),
                kind: StorageKind::Workspace,
                bytes: MIN_STORAGE_BYTES,
                binding: None,
                state: StorageState::Unknown,
            }],
        };
        assert!(quiescent(&snapshot, 7).is_err());
        snapshot.storage[0].state = StorageState::Present;
        assert!(quiescent(&snapshot, 7).is_ok());
        assert!(quiescent(&snapshot, 6).is_err());
        snapshot.entries.push(Reservation {
            instance_id: "private-agent".into(),
            owner_id: "host-root".into(),
            instance_revision: "definition-1".into(),
            state: ReservationState::Retained,
            resources: Resources::default(),
        });
        assert!(quiescent(&snapshot, 7).is_ok());
        for state in [
            ReservationState::Reserved,
            ReservationState::Active,
            ReservationState::Unknown,
        ] {
            snapshot.entries[0].state = state;
            assert!(quiescent(&snapshot, 7).is_err());
        }
        snapshot.entries.clear();
        snapshot.complete = false;
        assert!(quiescent(&snapshot, 7).is_err());
        for value in ["", "0", "-1", "+1", "1.0", "18446744073709551616"] {
            assert!(expected_revision(value).is_err());
        }
        assert_eq!(expected_revision("7").unwrap(), 7);
    }

    #[test]
    fn historical_retry_uses_original_request_and_redaction_keeps_outcome() {
        let candidate = authority("new-policy");
        let mut receipt = PublicationReceipt {
            request: PublicationRequest {
                operation_id: "private-change".into(),
                expected_ledger_revision: 4,
                expected_authority: authority("old-policy"),
                candidate: candidate.clone(),
            },
            outcome: PublicationOutcome::Published,
            begun_revision: 5,
            updated_revision: 6,
        };
        let candidate_path = Path::new(&candidate.manifest_path);
        assert!(matching_retry(&receipt, candidate_path, 4).is_ok());
        assert!(matching_retry(&receipt, candidate_path, 6).is_err());
        assert!(
            matching_retry(
                &receipt,
                Path::new(&authority("another-policy").manifest_path),
                4
            )
            .is_err()
        );
        assert!(
            matching_retry(
                &receipt,
                Path::new(&(candidate.manifest_path.clone() + "/")),
                4
            )
            .is_err()
        );
        redact_receipt(&mut receipt);
        let encoded = serde_json::to_string(&receipt).unwrap();
        assert!(!encoded.contains("private"));
        assert!(!encoded.contains("policy"));
        assert!(!encoded.contains("/nix/store"));
        assert!(encoded.contains("published"));
        assert_eq!(receipt.updated_revision, 6);
    }
}
