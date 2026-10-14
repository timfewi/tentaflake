//! Published ceilings are not admission, grants or live runtime evidence.
use super::session::AuthError;
use serde_json::{Value, json};
use tentaflake_admission::{HostPolicy, ledger::api::MAX_OPERATIONS};

const MIB: u64 = 1024 * 1024;
const CPU: u64 = 1_000_000_000;

fn bound(
    capacity: u64,
    reserved: u64,
    per_instance: u64,
    unit: u64,
    min: u64,
    wire_max: u64,
) -> Result<Value, AuthError> {
    let max = capacity
        .checked_sub(reserved)
        .ok_or(AuthError::Unavailable)?
        .min(per_instance)
        / unit;
    let max = max.min(wire_max);
    // The integer API cannot request a fraction of a CPU or a sub-minimum
    // filesystem. Zero means no request in this protocol can fit this ceiling.
    let (min, max) = if max < min { (0, 0) } else { (min, max) };
    Ok(json!({"min":min,"max":max,"default":min,"step":1}))
}

pub(super) fn project(policy: &HostPolicy, revisions: Value) -> Result<Value, AuthError> {
    let reserved = policy
        .host_reserve
        .checked_add(policy.research_reserve)
        .ok_or(AuthError::Unavailable)?;
    let reserved_cpu = reserved.nano_cpus.div_ceil(CPU);
    let reserved_memory = reserved.memory_bytes.div_ceil(MIB);
    if reserved_cpu > 1_048_576 || reserved_memory > 1_048_576 || policy.max_active == 0 {
        return Err(AuthError::Unavailable);
    }
    let unavailable = |id: &str, name: &str, detail: &str| {
        json!({
            "id":id,"name":name,"detail":detail,
            "availability":{"available":false,"state":"unsupported","reason":detail},
            "evidence":[],
        })
    };
    Ok(json!({
        "revisions":revisions,
        // An omitted action is denied. Add actions only with their actual
        // lifecycle implementation; a published declaration supplies no grant.
        "features":{},
        "isolation":[
            unavailable("container", "Container", "Dynamic environment dispatch is not implemented; containers share the host kernel."),
            unavailable("guest-kernel", "Separate guest kernel", "The host API has no accepted separate-kernel runtime backend."),
        ],
        "agentRuntimes":[],"optional":[],
        "limits":{
            "cpu":bound(policy.capacity.nano_cpus,reserved.nano_cpus,policy.per_instance.nano_cpus,CPU,1,1024)?,
            "memoryMiB":bound(policy.capacity.memory_bytes,reserved.memory_bytes,policy.per_instance.memory_bytes,MIB,128,1_048_576)?,
            "storageMiB":bound(policy.capacity.workspace_bytes,reserved.workspace_bytes,policy.per_instance.workspace_bytes,MIB,128,1_048_576)?,
            "maxActiveEnvironments":policy.max_active.min(1024),
            // These protocol caps do not advertise the disabled file/stream
            // routes. Those routes still refuse all access, including small requests.
            "maxEditBytes":super::MAX_BODY,"maxUploadBytes":super::MAX_BODY,
            "maxPageEntries":500,"maxStreamFrameBytes":65536,"maxStreamReplayBytes":1_048_576,
            "maxTerminals":1,"maxTerminalIdleSeconds":900,"maxOperationRecords":MAX_OPERATIONS,
            "reservedCpu":reserved_cpu,"reservedMemoryMiB":reserved_memory,
        },
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tentaflake_admission::Resources;

    fn policy() -> HostPolicy {
        HostPolicy {
            revision: "policy".into(),
            generation: "generation".into(),
            capacity: Resources {
                nano_cpus: 8 * CPU,
                memory_bytes: 8192 * MIB,
                workspace_bytes: 16384 * MIB,
                ..Resources::default()
            },
            host_reserve: Resources {
                nano_cpus: CPU,
                memory_bytes: 1024 * MIB,
                workspace_bytes: 1024 * MIB,
                ..Resources::default()
            },
            research_reserve: Resources {
                nano_cpus: CPU / 2,
                memory_bytes: 512 * MIB,
                ..Resources::default()
            },
            per_instance: Resources {
                nano_cpus: 4 * CPU,
                memory_bytes: 8192 * MIB,
                workspace_bytes: 8192 * MIB,
                ..Resources::default()
            },
            max_active: 3,
            evidence_max_age_seconds: 30,
            grants: vec![],
        }
    }

    #[test]
    fn declaration_exposes_reserve_adjusted_limits_without_granting_features() {
        let value = project(&policy(), json!({"host":"revision"})).unwrap();
        assert_eq!(value["revisions"], json!({"host":"revision"}));
        assert_eq!(value["features"], json!({}));
        assert!(value["isolation"].as_array().unwrap().iter().all(
            |item| item["availability"]["available"] == false && item["evidence"] == json!([])
        ));
        let limits = &value["limits"];
        assert_eq!(limits["cpu"]["max"], 4);
        assert_eq!(limits["memoryMiB"]["max"], 6656);
        assert_eq!(limits["storageMiB"]["max"], 8192);
        assert_eq!(limits["reservedCpu"], 2);
        assert_eq!(limits["reservedMemoryMiB"], 1536);
        assert_eq!(limits["maxActiveEnvironments"], 3);
    }

    #[test]
    fn ceilings_round_down_and_refuse_overflow_or_overcommitted_reserves() {
        let mut value = policy();
        value.per_instance.nano_cpus = CPU - 1;
        value.per_instance.memory_bytes = 128 * MIB - 1;
        let caps = project(&value, json!({})).unwrap();
        assert_eq!(
            caps["limits"]["cpu"],
            json!({"min":0,"max":0,"default":0,"step":1})
        );
        assert_eq!(caps["limits"]["memoryMiB"]["max"], 0);
        value.host_reserve.workspace_bytes = u64::MAX;
        assert!(project(&value, json!({})).is_err());
        value.research_reserve.workspace_bytes = 1;
        assert!(project(&value, json!({})).is_err());
    }
}
