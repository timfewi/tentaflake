//! Add stopped declarative workloads; activation remains an explicit host update.
use super::{Agent, Config, OutputMode, RUNTIME_CATALOG, output_with_timeout};
use serde_json::{Value, json};
use std::fs::{self, DirBuilder, File, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use toml_edit::{DocumentMut, Item};

const MAX_INPUT: u64 = 1024 * 1024;
const MAX_AGENTS: usize = 128;

pub(super) fn template(args: &[String], mode: OutputMode) -> Result<u8, String> {
    if mode.hide {
        return Err(
            "--hide is unavailable for importable definitions; use agent plan --hide".into(),
        );
    }
    let [preset, name, rest @ ..] = args else {
        return Err(
            "usage: agent template <preset> <name> [--image <digest-reference> -- <command>...]"
                .into(),
        );
    };
    if name.is_empty()
        || !name.as_bytes()[0].is_ascii_alphanumeric()
        || !name
            .bytes()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-')
    {
        return Err("agent names require lowercase ASCII letters, digits and hyphens".into());
    }
    let catalog: Value = serde_json::from_str(RUNTIME_CATALOG).map_err(|e| e.to_string())?;
    if catalog["presets"].get(preset).is_none() {
        return Err("unknown preset; run tentaflake runtimes".into());
    }
    let mut entry = json!({"adapter": preset, "name": name});
    if preset == "generic" {
        let [image_flag, image, separator, command @ ..] = rest else {
            return Err(
                "generic template requires --image <digest-reference> -- <command>...".into(),
            );
        };
        if image_flag != "--image" || separator != "--" || command.is_empty() {
            return Err(
                "generic template requires --image <digest-reference> -- <command>...".into(),
            );
        }
        entry["definition"] = json!({
            "schemaVersion": 1, "image": image, "command": command,
            "lifecycle": "stopped", "capabilities": ["files", "shell", "terminal"]
        });
    } else {
        if !rest.is_empty() {
            return Err("preset template accepts only preset and name".into());
        }
        entry["autoStart"] = json!(false);
    }
    println!(
        "{}",
        toml_edit::ser::to_document(&json!({"schemaVersion": 1, "agents": [entry]}))
            .map_err(|_| "cannot serialize onboarding TOML")?
    );
    Ok(0)
}

fn read_optional(path: &Path) -> Result<Option<Vec<u8>>, String> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err("cannot inspect onboarding configuration file".into()),
    };
    if !metadata.is_file() || metadata.len() > MAX_INPUT {
        return Err(
            "onboarding requires regular files of at most 1 MiB; symlinks are refused".into(),
        );
    }
    let mut bytes = Vec::new();
    File::open(path)
        .and_then(|file| file.take(MAX_INPUT + 1).read_to_end(&mut bytes))
        .map_err(|_| "cannot read onboarding configuration file")?;
    if bytes.len() as u64 > MAX_INPUT {
        return Err("onboarding input exceeds 1 MiB".into());
    }
    Ok(Some(bytes))
}

fn document(bytes: &[u8]) -> Result<DocumentMut, String> {
    std::str::from_utf8(bytes)
        .map_err(|_| "onboarding TOML must be UTF-8")?
        .parse()
        // Parser diagnostics include source excerpts; never expose operator data.
        .map_err(|_| "invalid onboarding TOML; JSON input requires explicit migration".into())
}

fn json_compatible(value: &toml_edit::Value) -> bool {
    match value {
        toml_edit::Value::Datetime(_) => false,
        toml_edit::Value::Float(value) => value.value().is_finite(),
        toml_edit::Value::Array(values) => values.iter().all(json_compatible),
        toml_edit::Value::InlineTable(values) => {
            values.iter().all(|(_, value)| json_compatible(value))
        }
        _ => true,
    }
}

fn compatible_item(item: &Item) -> bool {
    match item {
        Item::None => true,
        Item::Value(value) => json_compatible(value),
        Item::Table(table) => table.iter().all(|(_, value)| compatible_item(value)),
        Item::ArrayOfTables(tables) => tables
            .iter()
            .all(|table| table.iter().all(|(_, value)| compatible_item(value))),
    }
}

fn parse(bytes: &[u8]) -> Result<Value, String> {
    let document = document(bytes)?;
    if !document.iter().all(|(_, value)| compatible_item(value)) {
        return Err("onboarding TOML does not support dates, times or non-finite numbers".into());
    }
    toml_edit::de::from_document(document).map_err(|_| "invalid onboarding TOML data".into())
}

fn serialize_import(existing: Option<&[u8]>, incoming: &Value) -> Result<Vec<u8>, String> {
    let mut document = document(existing.unwrap_or(b"schemaVersion = 1\nagents = []\n"))?;
    for entry in additions(incoming)? {
        let table = toml_edit::ser::to_document(entry)
            .map_err(|_| "cannot serialize onboarding TOML")?
            .into_table();
        match &mut document["agents"] {
            Item::ArrayOfTables(tables) => tables.push(table),
            Item::Value(toml_edit::Value::Array(values)) => {
                values.push(table.into_inline_table());
            }
            _ => return Err("existing agents must be an array".into()),
        }
    }
    Ok(document.to_string().into_bytes())
}

fn refuse_legacy(directory: &Path) -> Result<(), String> {
    match fs::symlink_metadata(directory.join("agents.json")) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Ok(_) => Err(
            "legacy agents.json exists; explicitly migrate it to agents.toml and remove the JSON file before onboarding; no files changed".into(),
        ),
        Err(_) => Err("cannot inspect legacy agents.json; no files changed".into()),
    }
}

fn schema_entries(value: &Value) -> Result<&Vec<Value>, String> {
    let object = value.as_object().ok_or("agent input must be an object")?;
    if value["schemaVersion"] != 1
        || object
            .keys()
            .any(|key| !["schemaVersion", "agents", "_securityNote"].contains(&key.as_str()))
    {
        return Err(
            "agent input requires schemaVersion 1 and agents; unknown or legacy fields are refused"
                .into(),
        );
    }
    value["agents"]
        .as_array()
        .ok_or("agents must be an array".into())
}

fn additions(value: &Value) -> Result<&Vec<Value>, String> {
    let entries = schema_entries(value)?;
    if entries.is_empty() || entries.len() > MAX_AGENTS {
        return Err("onboarding requires between 1 and 128 agent additions".into());
    }
    Ok(entries)
}

// Conflict projection only: the shared Nix parser validates workload semantics.
fn identities(value: &Value) -> Result<Vec<String>, String> {
    let mut result = Vec::new();
    for entry in schema_entries(value)? {
        let name = entry["name"].as_str().ok_or("missing agent identity")?;
        result.push(format!("agent-{name}"));
    }
    Ok(result)
}

fn check_conflicts(
    existing: Option<&[u8]>,
    incoming: &Value,
    agents: &[Agent],
) -> Result<(), String> {
    let combined = match existing {
        Some(bytes) => parse(bytes)?,
        None => json!({"schemaVersion": 1, "agents": []}),
    };
    let mut known = identities(&combined)?;
    known.extend(agents.iter().map(|agent| agent.container.clone()));
    for identity in identities(incoming)? {
        if known.contains(&identity) {
            return Err("agent identity conflicts with existing configuration or active inventory; no files changed".into());
        }
        known.push(identity);
    }
    additions(incoming)?;
    Ok(())
}

struct PrivateInput(PathBuf);
impl PrivateInput {
    fn new(bytes: &[u8]) -> Result<Self, String> {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|e| e.to_string())?
            .as_nanos();
        let directory = std::env::temp_dir().join(format!(
            "tentaflake-onboarding-{}-{nonce}",
            std::process::id()
        ));
        DirBuilder::new()
            .mode(0o700)
            .create(&directory)
            .map_err(|_| "cannot create private validation directory")?;
        let input = Self(directory);
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(0o600)
            .open(input.path())
            .map_err(|_| "cannot create private validation input")?;
        file.write_all(bytes)
            .map_err(|_| "cannot write private validation input")?;
        Ok(input)
    }
    fn path(&self) -> PathBuf {
        self.0.join("input.json")
    }
}
impl Drop for PrivateInput {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn validate(config: &Config, bytes: &[u8]) -> Result<Value, String> {
    let input = PrivateInput::new(bytes)?;
    let entrypoint = config.flake_dir.join("lib/agentPlanEntry.nix");
    let mut command = Command::new("nix");
    command
        .args([
            "eval",
            "--offline",
            "--impure",
            "--json",
            "--no-write-lock-file",
            "--file",
        ])
        .arg(entrypoint)
        .arg("result")
        .args(["--argstr", "file"])
        .arg(input.path())
        .args(["--argstr", "flakeDir"])
        .arg(&config.flake_dir)
        .args([
            "--argstr",
            "backend",
            &config.backend,
            "--argstr",
            "hostName",
            &config.host_name,
        ]);
    let output = output_with_timeout(&mut command, Duration::from_secs(60))
        .map_err(|_| "bounded Nix validation failed; no files changed")?;
    // Nix source excerpts can contain private operator data; never print stderr.
    if !output.status.success() {
        return Err("Nix rejected the definition or installed inputs are unavailable. Check version, options, image, ownership and capabilities; no files changed.".into());
    }
    let plan: Value =
        serde_json::from_slice(&output.stdout).map_err(|_| "invalid Nix validation response")?;
    if plan["schemaVersion"] != 1
        || !plan["valid"].is_boolean()
        || !plan["instances"].is_array()
        || !plan["errors"].is_array()
    {
        return Err("invalid Nix validation response".into());
    }
    Ok(plan)
}

fn publish(path: &Path, bytes: &[u8], expected: Option<&[u8]>) -> Result<(), String> {
    let staged = path.with_extension("toml.pending");
    let mode = fs::metadata(path)
        .map(|m| m.permissions().mode() & 0o777)
        .unwrap_or(0o600);
    let mut file = OpenOptions::new().create_new(true).write(true).mode(mode).open(&staged)
        .map_err(|_| "cannot stage import; a pending file already exists or configuration is not writable")?;
    let result = (|| {
        file.write_all(bytes)
            .and_then(|()| file.sync_all())
            .map_err(|_| "cannot persist staged import")?;
        if read_optional(path)?.as_deref() != expected {
            return Err(
                "agents.toml changed during validation; retry after reviewing the conflict".into(),
            );
        }
        refuse_legacy(
            path.parent()
                .ok_or("inventory requires a parent directory")?,
        )?;
        fs::rename(&staged, path).map_err(|_| "cannot atomically publish agent additions")?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(staged);
    }
    result
}

pub(super) fn run(
    config: &Config,
    agents: &[Agent],
    args: &[String],
    mode: OutputMode,
) -> Result<u8, String> {
    let [action, filename] = args else {
        return Err("usage: agent validate|plan|import <file>; use agent template to create a stopped definition".into());
    };
    if !["validate", "plan", "import"].contains(&action.as_str()) {
        return Err("unknown agent command; use template, validate, plan or import".into());
    }
    let bytes =
        read_optional(Path::new(filename))?.ok_or("onboarding input file does not exist")?;
    let incoming = parse(&bytes)?;
    additions(&incoming)?;
    refuse_legacy(&config.flake_dir)?;
    let destination = config.flake_dir.join("agents.toml");
    let lock_path = config.flake_dir.join(".agents.toml.lock");
    // Imports from this CLI cooperate on one lock. Manual changes are checked
    // again before rename; an administrator can still edit after that check.
    let _lock = if action == "import" {
        if fs::symlink_metadata(&lock_path).is_ok_and(|m| !m.is_file()) {
            return Err("onboarding lock must be a regular file".into());
        }
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .mode(0o600)
            .open(&lock_path)
            .map_err(|_| "cannot open onboarding lock; configuration must be writable")?;
        file.try_lock()
            .map_err(|_| "another agent import is running; retry when it finishes")?;
        Some(file)
    } else {
        None
    };
    let existing = read_optional(&destination)?;
    check_conflicts(existing.as_deref(), &incoming, agents)?;
    let validation_bytes =
        serde_json::to_vec(&incoming).map_err(|_| "cannot encode internal validation input")?;
    let mut plan = validate(config, &validation_bytes)?;
    let valid = plan["valid"] == true;
    let imported = action == "import" && valid;
    if imported {
        let merged_bytes = serialize_import(existing.as_deref(), &incoming)?;
        if merged_bytes.len() as u64 > MAX_INPUT {
            return Err("combined agents.toml exceeds 1 MiB; no files changed".into());
        }
        publish(&destination, &merged_bytes, existing.as_deref())?;
    }
    plan["imported"] = json!(imported);
    plan["activation"] = json!("explicit-host-update-required");
    plan["host"] = json!(mode.host(&config.host_name));
    if mode.hide {
        for (index, instance) in plan["instances"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .enumerate()
        {
            instance["name"] = json!(mode.agent(index, ""));
            for key in ["container", "unit", "stateDir", "workspace", "image"] {
                instance[key] = json!("redacted");
            }
        }
        if !valid {
            plan["errors"] = json!(["validation failed; details redacted"]);
        }
    }
    if mode.json {
        println!("{plan}");
    } else {
        println!(
            "{}; {}. Host update: tentaflake rebuild",
            if valid {
                "Valid stopped additions"
            } else {
                "Validation failed"
            },
            if imported {
                "imported; running host unchanged"
            } else {
                "no configuration changed"
            }
        );
        for instance in plan["instances"].as_array().unwrap() {
            println!(
                "  {}: {} → {} (vendor acceptance: {})",
                instance["adapter"].as_str().unwrap_or("unknown"),
                instance["container"].as_str().unwrap_or("unknown"),
                instance["workspace"].as_str().unwrap_or("unknown"),
                instance["vendorAcceptance"].as_str().unwrap_or("unknown")
            );
        }
        for error in plan["errors"].as_array().unwrap() {
            println!("  {}", error.as_str().unwrap_or("validation error"));
        }
    }
    Ok(if valid { 0 } else { 1 })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn additions_preserve_metadata_and_detect_cross_adapter_collisions() {
        let existing = b"schemaVersion = 1\n_securityNote = 'keep operator note'\n[[agents]]\nadapter = 'hermes'\nname = 'original'\nautoStart = false\n";
        let incoming = json!({"schemaVersion":1,"agents":[{"adapter":"zeroclaw","name":"new","autoStart":false}]});
        check_conflicts(Some(existing), &incoming, &[]).unwrap();
        let result = parse(&serialize_import(Some(existing), &incoming).unwrap()).unwrap();
        assert_eq!(result["_securityNote"], "keep operator note");
        assert_eq!(result["agents"][1], incoming["agents"][0]);
        let collision = json!({"schemaVersion":1,"agents":[{"adapter":"zeroclaw","name":"original","autoStart":false}]});
        assert!(
            check_conflicts(Some(existing), &collision, &[])
                .unwrap_err()
                .contains("conflicts")
        );
        let duplicate = json!({"schemaVersion":1,"agents":[
            {"adapter":"hermes","name":"duplicate","autoStart":false},
            {"adapter":"zeroclaw","name":"duplicate","autoStart":false}
        ]});
        assert!(check_conflicts(None, &duplicate, &[]).is_err());
    }

    #[test]
    fn imports_refuse_legacy_and_unknown_root_fields() {
        let incoming =
            json!({"schemaVersion":1,"agents":[{"adapter":"pi","name":"new","autoStart":false}]});
        for invalid in [
            json!({"hermes":[]}),
            json!({"schemaVersion":1,"agents":[],"zeroclaw":[]}),
            json!({"schemaVersion":1,"agents":[],"unknown":true}),
            json!({"agents":[]}),
        ] {
            let bytes = toml_edit::ser::to_document(&invalid)
                .unwrap()
                .to_string()
                .into_bytes();
            assert!(check_conflicts(Some(&bytes), &incoming, &[]).is_err());
            assert!(check_conflicts(None, &invalid, &[]).is_err());
        }
    }

    #[test]
    fn imports_refuse_unsupported_existing_revisions_and_preserve_bytes_on_stale_write() {
        let incoming = json!({"schemaVersion":1,"agents":[{"adapter":"hermes","name":"new","autoStart":false}]});
        assert!(
            check_conflicts(Some(b"schemaVersion = 2\nagents = []\n"), &incoming, &[]).is_err()
        );
        let work = PrivateInput::new(b"operator revision").unwrap();
        let path = work.path();
        assert!(publish(&path, b"replacement", Some(b"stale revision")).is_err());
        assert_eq!(fs::read(&path).unwrap(), b"operator revision");
        assert!(!path.with_extension("toml.pending").exists());
        assert_eq!(
            fs::metadata(path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }

    #[test]
    fn toml_roundtrip_preserves_nested_values_and_existing_comments() {
        let incoming = json!({
            "schemaVersion": 1,
            "agents": [{
                "adapter": "generic",
                "name": "new",
                "definition": {
                    "schemaVersion": 1,
                    "image": "example.invalid/image@sha256:abc",
                    "command": ["/bin/sh", "-c", "printf 'hello\\n'"],
                    "lifecycle": "stopped",
                    "settings": {
                        "quoted.key": "quotes: \"value\" and \\ paths",
                        "enabled": true,
                        "count": 42,
                        "ratio": 1.25,
                        "nested": [{"name": "one", "values": [1, 2]}]
                    }
                }
            }]
        });
        let template = toml_edit::ser::to_document(&incoming).unwrap().to_string();
        assert_eq!(parse(template.as_bytes()).unwrap(), incoming);
        for existing in [
            "# operator note\nschemaVersion = 1\nagents = [] # keep empty-array comment\n",
            "# operator note\nschemaVersion = 1\nagents = [{ name = 'old', adapter = 'pi', autoStart = false }] # keep inline comment\n",
            "# operator note\nschemaVersion = 1\n[[agents]] # keep table comment\nname = 'old'\nadapter = 'pi'\nautoStart = false\n[agents.settings]\ntext = 'literal setting' # keep setting comment\n",
        ] {
            let rendered = serialize_import(Some(existing.as_bytes()), &incoming).unwrap();
            let text = std::str::from_utf8(&rendered).unwrap();
            assert!(text.contains("# operator note"));
            for comment in existing
                .lines()
                .filter_map(|line| line.split_once("# keep"))
            {
                assert!(text.contains(&format!("# keep{}", comment.1)));
            }
            let mut expected = parse(existing.as_bytes()).unwrap();
            expected["agents"]
                .as_array_mut()
                .unwrap()
                .push(incoming["agents"][0].clone());
            assert_eq!(parse(&rendered).unwrap(), expected);
        }
    }

    #[test]
    fn toml_rejects_json_dates_nonfinite_numbers_and_redacts_parse_errors() {
        for bytes in [
            br#"{"schemaVersion":1,"agents":[]}"#.as_slice(),
            b"schemaVersion = 1\nagents = []\nsecret = 1979-05-27T07:32:00Z\n",
            b"schemaVersion = 1\nagents = [{ name = 'new', settings = { value = nan } }]\n",
            b"schemaVersion = 1\nagents = [{ name = 'new', settings = { value = inf } }]\n",
            b"schemaVersion = 1\nagents = [{ name = 'new', settings = { value = 07:32:00 } }]\n",
        ] {
            assert!(parse(bytes).is_err());
        }
        let error = parse(b"secret = 'DO-NOT-PRINT\n").unwrap_err();
        assert!(!error.contains("DO-NOT-PRINT"));
    }

    #[test]
    fn legacy_inventory_blocks_publication_without_changing_inventory() {
        let work = PrivateInput::new(b"private input").unwrap();
        let path = work.0.join("agents.toml");
        let original = b"schemaVersion = 1\nagents = []\n";
        fs::write(&path, original).unwrap();
        let legacy = work.0.join("agents.json");
        fs::write(&legacy, b"{}").unwrap();
        assert!(refuse_legacy(&work.0).unwrap_err().contains("migrate"));
        assert!(publish(&path, b"replacement", Some(original)).is_err());
        assert_eq!(fs::read(&path).unwrap(), original);
        assert!(!path.with_extension("toml.pending").exists());
        fs::remove_file(&legacy).unwrap();
        std::os::unix::fs::symlink(work.0.join("missing"), &legacy).unwrap();
        assert!(refuse_legacy(&work.0).is_err());
    }
}
