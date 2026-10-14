//! Installed, private creation policy. This projection neither allocates storage
//! nor enables a public route; actual creation additionally needs its ledger
//! permit, authenticated supervisor and retained original manifest closure.
use crate::admission_host::{Helpers, HostPlan, read_store_document, store_path};
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use tentaflake_admission::ledger::api::ApiAction;
use tentaflake_admission::{Instance, Isolation, StorageKind};

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
            || template.issued_actions.iter().any(|action| {
                !matches!(
                    action,
                    ApiAction::EnvironmentsRead
                        | ApiAction::WorkspacesRead
                        | ApiAction::WorkspacesRetain
                )
            })
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
}
