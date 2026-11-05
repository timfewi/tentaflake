//! Data-only desired-system validation. The descriptor is shared with the Nix compiler.
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;
use std::sync::LazyLock;
use toml_edit::{DocumentMut, Item};

const SCHEMA: &str = include_str!("../../../schema/system-v1.json");
const MAX_INPUT: u64 = 1024 * 1024;

fn compatible(item: &Item) -> bool {
    fn scalar(value: &toml_edit::Value) -> bool {
        match value {
            toml_edit::Value::Datetime(_) => false,
            toml_edit::Value::Float(value) => value.value().is_finite(),
            toml_edit::Value::Array(values) => values.iter().all(scalar),
            toml_edit::Value::InlineTable(values) => values.iter().all(|(_, v)| scalar(v)),
            _ => true,
        }
    }
    match item {
        Item::None => true,
        Item::Value(value) => scalar(value),
        Item::Table(values) => values.iter().all(|(_, v)| compatible(v)),
        Item::ArrayOfTables(values) => values.iter().all(|v| v.iter().all(|(_, v)| compatible(v))),
    }
}

/// Parse TOML without echoing potentially private source in diagnostics.
pub fn parse(bytes: &[u8]) -> Result<Value, String> {
    if bytes.len() as u64 > MAX_INPUT {
        return Err("desired system exceeds 1 MiB".into());
    }
    let document: DocumentMut = std::str::from_utf8(bytes)
        .map_err(|_| "desired system must be UTF-8")?
        .parse()
        .map_err(|_| "invalid desired-system TOML")?;
    if !document.iter().all(|(_, item)| compatible(item)) {
        return Err("dates, times and non-finite numbers are unsupported".into());
    }
    let value =
        toml_edit::de::from_document(document).map_err(|_| "invalid desired-system data")?;
    validate(&value)?;
    Ok(value)
}

fn name(value: &str) -> bool {
    !value.is_empty()
        && value.as_bytes()[0].is_ascii_alphanumeric()
        && value
            .bytes()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-')
}

fn agent_id(value: &str) -> bool {
    value
        .strip_prefix("agent-")
        .is_some_and(|id| name(id) && id.len() <= 57)
}

fn canonical_backup_path(value: &str) -> bool {
    let suffix = if let Some(suffix) = value.strip_prefix("/var/lib/tentaflake/agents/") {
        suffix.strip_suffix("/workspace").unwrap_or(suffix)
    } else if let Some(suffix) = value.strip_prefix("/var/lib/tentaflake-broker-llm-agent-") {
        suffix
    } else {
        return false;
    };
    name(suffix) && suffix.len() <= 57
}

fn positive_decimal(value: &str) -> bool {
    value
        .as_bytes()
        .first()
        .is_some_and(|c| (b'1'..=b'9').contains(c))
        && value.bytes().all(|c| c.is_ascii_digit())
}

fn pinned_image(value: &str) -> bool {
    let Some((repository, digest)) = value.split_once("@sha256:") else {
        return false;
    };
    repository
        .as_bytes()
        .first()
        .is_some_and(u8::is_ascii_alphanumeric)
        && repository
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"._/:-".contains(&c))
        && !repository.rsplit('/').next().unwrap_or("").contains(':')
        && digest.len() == 64
        && digest
            .bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
}

fn typed(value: &Value, spec: &Value) -> Result<(), String> {
    let valid = match spec["type"].as_str().unwrap_or("") {
        "bool" => value.is_boolean(),
        "disabled" => value == false,
        "enum" => spec["values"]
            .as_array()
            .is_some_and(|values| values.contains(value)),
        "name" => value.as_str().is_some_and(|s| name(s) && s.len() <= 57),
        "hostName" => value
            .as_str()
            .is_some_and(|s| name(s) && s.len() <= 63 && !s.ends_with('-')),
        "uint" => value.as_u64().is_some_and(|n| n <= i64::MAX as u64),
        "agentId" => value.as_str().is_some_and(agent_id),
        "exactId" => value.as_str().is_some_and(|s| {
            !s.is_empty()
                && s.len() <= 64
                && s.bytes()
                    .next()
                    .is_some_and(|c| c.is_ascii_alphanumeric() || b"_-".contains(&c))
                && s.bytes()
                    .all(|c| c.is_ascii_alphanumeric() || b"_.-".contains(&c))
        }),
        "runtimePath" => value.as_str().is_some_and(|s| {
            s.len() <= 256
                && s.strip_prefix("/run/").is_some_and(|suffix| {
                    suffix.split('/').all(|part| {
                        part.bytes()
                            .next()
                            .is_some_and(|c| c.is_ascii_alphanumeric() || b"_-".contains(&c))
                            && part
                                .bytes()
                                .all(|c| c.is_ascii_alphanumeric() || b"_.-".contains(&c))
                    })
                })
        }),
        "list" => {
            let items = value.as_array().ok_or(": expected a list")?;
            if spec["max"]
                .as_u64()
                .is_some_and(|max| items.len() as u64 > max)
            {
                return Err(": too many items".into());
            }
            let unique_by = spec["uniqueBy"].as_str();
            let mut strings = BTreeSet::new();
            let mut numbers = BTreeSet::new();
            for (index, item) in items.iter().enumerate() {
                typed(item, &spec["items"]).map_err(|error| format!("[{index}]{error}"))?;
                if spec["unique"] == true || unique_by.is_some() {
                    let key = unique_by.map_or(item, |field| &item[field]);
                    let duplicate = if let Some(key) = key.as_str() {
                        !strings.insert(key)
                    } else if let Some(key) = key.as_u64() {
                        !numbers.insert(key)
                    } else {
                        items[..index]
                            .iter()
                            .any(|prior| unique_by.map_or(prior, |field| &prior[field]) == key)
                    };
                    if duplicate {
                        return Err(format!("[{index}]: duplicate item or unique field"));
                    }
                }
            }
            return Ok(());
        }
        "map" => {
            let items = value.as_object().ok_or(": expected a map")?;
            if spec["max"]
                .as_u64()
                .is_some_and(|max| items.len() as u64 > max)
            {
                return Err(": too many entries".into());
            }
            for (index, (key, item)) in items.iter().enumerate() {
                if spec["keys"]["type"] != "agentId" || !agent_id(key) {
                    return Err(format!("[{index}]: invalid canonical agent ID"));
                }
                typed(item, &spec["items"]).map_err(|error| format!("[{index}]{error}"))?;
            }
            return Ok(());
        }
        "version" => value.as_u64() == Some(1),
        "owner" => value
            .as_u64()
            .is_some_and(|n| (1..=4294967294).contains(&n)),
        "pinnedImage" => value.as_str().is_some_and(pinned_image),
        "command" => value.as_array().is_some_and(|argv| {
            argv.first()
                .and_then(Value::as_str)
                .is_some_and(|arg| !arg.is_empty())
                && argv.iter().all(|arg| {
                    arg.as_str()
                        .is_some_and(|s| !s.contains(['\0', '\r', '\n']))
                })
        }),
        "workspacePath" => value.as_str().is_some_and(|s| {
            s == "/workspace"
                || s.strip_prefix("/workspace/").is_some_and(|suffix| {
                    suffix.split('/').all(|part| {
                        !part.is_empty()
                            && part
                                .bytes()
                                .all(|c| c.is_ascii_alphanumeric() || b"_-".contains(&c))
                    })
                })
        }),
        "size" => value.as_str().is_some_and(|s| {
            let digits = s
                .strip_suffix(['b', 'k', 'm', 'g', 'B', 'K', 'M', 'G'])
                .unwrap_or(s);
            positive_decimal(digits)
        }),
        "cpus" => value.as_str().is_some_and(|s| {
            if let Some((whole, fraction)) = s.split_once('.') {
                !fraction.is_empty()
                    && fraction.bytes().all(|c| c.is_ascii_digit())
                    && (positive_decimal(whole)
                        || (whole == "0" && fraction.bytes().any(|c| c != b'0')))
            } else {
                positive_decimal(s)
            }
        }),
        "capabilities" => value.as_array().is_some_and(|values| {
            values.iter().enumerate().all(|(i, value)| {
                spec["values"].as_array().unwrap().contains(value) && !values[..i].contains(value)
            })
        }),
        "object" | "runtimeDefinition" => {
            object(value, &spec["fields"])?;
            if let Some(required) = spec["required"].as_array() {
                for key in required {
                    let key = key.as_str().unwrap();
                    if value.get(key).is_none() {
                        return Err(format!(".{key}: field is required"));
                    }
                }
            }
            return Ok(());
        }
        "statePath" | "backupPath" => value.as_str().is_some_and(|s| {
            (spec["type"] != "backupPath" || s.len() <= 256)
                && s.starts_with("/var/lib/")
                && s.split('/').skip(1).all(|p| {
                    !p.is_empty()
                        && p != "."
                        && p != ".."
                        && p.bytes()
                            .all(|c| c.is_ascii_alphanumeric() || b"-_".contains(&c))
                })
                && spec["excludedRoots"].as_array().is_some_and(|roots| {
                    roots.iter().all(|root| {
                        root.as_str().is_some_and(|root| {
                            s != root
                                && !s
                                    .strip_prefix(root)
                                    .is_some_and(|rest| rest.starts_with('/'))
                        })
                    })
                })
        }),
        "token" => value.as_str().is_some_and(|s| {
            !s.is_empty()
                && s.len() <= 128
                && s.bytes()
                    .all(|c| c.is_ascii_alphanumeric() || b"_-./+".contains(&c))
                && !s.split('/').any(|p| p == "..")
        }),
        _ => false,
    };
    if valid {
        Ok(())
    } else {
        Err(": invalid or unsupported value".into())
    }
}

// Error paths use descriptor-owned field names and numeric collection indexes only.
// Unknown keys stop at their containing object; map keys and source values never escape.
fn object(value: &Value, fields: &Value) -> Result<(), String> {
    let object = value.as_object().ok_or(": expected an object")?;
    let fields = fields.as_object().expect("descriptor fields");
    for (key, value) in object {
        let spec = fields.get(key).ok_or(": unknown or unsafe field")?;
        typed(value, spec).map_err(|error| format!(".{key}{error}"))?;
    }
    Ok(())
}

// Public parsing accepts intent for source-qualified adapters, not host admission.
// The Nix adapter and service modules must still enforce the exact prerequisites.
fn permanently_stopped(schema: &Value, adapter: &Value) -> bool {
    schema["stoppedAdapters"]
        .as_array()
        .unwrap()
        .contains(adapter)
        && !schema["hostAdmittedAdapters"]
            .as_array()
            .is_some_and(|adapters| adapters.contains(adapter))
}

/// Validate JSON transport against exactly the same descriptor as TOML and Nix.
pub fn validate(value: &Value) -> Result<(), String> {
    static DESCRIPTOR: LazyLock<Value> =
        LazyLock::new(|| serde_json::from_str(SCHEMA).expect("embedded schema descriptor"));
    let schema = &*DESCRIPTOR;
    let root = value
        .as_object()
        .ok_or("desired system: expected an object")?;
    if value["schemaVersion"].as_u64() != Some(1) {
        return Err("schemaVersion: must be integer 1".into());
    }
    for (key, section) in root {
        if key == "schemaVersion" || key == "agents" {
            continue;
        }
        if key == "_securityNote" {
            if !section.is_string() {
                return Err("_securityNote: expected a string".into());
            }
            continue;
        }
        let fields = schema["sections"]
            .get(key)
            .ok_or("desired system: unknown section")?;
        object(section, fields).map_err(|error| format!("{key}{error}"))?;
    }
    let agents = value["agents"]
        .as_array()
        .ok_or("agents: array is required")?;
    let mut identities = BTreeMap::new();
    for (index, agent) in agents.iter().enumerate() {
        object(agent, &schema["agent"]).map_err(|error| format!("agents[{index}]{error}"))?;
        let identity = agent["name"]
            .as_str()
            .ok_or_else(|| format!("agents[{index}].name: field is required"))?;
        if agent.get("adapter").is_none() {
            return Err(format!("agents[{index}].adapter: field is required"));
        }
        if agent["adapter"] == "generic" && agent.get("definition").is_none() {
            return Err(format!(
                "agents[{index}].definition: generic adapter requires a runtime definition"
            ));
        }
        if permanently_stopped(schema, &agent["adapter"]) && agent["autoStart"] == true {
            return Err(format!(
                "agents[{index}].autoStart: adapter has no admitted runnable implementation"
            ));
        }
        for key in agent.as_object().unwrap().keys() {
            if let Some(adapters) = schema["agent"][key]["adapters"].as_array()
                && !adapters.contains(&agent["adapter"])
            {
                return Err(format!(
                    "agents[{index}].{key}: field is unsupported by the selected adapter"
                ));
            }
        }
        if identities.insert(identity, agent).is_some() {
            return Err(format!(
                "agents[{index}].name: duplicate global agent identity"
            ));
        }
    }
    validate_services(value, &identities, schema)?;
    Ok(())
}

fn validate_services(
    value: &Value,
    agents: &BTreeMap<&str, &Value>,
    schema: &Value,
) -> Result<(), String> {
    let host = &value["hostApi"];
    let connectors = &value["connectors"];
    let mapping = &value["workloadMapping"];
    let storage = &value["storage"];
    let research = &value["research"];
    for (name, section) in [
        ("hostApi", host),
        ("connectors", connectors),
        ("workloadMapping", mapping),
        ("storage", storage),
        ("research", research),
    ] {
        if section["enable"] != true
            && section
                .as_object()
                .is_some_and(|fields| fields.keys().any(|key| key != "enable"))
        {
            return Err(format!(
                "{name}.enable: service configuration requires explicit enable"
            ));
        }
    }
    if host["enable"] == true && host.get("peers").is_none() {
        return Err("hostApi.peers: explicit peer grants are required".into());
    }
    if host["allowToolDecisions"] == true && host["allowToolRequests"] != true {
        return Err("hostApi.allowToolDecisions: requires allowToolRequests".into());
    }
    if connectors["enable"] == true && connectors.get("configFile").is_none() {
        return Err("connectors.configFile: runtime configuration is required".into());
    }
    if storage["enable"] == true {
        if storage["paths"].as_array().is_none_or(Vec::is_empty) {
            return Err("storage.paths: at least one backup path is required".into());
        }
        for field in ["repositoryFile", "passwordFile"] {
            if storage.get(field).is_none() {
                return Err(format!(
                    "storage.{field}: runtime file reference is required"
                ));
            }
        }
    }
    if let Some(paths) = storage["paths"].as_array() {
        let state_dirs: BTreeSet<_> = agents
            .values()
            .filter_map(|agent| agent["stateDir"].as_str())
            .collect();
        for (index, path) in paths.iter().enumerate() {
            let path = path.as_str().unwrap();
            if !canonical_backup_path(path) && !state_dirs.contains(path) {
                return Err(format!(
                    "storage.paths[{index}]: requires a canonical state tree or exact declared stateDir"
                ));
            }
        }
    }
    let declared = |id: &str| {
        id.strip_prefix("agent-")
            .and_then(|name| agents.get(name).copied())
    };
    if let Some(peers) = host["peers"].as_array() {
        for (index, peer) in peers.iter().enumerate() {
            let ids = peer["agents"].as_array().unwrap();
            let permissions = peer["permissions"].as_array().unwrap();
            let has = |p: &str| permissions.iter().any(|v| v == p);
            let runner = permissions.iter().any(|permission| {
                schema["runnerPermissions"]
                    .as_array()
                    .unwrap()
                    .contains(permission)
            });
            for (agent_index, id) in ids.iter().enumerate() {
                let agent = declared(id.as_str().unwrap()).ok_or_else(|| {
                    format!("hostApi.peers[{index}].agents[{agent_index}]: requires an exact declared agent ID")
                })?;
                if runner
                    && !schema["runnerAdapters"]
                        .as_array()
                        .unwrap()
                        .contains(&agent["adapter"])
                {
                    return Err(format!(
                        "hostApi.peers[{index}].agents[{agent_index}]: runner permissions require a declared Pi Durable agent"
                    ));
                }
            }
            if (ids.is_empty() && permissions.iter().any(|p| p != "host_health"))
                || (has("tool_decisions") && !has("tool_requests"))
                || [
                    ("actions", "allowActions"),
                    ("logs", "allowLogs"),
                    ("tool_requests", "allowToolRequests"),
                    ("tool_decisions", "allowToolDecisions"),
                ]
                .iter()
                .any(|(permission, gate)| has(permission) && host[*gate] != true)
            {
                return Err(format!(
                    "hostApi.peers[{index}].permissions: requires exact nonempty scope and explicit operation grants"
                ));
            }
        }
    }
    if mapping["enable"] == true {
        if value["container"]
            .get("backend")
            .is_some_and(|backend| backend != "docker")
        {
            return Err("workloadMapping: requires the Docker backend".into());
        }
        let bindings = mapping["bindings"]
            .as_object()
            .ok_or("workloadMapping.bindings: explicit bindings are required")?;
        let mut scopes = BTreeSet::new();
        for (index, (id, binding)) in bindings.iter().enumerate() {
            if declared(id).is_none_or(|agent| permanently_stopped(schema, &agent["adapter"])) {
                return Err(format!(
                    "workloadMapping.bindings[{index}]: requires an exact declared runnable agent"
                ));
            }
            if !scopes.insert((
                binding["tenant_id"].as_str().unwrap(),
                binding["workload_id"].as_str().unwrap(),
            )) {
                return Err(format!(
                    "workloadMapping.bindings[{index}]: duplicate tenant and workload scope"
                ));
            }
        }
    }
    if research["enable"] == true {
        static CATALOG: LazyLock<Value> = LazyLock::new(|| {
            serde_json::from_str(super::RUNTIME_CATALOG).expect("embedded runtime catalog")
        });
        let clients = research["agents"]
            .as_object()
            .filter(|clients| !clients.is_empty())
            .ok_or("research.agents: explicit per-agent relay grants are required")?;
        if value["sandbox"]["profile"] == "dev" {
            return Err("sandbox.profile: Research requires a secure profile".into());
        }
        let mut uids = BTreeSet::new();
        for (index, (id, client)) in clients.iter().enumerate() {
            if declared(id).is_none_or(|agent| {
                permanently_stopped(schema, &agent["adapter"])
                    || CATALOG["presets"][agent["adapter"].as_str().unwrap()]["research"]["supported"]
                        != true
            }) {
                return Err(format!(
                    "research.agents[{index}]: requires an exact declared runnable Research-capable agent"
                ));
            }
            if !uids.insert(client["uid"].as_u64().unwrap()) {
                return Err(format!("research.agents[{index}].uid: duplicate relay UID"));
            }
        }
    }
    Ok(())
}

/// Serialize data, never source code, then verify semantic preservation.
pub fn normalize(value: &Value) -> Result<String, String> {
    validate(value)?;
    let text = toml_edit::ser::to_document(value)
        .map_err(|_| "cannot serialize desired system")?
        .to_string();
    if parse(text.as_bytes())? != *value {
        return Err("normalization would change configuration semantics".into());
    }
    Ok(text)
}

// JSON duplicate keys are ambiguous migration input, not last-writer-wins data.
struct UniqueJson(Value);

impl<'de> serde::Deserialize<'de> for UniqueJson {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Visitor;
        impl<'de> serde::de::Visitor<'de> for Visitor {
            type Value = UniqueJson;
            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("unambiguous desired-system JSON")
            }
            fn visit_bool<E: serde::de::Error>(self, value: bool) -> Result<Self::Value, E> {
                Ok(UniqueJson(Value::Bool(value)))
            }
            fn visit_i64<E: serde::de::Error>(self, value: i64) -> Result<Self::Value, E> {
                Ok(UniqueJson(Value::from(value)))
            }
            fn visit_u64<E: serde::de::Error>(self, value: u64) -> Result<Self::Value, E> {
                Ok(UniqueJson(Value::from(value)))
            }
            fn visit_str<E: serde::de::Error>(self, value: &str) -> Result<Self::Value, E> {
                Ok(UniqueJson(Value::String(value.to_owned())))
            }
            fn visit_string<E: serde::de::Error>(self, value: String) -> Result<Self::Value, E> {
                Ok(UniqueJson(Value::String(value)))
            }
            fn visit_unit<E: serde::de::Error>(self) -> Result<Self::Value, E> {
                Ok(UniqueJson(Value::Null))
            }
            fn visit_seq<A: serde::de::SeqAccess<'de>>(
                self,
                mut sequence: A,
            ) -> Result<Self::Value, A::Error> {
                let mut values = Vec::new();
                while let Some(UniqueJson(value)) = sequence.next_element()? {
                    values.push(value);
                }
                Ok(UniqueJson(Value::Array(values)))
            }
            fn visit_map<A: serde::de::MapAccess<'de>>(
                self,
                mut map: A,
            ) -> Result<Self::Value, A::Error> {
                let mut values = serde_json::Map::new();
                while let Some((key, UniqueJson(value))) = map.next_entry::<String, UniqueJson>()? {
                    if values.insert(key, value).is_some() {
                        return Err(serde::de::Error::custom("duplicate JSON field"));
                    }
                }
                Ok(UniqueJson(Value::Object(values)))
            }
        }
        deserializer.deserialize_any(Visitor)
    }
}

/// Explicit offline migration never drops unrepresentable policy, ownership or paths.
pub fn migrate(bytes: &[u8], json: bool) -> Result<String, String> {
    let value = if json {
        if bytes.len() as u64 > MAX_INPUT {
            return Err("migration input exceeds 1 MiB".into());
        }
        serde_json::from_slice::<UniqueJson>(bytes)
            .map_err(|_| "invalid or ambiguous legacy JSON")?
            .0
    } else {
        // The versioned legacy root is a subset only when every field survives validation.
        parse(bytes)?
    };
    normalize(&value)
}

fn read_file(path: &Path) -> Result<Vec<u8>, String> {
    let metadata =
        fs::symlink_metadata(path).map_err(|_| "configuration file is missing or unreadable")?;
    if !metadata.is_file() || metadata.len() > MAX_INPUT {
        return Err(
            "configuration requires a regular file of at most 1 MiB; symlinks refused".into(),
        );
    }
    let mut bytes = Vec::new();
    File::open(path)
        .and_then(|file| file.take(MAX_INPUT + 1).read_to_end(&mut bytes))
        .map_err(|_| "cannot read configuration")?;
    if bytes.len() as u64 > MAX_INPUT {
        return Err("configuration exceeds 1 MiB".into());
    }
    Ok(bytes)
}

fn refuse_legacy(path: &Path) -> Result<(), String> {
    let directory = path.parent().unwrap_or(Path::new("."));
    for legacy in ["agents.toml", "agents.json", "my-agents.nix"] {
        match fs::symlink_metadata(directory.join(legacy)) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            _ => {
                return Err(
                    "legacy inventory exists; explicitly migrate and archive it before loading"
                        .into(),
                );
            }
        }
    }
    Ok(())
}

pub(super) fn run(args: &[String], mode: super::OutputMode) -> Result<u8, String> {
    match run_inner(args, mode) {
        Err(error) if mode.json => {
            println!(
                "{}",
                serde_json::json!({
                    "schemaVersion": 1,
                    "valid": false,
                    "error": "invalid_configuration",
                    "message": error
                })
            );
            Ok(2)
        }
        Err(error) if args.first().is_some_and(|action| action == "migrate") => Err(format!(
            "migration refused; legacy source retained; no activation: {error}"
        )),
        result => result,
    }
}

fn run_inner(args: &[String], mode: super::OutputMode) -> Result<u8, String> {
    if mode.hide {
        return Err("--hide is unavailable for desired-system data".into());
    }
    match args {
        [action, input] if action == "validate" || action == "normalize" => {
            let path = Path::new(input);
            if path.file_name().and_then(|name| name.to_str()) != Some("tentaflake.toml") {
                return Err("desired-system file must be named tentaflake.toml".into());
            }
            refuse_legacy(path)?;
            let value = parse(&read_file(path)?)?;
            if action == "normalize" {
                if mode.json {
                    println!("{value}");
                } else {
                    print!("{}", normalize(&value)?);
                }
            } else if mode.json {
                println!("{{\"schemaVersion\":1,\"valid\":true}}");
            } else {
                println!("Valid desired system; no files changed");
            }
        }
        [action, input, output] if action == "migrate" => {
            let input = Path::new(input);
            if !matches!(input.file_name().and_then(|n| n.to_str()), Some("agents.toml" | "agents.json")) {
                return Err("migration input must be an explicit agents.toml or agents.json file".into());
            }
            let output = Path::new(output);
            if output.file_name().and_then(|n| n.to_str()) != Some("tentaflake.toml") {
                return Err("migration output must be named tentaflake.toml".into());
            }
            let bytes = read_file(input)?;
            let text = migrate(&bytes, input.extension().is_some_and(|e| e == "json"))?;
            let mut file = OpenOptions::new().write(true).create_new(true).mode(0o600)
                .open(output).map_err(|_| "migration destination already exists or cannot be created; no overwrite")?;
            if file.write_all(text.as_bytes()).and_then(|()| file.sync_all()).is_err() {
                let _ = fs::remove_file(output);
                return Err("cannot persist migration output".into());
            }
            if mode.json {
                println!("{{\"schemaVersion\":1,\"valid\":true,\"migrated\":true,\"sourceRetained\":true}}");
            } else {
                println!("Migration preserved all supported data; source retained. Review and archive legacy files before loading.");
            }
        }
        _ => return Err("usage: config validate <tentaflake.toml> | normalize <tentaflake.toml> [--json] | migrate <agents.toml|agents.json> <tentaflake.toml>".into()),
    }
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn old_descriptor_metadata_remains_stopped_and_openclaw_is_never_admitted() {
        let old = json!({"stoppedAdapters": ["pi-durable", "openclaw"]});
        let current: Value = serde_json::from_str(SCHEMA).unwrap();
        assert!(permanently_stopped(&old, &json!("pi-durable")));
        assert!(!permanently_stopped(&current, &json!("pi-durable")));
        assert!(permanently_stopped(&current, &json!("openclaw")));
    }

    #[test]
    fn shared_golden_semantics_and_refusals() {
        let corpus: Value =
            serde_json::from_str(include_str!("../../../schema/system-v1-fixtures.json")).unwrap();
        for case in corpus["valid"].as_array().unwrap() {
            validate(case).unwrap();
            assert_eq!(parse(normalize(case).unwrap().as_bytes()).unwrap(), *case);
            assert_eq!(
                serde_json::from_str::<Value>(&serde_json::to_string(case).unwrap()).unwrap(),
                *case
            );
        }
        for case in corpus["invalid"].as_array().unwrap() {
            assert!(validate(case).is_err(), "accepted invalid corpus entry");
        }
    }

    #[test]
    fn pi_tool_subsets_roundtrip_without_changing_omission_or_explicit_none() {
        let schema: Value = serde_json::from_str(SCHEMA).unwrap();
        let all = &schema["agent"]["tools"]["items"]["values"];
        for tools in [
            None,
            Some(json!([])),
            Some(json!(["research_search"])),
            Some(all.clone()),
        ] {
            let mut value =
                json!({"schemaVersion":1,"agents":[{"name":"specialist","adapter":"pi-durable"}]});
            if let Some(tools) = tools {
                value["agents"][0]["tools"] = tools;
            }
            let normalized = normalize(&value).unwrap();
            assert_eq!(parse(normalized.as_bytes()).unwrap(), value);
            assert_eq!(
                normalized.contains("tools ="),
                value["agents"][0].get("tools").is_some()
            );
            for adapter in ["hermes", "zeroclaw", "openclaw"] {
                if value["agents"][0].get("tools").is_some() {
                    let mut invalid = value.clone();
                    invalid["agents"][0]["adapter"] = json!(adapter);
                    assert!(validate(&invalid).unwrap_err().contains(".tools:"));
                }
            }
        }
        for tools in [
            json!(["read"]),
            json!(["research_search", "research_search"]),
            Value::Null,
        ] {
            let invalid = json!({"schemaVersion":1,"agents":[{"name":"specialist","adapter":"pi-durable","tools":tools}]});
            assert!(validate(&invalid).unwrap_err().contains(".tools"));
        }
    }

    #[test]
    fn runner_grants_are_independent_and_only_cover_declared_pi_durable_agents() {
        for permission in ["run_read", "run_submit", "run_stop"] {
            let value = json!({
                "schemaVersion":1,"agents":[{"name":"one","adapter":"pi-durable"}],
                "hostApi":{"enable":true,"peers":[{
                    "uid":1000,"agents":["agent-one"],"permissions":[permission]
                }]}
            });
            assert_eq!(parse(normalize(&value).unwrap().as_bytes()).unwrap(), value);
            for adapter in ["hermes", "zeroclaw", "openclaw", "generic"] {
                let mut invalid = value.clone();
                invalid["agents"][0]["adapter"] = json!(adapter);
                if adapter == "generic" {
                    invalid["agents"][0]["definition"] = json!({
                        "schemaVersion":1,
                        "image":"test/image@sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                        "command":["/bin/true"]
                    });
                }
                assert!(
                    validate(&invalid)
                        .unwrap_err()
                        .starts_with("hostApi.peers[0].agents[0]:")
                );
            }
        }
        let agents: Vec<_> = (0..300)
            .map(|i| json!({"name":format!("runner-{i}"),"adapter":"pi-durable","autoStart":false}))
            .collect();
        let ids: Vec<_> = (0..300).map(|i| format!("agent-runner-{i}")).collect();
        let mut fleet = json!({
            "schemaVersion":1,"agents":agents,
            "hostApi":{"enable":true,"peers":[{
                "uid":1000,"agents":ids,"permissions":["run_read","run_submit","run_stop"]
            }]}
        });
        assert_eq!(parse(normalize(&fleet).unwrap().as_bytes()).unwrap(), fleet);
        fleet["agents"][299]["adapter"] = json!("hermes");
        assert_eq!(
            validate(&fleet).unwrap_err(),
            "hostApi.peers[0].agents[299]: runner permissions require a declared Pi Durable agent"
        );
    }

    #[test]
    fn validation_diagnostics_use_fixed_fields_and_indexes_not_input_secrets() {
        for (value, expected) in [
            (
                json!({"schemaVersion":1,"agents":[],"DO-NOT-PRINT":{"secret":"DO-NOT-PRINT"}}),
                "desired system: unknown section",
            ),
            (
                json!({"schemaVersion":1,"agents":[],"hostApi":{"DO-NOT-PRINT":"DO-NOT-PRINT"}}),
                "hostApi: unknown or unsafe field",
            ),
            (
                json!({"schemaVersion":1,"agents":[{"name":"do-not-print","adapter":"hermes","DO-NOT-PRINT":"DO-NOT-PRINT"}]}),
                "agents[0]: unknown or unsafe field",
            ),
            (
                json!({"schemaVersion":1,"agents":[{"name":"do-not-print","adapter":"DO-NOT-PRINT"}]}),
                "agents[0].adapter: invalid or unsupported value",
            ),
            (
                json!({"schemaVersion":1,"agents":[],"research":{"enable":true,"agents":{"DO-NOT-PRINT":{"uid":1}}}}),
                "research.agents[0]: invalid canonical agent ID",
            ),
            (
                json!({"schemaVersion":1,"agents":[],"research":{"enable":true,"agents":{"agent-do-not-print":{"DO-NOT-PRINT":"DO-NOT-PRINT"}}}}),
                "research.agents[0]: unknown or unsafe field",
            ),
            (
                json!({"schemaVersion":1,"agents":[],"research":{"enable":true,"agents":{"agent-do-not-print":{"uid":1}}}}),
                "research.agents[0]: requires an exact declared runnable Research-capable agent",
            ),
            (
                json!({"schemaVersion":1,"agents":[],"hostApi":{"enable":true,"peers":[{"uid":1,"agents":["agent-do-not-print"],"permissions":["read"]}]}}),
                "hostApi.peers[0].agents[0]: requires an exact declared agent ID",
            ),
            (
                json!({"schemaVersion":1,"agents":[],"workloadMapping":{"enable":true,"bindings":{"agent-do-not-print":{"tenant_id":"DO-NOT-PRINT","workload_id":"DO-NOT-PRINT","policy_revision":"DO-NOT-PRINT"}}}}),
                "workloadMapping.bindings[0]: requires an exact declared runnable agent",
            ),
            (
                json!({"schemaVersion":1,"agents":[],"storage":{"enable":true,"paths":["/var/lib/do-not-print"],"repositoryFile":"/run/DO-NOT-PRINT","passwordFile":"/run/DO-NOT-PRINT"}}),
                "storage.paths[0]: requires a canonical state tree or exact declared stateDir",
            ),
        ] {
            let error = validate(&value).unwrap_err();
            assert_eq!(error, expected);
            assert!(!error.to_ascii_lowercase().contains("do-not-print"));
            assert!(!error.chars().any(char::is_control));
            assert!(error.len() <= 2048);
        }
        let mut nested = json!({
            "schemaVersion":1,"agents":[{"name":"do-not-print","adapter":"generic","definition":{
                "schemaVersion":1,
                "image":"test/image@sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                "command":["/bin/true"],"resources":{"memory":"DO-NOT-PRINT"}
            }}]
        });
        assert_eq!(
            validate(&nested).unwrap_err(),
            "agents[0].definition.resources.memory: invalid or unsupported value"
        );
        nested["agents"][0]["definition"]["resources"] = json!({"DO-NOT-PRINT":"DO-NOT-PRINT"});
        assert_eq!(
            validate(&nested).unwrap_err(),
            "agents[0].definition.resources: unknown or unsafe field"
        );
        assert_eq!(
            typed(
                &json!({"uid":1}),
                &json!({
                    "type":"object","required":["agents"],"fields":{"uid":{"type":"owner"}}
                }),
            )
            .unwrap_err(),
            ".agents: field is required"
        );
    }

    #[test]
    fn desired_input_stays_bounded_to_one_mib() {
        assert_eq!(
            parse(&vec![b' '; MAX_INPUT as usize + 1]).unwrap_err(),
            "desired system exceeds 1 MiB"
        );
    }

    #[test]
    fn optional_collection_maxima_preserve_explicit_finite_bounds() {
        let values = json!(["agent-one", "agent-two", "agent-three"]);
        let mut list = json!({"type":"list","unique":true,"items":{"type":"agentId"}});
        typed(&values, &list).unwrap();
        list["max"] = json!(2);
        assert_eq!(typed(&values, &list).unwrap_err(), ": too many items");
        let values = json!({"agent-one":{"uid":1},"agent-two":{"uid":2}});
        let mut map = json!({
            "type":"map","keys":{"type":"agentId"},
            "items":{"type":"object","required":["uid"],"fields":{"uid":{"type":"owner"}}}
        });
        typed(&values, &map).unwrap();
        map["max"] = json!(1);
        assert_eq!(typed(&values, &map).unwrap_err(), ": too many entries");
    }

    #[test]
    fn research_relay_fleets_are_not_count_limited() {
        for count in [128, 129, 300] {
            let agents: Vec<_> = (0..count)
                .map(|i| json!({"name":format!("r{i}"),"adapter":"hermes","autoStart":false}))
                .collect();
            let clients: serde_json::Map<String, Value> = (0..count)
                .map(|i| (format!("agent-r{i}"), json!({"uid":i + 1})))
                .collect();
            let value = json!({
                "schemaVersion":1, "agents":agents,
                "research":{"enable":true,"agents":clients}
            });
            validate(&value).unwrap();
            assert_eq!(parse(normalize(&value).unwrap().as_bytes()).unwrap(), value);
            let mut duplicate_uid = value.clone();
            duplicate_uid["research"]["agents"]["agent-r1"]["uid"] = json!(1);
            assert!(validate(&duplicate_uid).is_err());
        }
    }

    #[test]
    fn saved_service_fleets_scale_without_weakening_field_limits() {
        let service = |count: usize| {
            let agents: Vec<_> = (0..count)
                .map(|i| {
                    json!({
                        "name": format!("a{i}"), "adapter": "hermes", "autoStart": false
                    })
                })
                .collect();
            let peers: Vec<_> = (0..count)
                .map(|i| {
                    json!({
                        "uid": i + 1, "agents": [format!("agent-a{i}")], "permissions": ["read"]
                    })
                })
                .collect();
            let bindings: serde_json::Map<String, Value> = (0..count).map(|i| (
                format!("agent-a{i}"),
                json!({"tenant_id":"tenant","workload_id":format!("w{i}"),"policy_revision":"r"})
            )).collect();
            let paths: Vec<_> = (0..count)
                .map(|i| format!("/var/lib/tentaflake/agents/a{i}"))
                .collect();
            json!({"schemaVersion":1,"agents":agents,"hostApi":{"enable":true,"peers":peers},
                "workloadMapping":{"enable":true,"bindings":bindings},
                "storage":{"enable":true,"paths":paths,
                    "repositoryFile":"/run/backup/repository","passwordFile":"/run/backup/password"}})
        };
        for count in [64, 65, 128, 129, 300] {
            let fleet = service(count);
            validate(&fleet).unwrap();
            let mut inventory = fleet;
            inventory["hostApi"] = json!({"enable":false});
            inventory["workloadMapping"] = json!({"enable":false});
            validate(&inventory).unwrap();
        }
        let mut valid = service(300);
        let all_ids: Vec<_> = (0..300).map(|i| format!("agent-a{i}")).collect();
        valid["hostApi"]["peers"]
            .as_array_mut()
            .unwrap()
            .push(json!({"uid":301,"agents":all_ids,"permissions":["read"]}));
        let paths: Vec<_> = (0..300)
            .flat_map(|i| {
                [
                    format!("/var/lib/tentaflake/agents/a{i}"),
                    format!("/var/lib/tentaflake/agents/a{i}/workspace"),
                    format!("/var/lib/tentaflake-broker-llm-agent-a{i}"),
                ]
            })
            .collect();
        valid["storage"]["paths"] = json!(paths);
        validate(&valid).unwrap();
        let migrated = migrate(&serde_json::to_vec(&valid).unwrap(), true).unwrap();
        assert_eq!(parse(migrated.as_bytes()).unwrap(), valid);
        let mut host_only = service(65);
        host_only["workloadMapping"] = json!({"enable":false});
        validate(&host_only).unwrap();
        let mut mapping_only = service(65);
        mapping_only["hostApi"] = json!({"enable":false});
        validate(&mapping_only).unwrap();
        let mut duplicate_peer = valid.clone();
        duplicate_peer["hostApi"]["peers"][300]["uid"] = json!(1);
        assert!(validate(&duplicate_peer).is_err());
        let mut duplicate_id = valid.clone();
        duplicate_id["hostApi"]["peers"][300]["agents"][299] = json!("agent-a0");
        assert!(validate(&duplicate_id).is_err());
        let mut duplicate_path = valid.clone();
        duplicate_path["storage"]["paths"][899] = duplicate_path["storage"]["paths"][0].clone();
        assert!(validate(&duplicate_path).is_err());
        let mut duplicate_binding = valid.clone();
        duplicate_binding["workloadMapping"]["bindings"]["agent-a1"]["workload_id"] = json!("w0");
        assert!(validate(&duplicate_binding).is_err());
        assert!(
            validate(&json!({"schemaVersion":1,"agents":[],
                "connectors":{"enable":true,"configFile":format!("/run/{}", "a".repeat(252))}
            }))
            .is_err()
        );
        let mut long_id = service(1);
        long_id["workloadMapping"]["bindings"]["agent-a0"]["tenant_id"] = json!("a".repeat(65));
        assert!(validate(&long_id).is_err());
    }

    #[test]
    fn argv_rejects_nul_before_nix_serialization() {
        // Nix rejects NUL at its parser boundary, so it cannot occur in the
        // shared JSON corpus. The Rust boundary must reject it as well.
        assert!(typed(&json!(["/bin/true\0"]), &json!({"type":"command"})).is_err());
    }

    #[test]
    fn identity_limits_include_the_runtime_prefix() {
        assert!(typed(&json!("a".repeat(57)), &json!({"type":"name"})).is_ok());
        assert!(typed(&json!("a".repeat(58)), &json!({"type":"name"})).is_err());
        assert!(typed(&json!("a".repeat(63)), &json!({"type":"hostName"})).is_ok());
        assert!(typed(&json!("a".repeat(64)), &json!({"type":"hostName"})).is_err());
        assert!(typed(&json!("host-"), &json!({"type":"hostName"})).is_err());
    }

    #[test]
    fn rejects_nonfinite_dates_secrets_and_parser_disclosures() {
        for value in ["nan", "inf", "-inf", "1979-05-27T07:32:00Z"] {
            assert!(
                parse(
                    format!("schemaVersion = 1\nagents = []\n[system]\nhostName = {value}\n")
                        .as_bytes()
                )
                .is_err()
            );
        }
        assert!(
            !parse(b"secret = 'DO-NOT-PRINT")
                .unwrap_err()
                .contains("DO-NOT-PRINT")
        );
        assert!(
            validate(&json!({"schemaVersion":1,"agents":[],"llm":{"apiKey":"secret"}})).is_err()
        );
    }

    #[test]
    fn migration_preserves_or_refuses_without_silently_dropping_policy() {
        let value = json!({"schemaVersion":1,"agents":[{"name":"kept","adapter":"hermes","autoStart":false,"stateDir":"/var/lib/private/kept","uid":1234,"gid":1234}]});
        let output = migrate(&serde_json::to_vec(&value).unwrap(), true).unwrap();
        assert_eq!(parse(output.as_bytes()).unwrap(), value);
        for key in [
            "settings",
            "extraVolumes",
            "cmd",
            "envFile",
            "extraContainerConfig",
        ] {
            let mut unsafe_value = value.clone();
            unsafe_value["agents"][0][key] = json!("preserve-or-refuse");
            assert!(migrate(&serde_json::to_vec(&unsafe_value).unwrap(), true).is_err());
        }
        let metadata = json!({"schemaVersion":1,"agents":[],"_securityNote":"must not drop"});
        let migrated = migrate(&serde_json::to_vec(&metadata).unwrap(), true).unwrap();
        assert_eq!(parse(migrated.as_bytes()).unwrap(), metadata);
        assert!(validate(&json!({"schemaVersion":1,"agents":[],"_securityNote":true})).is_err());
        assert!(migrate(br#"{"hermes":[]}"#, true).is_err());
        assert!(migrate(br#"{"schemaVersion":1,"agents":[],"agents":[]}"#, true).is_err());
        assert!(
            migrate(
                br#"{"schemaVersion":1,"agents":[{"name":"first","name":"last","adapter":"pi-durable"}]}"#,
                true
            )
            .is_err()
        );
    }

    #[test]
    fn offline_migration_is_create_only_and_legacy_loading_is_explicit() {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let directory =
            std::env::temp_dir().join(format!("tentaflake-schema-{}-{nonce}", std::process::id()));
        fs::create_dir(&directory).unwrap();
        let source = directory.join("agents.toml");
        let destination = directory.join("tentaflake.toml");
        let original = b"schemaVersion = 1\nagents = []\n";
        fs::write(&source, original).unwrap();
        let args = vec![
            "migrate".into(),
            source.to_str().unwrap().into(),
            destination.to_str().unwrap().into(),
        ];
        assert_eq!(
            run_inner(&args, super::super::OutputMode::default()).unwrap(),
            0
        );
        assert_eq!(fs::read(&source).unwrap(), original);
        let migrated = fs::read(&destination).unwrap();
        assert_eq!(parse(&migrated).unwrap(), parse(original).unwrap());
        assert!(run_inner(&args, super::super::OutputMode::default()).is_err());
        assert_eq!(fs::read(&destination).unwrap(), migrated);
        assert!(refuse_legacy(&destination).is_err());
        fs::remove_file(&source).unwrap();
        assert!(refuse_legacy(&destination).is_ok());
        fs::remove_file(&destination).unwrap();
        assert!(read_file(&destination).is_err());
        let unsupported = b"schemaVersion = 1\nagents = []\n_unknownMetadata = 'must remain'\n";
        fs::write(&source, unsupported).unwrap();
        assert!(run_inner(&args, super::super::OutputMode::default()).is_err());
        assert_eq!(fs::read(&source).unwrap(), unsupported);
        assert!(!destination.exists());
        fs::remove_file(&source).unwrap();
        std::os::unix::fs::symlink(directory.join("missing"), &source).unwrap();
        assert!(refuse_legacy(&destination).is_err());
        assert!(read_file(&source).is_err());
        fs::remove_dir_all(directory).unwrap();
    }
}
