//! Offline explanation of a recorded, non-secret admission snapshot.
//! This command does not collect live evidence, claim capacity or dispatch work.

use crate::OutputMode;
use std::fs::OpenOptions;
use std::io::Read;
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;
use tentaflake_admission::{Decision, RecordedAdmission, decide};

const MAX_SNAPSHOT_BYTES: u64 = 4 * 1024 * 1024;

pub(crate) fn run(args: &[String], mode: OutputMode) -> Result<u8, String> {
    let [command, file] = args else {
        return Err("admission requires: explain <snapshot-file> [--hide]".into());
    };
    if command != "explain" {
        return Err("admission supports only offline explain".into());
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
    fn rejects_non_files_and_never_opens_a_mutation_command() {
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
