//! Recorded decisions, diagnostic inventory and root-owned storage lifecycle.

use crate::OutputMode;
use std::fs::OpenOptions;
use std::io::Read;
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;
use tentaflake_admission::{Decision, RecordedAdmission, decide};

const MAX_SNAPSHOT_BYTES: u64 = 4 * 1024 * 1024;

pub(crate) fn run(args: &[String], mode: OutputMode) -> Result<u8, String> {
    match args {
        [command, file, action, rest @ ..] if command == "worker-run" => {
            return crate::admission_worker::run(Path::new(file), action, rest);
        }
        [command, file] if command == "worker-load" => {
            return crate::admission_worker::load_image(Path::new(file));
        }
        [command, file, revision, claimed] if command == "worker-load-recover" => {
            return crate::admission_worker::recover_image(Path::new(file), revision, claimed);
        }
        [command, file, id] if command == "image-acquire" => {
            return crate::admission_oci::acquire(Path::new(file), id);
        }
        [command, file, id] if command == "runtime-start" || command == "runtime-stop" => {
            return crate::admission_oci::lifecycle(
                Path::new(file),
                id,
                command == "runtime-start",
            );
        }
        [command, rest @ ..] if command == "publication" => {
            return crate::admission_publication::run(rest, mode);
        }
        [command, file] if command == "bootstrap" => {
            return crate::admission_runtime::bootstrap(Path::new(file));
        }
        [command, file] if command == "bootstrap-fence" => {
            return crate::admission_bootstrap::fence(Path::new(file));
        }
        [command, file, id] if command == "volume-start" || command == "volume-stop" => {
            return crate::admission_runtime::lifecycle(
                Path::new(file),
                id,
                command == "volume-start",
            );
        }
        [command, file, id] if command == "storage-recover" => {
            return crate::admission_runtime::recover(Path::new(file), id);
        }
        _ => {}
    }
    let [command, file] = args else {
        return Err("admission requires explain <snapshot-file>, inventory|bootstrap <generated-store-plan>, or storage-recover <generated-store-plan> <allocation-id>".into());
    };
    if command == "inventory" {
        return crate::admission_host::inventory(Path::new(file), mode);
    }
    if command != "explain" {
        return Err("unsupported admission command".into());
    }
    let mut decision = explain_file(Path::new(file))?;
    if mode.hide
        && let Some(reservation) = &mut decision.reservation
    {
        reservation.instance_id = "redacted".into();
        reservation.owner_id = "redacted".into();
        reservation.instance_revision = "redacted".into();
    }
    println!(
        "{}",
        serde_json::to_string(&decision).map_err(|_| "cannot encode admission decision")?
    );
    Ok(u8::from(!decision.allowed))
}

fn explain_file(path: &Path) -> Result<Decision, String> {
    // O_NONBLOCK prevents a FIFO from hanging before its type can be rejected.
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
        .map_err(|_| "cannot open admission snapshot as a regular file")?;
    let metadata = file
        .metadata()
        .map_err(|_| "cannot inspect admission snapshot")?;
    if !metadata.is_file() || metadata.len() > MAX_SNAPSHOT_BYTES {
        return Err("admission snapshot must be a regular file of at most 4 MiB".into());
    }
    let mut bytes = Vec::new();
    file.take(MAX_SNAPSHOT_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "cannot read admission snapshot")?;
    explain_bytes(&bytes)
}

fn explain_bytes(bytes: &[u8]) -> Result<Decision, String> {
    if bytes.len() as u64 > MAX_SNAPSHOT_BYTES {
        return Err("admission snapshot exceeds 4 MiB".into());
    }
    let recorded: RecordedAdmission = serde_json::from_slice(bytes)
        .map_err(|_| "invalid recorded admission snapshot; see docs/10-security-profiles.md")?;
    Ok(decide(
        &recorded.snapshot,
        &recorded.request,
        recorded.now_unix_seconds,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_unbounded_or_malformed_input_without_echoing_it() {
        assert_eq!(
            explain_bytes(&vec![b' '; MAX_SNAPSHOT_BYTES as usize + 1]).unwrap_err(),
            "admission snapshot exceeds 4 MiB"
        );
        let error = explain_bytes(br#"{"unexpected":"private-value"}"#).unwrap_err();
        assert!(!error.contains("private-value"));
        assert!(!error.contains("unexpected"));
        assert!(explain_bytes(b"null").is_err());
    }

    #[test]
    fn rejects_non_files_and_unknown_or_incomplete_commands() {
        assert!(explain_file(Path::new(".")).is_err());
        assert!(run(&["start".into(), "missing".into()], OutputMode::default()).is_err());
        assert!(run(&["explain".into()], OutputMode::default()).is_err());
    }

    #[test]
    fn recorded_decision_and_principal_mismatch_use_the_shared_core() {
        let bytes = include_bytes!("../../tentaflake-admission/tests/recorded-start.json");
        assert!(explain_bytes(bytes).unwrap().allowed);
        let mut value: serde_json::Value = serde_json::from_slice(bytes).unwrap();
        value["request"]["principalId"] = "different-owner".into();
        let decision = explain_bytes(&serde_json::to_vec(&value).unwrap()).unwrap();
        assert!(!decision.allowed);
        assert!(decision.reservation.is_none());
        assert!(
            decision
                .reasons
                .iter()
                .any(|reason| { reason.code == tentaflake_admission::ReasonCode::WrongPrincipal })
        );
    }
}
