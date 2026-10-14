//! Installed, private creation policy. This projection neither allocates storage
//! nor enables a public route; actual creation additionally needs its ledger
//! permit, authenticated supervisor and retained original manifest closure.
use crate::admission_host::{Helpers, HostPlan, read_store_document, store_path};
use crate::admission_storage::{VolumeSpec, validate_specs};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use tentaflake_admission::ledger::api::{ActionGrant, ApiAction, ApiTarget, EnvironmentSpec};
use tentaflake_admission::{
    Instance, Isolation, StorageKind, StorageRequirement, validate_authority,
};

const MIB: u64 = 1024 * 1024;
const CPU: u64 = 1_000_000_000;

// Deliberately no Serialize: the source identity, reference and helper paths are
// private host data. Only public_metadata constructs the public wire fields.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(crate) struct CreationTemplate {
    pub source_instance: String,
    pub source_revision: String,
    pub revision: String,
    pub reference: String,
    pub image: TemplateImage,
    pub principals: Vec<String>,
    pub issued_actions: Vec<ApiAction>,
    pub helpers: TemplateHelpers,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(crate) struct TemplateImage {
    pub id: String,
    pub distro: Distro,
    pub release: String,
    pub label: String,
    pub digest: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Distro {
    Debian,
    Ubuntu,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct TemplateHelpers {
    pub state: Helpers,
    pub workspace: Helpers,
}

// Private host paths are deliberately not serializable as public metadata.
pub(crate) struct ReconstructedCreation {
    pub environment: EnvironmentSpec,
    /// Stable preparation order; these volumes never enter the static HostPlan.
    pub volumes: [VolumeSpec; 2],
}

fn issued_action(action: ApiAction) -> Option<&'static str> {
    match action {
        ApiAction::EnvironmentsRead => Some("environment-read"),
        ApiAction::WorkspacesRead => Some("workspace-read"),
        ApiAction::WorkspacesRetain => Some("workspace-retain"),
        _ => None,
    }
}

fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    bytes
        .iter()
        .flat_map(|byte| {
            [
                char::from(DIGITS[usize::from(byte >> 4)]),
                char::from(DIGITS[usize::from(byte & 15)]),
            ]
        })
        .collect()
}

fn revision(value: impl Serialize) -> Result<String, String> {
    let bytes = serde_json::to_vec(&value).map_err(|_| "cannot encode creation binding")?;
    Ok(format!(
        "sha256:{}",
        hex(ring::digest::digest(&ring::digest::SHA256, &bytes).as_ref())
    ))
}

/// Pure reconstruction from an authenticated original HostPlan, already checked
/// by load_plan against its retained catalog. Choosing a template or passing a
/// nonce grants no authority: the ledger and supervised caller still authorize
/// creation. Randomness, current installation and principal authentication are
/// caller responsibilities; this function performs no I/O or resource reservation.
pub(crate) fn reconstruct(
    plan: &HostPlan,
    template_id: &str,
    nonce: [u8; 16],
    principal: &str,
    name: &str,
) -> Result<ReconstructedCreation, String> {
    let template = plan
        .creation_templates
        .get(template_id)
        .ok_or("unknown creation template")?;
    let source = plan
        .instances
        .get(&template.source_instance)
        .ok_or("missing creation source")?;
    if !id(template_id)
        || !id(principal)
        || name.is_empty()
        || name.chars().count() > 160
        || name.chars().any(char::is_control)
        || !template
            .principals
            .iter()
            .any(|allowed| allowed == principal)
        || template.source_revision != source.revision
        || !digest(&template.revision)
        || source.isolation != Isolation::Container
        || !source.requested_capabilities.is_empty()
        || template.issued_actions.is_empty()
        || template.issued_actions.len() > 3
        || template
            .issued_actions
            .iter()
            .any(|action| issued_action(*action).is_none())
        || template
            .issued_actions
            .iter()
            .collect::<BTreeSet<_>>()
            .len()
            != template.issued_actions.len()
    {
        return Err("invalid creation identity or policy".into());
    }
    crate::admission_oci::validate_template_source(plan, &template.source_instance)?;
    exact_resources(source)?;
    let nonce = hex(&nonce);
    let environment_id = format!("env-{nonce}");
    let workspace_id = format!("ws-{nonce}");
    if plan.instances.contains_key(&environment_id) || plan.instances.contains_key(&workspace_id) {
        return Err("creation identity collides with a static instance".into());
    }
    let make_volume = |kind: StorageKind| -> Result<VolumeSpec, String> {
        let original = plan
            .volumes
            .iter()
            .find(|volume| {
                volume.spec.instance_id == template.source_instance && volume.spec.kind == kind
            })
            .ok_or("missing creation source volume")?;
        let (kind_name, image_directory) = match kind {
            StorageKind::State => ("state", "/var/lib/tentaflake-workspace-volumes/state"),
            StorageKind::Workspace => ("workspace", "/var/lib/tentaflake-workspace-volumes"),
        };
        let mut volume = VolumeSpec {
            instance_id: environment_id.clone(),
            owner_id: principal.into(),
            allocation_id: format!("{kind_name}:{environment_id}"),
            kind,
            image_path: format!("{image_directory}/{environment_id}.img").into(),
            mount_path: format!("/var/lib/tentaflake-environments/{environment_id}/{kind_name}")
                .into(),
            bytes: kind.bytes(source.resources),
            owner_uid: original.spec.owner_uid,
            owner_gid: original.spec.owner_gid,
            binding_revision: String::new(),
        };
        // Versioned ordered tuples avoid depending on map/key serialization or
        // the aggregate policy revision. Capacity-only changes do not rebind.
        volume.binding_revision = revision((
            "tentaflake.dynamic-volume.v1",
            template_id,
            &template.revision,
            &volume.instance_id,
            &volume.owner_id,
            volume.kind,
            &volume.allocation_id,
            &volume.image_path,
            &volume.mount_path,
            volume.bytes,
            volume.owner_uid,
            volume.owner_gid,
        ))?;
        Ok(volume)
    };
    let volumes = [
        make_volume(StorageKind::State)?,
        make_volume(StorageKind::Workspace)?,
    ];
    validate_specs(&volumes)?;
    let storage = volumes
        .iter()
        .map(|volume| StorageRequirement {
            id: volume.allocation_id.clone(),
            kind: volume.kind,
            binding_revision: volume.binding_revision.clone(),
        })
        .collect::<Vec<_>>();
    let instance = Instance {
        revision: revision((
            "tentaflake.dynamic-instance.v1",
            template_id,
            &template.revision,
            &template.source_revision,
            &environment_id,
            principal,
            source.isolation,
            source.resources,
            &storage,
        ))?,
        id: environment_id.clone(),
        owner_id: principal.into(),
        isolation: source.isolation,
        resources: source.resources,
        requested_capabilities: Vec::new(),
        storage,
    };
    if !validate_authority(&plan.policy, &instance).is_empty() {
        return Err("invalid reconstructed creation authority".into());
    }
    let grants = template
        .issued_actions
        .iter()
        .map(|action| ActionGrant {
            id: format!(
                "grant-{nonce}-{}",
                issued_action(*action).expect("validated action")
            ),
            action: *action,
            target: if *action == ApiAction::EnvironmentsRead {
                ApiTarget::Environment {
                    id: environment_id.clone(),
                }
            } else {
                ApiTarget::Workspace {
                    id: workspace_id.clone(),
                }
            },
            expires_at: None,
        })
        .collect();
    Ok(ReconstructedCreation {
        environment: EnvironmentSpec {
            template_id: template_id.into(),
            template_revision: template.revision.clone(),
            name: name.into(),
            image_id: template.image.id.clone(),
            agent_runtime: None,
            tools: Vec::new(),
            skills: Vec::new(),
            instance,
            workspace_id,
            grants,
        },
        volumes,
    })
}

/// Recorded identity and bindings must equal the retained original template's
/// reconstruction. Current templates/catalogs never substitute for removed ones.
pub(crate) fn verify_recorded(
    plan: &HostPlan,
    recorded: &EnvironmentSpec,
) -> Result<ReconstructedCreation, String> {
    let slot = recorded
        .instance
        .id
        .strip_prefix("env-")
        .filter(|slot| {
            slot.len() == 32
                && slot
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        })
        .ok_or("invalid recorded creation slot")?;
    let mut nonce = [0; 16];
    for (index, byte) in nonce.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&slot[index * 2..index * 2 + 2], 16)
            .map_err(|_| "invalid recorded creation slot")?;
    }
    let reconstructed = reconstruct(
        plan,
        &recorded.template_id,
        nonce,
        &recorded.instance.owner_id,
        &recorded.name,
    )?;
    if reconstructed.environment != *recorded {
        return Err("recorded creation differs from its original template".into());
    }
    Ok(reconstructed)
}

fn id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 96
        && value.as_bytes()[0].is_ascii_alphanumeric()
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b))
}

fn digest(value: &str) -> bool {
    value.strip_prefix("sha256:").is_some_and(|hex| {
        hex.len() == 64
            && hex
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    })
}

fn text(value: &str, limit: usize) -> bool {
    !value.is_empty() && value.len() <= limit && !value.chars().any(char::is_control)
}

fn reviewed_reference(image: &TemplateImage, catalog: &Value) -> Result<String, String> {
    // Read the catalog bound into this authority, not the running CLI's newer
    // source tree. Removed pins must remain verifiable for retained cleanup.
    let system = match std::env::consts::ARCH {
        "x86_64" => "x86_64-linux",
        "aarch64" => "aarch64-linux",
        _ => return Err("unreviewed creation image platform".into()),
    };
    let row = &catalog["images"][&image.id];
    let distro = match image.distro {
        Distro::Debian => "debian",
        Distro::Ubuntu => "ubuntu",
    };
    if row["distro"].as_str() != Some(distro)
        || row["release"].as_str() != Some(&image.release)
        || row["label"].as_str() != Some(&image.label)
        || row["platforms"][system]["manifestDigest"].as_str() != Some(&image.digest)
    {
        return Err("creation image does not match the reviewed platform catalog".into());
    }
    Ok(format!(
        "{}@{}",
        row["repository"]
            .as_str()
            .ok_or("missing reviewed image repository")?,
        image.digest
    ))
}

fn exact_resources(instance: &Instance) -> Result<Value, String> {
    let r = instance.resources;
    if r.nano_cpus < CPU
        || r.nano_cpus > 1024 * CPU
        || !r.nano_cpus.is_multiple_of(CPU)
        || [r.memory_bytes, r.workspace_bytes].iter().any(|bytes| {
            *bytes < 128 * MIB || *bytes > 1_048_576 * MIB || !bytes.is_multiple_of(MIB)
        })
    {
        return Err("creation resources cannot be represented exactly".into());
    }
    Ok(
        json!({"cpu": r.nano_cpus / CPU, "memoryMiB": r.memory_bytes / MIB,
        "storageMiB": r.workspace_bytes / MIB}),
    )
}

impl CreationTemplate {
    /// Explicit allowlist matching Image and EnvConfig. Availability remains
    /// false until the actual provisioning/abort/recovery integration exists.
    pub(crate) fn public_metadata(&self, instance: &Instance) -> Result<(Value, Value), String> {
        let image = json!({
            "id":self.image.id,
            "distro":match self.image.distro { Distro::Debian => "debian", Distro::Ubuntu => "ubuntu" },
            "release":self.image.release,"label":self.image.label,"digest":self.image.digest,
            "availability":{"available":false,"state":"unsupported","reason":"creation backend unavailable"},
            "evidence":[]
        });
        let config = json!({"image":self.image.id,"isolation":"container","agentRuntime":null,
            "tools":[],"skills":[],"capabilities":[],"resources":exact_resources(instance)?});
        Ok((image, config))
    }
}

pub(crate) fn validate_plan(plan: &HostPlan) -> Result<(), String> {
    if plan.creation_templates.is_empty() {
        if plan.creation_catalog.is_some() {
            return Err("unused creation catalog".into());
        }
        return Ok(());
    }
    let path = plan
        .creation_catalog
        .as_deref()
        .ok_or("creation catalog is missing")?;
    let bytes = read_store_document(path, 64 * 1024)?;
    let catalog: Value = serde_json::from_slice(&bytes).map_err(|_| "invalid creation catalog")?;
    validate_catalog_plan(plan, &catalog)
}

fn validate_catalog_plan(plan: &HostPlan, catalog: &Value) -> Result<(), String> {
    if catalog["schemaVersion"] != 1
        || catalog["images"]
            .as_object()
            .is_none_or(|rows| rows.is_empty() || rows.len() > 128)
    {
        return Err("invalid original creation catalog".into());
    }
    if plan.creation_templates.len() > 128 {
        return Err("too many creation templates".into());
    }
    if !plan.creation_templates.is_empty()
        && plan.volumes.iter().any(|volume| {
            let id = &volume.spec.instance_id;
            let reserved_slot = id.strip_prefix("env-").is_some_and(|slot| {
                slot.len() == 32
                    && slot
                        .bytes()
                        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            });
            let root = std::path::Path::new("/var/lib/tentaflake-environments");
            reserved_slot
                || volume.spec.mount_path.starts_with(root)
                || root.starts_with(&volume.spec.mount_path)
        })
    {
        return Err("static storage overlaps the dynamic creation namespace".into());
    }
    let mut signatures = BTreeSet::new();
    let mut images = BTreeMap::new();
    for (name, template) in &plan.creation_templates {
        let instance = plan
            .instances
            .get(&template.source_instance)
            .ok_or("creation source instance is missing")?;
        let runtime = plan
            .runtimes
            .get(&template.source_instance)
            .ok_or("creation source runtime is missing")?;
        crate::admission_oci::validate_template_source(plan, &template.source_instance)?;
        if !id(name)
            || !id(&template.source_instance)
            || template.source_revision != instance.revision
            || !digest(&template.revision)
            || instance.isolation != Isolation::Container
            || !instance.requested_capabilities.is_empty()
            || instance.storage.len() != 2
            || ![StorageKind::Workspace, StorageKind::State]
                .iter()
                .all(|kind| instance.storage.iter().filter(|r| r.kind == *kind).count() == 1)
            || template.principals.is_empty()
            || template.principals.len() > 128
            || template.principals.iter().any(|principal| !id(principal))
            || template.principals.iter().collect::<BTreeSet<_>>().len()
                != template.principals.len()
            || template.issued_actions.is_empty()
            || template.issued_actions.len() > 3
            || template
                .issued_actions
                .iter()
                .any(|action| issued_action(*action).is_none())
            || template
                .issued_actions
                .iter()
                .collect::<BTreeSet<_>>()
                .len()
                != template.issued_actions.len()
            || !id(&template.image.id)
            || !text(&template.image.release, 64)
            || !text(&template.image.label, 160)
            || !digest(&template.image.digest)
            || template.reference != reviewed_reference(&template.image, catalog)?
            || !matches!(&runtime.image, tentaflake_admission::ledger::RuntimeImage::Registry { reference }
                if reference == &template.reference)
        {
            return Err("invalid installed creation template".into());
        }
        for helpers in [&template.helpers.state, &template.helpers.workspace] {
            if [
                &helpers.create,
                &helpers.verify,
                &helpers.owner,
                &helpers.mount,
                &helpers.unmount,
            ]
            .iter()
            .any(|path| !store_path(path))
            {
                return Err("creation helpers must be immutable".into());
            }
        }
        let (image, config) = template.public_metadata(instance)?;
        if !signatures.insert(config.to_string()) {
            return Err("ambiguous creation templates".into());
        }
        if images
            .insert(&template.image.id, image.clone())
            .is_some_and(|previous| previous != image)
        {
            return Err("inconsistent creation image metadata".into());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn validate(plan: &HostPlan) -> Result<(), String> {
        let catalog: Value =
            serde_json::from_str(include_str!("../../../images/catalog.json")).unwrap();
        validate_catalog_plan(plan, &catalog)
    }

    fn template() -> Value {
        let helpers = json!({"create":"/nix/store/00000000000000000000000000000000-create",
            "verify":"/nix/store/00000000000000000000000000000000-verify",
            "owner":"/nix/store/00000000000000000000000000000000-owner",
            "mount":"/nix/store/00000000000000000000000000000000-mount",
            "unmount":"/nix/store/00000000000000000000000000000000-unmount"});
        let plan = crate::admission_oci::tests::template_plan_fixture();
        let catalog: Value =
            serde_json::from_str(include_str!("../../../images/catalog.json")).unwrap();
        let row = &catalog["images"]["debian-13"];
        let system = format!("{}-linux", std::env::consts::ARCH);
        let digest = row["platforms"][&system]["manifestDigest"]
            .as_str()
            .unwrap();
        json!({"sourceInstance":"agent-fixture","sourceRevision":plan.instances["agent-fixture"].revision,
            "revision":format!("sha256:{}","b".repeat(64)),
            "reference":format!("{}@{digest}",row["repository"].as_str().unwrap()),
            "image":{"id":"debian-13","distro":row["distro"],"release":row["release"],"label":row["label"],
                "digest":digest},
            "principals":["operator"],"issuedActions":["environments.read","workspaces.read"],
            "helpers":{"state":helpers,"workspace":helpers}})
    }

    fn plan(value: Value) -> HostPlan {
        let mut plan = crate::admission_oci::tests::template_plan_fixture();
        let original = template();
        plan.runtimes.get_mut("agent-fixture").unwrap().image =
            tentaflake_admission::ledger::RuntimeImage::Registry {
                reference: original["reference"].as_str().unwrap().into(),
            };
        plan.creation_templates
            .insert("public-base".into(), serde_json::from_value(value).unwrap());
        plan
    }

    #[test]
    fn safe_projection_is_only_catalog_and_fixed_config() {
        let plan = plan(template());
        validate(&plan).unwrap();
        let template = &plan.creation_templates["public-base"];
        let (image, config) = template
            .public_metadata(&plan.instances["agent-fixture"])
            .unwrap();
        assert_eq!(image["availability"]["available"], false);
        assert_eq!(config["agentRuntime"], Value::Null);
        assert_eq!(config["tools"], json!([]));
        let public = format!("{image}{config}");
        for private in [
            "agent-fixture",
            "/nix/store/",
            "sleep",
            "FIXTURE",
            "principals",
            "sourceRevision",
        ] {
            assert!(!public.contains(private));
        }
    }

    #[test]
    fn refuses_stale_sources_image_substitution_and_invalid_grants() {
        for case in 0..10 {
            let mut value = template();
            match case {
                0 => value["sourceRevision"] = "stale".into(),
                1 => value["sourceInstance"] = "missing".into(),
                2 => value["image"]["digest"] = format!("sha256:{}", "c".repeat(64)).into(),
                3 => {
                    value["reference"] =
                        format!("docker.io/foreign@sha256:{}", "a".repeat(64)).into()
                }
                4 => value["principals"] = json!([]),
                5 => value["principals"] = json!(["operator", "operator"]),
                6 => value["principals"] = json!(["*"]),
                7 => value["issuedActions"] = json!(["environments.create"]),
                8 => value["issuedActions"] = json!(["environments.read", "environments.read"]),
                _ => value["helpers"]["state"]["create"] = "/run/create".into(),
            }
            assert!(validate(&plan(value)).is_err(), "case {case}");
        }
    }

    #[test]
    fn refuses_ambiguous_configs_and_lossy_resource_mapping() {
        let mut configured = plan(template());
        configured
            .creation_templates
            .insert("other".into(), serde_json::from_value(template()).unwrap());
        assert!(validate(&configured).is_err());
        let mut instance = configured.instances["agent-fixture"].clone();
        instance.resources.nano_cpus = CPU / 2;
        assert!(exact_resources(&instance).is_err());
        instance.resources.nano_cpus = CPU;
        instance.resources.memory_bytes += 1;
        assert!(exact_resources(&instance).is_err());
    }

    #[test]
    fn historical_templates_use_their_bound_catalog() {
        let configured = plan(template());
        let original: Value =
            serde_json::from_str(include_str!("../../../images/catalog.json")).unwrap();
        validate_catalog_plan(&configured, &original).unwrap();
        let mut changed = original.clone();
        let system = format!("{}-linux", std::env::consts::ARCH);
        changed["images"]["debian-13"]["platforms"][&system]["manifestDigest"] =
            format!("sha256:{}", "f".repeat(64)).into();
        assert!(validate_catalog_plan(&configured, &changed).is_err());
        validate_catalog_plan(&configured, &original).unwrap();
        assert!(
            validate_plan(&configured).is_err(),
            "missing catalog must not fall back to current compiled facts"
        );
    }

    #[test]
    fn rejects_unknown_private_fields_and_unrepresented_distros() {
        let mut value = template();
        value["command"] = json!(["private"]);
        assert!(serde_json::from_value::<CreationTemplate>(value).is_err());
        let mut value = template();
        value["image"]["distro"] = "unknown".into();
        assert!(serde_json::from_value::<CreationTemplate>(value).is_err());
    }

    fn reconstructed(plan: &HostPlan) -> ReconstructedCreation {
        reconstruct(plan, "public-base", [0x12; 16], "operator", "Build project").unwrap()
    }

    #[test]
    fn reconstruction_rebinds_exact_owned_slots_and_only_issued_grants() {
        let configured = plan(template());
        let before_instances = configured.instances.len();
        let created = reconstructed(&configured);
        let env = &created.environment;
        let slot = format!("env-{}", "12".repeat(16));
        assert_eq!(env.instance.id, slot);
        assert_eq!(env.workspace_id, format!("ws-{}", "12".repeat(16)));
        assert_eq!(env.instance.owner_id, "operator");
        assert_eq!(
            env.instance.resources,
            configured.instances["agent-fixture"].resources
        );
        assert!(env.instance.requested_capabilities.is_empty());
        assert!(env.agent_runtime.is_none());
        assert!(env.tools.is_empty() && env.skills.is_empty());
        for (volume, kind, image_parent) in [
            (
                &created.volumes[0],
                StorageKind::State,
                "/var/lib/tentaflake-workspace-volumes/state",
            ),
            (
                &created.volumes[1],
                StorageKind::Workspace,
                "/var/lib/tentaflake-workspace-volumes",
            ),
        ] {
            let label = if kind == StorageKind::State {
                "state"
            } else {
                "workspace"
            };
            let source = configured
                .volumes
                .iter()
                .find(|v| v.spec.kind == kind)
                .unwrap();
            assert_eq!(volume.kind, kind);
            assert_eq!(volume.allocation_id, format!("{label}:{slot}"));
            assert_eq!(volume.instance_id, slot);
            assert_eq!(volume.owner_id, "operator");
            assert_eq!(
                volume.image_path,
                std::path::PathBuf::from(format!("{image_parent}/{slot}.img"))
            );
            assert_eq!(
                volume.mount_path,
                std::path::PathBuf::from(format!(
                    "/var/lib/tentaflake-environments/{slot}/{label}"
                ))
            );
            assert_eq!(volume.bytes, kind.bytes(env.instance.resources));
            assert_eq!(
                (volume.owner_uid, volume.owner_gid),
                (source.spec.owner_uid, source.spec.owner_gid)
            );
            assert!(digest(&volume.binding_revision));
            assert!(
                env.instance
                    .storage
                    .iter()
                    .any(|r| r.id == volume.allocation_id
                        && r.kind == kind
                        && r.binding_revision == volume.binding_revision)
            );
        }
        assert_eq!(env.grants.len(), 2);
        assert_eq!(
            env.grants[0].target,
            ApiTarget::Environment { id: slot.clone() }
        );
        assert_eq!(
            env.grants[1].target,
            ApiTarget::Workspace {
                id: env.workspace_id.clone()
            }
        );
        assert!(env.grants.iter().all(|grant| grant.expires_at.is_none()));
        assert_eq!(
            verify_recorded(&configured, env).unwrap().volumes,
            created.volumes
        );
        assert_eq!(configured.instances.len(), before_instances);
        assert!(!configured.instances.contains_key(&slot));
        let stored = serde_json::to_string(env).unwrap();
        for private in [
            "agent-fixture",
            "/nix/store/",
            "/var/lib/",
            "sleep",
            "FIXTURE",
            "helpers",
        ] {
            assert!(!stored.contains(private), "private source leaked");
        }
    }

    #[test]
    fn nonce_and_owner_change_bindings_but_display_name_and_capacity_do_not() {
        let mut value = template();
        value["principals"] = json!(["operator", "other"]);
        let mut configured = plan(value);
        let first = reconstructed(&configured);
        let next = reconstruct(
            &configured,
            "public-base",
            [0x34; 16],
            "operator",
            "Build project",
        )
        .unwrap();
        assert_ne!(first.environment.instance.id, next.environment.instance.id);
        assert_ne!(
            first.environment.workspace_id,
            next.environment.workspace_id
        );
        assert_ne!(first.environment.grants, next.environment.grants);
        let other = reconstruct(
            &configured,
            "public-base",
            [0x12; 16],
            "other",
            "Build project",
        )
        .unwrap();
        for changed in [&next, &other] {
            assert_ne!(
                first.environment.instance.revision,
                changed.environment.instance.revision
            );
            for (a, b) in first.volumes.iter().zip(&changed.volumes) {
                assert_ne!(a.binding_revision, b.binding_revision);
            }
        }
        let renamed = reconstruct(
            &configured,
            "public-base",
            [0x12; 16],
            "operator",
            "é".repeat(160).as_str(),
        )
        .unwrap();
        assert_eq!(first.volumes, renamed.volumes);
        assert_eq!(first.environment.instance, renamed.environment.instance);
        configured.policy.capacity.workspace_bytes += MIB;
        configured.policy.revision = format!("sha256:{}", "e".repeat(64));
        assert_eq!(
            verify_recorded(&configured, &first.environment)
                .unwrap()
                .volumes,
            first.volumes
        );
    }

    #[test]
    fn recorded_authority_fields_cannot_be_substituted() {
        let mut value = template();
        value["principals"] = json!(["operator", "other"]);
        let configured = plan(value);
        let original = reconstructed(&configured).environment;
        for case in 0..25 {
            let mut altered = original.clone();
            match case {
                0 => altered.template_id = "missing".into(),
                1 => altered.template_revision = format!("sha256:{}", "f".repeat(64)),
                2 => altered.image_id = "ubuntu-26.04".into(),
                3 => altered.agent_runtime = Some("agent".into()),
                4 => altered.tools.push("tool".into()),
                5 => altered.skills.push("skill".into()),
                6 => altered.instance.id = format!("env-{}", "34".repeat(16)),
                7 => altered.instance.owner_id = "other".into(),
                8 => altered.instance.revision = format!("sha256:{}", "f".repeat(64)),
                9 => altered.instance.isolation = Isolation::GuestKernel,
                10 => altered
                    .instance
                    .requested_capabilities
                    .push(tentaflake_admission::Capability::Model),
                11 => altered.instance.resources.memory_bytes += MIB,
                12 => altered.instance.resources.memory_swap_bytes += MIB,
                13 => altered.instance.resources.nano_cpus += CPU,
                14 => altered.instance.resources.pids += 1,
                15 => altered.instance.resources.workspace_bytes += MIB,
                16 => altered.instance.resources.state_bytes += MIB,
                17 => altered.instance.storage[0].id = "state:foreign".into(),
                18 => altered.instance.storage[0].kind = StorageKind::Workspace,
                19 => {
                    altered.instance.storage[0].binding_revision =
                        format!("sha256:{}", "f".repeat(64))
                }
                20 => altered.workspace_id = "ws-foreign".into(),
                21 => altered.grants[0].id = "grant-foreign".into(),
                22 => altered.grants[0].action = ApiAction::EnvironmentsStart,
                23 => altered.grants[0].target = ApiTarget::Host { id: "host".into() },
                _ => altered.grants[0].expires_at = Some(123),
            }
            assert!(
                verify_recorded(&configured, &altered).is_err(),
                "case {case}"
            );
        }
        for case in 0..3 {
            let mut altered = original.clone();
            match case {
                0 => altered.grants.clear(),
                1 => altered.grants.push(altered.grants[0].clone()),
                _ => altered.instance.storage.pop().map(|_| ()).unwrap(),
            }
            assert!(verify_recorded(&configured, &altered).is_err());
        }
    }

    #[test]
    fn reconstruction_refuses_invalid_names_principals_slots_and_actions() {
        let configured = plan(template());
        for principal in ["", "*", "ungranted", "operator\n"] {
            assert!(reconstruct(&configured, "public-base", [1; 16], principal, "Name").is_err());
        }
        for name in [
            String::new(),
            "x".repeat(161),
            "private\nvalue".into(),
            "\0".into(),
        ] {
            assert!(reconstruct(&configured, "public-base", [1; 16], "operator", &name).is_err());
        }
        for slot in [
            "env-",
            "env-../foreign",
            "env-0123",
            "env-ABCDEF0123456789ABCDEF0123456789",
            "env-0123456789abcdef0123456789abcdef/child",
        ] {
            let mut recorded = reconstructed(&configured).environment;
            recorded.instance.id = slot.into();
            assert!(verify_recorded(&configured, &recorded).is_err());
        }
        for actions in [
            json!([]),
            json!(["environments.create"]),
            json!(["environments.start"]),
            json!(["environments.read", "environments.read"]),
        ] {
            let mut value = template();
            value["issuedActions"] = actions;
            assert!(reconstruct(&plan(value), "public-base", [1; 16], "operator", "Name").is_err());
        }
    }

    #[test]
    fn recorded_reconstruction_needs_original_template_not_current_catalog() {
        let original = plan(template());
        let recorded = reconstructed(&original).environment;
        let mut current = plan(template());
        current.creation_templates.clear();
        current.creation_catalog = Some("/unavailable/current/catalog".into());
        assert!(verify_recorded(&current, &recorded).is_err());
        let mut retained = original;
        // The already authenticated plan needs no catalog/filesystem read during
        // this pure step. load_plan owns original-catalog authentication.
        retained.creation_catalog = Some("/must/not/be/read".into());
        assert_eq!(
            verify_recorded(&retained, &recorded).unwrap().environment,
            recorded
        );
        retained
            .creation_templates
            .get_mut("public-base")
            .unwrap()
            .source_revision = "stale".into();
        assert!(verify_recorded(&retained, &recorded).is_err());
    }
}
