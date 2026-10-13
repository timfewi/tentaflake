//! Immutable declarative input and read-only inventory for host admission.
//! Loading a plan or listing images never establishes bootstrap completeness.
use crate::OutputMode;
use crate::admission_storage::{VolumeSpec, scan_inventory, validate_specs};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::fs::{self, OpenOptions};
use std::io::Read;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Component, Path, PathBuf};
use tentaflake_admission::{HostPolicy, Instance, identifier, validate_authority, validate_policy};

const MAX_PLAN_BYTES: u64 = 4 * 1024 * 1024;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(crate) struct HostPlan {
    schema_version: u32,
    pub policy: HostPolicy,
    pub instances: BTreeMap<String, Instance>,
    pub volumes: Vec<Volume>,
    pub tools: Tools,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(crate) struct Volume {
    pub spec: VolumeSpec,
    pub units: Units,
    pub helpers: Helpers,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(crate) struct Units {
    pub prepare: String,
    pub owner: String,
    pub mount: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(crate) struct Helpers {
    pub create: PathBuf,
    pub verify: PathBuf,
    pub owner: PathBuf,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(crate) struct Tools {
    pub cli: PathBuf,
    pub systemctl: PathBuf,
    pub busctl: PathBuf,
    pub findmnt: PathBuf,
    pub losetup: PathBuf,
    pub blkid: PathBuf,
    pub btrfs: PathBuf,
    pub docker: PathBuf,
}

fn store_path(path: &Path) -> bool {
    let parts: Vec<_> = path.components().collect();
    let Some(Component::Normal(object)) = parts.get(3) else {
        return false;
    };
    let Some(object) = object.to_str() else {
        return false;
    };
    parts.first() == Some(&Component::RootDir)
        && parts.get(1) == Some(&Component::Normal("nix".as_ref()))
        && parts.get(2) == Some(&Component::Normal("store".as_ref()))
        && parts[3..]
            .iter()
            .all(|part| matches!(part, Component::Normal(_)))
        && object.len() > 33
        && object.as_bytes()[32] == b'-'
        && object.as_bytes()[..32]
            .iter()
            .all(|byte| b"0123456789abcdfghijklmnpqrsvwxyz".contains(byte))
        && !path
            .as_os_str()
            .as_encoded_bytes()
            .iter()
            .any(u8::is_ascii_control)
        && path.to_str().is_some_and(|value| {
            value[1..]
                .split('/')
                .all(|part| !part.is_empty() && part != "." && part != "..")
        })
}

fn mount_unit(path: &Path) -> String {
    // validate_specs already requires a normalized ASCII path below /var/lib.
    let mut unit = String::new();
    for byte in &path.as_os_str().as_encoded_bytes()[1..] {
        match byte {
            b'/' => unit.push('-'),
            byte if byte.is_ascii_alphanumeric() || b":_.".contains(byte) => {
                unit.push(char::from(*byte))
            }
            byte => write!(unit, "\\x{byte:02x}").expect("writing to String cannot fail"),
        }
    }
    unit.push_str(".mount");
    unit
}

impl HostPlan {
    fn validate(&self) -> Result<(), String> {
        if self.schema_version != 1
            || !identifier(&self.policy.generation)
            || !identifier(&self.policy.revision)
            || !validate_policy(&self.policy).is_empty()
        {
            return Err("invalid host admission policy".into());
        }
        let specs: Vec<_> = self
            .volumes
            .iter()
            .map(|volume| volume.spec.clone())
            .collect();
        validate_specs(&specs)?;
        if self.policy.grants.iter().any(|grant| {
            self.instances
                .get(&grant.instance_id)
                .is_none_or(|instance| instance.owner_id != grant.owner_id)
        }) {
            return Err("host capability grant has no matching declared owner".into());
        }
        for (name, instance) in &self.instances {
            if name != &instance.id
                || !validate_authority(&self.policy, instance).is_empty()
                || instance.owner_id != "host-root"
                || instance.storage.is_empty()
                || instance.storage.len() > 2
                || instance.storage.len()
                    != specs
                        .iter()
                        .filter(|spec| &spec.instance_id == name)
                        .count()
            {
                return Err("invalid host admission instance".into());
            }
        }
        for volume in &self.volumes {
            let spec = &volume.spec;
            let instance = self
                .instances
                .get(&spec.instance_id)
                .ok_or("host volume has no declared instance")?;
            if spec.owner_id != instance.owner_id
                || spec.kind.bytes(instance.resources) != spec.bytes
                || instance
                    .storage
                    .iter()
                    .filter(|required| {
                        required.id == spec.allocation_id
                            && required.kind == spec.kind
                            && required.binding_revision == spec.binding_revision
                    })
                    .count()
                    != 1
            {
                return Err("host volume does not match its declared instance".into());
            }
            let kind = match spec.kind {
                tentaflake_admission::StorageKind::Workspace => "workspace",
                tentaflake_admission::StorageKind::State => "state",
            };
            if volume.units.prepare
                != format!(
                    "tentaflake-{kind}-quota-prepare-{}.service",
                    spec.instance_id
                )
                || volume.units.owner
                    != format!("tentaflake-{kind}-quota-{}.service", spec.instance_id)
                || volume.units.mount != mount_unit(&spec.mount_path)
                || volume.units.mount.len() > 255
                || [
                    &volume.helpers.create,
                    &volume.helpers.verify,
                    &volume.helpers.owner,
                ]
                .iter()
                .any(|path| !store_path(path))
            {
                return Err("invalid host quota unit or helper".into());
            }
        }
        if [
            &self.tools.cli,
            &self.tools.systemctl,
            &self.tools.busctl,
            &self.tools.findmnt,
            &self.tools.losetup,
            &self.tools.blkid,
            &self.tools.btrfs,
            &self.tools.docker,
        ]
        .iter()
        .any(|path| !store_path(path))
        {
            return Err("host admission tools must use immutable store paths".into());
        }
        Ok(())
    }
}

fn parse_plan(bytes: &[u8]) -> Result<HostPlan, String> {
    if bytes.len() as u64 > MAX_PLAN_BYTES {
        return Err("host admission plan exceeds 4 MiB".into());
    }
    let plan: HostPlan =
        serde_json::from_slice(bytes).map_err(|_| "invalid host admission plan")?;
    plan.validate()?;
    Ok(plan)
}

fn load_plan(path: &Path) -> Result<HostPlan, String> {
    if !store_path(path) || path.parent() != Some(Path::new("/nix/store")) {
        return Err("host admission requires the exact generated store file".into());
    }
    for parent in ["/", "/nix", "/nix/store"] {
        let metadata =
            fs::symlink_metadata(parent).map_err(|_| "cannot inspect host plan ancestry")?;
        if !metadata.is_dir()
            || metadata.uid() != 0
            || (metadata.mode() & 0o022 != 0 && metadata.mode() & 0o1000 == 0)
        {
            return Err("unsafe host plan ancestry".into());
        }
    }
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
        .map_err(|_| "cannot open host admission plan")?;
    let metadata = file
        .metadata()
        .map_err(|_| "cannot inspect host admission plan")?;
    if !metadata.is_file()
        || metadata.uid() != 0
        || metadata.mode() & 0o222 != 0
        || metadata.len() > MAX_PLAN_BYTES
    {
        return Err(
            "host admission plan must be an immutable root-owned file of at most 4 MiB".into(),
        );
    }
    let mut bytes = Vec::new();
    file.take(MAX_PLAN_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "cannot read host admission plan")?;
    parse_plan(&bytes)
}

pub(crate) fn inventory(path: &Path, mode: OutputMode) -> Result<u8, String> {
    if unsafe { libc::geteuid() } != 0 {
        return Err("host admission inventory requires root".into());
    }
    let plan = load_plan(path)?;
    let specs: Vec<_> = plan.volumes.into_iter().map(|volume| volume.spec).collect();
    let mut inventory = scan_inventory(&specs)?;
    if mode.hide {
        for allocation in &mut inventory.allocations {
            allocation.id = "redacted".into();
            if let Some(binding) = &mut allocation.binding {
                binding.instance_id = "redacted".into();
                binding.owner_id = "redacted".into();
                binding.revision = "redacted".into();
            }
        }
    }
    // An unfenced diagnostic scan cannot authorize ledger bootstrap, even when
    // every file was enumerated. All allocations are Unknown until verified.
    println!(
        "{}",
        serde_json::json!({"complete": false, "allocations": inventory.allocations})
    );
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};
    use tentaflake_admission::{RecordedAdmission, StorageKind, StorageRequirement};

    const INSTANCE: &str = "agent-fixture";
    const STORE: &str = "/nix/store/00000000000000000000000000000000-fixture-tools";

    fn plan_fixture() -> Value {
        let recorded: RecordedAdmission = serde_json::from_slice(include_bytes!(
            "../../tentaflake-admission/tests/recorded-start.json"
        ))
        .unwrap();
        let mut instance = recorded.snapshot.instance;
        instance.id = INSTANCE.into();
        instance.owner_id = "host-root".into();
        instance.storage.clear();
        let mut volumes = Vec::new();
        // Literal expected unit names keep the fixture independent of mount_unit.
        // A workspace inside its own state mount is a supported Nix projection.
        for (kind, label, mount_path, unit, hash_digit) in [
            (
                StorageKind::Workspace,
                "workspace",
                "/var/lib/agent-fixture/work+space",
                "var-lib-agent\\x2dfixture-work\\x2bspace.mount",
                "3",
            ),
            (
                StorageKind::State,
                "state",
                "/var/lib/agent-fixture",
                "var-lib-agent\\x2dfixture.mount",
                "4",
            ),
        ] {
            let allocation_id = format!("{label}:{INSTANCE}");
            let binding_revision = format!("sha256:{}", hash_digit.repeat(64));
            instance.storage.push(StorageRequirement {
                id: allocation_id.clone(),
                kind,
                binding_revision: binding_revision.clone(),
            });
            let image_directory = if kind == StorageKind::State {
                "/var/lib/tentaflake-workspace-volumes/state"
            } else {
                "/var/lib/tentaflake-workspace-volumes"
            };
            volumes.push(json!({
                "spec": {
                    "instanceId": INSTANCE, "ownerId": "host-root", "allocationId": allocation_id,
                    "kind": label, "imagePath": format!("{image_directory}/{INSTANCE}.img"),
                    "mountPath": mount_path, "bytes": kind.bytes(instance.resources),
                    "ownerUid": 1000, "ownerGid": 1000, "bindingRevision": binding_revision
                },
                "units": {
                    "prepare": format!("tentaflake-{label}-quota-prepare-{INSTANCE}.service"),
                    "owner": format!("tentaflake-{label}-quota-{INSTANCE}.service"),
                    "mount": unit
                },
                "helpers": {
                    "create": format!("{STORE}/bin/{label}-create"),
                    "verify": format!("{STORE}/bin/{label}-verify"),
                    "owner": format!("{STORE}/bin/{label}-owner")
                }
            }));
        }
        let tools: BTreeMap<_, _> = [
            ("cli", "tentaflake"),
            ("systemctl", "systemctl"),
            ("busctl", "busctl"),
            ("findmnt", "findmnt"),
            ("losetup", "losetup"),
            ("blkid", "blkid"),
            ("btrfs", "btrfs"),
            ("docker", "docker"),
        ]
        .into_iter()
        .map(|(name, binary)| (name, format!("{STORE}/bin/{binary}")))
        .collect();
        json!({
            "schemaVersion": 1,
            "policy": recorded.snapshot.policy,
            "instances": BTreeMap::from([(INSTANCE, instance)]),
            "volumes": volumes,
            "tools": tools
        })
    }

    fn parse_fixture(value: &Value) -> Result<HostPlan, String> {
        parse_plan(&serde_json::to_vec(value).unwrap())
    }

    fn model_grant() -> Value {
        json!({"instanceId": INSTANCE, "ownerId": "host-root", "capability": "model", "expiresAt": null})
    }

    #[test]
    fn accepts_nix_shaped_plan_without_live_files_or_readiness() {
        let plan = parse_fixture(&plan_fixture()).unwrap();
        assert_eq!(plan.schema_version, 1);
        assert_eq!(plan.instances[INSTANCE].owner_id, "host-root");
        assert_eq!(plan.volumes.len(), 2);
        assert_eq!(
            plan.volumes[0].spec.allocation_id,
            "workspace:agent-fixture"
        );
        assert_eq!(plan.volumes[1].spec.allocation_id, "state:agent-fixture");
        assert!(plan.policy.grants.is_empty());
    }

    #[test]
    fn mount_units_must_match_the_exact_escaped_mount_path() {
        for unit in [
            "unrelated.mount",
            "*.mount",
            "var-lib-agent-fixture-work+space.mount",
            "var-lib-agent\\x2dfixture.mount",
        ] {
            let mut plan = plan_fixture();
            plan["volumes"][0]["units"]["mount"] = unit.into();
            assert!(parse_fixture(&plan).is_err(), "accepted mount unit {unit}");
        }
    }

    #[test]
    fn bindings_must_agree_with_declared_owner_resources_and_requirements() {
        let original = plan_fixture();
        let bytes = original["volumes"][0]["spec"]["bytes"].as_u64().unwrap();
        for (pointer, replacement) in [
            ("/volumes/0/spec/ownerId", json!("another-owner")),
            ("/instances/agent-fixture/ownerId", json!("another-owner")),
            ("/volumes/0/spec/bytes", json!(bytes + 1)),
            ("/volumes/0/spec/bindingRevision", json!("changed-binding")),
            (
                "/instances/agent-fixture/storage/0/id",
                json!("workspace:another-instance"),
            ),
        ] {
            let mut plan = original.clone();
            *plan.pointer_mut(pointer).unwrap() = replacement;
            assert!(
                parse_fixture(&plan).is_err(),
                "accepted mismatch at {pointer}"
            );
        }
    }

    #[test]
    fn capability_and_grant_shapes_preserve_exact_declared_authority() {
        let mut valid = plan_fixture();
        valid["instances"][INSTANCE]["requestedCapabilities"] = json!(["model"]);
        valid["policy"]["grants"] = json!([model_grant()]);
        assert!(parse_fixture(&valid).is_ok());

        let mut duplicate_capability = valid.clone();
        duplicate_capability["instances"][INSTANCE]["requestedCapabilities"] =
            json!(["model", "model"]);
        assert!(parse_fixture(&duplicate_capability).is_err());

        let mut duplicate_grant = valid.clone();
        duplicate_grant["policy"]["grants"] = json!([model_grant(), model_grant()]);
        assert!(parse_fixture(&duplicate_grant).is_err());

        for (field, replacement) in [
            ("ownerId", json!("another-owner")),
            ("ownerId", json!("invalid owner")),
            ("instanceId", json!("undeclared-instance")),
            ("instanceId", json!("x".repeat(129))),
            ("capability", json!("shell")),
            ("expiresAt", json!("tomorrow")),
        ] {
            let mut plan = valid.clone();
            plan["policy"]["grants"][0][field] = replacement;
            assert!(
                parse_fixture(&plan).is_err(),
                "accepted malformed/foreign grant field {field}"
            );
        }
    }

    #[test]
    fn immutable_tool_and_helper_paths_reject_trailing_aliases() {
        for pointer in ["/tools/systemctl", "/volumes/0/helpers/create"] {
            for suffix in ["/", "/.", "/.."] {
                let mut plan = plan_fixture();
                let original = plan.pointer(pointer).unwrap().as_str().unwrap();
                let alias = format!("{original}{suffix}");
                *plan.pointer_mut(pointer).unwrap() = alias.into();
                assert!(
                    parse_fixture(&plan).is_err(),
                    "accepted {pointer} alias {suffix}"
                );
            }
        }
    }

    #[test]
    fn plan_and_nested_records_reject_undeclared_fields() {
        for pointer in [
            "",
            "/policy",
            "/instances/agent-fixture",
            "/volumes/0",
            "/volumes/0/spec",
            "/volumes/0/units",
            "/volumes/0/helpers",
            "/tools",
        ] {
            let mut plan = plan_fixture();
            plan.pointer_mut(pointer)
                .unwrap()
                .as_object_mut()
                .unwrap()
                .insert("ready".into(), true.into());
            assert!(
                parse_fixture(&plan).is_err(),
                "accepted undeclared field in {pointer}"
            );
        }
        let mut wrong_version = plan_fixture();
        wrong_version["schemaVersion"] = 2.into();
        assert!(parse_fixture(&wrong_version).is_err());
    }

    #[test]
    fn paths_cannot_escape_or_alias_the_store() {
        assert!(store_path(Path::new(
            "/nix/store/00000000000000000000000000000000-tools/bin/probe"
        )));
        for path in [
            "/tmp/plan.json",
            "/nix/store/not-an-object",
            "/nix/store/00000000000000000000000000000000-tools/../probe",
            "/nix/store/00000000000000000000000000000000-tools/./probe",
            "/nix/store/00000000000000000000000000000000-tools//probe",
            "/nix/store/00000000000000000000000000000000-tools/bin/probe/",
            "/nix/store/00000000000000000000000000000000-tools/bin/probe/.",
        ] {
            assert!(!store_path(Path::new(path)), "accepted alias {path}");
        }
        assert_eq!(
            mount_unit(Path::new("/var/lib/agent-one/work+space")),
            "var-lib-agent\\x2done-work\\x2bspace.mount"
        );
    }

    #[test]
    fn invalid_plan_never_echoes_source_or_accepts_mutable_input() {
        let error = parse_plan(br#"{"secret":"private-value"}"#).unwrap_err();
        assert!(!error.contains("private-value"));
        assert!(load_plan(Path::new("/tmp/plan.json")).is_err());
        assert!(parse_plan(&vec![b' '; MAX_PLAN_BYTES as usize + 1]).is_err());
    }
}
