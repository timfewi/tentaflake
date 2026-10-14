//! Durable Docker dispatch identity; collecting and authenticating these facts
//! belongs to the trusted host supervisor, never to the requesting workload.

use crate::ledger::LedgerError;
use crate::{AdmissionRequest, AdmissionSnapshot, EvidenceKind, identifier};
use serde::{Deserialize, Serialize};

/// Fresh host evidence collected under the claim lock. This is deliberately not
/// deserializable: a client snapshot is not permission to dispatch a container.
pub struct RuntimeContext {
    pub snapshot: AdmissionSnapshot,
    pub now: u64,
    /// Required for a new allowed claim. Historical retries need current
    /// authorization, but must not fabricate identity when the daemon is down.
    pub intent: Option<RuntimeIntent>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct DaemonIdentity {
    /// Docker's persistent daemon ID alone does not identify a running process.
    pub id: String,
    pub invocation_id: String,
    pub pid: u32,
    pub start_time_ticks: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct SupervisorIdentity {
    pub unit: String,
    pub invocation_id: String,
    pub pid: u32,
    pub start_time_ticks: u64,
}

/// Acquisition is separate from dispatch. A local artifact is authenticated by
/// its immutable descriptor and archive, never by an invented registry digest.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields, tag = "kind", rename_all = "kebab-case")]
pub enum RuntimeImage {
    Registry { reference: String },
    LocalArtifact { descriptor: String },
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct RuntimeIntent {
    pub boot_id: String,
    pub daemon: DaemonIdentity,
    pub supervisor: SupervisorIdentity,
    /// Matches the independently collected current Runtime readiness source.
    pub runtime_generation: String,
    /// Must cover the full effective OCI configuration, including image source,
    /// create arguments and security expectations, and equal the instance revision.
    pub spec_revision: String,
    pub image: RuntimeImage,
    /// Exact local image configuration digest used for create with pull=never.
    pub image_id: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct RuntimeDispatch {
    pub intent: RuntimeIntent,
    /// Bound once after create and exact inspection, before any start request.
    pub container_id: Option<String>,
}

pub(crate) fn full_container_id(value: &str) -> bool {
    hex(value, 64)
}

fn hex(value: &str, length: usize) -> bool {
    value.len() == length
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn invocation(value: &str) -> bool {
    hex(value, 32) && value.bytes().any(|byte| byte != b'0')
}

fn digest(value: &str) -> bool {
    value.strip_prefix("sha256:").is_some_and(full_container_id)
}

fn store_file(value: &str) -> bool {
    let Some(relative) = value.strip_prefix("/nix/store/") else {
        return false;
    };
    let mut components = relative.split('/');
    let first = components.next().unwrap_or_default();
    value.len() <= 4096
        && first.len() > 33
        && first.as_bytes()[..32]
            .iter()
            .all(|byte| b"0123456789abcdfghijklmnpqrsvwxyz".contains(byte))
        && first.as_bytes()[32] == b'-'
        && std::iter::once(&first[33..]).chain(components).all(|part| {
            !part.is_empty()
                && part != "."
                && part != ".."
                && part
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b"+-._?=".contains(&byte))
        })
}

impl RuntimeImage {
    /// Validate the source locator syntax. This neither authenticates a store
    /// file nor proves the presence, content or provenance of an acquired image.
    pub fn validate(&self) -> Result<(), LedgerError> {
        let valid = match self {
            RuntimeImage::Registry { reference } => {
                reference.len() <= 512
                    && reference.split_once('@').is_some_and(|(name, pin)| {
                        !name.is_empty()
                            && name.as_bytes()[0].is_ascii_alphanumeric()
                            && name.bytes().all(|byte| {
                                byte.is_ascii_alphanumeric() || b"/:._-".contains(&byte)
                            })
                            && !name
                                .rsplit('/')
                                .next()
                                .is_some_and(|part| part.contains(':'))
                            && digest(pin)
                    })
            }
            RuntimeImage::LocalArtifact { descriptor } => store_file(descriptor),
        };
        if !valid {
            return Err(LedgerError::Invalid("runtime image source"));
        }
        Ok(())
    }
}

impl RuntimeIntent {
    pub(crate) fn validate(&self, request: &AdmissionRequest) -> Result<(), LedgerError> {
        self.image.validate()?;
        let boot = self.boot_id.as_bytes();
        let valid_boot = boot.len() == 36
            && boot.iter().enumerate().all(|(index, byte)| {
                if [8, 13, 18, 23].contains(&index) {
                    *byte == b'-'
                } else {
                    byte.is_ascii_digit() || (b'a'..=b'f').contains(byte)
                }
            })
            && boot
                .iter()
                .any(|byte| (b'1'..=b'9').contains(byte) || (b'a'..=b'f').contains(byte));
        let valid_process = |pid: u32, ticks: u64| pid > 0 && pid <= i32::MAX as u32 && ticks > 0;
        let unit = &self.supervisor.unit;
        if !valid_boot
            || !identifier(&self.daemon.id)
            || !invocation(&self.daemon.invocation_id)
            || !valid_process(self.daemon.pid, self.daemon.start_time_ticks)
            || !invocation(&self.supervisor.invocation_id)
            || !valid_process(self.supervisor.pid, self.supervisor.start_time_ticks)
            || unit.len() <= ".service".len()
            || unit.len() > 255
            || !unit.ends_with(".service")
            || !unit
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"_.@:-".contains(&byte))
            || !identifier(&self.runtime_generation)
            || !identifier(&self.spec_revision)
            || self.spec_revision != request.expected.instance
            || !digest(&self.image_id)
        {
            return Err(LedgerError::Invalid("runtime dispatch identity"));
        }
        Ok(())
    }

    pub(crate) fn validate_current(
        &self,
        snapshot: &AdmissionSnapshot,
        request: &AdmissionRequest,
    ) -> Result<(), LedgerError> {
        self.validate(request)?;
        if self.spec_revision != snapshot.instance.revision
            || !snapshot.readiness.sources.iter().any(|source| {
                source.kind == EvidenceKind::Runtime && source.generation == self.runtime_generation
            })
        {
            return Err(LedgerError::Invalid("runtime dispatch generation"));
        }
        Ok(())
    }
}
