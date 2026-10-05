//! Add stopped declarative workloads; activation remains an explicit host update.
use super::{Agent, Config, OutputMode, RUNTIME_CATALOG, output_with_timeout};
use serde_json::{Value, json};
use std::fs::{self, DirBuilder, File, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

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
        serde_json::to_string_pretty(&json!({"schemaVersion": 1, "agents": [entry]})).unwrap()
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

fn parse(bytes: &[u8]) -> Result<Value, String> {
    serde_json::from_slice(bytes).map_err(|e| {
        format!(
            "invalid onboarding JSON at line {}, column {}",
            e.line(),
            e.column()
        )
    })
}

fn additions(value: &Value) -> Result<&Vec<Value>, String> {
    let object = value
        .as_object()
        .ok_or("onboarding input must be an object")?;
    if object.len() != 2 || value["schemaVersion"] != 1 || !object.contains_key("agents") {
        return Err("onboarding requires schemaVersion 1 and an agents array only".into());
    }
    let entries = value["agents"]
        .as_array()
        .ok_or("agents must be an array")?;
    if entries.is_empty() || entries.len() > MAX_AGENTS {
        return Err("onboarding requires between 1 and 128 agent additions".into());
    }
    Ok(entries)
}

// Conflict projection only: the shared Nix parser validates workload semantics.
fn identities(value: &Value) -> Result<Vec<String>, String> {
    let mut result = Vec::new();
    for (field, preset) in [
        ("agents", None),
        ("hermes", Some("hermes")),
        ("zeroclaw", Some("zeroclaw")),
    ] {
        if let Some(entries) = value.get(field) {
            for entry in entries
                .as_array()
                .ok_or("existing agent arrays are malformed")?
            {
                let adapter = preset
                    .or_else(|| entry["adapter"].as_str())
                    .ok_or("missing adapter identity")?;
                let name = entry["name"].as_str().ok_or("missing agent identity")?;
                result.push(format!("{adapter}-{name}"));
            }
        }
    }
    Ok(result)
}

fn merge(existing: Option<&[u8]>, incoming: &Value, agents: &[Agent]) -> Result<Value, String> {
    let mut combined = match existing {
        Some(bytes) => parse(bytes)?,
        None => json!({"schemaVersion": 1, "agents": []}),
    };
    let object = combined
        .as_object()
        .ok_or("existing agents.json must be an object")?;
    if object.keys().any(|key| {
        ![
            "schemaVersion",
            "agents",
            "hermes",
            "zeroclaw",
            "_securityNote",
        ]
        .contains(&key.as_str())
    }) || object
        .get("schemaVersion")
        .is_some_and(|version| version != &json!(1))
    {
        return Err("existing agents.json has unknown fields or an unsupported version".into());
    }
    let mut known = identities(&combined)?;
    known.extend(agents.iter().map(|agent| agent.container.clone()));
    for identity in identities(incoming)? {
        if known.contains(&identity) {
            return Err("agent identity conflicts with existing configuration or active inventory; no files changed".into());
        }
        known.push(identity);
    }
    combined["schemaVersion"] = json!(1);
    if combined.get("agents").is_none() {
        combined["agents"] = json!([]);
    }
    combined["agents"]
        .as_array_mut()
        .ok_or("existing agents must be an array")?
        .extend(additions(incoming)?.iter().cloned());
    Ok(combined)
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
    let staged = path.with_extension("json.pending");
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
                "agents.json changed during validation; retry after reviewing the conflict".into(),
            );
        }
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
    let destination = config.flake_dir.join("agents.json");
    let lock_path = config.flake_dir.join(".agents.json.lock");
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
    let combined = merge(existing.as_deref(), &incoming, agents)?;
    let mut plan = validate(config, &bytes)?;
    let valid = plan["valid"] == true;
    let imported = action == "import" && valid;
    if imported {
        let merged_bytes = serde_json::to_vec_pretty(&combined).map_err(|e| e.to_string())?;
        if merged_bytes.len() as u64 > MAX_INPUT {
            return Err("combined agents.json exceeds 1 MiB; no files changed".into());
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
    fn additions_preserve_legacy_configuration_and_detect_cross_schema_collisions() {
        let existing = br#"{"hermes":[{"name":"original","provider":"fixture","model":"fixture","envFile":null}],"_securityNote":"keep operator note"}"#;
        let incoming = json!({"schemaVersion":1,"agents":[{"adapter":"zeroclaw","name":"new","autoStart":false}]});
        let result = merge(Some(existing), &incoming, &[]).unwrap();
        let previous = parse(existing).unwrap();
        assert_eq!(result["hermes"], previous["hermes"]);
        assert_eq!(result["_securityNote"], previous["_securityNote"]);
        assert_eq!(result["agents"], incoming["agents"]);
        let collision = json!({"schemaVersion":1,"agents":[{"adapter":"hermes","name":"original","autoStart":false}]});
        assert!(
            merge(Some(existing), &collision, &[])
                .unwrap_err()
                .contains("conflicts")
        );
        let duplicate =
            json!({"schemaVersion":1,"agents":[incoming["agents"][0], incoming["agents"][0]]});
        assert!(merge(Some(existing), &duplicate, &[]).is_err());
    }

    #[test]
    fn imports_refuse_unsupported_existing_revisions_and_preserve_bytes_on_stale_write() {
        let incoming = json!({"schemaVersion":1,"agents":[{"adapter":"hermes","name":"new","autoStart":false}]});
        assert!(merge(Some(br#"{"schemaVersion":2,"agents":[]}"#), &incoming, &[]).is_err());
        let work = PrivateInput::new(b"operator revision").unwrap();
        let path = work.path();
        assert!(publish(&path, b"replacement", Some(b"stale revision")).is_err());
        assert_eq!(fs::read(&path).unwrap(), b"operator revision");
        assert!(!path.with_extension("json.pending").exists());
        assert_eq!(
            fs::metadata(path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
}
