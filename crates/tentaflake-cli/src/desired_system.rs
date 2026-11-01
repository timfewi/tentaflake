//! Data-only desired-system validation. The descriptor is shared with the Nix compiler.
use serde_json::Value;
use std::collections::BTreeSet;
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

fn matches_type(value: &Value, spec: &Value) -> bool {
    match spec["type"].as_str().unwrap_or("") {
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
        "list" => value.as_array().is_some_and(|items| {
            items.len() <= spec["max"].as_u64().unwrap() as usize
                && items.iter().all(|item| matches_type(item, &spec["items"]))
                && items.iter().enumerate().all(|(i, item)| {
                    (spec["unique"] != true || !items[..i].contains(item))
                        && spec["uniqueBy"].as_str().is_none_or(|key| {
                            !items[..i].iter().any(|prior| prior[key] == item[key])
                        })
                })
        }),
        "map" => value.as_object().is_some_and(|items| {
            items.len() <= spec["max"].as_u64().unwrap() as usize
                && items.iter().all(|(key, item)| {
                    spec["keys"]["type"] == "agentId"
                        && agent_id(key)
                        && matches_type(item, &spec["items"])
                })
        }),
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
            object(value, &spec["fields"]).is_ok()
                && spec["required"].as_array().is_none_or(|required| {
                    required
                        .iter()
                        .all(|key| value.get(key.as_str().unwrap()).is_some())
                })
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
    }
}

fn object(value: &Value, fields: &Value) -> Result<(), String> {
    let object = value
        .as_object()
        .ok_or("configuration section must be an object")?;
    for (key, value) in object {
        let spec = fields
            .get(key)
            .ok_or("unknown or unsafe configuration field")?;
        if !matches_type(value, spec) {
            return Err("unsupported configuration value or invalid field type".into());
        }
    }
    Ok(())
}

/// Validate JSON transport against exactly the same descriptor as TOML and Nix.
pub fn validate(value: &Value) -> Result<(), String> {
    static DESCRIPTOR: LazyLock<Value> =
        LazyLock::new(|| serde_json::from_str(SCHEMA).expect("embedded schema descriptor"));
    let schema = &*DESCRIPTOR;
    let root = value
        .as_object()
        .ok_or("desired system must be an object")?;
    if value["schemaVersion"].as_u64() != Some(1) {
        return Err("schemaVersion must be integer 1".into());
    }
    for (key, section) in root {
        if key == "schemaVersion" || key == "agents" {
            continue;
        }
        if key == "_securityNote" {
            if !section.is_string() {
                return Err("_securityNote must be a string".into());
            }
            continue;
        }
        let fields = schema["sections"]
            .get(key)
            .ok_or("unknown desired-system section")?;
        object(section, fields)?;
    }
    let agents = value["agents"]
        .as_array()
        .ok_or("agents array is required")?;
    if agents.len() > 128 {
        return Err("at most 128 agents are supported".into());
    }
    let mut identities = BTreeSet::new();
    for agent in agents {
        object(agent, &schema["agent"])?;
        let identity = agent["name"].as_str().ok_or("agent name is required")?;
        if agent.get("adapter").is_none() {
            return Err("agent adapter is required".into());
        }
        if agent["adapter"] == "generic" && agent.get("definition").is_none() {
            return Err("generic adapter requires a runtime definition".into());
        }
        if schema["stoppedAdapters"]
            .as_array()
            .unwrap()
            .contains(&agent["adapter"])
            && agent["autoStart"] == true
        {
            return Err("adapter has no admitted runnable implementation".into());
        }
        for key in agent.as_object().unwrap().keys() {
            if let Some(adapters) = schema["agent"][key]["adapters"].as_array()
                && !adapters.contains(&agent["adapter"])
            {
                return Err("field is unsupported by the selected adapter".into());
            }
        }
        if !identities.insert(identity) {
            return Err("duplicate global agent identity".into());
        }
    }
    validate_services(value, agents, schema)?;
    Ok(())
}

fn validate_services(value: &Value, agents: &[Value], schema: &Value) -> Result<(), String> {
    let host = &value["hostApi"];
    let connectors = &value["connectors"];
    let mapping = &value["workloadMapping"];
    let storage = &value["storage"];
    let research = &value["research"];
    for section in [host, connectors, mapping, storage, research] {
        if section["enable"] != true
            && section
                .as_object()
                .is_some_and(|fields| fields.keys().any(|key| key != "enable"))
        {
            return Err("service configuration requires explicit enable".into());
        }
    }
    if (host["enable"] == true && host.get("peers").is_none())
        || (host["allowToolDecisions"] == true && host["allowToolRequests"] != true)
        || (connectors["enable"] == true && connectors.get("configFile").is_none())
        || (storage["enable"] == true
            && (storage["paths"].as_array().is_none_or(Vec::is_empty)
                || storage.get("repositoryFile").is_none()
                || storage.get("passwordFile").is_none()))
    {
        return Err("service requires explicit grants or runtime configuration".into());
    }
    if let Some(paths) = storage["paths"].as_array()
        && paths.iter().any(|path| {
            !canonical_backup_path(path.as_str().unwrap())
                && !agents
                    .iter()
                    .any(|agent| agent.get("stateDir") == Some(path))
        })
    {
        return Err(
            "backup paths require a canonical state tree or exact declared stateDir".into(),
        );
    }
    let declared = |id: &Value| {
        id.as_str()
            .and_then(|id| id.strip_prefix("agent-"))
            .and_then(|name| agents.iter().find(|agent| agent["name"] == name))
    };
    if let Some(peers) = host["peers"].as_array() {
        for peer in peers {
            let ids = peer["agents"].as_array().unwrap();
            let permissions = peer["permissions"].as_array().unwrap();
            let has = |p: &str| permissions.iter().any(|v| v == p);
            if ids.iter().any(|id| declared(id).is_none())
                || (ids.is_empty() && permissions.iter().any(|p| p != "host_health"))
                || (has("tool_decisions") && !has("tool_requests"))
                || ["run_read", "run_submit", "run_stop"]
                    .iter()
                    .any(|p| has(p))
                || [
                    ("actions", "allowActions"),
                    ("logs", "allowLogs"),
                    ("tool_requests", "allowToolRequests"),
                    ("tool_decisions", "allowToolDecisions"),
                ]
                .iter()
                .any(|(permission, gate)| has(permission) && host[*gate] != true)
            {
                return Err("invalid or unsupported exact peer grant".into());
            }
        }
    }
    if mapping["enable"] == true {
        if value["container"]
            .get("backend")
            .is_some_and(|backend| backend != "docker")
            || mapping.get("bindings").is_none()
        {
            return Err("workload mapping requires Docker and explicit bindings".into());
        }
        let mut scopes = BTreeSet::new();
        for (id, binding) in mapping["bindings"].as_object().unwrap() {
            let agent = agents.iter().find(|agent| {
                id.strip_prefix("agent-")
                    .is_some_and(|name| agent["name"] == name)
            });
            if agent.is_none_or(|agent| agent["adapter"] == "pi" || agent["adapter"] == "openclaw")
                || !scopes.insert((
                    binding["tenant_id"].as_str().unwrap(),
                    binding["workload_id"].as_str().unwrap(),
                ))
            {
                return Err(
                    "workload binding requires an unambiguous declared runnable agent".into(),
                );
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
            .ok_or("Research requires explicit per-agent relay grants")?;
        if value["sandbox"]["profile"] == "dev" {
            return Err("Research requires a secure profile".into());
        }
        let mut uids = BTreeSet::new();
        for (id, client) in clients {
            let agent = agents.iter().find(|agent| {
                id.strip_prefix("agent-")
                    .is_some_and(|name| agent["name"] == name)
            });
            if agent.is_none_or(|agent| {
                schema["stoppedAdapters"]
                    .as_array()
                    .unwrap()
                    .contains(&agent["adapter"])
                    || CATALOG["presets"][agent["adapter"].as_str().unwrap()]["research"]["supported"]
                        != true
            }) || !uids.insert(client["uid"].as_u64().unwrap())
            {
                return Err(
                    "Research requires declared runnable capable agents and distinct relay UIDs"
                        .into(),
                );
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
        Err(_) if mode.json => {
            println!("{{\"schemaVersion\":1,\"valid\":false,\"error\":\"invalid_configuration\"}}");
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
    fn research_relay_limits() {
        for (count, accepted) in [(128, true), (129, false)] {
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
            assert_eq!(validate(&value).is_ok(), accepted);
        }
    }

    #[test]
    fn service_limits_and_migration_preservation() {
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
            json!({"schemaVersion":1,"agents":agents,"hostApi":{"enable":true,"peers":peers},
                "workloadMapping":{"enable":true,"bindings":bindings}})
        };
        for (count, accepted) in [(128, true), (129, false)] {
            let mut inventory = service(count);
            inventory["hostApi"] = json!({"enable":false});
            inventory["workloadMapping"] = json!({"enable":false});
            assert_eq!(validate(&inventory).is_ok(), accepted);
        }
        let valid = service(64);
        validate(&valid).unwrap();
        let migrated = migrate(&serde_json::to_vec(&valid).unwrap(), true).unwrap();
        assert_eq!(parse(migrated.as_bytes()).unwrap(), valid);
        let mut too_many = service(65);
        too_many["workloadMapping"] = json!({"enable":false});
        assert!(validate(&too_many).is_err());
        too_many = service(65);
        too_many["hostApi"] = json!({"enable":false});
        assert!(validate(&too_many).is_err());
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
        assert!(!matches_type(
            &json!(["/bin/true\0"]),
            &json!({"type":"command"})
        ));
    }

    #[test]
    fn identity_limits_include_the_runtime_prefix() {
        assert!(matches_type(
            &json!("a".repeat(57)),
            &json!({"type":"name"})
        ));
        assert!(!matches_type(
            &json!("a".repeat(58)),
            &json!({"type":"name"})
        ));
        assert!(matches_type(
            &json!("a".repeat(63)),
            &json!({"type":"hostName"})
        ));
        assert!(!matches_type(
            &json!("a".repeat(64)),
            &json!({"type":"hostName"})
        ));
        assert!(!matches_type(&json!("host-"), &json!({"type":"hostName"})));
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
                br#"{"schemaVersion":1,"agents":[{"name":"first","name":"last","adapter":"pi"}]}"#,
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
