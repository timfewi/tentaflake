//! VM-only accepted-operation preparation, compiled into the Rust test harness.
//! Queue mode exercises the real production supervisor afterwards. Unknown modes
//! deliberately prepare core states: their supervisor identity is synthetic and
//! must never be described as evidence of a production supervisor crash.

use super::*;
use ring::digest::{SHA256, digest};
use tentaflake_admission::ledger::api::{
    ActionGrant, ApiAction, ApiKey, ApiMethod, ApiMutation, ApiQueued, ApiTarget, CreatePolicy,
};

const HOST: &str = "creation-fixture";
const PRINCIPAL: &str = "creation-operator";
const NAME: &str = "Creation fixture";

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Queue,
    Empty,
    State,
    Complete,
}

fn setting(name: &str) -> Result<String, String> {
    let value = std::env::var(name).map_err(|_| "missing VM creation fixture setting")?;
    if value.is_empty() || value.len() > 4096 || value.chars().any(char::is_control) {
        return Err("invalid VM creation fixture setting".into());
    }
    Ok(value)
}

fn run() -> Result<(), String> {
    // Parse every setting before opening the ledger or acquiring a permit.
    let path = std::path::PathBuf::from(setting("TF_CREATION_PLAN")?);
    let template_id = setting("TF_CREATION_TEMPLATE")?;
    let operation_id = setting("TF_CREATION_OPERATION")?;
    let nonce = setting("TF_CREATION_NONCE")?;
    let mode = match setting("TF_CREATION_MODE")?.as_str() {
        "queue" => Mode::Queue,
        "unknown-empty" => Mode::Empty,
        "unknown-state" => Mode::State,
        "unknown-complete" => Mode::Complete,
        _ => return Err("invalid VM creation fixture mode".into()),
    };
    if !tentaflake_admission::identifier(&template_id)
        || !tentaflake_admission::identifier(&operation_id)
        || nonce.len() != 32
        || !nonce
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err("invalid VM creation fixture identity".into());
    }
    let mut nonce_bytes = [0; 16];
    for (index, byte) in nonce_bytes.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&nonce[index * 2..index * 2 + 2], 16)
            .map_err(|_| "invalid VM creation fixture nonce")?;
    }
    require_root()?;
    let fence = host_fence::exclusive()?;
    installed(&path)?;
    let plan = load_plan(&path)?;
    let published = authority(&plan, &path);
    let ledger = Ledger::open(Path::new(LEDGER)).map_err(|_| "fixture ledger unavailable")?;
    let creation =
        crate::admission_template::reconstruct(&plan, &template_id, nonce_bytes, PRINCIPAL, NAME)?;
    let history = maintenance_inventory(&ledger, &published, &fence)?;
    let mut specs: Vec<_> = plan
        .volumes
        .iter()
        .map(|volume| volume.spec.clone())
        .collect();
    specs.extend(history.volumes);
    specs.extend(creation.volumes.iter().cloned());
    let context = ApiContext {
        authority: published.clone(),
        host: HOST.into(),
        principal: PRINCIPAL.into(),
        epoch: "creation-vm".into(),
        now: now()?,
    };
    let grants_revision = ledger
        .api_snapshot(&context)
        .map_err(|_| "fixture grants unavailable")?
        .grants_revision;
    crate::admission_retention::retain(
        &published,
        plan.tools
            .nix_store
            .as_deref()
            .ok_or("fixture retention tool missing")?,
        &fence,
    )?;
    let request = serde_json::to_vec(&(1u32, HOST, &creation.environment))
        .map_err(|_| "cannot encode fixture create request")?;
    let request_digest = format!(
        "sha256:{}",
        digest(&SHA256, &request)
            .as_ref()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
    );
    let mutation = ApiMutation {
        operation_id: operation_id.clone(),
        key: ApiKey {
            host: HOST.into(),
            principal: PRINCIPAL.into(),
            epoch: context.epoch.clone(),
            method: ApiMethod::Post,
            canonical_target: "/api/v1/environments".into(),
            key: format!(
                "{}-{}-{}-{}-{}",
                &nonce[..8],
                &nonce[8..12],
                &nonce[12..16],
                &nonce[16..20],
                &nonce[20..]
            ),
        },
        request_digest,
    };
    let queued = ledger
        .api_queue_create(
            &context,
            &mutation,
            &creation.environment,
            grants_revision,
            |current, snapshot, recorded| {
                installed(&path)?;
                if current != &published || *recorded != creation.environment {
                    return Err("fixture creation authority changed".into());
                }
                crate::admission_template::verify_recorded(&plan, recorded)?;
                stopped_storage(&plan)?;
                check_creation_space(
                    &specs,
                    &snapshot.storage,
                    CreationTargets::New(&creation.volumes),
                    plan.physical_space_headroom_bytes,
                )?;
                Ok(CreatePolicy {
                    policy: plan.policy.clone(),
                    create_grant: ActionGrant {
                        id: "creation-fixture-accept".into(),
                        action: ApiAction::EnvironmentsCreate,
                        target: ApiTarget::Host { id: HOST.into() },
                        expires_at: None,
                    },
                    issued_grants: recorded.grants.clone(),
                })
            },
        )
        .map_err(|_| "fixture creation queue refused")?;
    let (accepted, new) = match queued {
        ApiQueued::New(operation) => (*operation, true),
        ApiQueued::Existing(operation) => (*operation, false),
    };
    // Retrying fixture preparation returns history; it cannot claim or provision
    // a previously accepted operation, including a still-Queued operation.
    if new && mode != Mode::Queue {
        let execution = ApiExecution {
            boot_id: fs::read_to_string("/proc/sys/kernel/random/boot_id")
                .map_err(|_| "fixture boot identity unavailable")?
                .trim()
                .into(),
            supervisor: SupervisorIdentity {
                unit: unit(&operation_id)?,
                // Explicit fixture identity, not a claimed systemd observation.
                invocation_id: "f".repeat(32),
                pid: std::process::id(),
                start_time_ticks: admission_systemd::process_start(std::process::id())?,
            },
        };
        let claim = ledger
            .api_claim_create(
                &context,
                &operation_id,
                &execution,
                |current, snapshot, recorded, expected_execution| {
                    installed(&path)?;
                    if current != &published
                        || *recorded != creation.environment
                        || *expected_execution != execution
                    {
                        return Err("fixture claim authority changed".into());
                    }
                    crate::admission_template::verify_recorded(&plan, recorded)?;
                    stopped_storage(&plan)?;
                    check_creation_space(
                        &specs,
                        &snapshot.storage,
                        CreationTargets::Reserved(&creation.volumes),
                        plan.physical_space_headroom_bytes,
                    )
                },
            )
            .map_err(|_| "fixture creation claim refused")?;
        let CreateClaim::New(mut permit) = claim else {
            return Err("fixture preparation did not receive a new permit".into());
        };
        let template = &plan.creation_templates[&template_id];
        for volume in &creation.volumes {
            if mode == Mode::Empty || (mode == Mode::State && volume.kind != StorageKind::State) {
                continue;
            }
            let helpers = match volume.kind {
                StorageKind::State => &template.helpers.state,
                StorageKind::Workspace => &template.helpers.workspace,
            };
            permit
                .provision(volume.kind, |_| {
                    installed(&path)?;
                    let slot = &creation.environment.instance.id;
                    helper(&helpers.create, slot, &fence)?;
                    helper(&helpers.mount, slot, &fence)?;
                    helper(&helpers.owner, slot, &fence)?;
                    helper(&helpers.unmount, slot, &fence)?;
                    helper(&helpers.verify, slot, &fence)
                })
                .map_err(|_| "fixture provisioning failed; preserve Unknown and all files")?;
        }
        stopped_storage(&plan)?;
        // Deliberately omit complete(): real durable native row writes remain,
        // with an Unknown operation. Recovery still requires a different boot.
        drop(permit);
    }
    let operation = ledger
        .api_environments(&published)
        .map_err(|_| "cannot inspect prepared fixture operation")?
        .into_iter()
        .find(|(_, op)| op.mutation.operation_id == accepted.mutation.operation_id)
        .map(|(_, op)| op)
        .ok_or("prepared fixture operation missing")?;
    let revision = ledger
        .snapshot()
        .map_err(|_| "cannot inspect fixture revision")?
        .revision;
    println!(
        "\nTF_CREATION_RESULT={}",
        serde_json::json!({
            "operationId":operation.mutation.operation_id, "environmentId":operation.environment_id,
            "workspaceId":creation.environment.workspace_id, "state":operation.state,
            "claimedRevision":operation.claimed_revision, "ledgerRevision":revision,
            "new":new, "volumes":creation.volumes,
        })
    );
    Ok(())
}

#[test]
#[ignore = "root-only NixOS VM fixture; never part of the production CLI"]
fn prepare() {
    run().expect("VM accepted-operation fixture failed");
}
