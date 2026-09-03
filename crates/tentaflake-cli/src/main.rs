use std::collections::BTreeMap;
use std::env;
use std::ffi::OsString;
use std::fs;
use std::io::{self, IsTerminal, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Output};
use std::time::{Duration, Instant, SystemTime};

const DEFAULT_CONFIG: &str = "/etc/tentaflake/cli.conf";
const DEFAULT_AGENTS: &str = "/etc/tentaflake/agents.tsv";
const MAX_CONTROLLER_MEMORY_BYTES: u64 = 1024 * 1024 * 1024 * 1024;
const MAX_CONTROLLER_NANO_CPUS: u64 = 1024 * 1_000_000_000;
const LIVE_INSPECTION_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_PARALLEL_BROKER_HEALTH_PROBES: usize = 16;

#[derive(Clone, Debug, Eq, PartialEq)]
struct Config {
    backend: String,
    flake_dir: PathBuf,
    host_name: String,
    agents_file: PathBuf,
    security_profile: String,
    security_file: PathBuf,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct Agent {
    runtime: String,
    name: String,
    container: String,
    unit: String,
    state_dir: PathBuf,
}

#[derive(Clone, Copy, Debug, Default)]
struct OutputMode {
    hide: bool,
    json: bool,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct DeclaredMount {
    source: String,
    destination: String,
    writable: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct DeclaredLiveResources {
    memory_bytes: u64,
    memory_swap_bytes: u64,
    nano_cpus: u64,
    pids_limit: u64,
    nofile: u64,
    nproc: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct SecurityAgent {
    name: String,
    profile: String,
    network_isolated: bool,
    ports_private: bool,
    unprivileged: bool,
    non_root: bool,
    capabilities_empty: bool,
    no_new_privileges: bool,
    read_only_root: bool,
    mounts_safe: bool,
    runsc: bool,
    resources_limited: bool,
    image_pinned: bool,
    env_files_absent: bool,
    disposable_worker: bool,
    workspace_quota: bool,
    seccomp_confined: bool,
    apparmor_confined: bool,
    provenance_gate_configured: bool,
    brokered_egress: bool,
    llm_broker_enabled: bool,
    llm_broker_endpoint: Option<SocketAddr>,
    fetch_broker_enabled: bool,
    fetch_broker_endpoint: Option<SocketAddr>,
    broker_network: Option<String>,
    declared_mounts: Option<Vec<DeclaredMount>>,
    declared_tmpfs: Option<BTreeMap<String, u64>>,
    declared_ports_absent: Option<bool>,
    declared_live_resources: Option<DeclaredLiveResources>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct SecurityState {
    profile: String,
    openssh_enabled: bool,
    broker_configured: bool,
    tailscale_enabled: bool,
    apparmor_enabled: bool,
    docker_group_absent: bool,
    backup_enabled: bool,
    backup_max_age_hours: u64,
    agents: Vec<SecurityAgent>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct Finding {
    id: &'static str,
    severity: &'static str,
    agent: Option<String>,
    explanation: &'static str,
    remediation: &'static str,
}

fn main() -> ExitCode {
    let argv: Vec<OsString> = env::args_os().collect();
    let invoked_as = argv
        .first()
        .and_then(|value| Path::new(value).file_name())
        .and_then(|value| value.to_str())
        .unwrap_or("tentaflake")
        .to_owned();

    let mut args: Vec<String> = argv
        .into_iter()
        .skip(1)
        .map(|value| value.to_string_lossy().into_owned())
        .collect();

    if invoked_as == "tentaflake-status" {
        args.insert(0, "status".into());
    } else if invoked_as == "hermes" {
        eprintln!("warning: `hermes` is deprecated; use `tentaflake`");
    }

    match run(args) {
        Ok(code) => ExitCode::from(code),
        Err(message) => {
            eprintln!("tentaflake: {message}");
            ExitCode::from(2)
        }
    }
}

fn run(mut args: Vec<String>) -> Result<u8, String> {
    let mode = take_output_flags(&mut args);
    if args.first().map(String::as_str) == Some("remote-check") {
        return remote_check(&args[1..]);
    }
    let config = load_config()?;
    let agents = load_agents(&config.agents_file)?;
    let command = args.first().map(String::as_str).unwrap_or("status");

    match command {
        "status" => status(&config, &agents, mode),
        "health" => health(&config, &agents, mode),
        "doctor" if args[1..].iter().any(|arg| arg == "--security") => {
            security_doctor(&config, mode)
        }
        "doctor" => doctor(&config, &agents, mode),
        "stats" => stats(&config, &agents),
        "logs" => logs(&agents, &args[1..]),
        "restart" | "start" | "stop" => lifecycle(command, &agents, &args[1..]),
        "shell" => container_shell(&config, &agents, &args[1..]),
        "exec" => container_exec(&config, &agents, &args[1..]),
        "ps" => backend_passthrough(&config, &["ps"]),
        "backup" => backup(&agents, &args[1..]),
        "rebuild" => rebuild(&config),
        "update" => update(&config),
        "agent" => Err(
            "the interactive agent wizard was removed; edit agents.json or my-agents.nix, then rebuild"
                .into(),
        ),
        "help" | "--help" | "-h" => {
            print_help(&config.backend);
            Ok(0)
        }
        "top" | "console" => Err(format!(
            "`{command}` was removed with the auditd/SQLite web-console stack; use the observability profile"
        )),
        other => Err(format!("unknown command `{other}`; run `tentaflake help`")),
    }
}

fn remote_check(args: &[String]) -> Result<u8, String> {
    let remote = args
        .first()
        .ok_or("remote-check requires an actual URL and at least one allowed URL")?;
    if args.len() < 2 {
        return Err("remote-check requires at least one allowed URL".into());
    }
    let actual = canonical_github_remote(remote)?;
    for allowed in &args[1..] {
        if canonical_github_remote(allowed)? == actual {
            return Ok(0);
        }
    }
    eprintln!("remote denied: {remote}");
    Ok(1)
}

fn canonical_github_remote(value: &str) -> Result<String, String> {
    let rest = value
        .strip_prefix("https://")
        .ok_or_else(|| format!("remote must use canonical HTTPS: {value}"))?;
    if rest.contains(['@', '?', '#', '%', '\\']) || rest.ends_with('/') {
        return Err(format!("remote contains forbidden URL syntax: {value}"));
    }
    let mut fields = rest.split('/');
    let host = fields.next().unwrap_or_default();
    let owner = fields.next().unwrap_or_default();
    let repository = fields.next().unwrap_or_default();
    if fields.next().is_some() || !host.eq_ignore_ascii_case("github.com") {
        return Err(format!("remote must target github.com/OWNER/REPO: {value}"));
    }
    let repository = repository.strip_suffix(".git").unwrap_or(repository);
    if owner.is_empty()
        || repository.is_empty()
        || repository.ends_with(".git")
        || !remote_component_is_safe(owner)
        || !remote_component_is_safe(repository)
    {
        return Err(format!("remote owner or repository is invalid: {value}"));
    }
    Ok(format!(
        "https://github.com/{}/{}",
        owner.to_ascii_lowercase(),
        repository.to_ascii_lowercase()
    ))
}

fn remote_component_is_safe(value: &str) -> bool {
    value != "."
        && value != ".."
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

fn take_output_flags(args: &mut Vec<String>) -> OutputMode {
    let mut mode = OutputMode::default();
    args.retain(|arg| match arg.as_str() {
        "--hide" | "-H" => {
            mode.hide = true;
            false
        }
        "--json" => {
            mode.json = true;
            false
        }
        _ => true,
    });
    mode
}

fn load_config() -> Result<Config, String> {
    let path = env::var_os("TENTAFLAKE_CONFIG")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(DEFAULT_CONFIG));
    let text = fs::read_to_string(&path)
        .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
    parse_config(&text)
}

fn parse_config(text: &str) -> Result<Config, String> {
    let mut backend = None;
    let mut flake_dir = None;
    let mut host_name = None;
    let mut agents_file = None;
    let mut security_profile = None;
    let mut security_file = None;

    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (key, value) = line
            .split_once('=')
            .ok_or_else(|| format!("invalid config line: {line}"))?;
        match key.trim() {
            "backend" => backend = Some(value.trim().to_owned()),
            "flake_dir" => flake_dir = Some(PathBuf::from(value.trim())),
            "host_name" => host_name = Some(value.trim().to_owned()),
            "agents_file" => agents_file = Some(PathBuf::from(value.trim())),
            "security_profile" => security_profile = Some(value.trim().to_owned()),
            "security_file" => security_file = Some(PathBuf::from(value.trim())),
            unknown => return Err(format!("unknown config key `{unknown}`")),
        }
    }

    let backend = backend.ok_or("missing config key `backend`")?;
    if backend != "docker" && backend != "podman" {
        return Err(format!("unsupported container backend `{backend}`"));
    }

    let security_profile = security_profile.unwrap_or_else(|| "dev".into());
    if !matches!(security_profile.as_str(), "dev" | "balanced" | "strict") {
        return Err(format!("unsupported security profile `{security_profile}`"));
    }

    Ok(Config {
        backend,
        flake_dir: flake_dir.ok_or("missing config key `flake_dir`")?,
        host_name: host_name.ok_or("missing config key `host_name`")?,
        agents_file: agents_file.unwrap_or_else(|| PathBuf::from(DEFAULT_AGENTS)),
        security_profile,
        security_file: security_file
            .unwrap_or_else(|| PathBuf::from("/etc/tentaflake/security.tsv")),
    })
}

fn security_doctor(config: &Config, mode: OutputMode) -> Result<u8, String> {
    let text = fs::read_to_string(&config.security_file)
        .map_err(|error| format!("cannot read {}: {error}", config.security_file.display()))?;
    let state = parse_security_state(&text)?;
    if state.profile != config.security_profile {
        return Err(format!(
            "security profile mismatch: cli.conf={}, manifest={}",
            config.security_profile, state.profile
        ));
    }
    let mut findings = security_findings(&state);
    findings.extend(security_live_findings(config, &state));
    if mode.json {
        print!(
            "{{\"profile\":\"{}\",\"findings\":[",
            json_escape(&state.profile)
        );
        for (index, finding) in findings.iter().enumerate() {
            if index != 0 {
                print!(",");
            }
            let agent = finding.agent.as_ref().map(|name| {
                if mode.hide {
                    "redacted".to_owned()
                } else {
                    name.clone()
                }
            });
            print!(
                "{{\"id\":\"{}\",\"severity\":\"{}\",\"agent\":{},\"explanation\":\"{}\",\"remediation\":\"{}\"}}",
                finding.id,
                finding.severity,
                agent.map_or("null".into(), |name| format!("\"{}\"", json_escape(&name))),
                json_escape(finding.explanation),
                json_escape(finding.remediation)
            );
        }
        println!("]}}");
    } else {
        println!("Tentaflake security doctor — {}", state.profile);
        if findings.is_empty() {
            println!("  ✓ no security findings");
        }
        for finding in &findings {
            let agent = finding.agent.as_deref().map_or(String::new(), |name| {
                format!(" [{}]", if mode.hide { "redacted" } else { name })
            });
            println!(
                "  {} {}{}: {}",
                finding.severity, finding.id, agent, finding.explanation
            );
            println!("    remediation: {}", finding.remediation);
        }
    }
    Ok(
        if findings
            .iter()
            .any(|finding| matches!(finding.severity, "critical" | "high"))
        {
            1
        } else {
            0
        },
    )
}

fn parse_security_state(text: &str) -> Result<SecurityState, String> {
    let mut state = None;
    let mut agents = Vec::new();
    for (index, line) in text.lines().enumerate() {
        if line.trim().is_empty() || line.starts_with('#') {
            continue;
        }
        let fields: Vec<&str> = line.split('\t').collect();
        match fields.first().copied() {
            Some("host") if fields.len() == 9 => {
                if state.is_some() {
                    return Err("security manifest contains multiple host records".into());
                }
                state = Some(SecurityState {
                    profile: fields[1].into(),
                    openssh_enabled: parse_bool(fields[2], index + 1)?,
                    broker_configured: parse_bool(fields[3], index + 1)?,
                    tailscale_enabled: parse_bool(fields[4], index + 1)?,
                    apparmor_enabled: parse_bool(fields[5], index + 1)?,
                    docker_group_absent: parse_bool(fields[6], index + 1)?,
                    backup_enabled: parse_bool(fields[7], index + 1)?,
                    backup_max_age_hours: fields[8].parse().map_err(|_| {
                        format!(
                            "invalid backup age `{}` on security manifest line {}",
                            fields[8],
                            index + 1
                        )
                    })?,
                    agents: Vec::new(),
                });
            }
            Some("agent") if (25..=30).contains(&fields.len()) => agents.push(SecurityAgent {
                name: fields[1].into(),
                profile: fields[2].into(),
                network_isolated: parse_bool(fields[3], index + 1)?,
                ports_private: parse_bool(fields[4], index + 1)?,
                unprivileged: parse_bool(fields[5], index + 1)?,
                non_root: parse_bool(fields[6], index + 1)?,
                capabilities_empty: parse_bool(fields[7], index + 1)?,
                no_new_privileges: parse_bool(fields[8], index + 1)?,
                read_only_root: parse_bool(fields[9], index + 1)?,
                mounts_safe: parse_bool(fields[10], index + 1)?,
                runsc: parse_bool(fields[11], index + 1)?,
                resources_limited: parse_bool(fields[12], index + 1)?,
                image_pinned: parse_bool(fields[13], index + 1)?,
                env_files_absent: parse_bool(fields[14], index + 1)?,
                disposable_worker: parse_bool(fields[15], index + 1)?,
                workspace_quota: parse_bool(fields[16], index + 1)?,
                seccomp_confined: parse_bool(fields[17], index + 1)?,
                apparmor_confined: parse_bool(fields[18], index + 1)?,
                provenance_gate_configured: parse_bool(fields[19], index + 1)?,
                brokered_egress: parse_bool(fields[20], index + 1)?,
                llm_broker_enabled: parse_bool(fields[21], index + 1)?,
                llm_broker_endpoint: parse_optional_socket(fields[22], index + 1)?,
                fetch_broker_enabled: parse_bool(fields[23], index + 1)?,
                fetch_broker_endpoint: parse_optional_socket(fields[24], index + 1)?,
                broker_network: fields
                    .get(25)
                    .map_or(Ok(None), |value| parse_optional_network(value, index + 1))?,
                declared_mounts: fields
                    .get(26)
                    .map(|value| parse_declared_mounts(value, index + 1))
                    .transpose()?
                    .flatten(),
                declared_tmpfs: fields
                    .get(27)
                    .map(|value| parse_declared_tmpfs(value, index + 1))
                    .transpose()?
                    .flatten(),
                declared_ports_absent: fields
                    .get(28)
                    .map(|value| parse_optional_bool(value, index + 1))
                    .transpose()?
                    .flatten(),
                declared_live_resources: fields
                    .get(29)
                    .map(|value| parse_declared_live_resources(value, index + 1))
                    .transpose()?
                    .flatten(),
            }),
            _ => {
                return Err(format!(
                    "invalid security manifest record on line {}",
                    index + 1
                ));
            }
        }
    }
    let mut state = state.ok_or("security manifest has no host record")?;
    state.agents = agents;
    Ok(state)
}

fn parse_bool(value: &str, line: usize) -> Result<bool, String> {
    match value {
        "true" => Ok(true),
        "false" => Ok(false),
        _ => Err(format!(
            "invalid boolean `{value}` on security manifest line {line}"
        )),
    }
}

fn parse_optional_bool(value: &str, line: usize) -> Result<Option<bool>, String> {
    if value == "-" {
        Ok(None)
    } else {
        parse_bool(value, line).map(Some)
    }
}

fn parse_optional_socket(value: &str, line: usize) -> Result<Option<SocketAddr>, String> {
    if value == "-" {
        Ok(None)
    } else {
        value
            .parse()
            .map(Some)
            .map_err(|_| format!("invalid broker endpoint `{value}` on manifest line {line}"))
    }
}

fn parse_optional_network(value: &str, line: usize) -> Result<Option<String>, String> {
    if value == "-" {
        return Ok(None);
    }
    let valid = !value.is_empty()
        && value.len() <= 63
        && value.bytes().enumerate().all(|(index, byte)| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || (index > 0 && byte == b'-')
        });
    if valid {
        Ok(Some(value.to_owned()))
    } else {
        Err(format!(
            "invalid broker network `{value}` on security manifest line {line}"
        ))
    }
}

fn parse_declared_mounts(value: &str, line: usize) -> Result<Option<Vec<DeclaredMount>>, String> {
    if value == "-" {
        return Ok(None);
    }
    let volumes: Vec<String> = serde_json::from_str(value)
        .map_err(|_| format!("invalid declared-mount JSON on security manifest line {line}"))?;
    let mut mounts = Vec::with_capacity(volumes.len());
    for volume in volumes {
        let fields = volume.split(':').collect::<Vec<_>>();
        if !(2..=3).contains(&fields.len())
            || !fields[0].starts_with('/')
            || !fields[1].starts_with('/')
        {
            return Err(format!(
                "invalid declared mount `{volume}` on security manifest line {line}"
            ));
        }
        let writable = match fields.get(2).copied() {
            None | Some("rw") => true,
            Some("ro") => false,
            Some(_) => {
                return Err(format!(
                    "unsupported declared mount mode in `{volume}` on security manifest line {line}"
                ));
            }
        };
        mounts.push(DeclaredMount {
            source: fields[0].to_owned(),
            destination: fields[1].to_owned(),
            writable,
        });
    }
    mounts.sort();
    if mounts.windows(2).any(|pair| pair[0] == pair[1]) {
        return Err(format!(
            "duplicate declared mount on security manifest line {line}"
        ));
    }
    Ok(Some(mounts))
}

fn parse_declared_tmpfs(value: &str, line: usize) -> Result<Option<BTreeMap<String, u64>>, String> {
    if value == "-" {
        return Ok(None);
    }
    let tmpfs: BTreeMap<String, u64> = serde_json::from_str(value)
        .map_err(|_| format!("invalid declared-tmpfs JSON on security manifest line {line}"))?;
    let expected_destinations = ["/run", "/tmp", "/var/tmp"];
    if tmpfs.len() != expected_destinations.len()
        || expected_destinations
            .iter()
            .any(|destination| !tmpfs.contains_key(*destination))
        || tmpfs.values().any(|size| *size == 0)
    {
        return Err(format!(
            "declared tmpfs set must contain positive sizes for /run, /tmp, and /var/tmp on security manifest line {line}"
        ));
    }
    Ok(Some(tmpfs))
}

fn parse_declared_live_resources(
    value: &str,
    line: usize,
) -> Result<Option<DeclaredLiveResources>, String> {
    if value == "-" {
        return Ok(None);
    }
    let parsed: serde_json::Value = serde_json::from_str(value).map_err(|_| {
        format!("invalid declared-live-resource JSON on security manifest line {line}")
    })?;
    let Some(fields) = parsed.as_object() else {
        return Err(format!(
            "declared live resources must be a JSON object on security manifest line {line}"
        ));
    };
    let expected_fields = [
        "memoryBytes",
        "memorySwapBytes",
        "nanoCpus",
        "pidsLimit",
        "nofile",
        "nproc",
    ];
    if fields.len() != expected_fields.len()
        || expected_fields
            .iter()
            .any(|field| !fields.contains_key(*field))
    {
        return Err(format!(
            "declared live resources have an invalid field set on security manifest line {line}"
        ));
    }
    let positive = |field: &str| {
        fields
            .get(field)
            .and_then(serde_json::Value::as_u64)
            .filter(|value| *value > 0)
            .ok_or_else(|| {
                format!(
                    "declared live resource `{field}` must be a positive integer on security manifest line {line}"
                )
            })
    };
    let resources = DeclaredLiveResources {
        memory_bytes: positive("memoryBytes")?,
        memory_swap_bytes: positive("memorySwapBytes")?,
        nano_cpus: positive("nanoCpus")?,
        pids_limit: positive("pidsLimit")?,
        nofile: positive("nofile")?,
        nproc: positive("nproc")?,
    };
    if resources.memory_bytes > MAX_CONTROLLER_MEMORY_BYTES
        || resources.memory_swap_bytes > MAX_CONTROLLER_MEMORY_BYTES
        || resources.memory_swap_bytes < resources.memory_bytes
        || resources.nano_cpus > MAX_CONTROLLER_NANO_CPUS
        || !resources.nano_cpus.is_multiple_of(10_000)
        || resources.nproc != resources.pids_limit
        || resources.pids_limit > i64::MAX as u64
        || resources.nofile > i64::MAX as u64
    {
        return Err(format!(
            "declared live resources violate controller bounds on security manifest line {line}"
        ));
    }
    Ok(Some(resources))
}

fn security_findings(state: &SecurityState) -> Vec<Finding> {
    let mut findings = Vec::new();
    if state.profile == "dev" {
        findings.push(host_finding(
            "TFSEC-001",
            "high",
            "the dev profile is not a security boundary for untrusted 24/7 agents",
            "migrate to balanced and satisfy its fail-closed assertions",
        ));
    }
    if state.openssh_enabled {
        findings.push(host_finding(
            "TFSEC-015",
            "high",
            "OpenSSH is enabled; the secure management baseline expects the private Tailscale path",
            "disable tentaflake.ssh or document and restrict the exceptional SSH path",
        ));
    }
    if !state.broker_configured && state.profile != "dev" {
        findings.push(host_finding(
            "TFSEC-013",
            "warning",
            "brokered egress is not configured; agents remain fail-closed with network=none",
            "configure the Phase B broker network before granting external connectivity",
        ));
    }
    if !state.tailscale_enabled && state.profile != "dev" {
        findings.push(host_finding(
            "TFSEC-019",
            "critical",
            "the private Tailscale management path is disabled",
            "enable Tailscale and install a restrictive grants plus SSH policy",
        ));
    }
    if !state.apparmor_enabled && state.profile != "dev" {
        findings.push(host_finding(
            "TFSEC-017",
            "critical",
            "AppArmor is disabled on a secure-profile host",
            "enable tentaflake hardening and AppArmor before running agents",
        ));
    }
    if !state.docker_group_absent && state.profile != "dev" {
        findings.push(host_finding(
            "TFSEC-018",
            "critical",
            "the administrative user belongs to the root-equivalent Docker group",
            "remove Docker-group membership and use the narrow sudo-backed CLI path",
        ));
    }
    if !state.backup_enabled && state.profile != "dev" {
        findings.push(host_finding(
            "TFSEC-026",
            "warning",
            "encrypted state/audit backups are not configured",
            "configure tentaflake.backup with explicit state and broker paths plus runtime-only Restic credentials",
        ));
    }
    for agent in &state.agents {
        if agent.brokered_egress != (agent.llm_broker_enabled || agent.fetch_broker_enabled)
            || agent.llm_broker_enabled != agent.llm_broker_endpoint.is_some()
            || agent.fetch_broker_enabled != agent.fetch_broker_endpoint.is_some()
        {
            findings.push(agent_finding(
                "TFSEC-037",
                "critical",
                agent,
                "broker network label, enabled modes, and declared health endpoints are inconsistent",
                "rebuild the agent and broker from one exact tentaflake.broker.agents policy",
            ));
        }
        if agent.profile != state.profile {
            findings.push(agent_finding(
                "TFSEC-016",
                "critical",
                agent,
                "agent and host security profiles differ",
                "rebuild the agent from the selected host security profile",
            ));
        }
        check_agent(
            &mut findings,
            agent.network_isolated,
            "TFSEC-002",
            "critical",
            agent,
            "agent has direct or unreviewed network access",
            "use network=none or exactly one isolated internal broker network",
        );
        check_agent(
            &mut findings,
            agent.ports_private,
            "TFSEC-003",
            "critical",
            agent,
            "agent publishes a non-loopback port",
            "remove the publication or bind an authenticated operator service to loopback",
        );
        check_agent(
            &mut findings,
            agent.unprivileged,
            "TFSEC-004",
            "critical",
            agent,
            "agent container is privileged",
            "remove privileged mode",
        );
        check_agent(
            &mut findings,
            agent.non_root,
            "TFSEC-005",
            "critical",
            agent,
            "agent container user is root or unspecified",
            "set an explicit non-root UID and GID",
        );
        check_agent(
            &mut findings,
            agent.capabilities_empty,
            "TFSEC-006",
            "critical",
            agent,
            "agent has added Linux capabilities",
            "drop all capabilities and justify any future exception separately",
        );
        check_agent(
            &mut findings,
            agent.no_new_privileges,
            "TFSEC-007",
            "high",
            agent,
            "no-new-privileges is missing",
            "enable the OCI no-new-privileges security option",
        );
        check_agent(
            &mut findings,
            agent.read_only_root,
            "TFSEC-008",
            "high",
            agent,
            "container root filesystem is writable",
            "enable read-only root and use bounded tmpfs/state mounts",
        );
        check_agent(
            &mut findings,
            agent.mounts_safe,
            "TFSEC-009",
            "critical",
            agent,
            "agent has a sensitive host or runtime-socket mount",
            "remove the mount and expose only explicit state/workspace paths",
        );
        check_agent(
            &mut findings,
            agent.resources_limited,
            "TFSEC-010",
            "high",
            agent,
            "one or more CPU, memory, swap, PID, or ulimit controls are missing",
            "restore the balanced resource controls",
        );
        check_agent(
            &mut findings,
            agent.runsc,
            "TFSEC-011",
            "critical",
            agent,
            "the gVisor runsc runtime is missing",
            "install the declarative gVisor runtime; never fall back silently to runc",
        );
        check_agent(
            &mut findings,
            agent.image_pinned,
            "TFSEC-012",
            "high",
            agent,
            "container image is not pinned to a SHA-256 digest",
            "pin the reviewed image digest",
        );
        check_agent(
            &mut findings,
            agent.env_files_absent,
            "TFSEC-014",
            "critical",
            agent,
            "agent receives an environment file that may contain real credentials",
            "move provider and infrastructure credentials to the broker",
        );
        if state.profile != "dev" {
            check_agent(
                &mut findings,
                agent.disposable_worker,
                "TFSEC-020",
                "high",
                agent,
                "agent has no disposable tool-worker boundary for untrusted code",
                "enable tentaflake.worker.agents.<container> and route shell, build, and foreign-code jobs through its inbox",
            );
            check_agent(
                &mut findings,
                agent.workspace_quota,
                "TFSEC-021",
                "high",
                agent,
                "agent persistent workspace has no managed hard size ceiling",
                "enable tentaflake.workspaceQuota.agents.<container> after explicitly migrating any existing workspace",
            );
            check_agent(
                &mut findings,
                agent.seccomp_confined,
                "TFSEC-022",
                "critical",
                agent,
                "container seccomp policy is unconfined or missing",
                "restore the runtime default seccomp profile and remove every unconfined override",
            );
            check_agent(
                &mut findings,
                agent.apparmor_confined,
                "TFSEC-023",
                "critical",
                agent,
                "container has no enforced AppArmor profile",
                "enable host AppArmor and the runtime default container profile",
            );
            check_agent(
                &mut findings,
                agent.provenance_gate_configured,
                "TFSEC-024",
                "warning",
                agent,
                "the pinned image has no configured publisher-signature verification gate",
                "record the upstream Cosign identity or public key and configure tentaflake.imageProvenance.agents.<container>",
            );
        }
    }
    findings
}

fn security_live_findings(config: &Config, state: &SecurityState) -> Vec<Finding> {
    if state.profile == "dev" {
        return Vec::new();
    }
    let mut findings = Vec::new();

    match output("df", &["-P", "/"]) {
        Ok(result) if result.status.success() => {
            match parse_disk_percent(&String::from_utf8_lossy(&result.stdout)) {
                Some(percent) if percent >= 90 => findings.push(host_finding(
                    "TFSEC-031",
                    "critical",
                    "root filesystem usage is at or above 90 percent",
                    "stop affected agents if needed, preserve evidence, and free or extend reviewed storage before resuming",
                )),
                Some(_) => {}
                None => findings.push(host_finding(
                    "TFSEC-032",
                    "warning",
                    "root filesystem pressure could not be parsed",
                    "run df -P / from the operator path and verify capacity monitoring",
                )),
            }
        }
        _ => findings.push(host_finding(
            "TFSEC-032",
            "warning",
            "root filesystem pressure could not be inspected",
            "run df -P / from the operator path and verify capacity monitoring",
        )),
    }

    if state.tailscale_enabled {
        match output("tailscale", &["serve", "status", "--json"]) {
            Ok(result) if result.status.success() => {
                match serde_json::from_slice::<serde_json::Value>(&result.stdout) {
                    Ok(value) => {
                        if json_has_truthy_key(&value, "funnel") {
                            findings.push(host_finding(
                                "TFSEC-029",
                                "critical",
                                "Tailscale Funnel is active on the secure management host",
                                "disable Funnel and keep management services private to a restrictive tailnet policy",
                            ));
                        } else if json_is_nonempty(&value) {
                            findings.push(host_finding(
                                "TFSEC-030",
                                "warning",
                                "Tailscale Serve has an active local configuration",
                                "verify every served handler is intentional, authenticated, and covered by restrictive grants",
                            ));
                        }
                    }
                    Err(_) => findings.push(host_finding(
                        "TFSEC-028",
                        "warning",
                        "active Tailscale Serve/Funnel state returned invalid JSON",
                        "run tailscale serve status --json from the operator path and inspect the exact state",
                    )),
                }
            }
            _ => findings.push(host_finding(
                "TFSEC-028",
                "warning",
                "active Tailscale Serve/Funnel state could not be inspected",
                "run tailscale serve status --json from the operator path; blocked or unknown evidence is not green",
            )),
        }
    }

    if state.backup_enabled {
        let stamp = Path::new("/var/lib/tentaflake-backup/last-success");
        let fresh = fs::metadata(stamp)
            .and_then(|metadata| metadata.modified())
            .ok()
            .and_then(|modified| SystemTime::now().duration_since(modified).ok())
            .is_some_and(|age| age.as_secs() <= state.backup_max_age_hours.saturating_mul(3600));
        if !fresh {
            findings.push(host_finding(
                "TFSEC-027",
                "warning",
                "the last successful Restic backup is missing or stale",
                "inspect restic-backups-tentaflake.service, run the approved backup/check flow, and perform a restore drill",
            ));
        }
    }

    let broker_endpoints = state
        .agents
        .iter()
        .flat_map(|agent| [agent.llm_broker_endpoint, agent.fetch_broker_endpoint])
        .flatten()
        .collect::<Vec<_>>();
    let mut broker_health = probe_broker_health_bounded(&broker_endpoints).into_iter();

    for agent in &state.agents {
        for _endpoint in [agent.llm_broker_endpoint, agent.fetch_broker_endpoint]
            .into_iter()
            .flatten()
        {
            match broker_health.next().unwrap_or(BrokerHealth::Unavailable) {
                BrokerHealth::Ready => {}
                BrokerHealth::Unavailable => findings.push(agent_finding(
                    "TFSEC-035",
                    "warning",
                    agent,
                    "a configured broker health endpoint could not be reached",
                    "inspect the exact broker unit and /healthz from the host; unavailable runtime evidence is not green",
                )),
                BrokerHealth::Unhealthy => findings.push(agent_finding(
                    "TFSEC-036",
                    "high",
                    agent,
                    "a configured broker answered but did not report credential, policy, and audit readiness",
                    "stop the affected controller and repair its broker credentials, policy, state, or audit path before restart",
                )),
            }
        }

        let mut command = Command::new("sudo");
        command.args(inspect_arguments_for_agent(&config.backend, agent));
        match output_with_timeout(&mut command, LIVE_INSPECTION_TIMEOUT) {
            Ok(result) if result.status.success() => {
                let expected_network = if agent.brokered_egress {
                    agent.broker_network.as_deref()
                } else {
                    Some("none")
                };
                let container_value =
                    serde_json::from_slice::<serde_json::Value>(&result.stdout).ok();
                let mut live_state = live_container_security_from_manifest(
                    agent,
                    container_value.as_ref(),
                    &config.backend,
                    expected_network,
                );
                if live_state != LiveContainerState::Unsafe && agent.brokered_egress {
                    let network_state = match agent.broker_network.as_deref() {
                        Some(network) => {
                            let mut network_command = Command::new("sudo");
                            network_command.args(network_inspect_arguments(&config.backend, network));
                            match output_with_timeout(
                                &mut network_command,
                                LIVE_INSPECTION_TIMEOUT,
                            ) {
                                Ok(network_result) if network_result.status.success() => {
                                    serde_json::from_slice::<serde_json::Value>(
                                        &network_result.stdout,
                                    )
                                    .ok()
                                    .map_or(LiveContainerState::Unknown, |network_value| {
                                        live_broker_network_security(
                                            &network_value,
                                            &config.backend,
                                            network,
                                        )
                                    })
                                }
                                _ => LiveContainerState::Unknown,
                            }
                        }
                        None => LiveContainerState::Unknown,
                    };
                    live_state = merge_live_state(live_state, network_state);
                }
                match live_state {
                    LiveContainerState::Secure => {}
                    LiveContainerState::Unsafe => findings.push(agent_finding(
                        "TFSEC-034",
                        "critical",
                        agent,
                        "live OCI state differs from one or more secure capsule invariants",
                        "stop the exact agent, inspect the backend JSON, and rebuild the declared capsule before restart",
                    )),
                    LiveContainerState::Unknown => findings.push(agent_finding(
                        "TFSEC-033",
                        "warning",
                        agent,
                        "live OCI state was returned but its security fields were incomplete or unrecognized",
                        "inspect the exact backend JSON and keep the live posture unknown until every invariant is accounted for",
                    )),
                }
            }
            _ => findings.push(agent_finding(
                "TFSEC-033",
                "warning",
                agent,
                "live OCI state could not be inspected without prompting",
                "run the security doctor through an approved root/operator path; unavailable live evidence is not green",
            )),
        }
    }

    findings
}

fn inspect_arguments_for_agent<'a>(backend: &'a str, agent: &'a SecurityAgent) -> [&'a str; 4] {
    inspect_arguments(backend, &agent.name)
}

fn inspect_arguments<'a>(backend: &'a str, container: &'a str) -> [&'a str; 4] {
    ["-n", backend, "inspect", container]
}

fn network_inspect_arguments<'a>(backend: &'a str, network: &'a str) -> [&'a str; 5] {
    ["-n", backend, "network", "inspect", network]
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum BrokerHealth {
    Ready,
    Unavailable,
    Unhealthy,
}

fn probe_broker_health(endpoint: SocketAddr) -> BrokerHealth {
    let timeout = std::time::Duration::from_secs(2);
    let Ok(mut stream) = TcpStream::connect_timeout(&endpoint, timeout) else {
        return BrokerHealth::Unavailable;
    };
    if stream.set_read_timeout(Some(timeout)).is_err()
        || stream.set_write_timeout(Some(timeout)).is_err()
    {
        return BrokerHealth::Unavailable;
    }
    let request = format!(
        "GET /healthz HTTP/1.1\r\nHost: {}\r\nConnection: close\r\n\r\n",
        endpoint.ip()
    );
    if stream.write_all(request.as_bytes()).is_err() {
        return BrokerHealth::Unavailable;
    }
    let mut response = Vec::new();
    if stream.take(8193).read_to_end(&mut response).is_err() || response.len() > 8192 {
        return BrokerHealth::Unavailable;
    }
    let response = String::from_utf8_lossy(&response);
    let status_ready =
        response.starts_with("HTTP/1.1 200 ") || response.starts_with("HTTP/1.0 200 ");
    if status_ready && response.contains("\"status\":\"ready\"") {
        BrokerHealth::Ready
    } else {
        BrokerHealth::Unhealthy
    }
}

fn probe_broker_health_bounded(endpoints: &[SocketAddr]) -> Vec<BrokerHealth> {
    endpoints
        .chunks(MAX_PARALLEL_BROKER_HEALTH_PROBES)
        .flat_map(|batch| {
            std::thread::scope(|scope| {
                let handles = batch
                    .iter()
                    .map(|endpoint| scope.spawn(move || probe_broker_health(*endpoint)))
                    .collect::<Vec<_>>();
                handles
                    .into_iter()
                    .map(|handle| handle.join().unwrap_or(BrokerHealth::Unavailable))
                    .collect::<Vec<_>>()
            })
        })
        .collect()
}

fn parse_disk_percent(text: &str) -> Option<u8> {
    text.lines()
        .nth(1)
        .and_then(|line| line.split_whitespace().nth(4))
        .and_then(|value| value.trim_end_matches('%').parse().ok())
}

fn json_is_nonempty(value: &serde_json::Value) -> bool {
    match value {
        serde_json::Value::Null => false,
        serde_json::Value::Bool(value) => *value,
        serde_json::Value::Number(value) => value.as_u64().is_some_and(|value| value != 0),
        serde_json::Value::String(value) => !value.is_empty(),
        serde_json::Value::Array(value) => value.iter().any(json_is_nonempty),
        serde_json::Value::Object(value) => value.values().any(json_is_nonempty),
    }
}

fn json_has_truthy_key(value: &serde_json::Value, needle: &str) -> bool {
    match value {
        serde_json::Value::Array(values) => values
            .iter()
            .any(|value| json_has_truthy_key(value, needle)),
        serde_json::Value::Object(values) => values.iter().any(|(key, value)| {
            (key.to_ascii_lowercase().contains(needle) && json_is_nonempty(value))
                || json_has_truthy_key(value, needle)
        }),
        _ => false,
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum LiveContainerState {
    Secure,
    Unsafe,
    Unknown,
}

fn live_container_security_from_manifest(
    agent: &SecurityAgent,
    value: Option<&serde_json::Value>,
    backend: &str,
    expected_network: Option<&str>,
) -> LiveContainerState {
    match (
        agent.declared_ports_absent,
        agent.declared_mounts.as_deref(),
        agent.declared_tmpfs.as_ref(),
        agent.declared_live_resources.as_ref(),
        value,
        expected_network,
    ) {
        (Some(false), _, _, _, _, _) => LiveContainerState::Unsafe,
        (Some(true), Some(mounts), Some(tmpfs), Some(resources), Some(value), Some(network)) => {
            live_container_security(value, backend, network, mounts, tmpfs, resources)
        }
        _ => LiveContainerState::Unknown,
    }
}

fn live_container_security(
    value: &serde_json::Value,
    backend: &str,
    expected_network: &str,
    declared_mounts: &[DeclaredMount],
    declared_tmpfs: &BTreeMap<String, u64>,
    declared_live_resources: &DeclaredLiveResources,
) -> LiveContainerState {
    let Some(containers) = value.as_array() else {
        return LiveContainerState::Unknown;
    };
    if containers.len() != 1 {
        return LiveContainerState::Unknown;
    }
    let Some(container) = containers.first().and_then(serde_json::Value::as_object) else {
        return LiveContainerState::Unknown;
    };
    let Some(true) = container
        .get("State")
        .and_then(serde_json::Value::as_object)
        .and_then(|state| state.get("Running"))
        .and_then(serde_json::Value::as_bool)
    else {
        return LiveContainerState::Unknown;
    };
    live_observed_security_drift(
        container,
        backend,
        expected_network,
        declared_mounts,
        declared_tmpfs,
        declared_live_resources,
    )
}

fn live_expected_bool_security(
    value: Option<&serde_json::Value>,
    expected: bool,
) -> LiveContainerState {
    match value.and_then(serde_json::Value::as_bool) {
        Some(value) if value == expected => LiveContainerState::Secure,
        Some(_) => LiveContainerState::Unsafe,
        None => LiveContainerState::Unknown,
    }
}

fn live_network_attachment_security(
    network_settings: Option<&serde_json::Map<String, serde_json::Value>>,
    backend: &str,
    expected_network: &str,
) -> LiveContainerState {
    let Some(network_settings) = network_settings else {
        return LiveContainerState::Unknown;
    };
    match network_settings.get("Networks") {
        Some(serde_json::Value::Object(networks)) if expected_network == "none" => {
            if networks.is_empty() || (networks.len() == 1 && networks.contains_key("none")) {
                LiveContainerState::Secure
            } else {
                LiveContainerState::Unsafe
            }
        }
        Some(serde_json::Value::Object(networks)) => {
            if networks.len() == 1 && networks.contains_key(expected_network) {
                LiveContainerState::Secure
            } else {
                LiveContainerState::Unsafe
            }
        }
        None | Some(serde_json::Value::Null)
            if backend == "podman" && expected_network == "none" =>
        {
            LiveContainerState::Secure
        }
        _ => LiveContainerState::Unknown,
    }
}

fn live_security_options_security(
    host: Option<&serde_json::Map<String, serde_json::Value>>,
) -> LiveContainerState {
    let Some(host) = host else {
        return LiveContainerState::Unknown;
    };
    let explicit_no_new_privileges = match host.get("NoNewPrivileges") {
        Some(serde_json::Value::Bool(value)) => Some(*value),
        _ => None,
    };
    let options = match host.get("SecurityOpt") {
        Some(serde_json::Value::Array(options)) => options.as_slice(),
        Some(serde_json::Value::Null) => &[],
        _ => {
            return match explicit_no_new_privileges {
                Some(false) => LiveContainerState::Unsafe,
                _ => LiveContainerState::Unknown,
            };
        }
    };

    let mut state = match host.get("NoNewPrivileges") {
        Some(serde_json::Value::Bool(false)) => LiveContainerState::Unsafe,
        Some(serde_json::Value::Bool(true)) | None => LiveContainerState::Secure,
        Some(_) => LiveContainerState::Unknown,
    };
    let mut no_new_privileges = explicit_no_new_privileges == Some(true);
    for option in options {
        let Some(option) = option.as_str() else {
            state = merge_live_state(state, LiveContainerState::Unknown);
            continue;
        };
        match option {
            "no-new-privileges" | "no-new-privileges=true" | "no-new-privileges:true" => {
                no_new_privileges = true
            }
            "no-new-privileges=false"
            | "no-new-privileges:false"
            | "seccomp=unconfined"
            | "seccomp:unconfined"
            | "apparmor=unconfined"
            | "apparmor:unconfined" => {
                state = merge_live_state(state, LiveContainerState::Unsafe);
            }
            _ => {}
        }
    }
    if !no_new_privileges {
        state = merge_live_state(state, LiveContainerState::Unsafe);
    }
    state
}

fn live_mounts_security(
    value: Option<&serde_json::Value>,
    declared_mounts: &[DeclaredMount],
) -> LiveContainerState {
    let Some(mounts) = value.and_then(serde_json::Value::as_array) else {
        return LiveContainerState::Unknown;
    };
    let mut state = LiveContainerState::Secure;
    let mut complete = true;
    let mut observed_mounts = Vec::new();
    for mount in mounts {
        let Some(mount) = mount.as_object() else {
            complete = false;
            state = merge_live_state(state, LiveContainerState::Unknown);
            continue;
        };
        if mount.get("Type").and_then(serde_json::Value::as_str) == Some("tmpfs") {
            continue;
        }
        let source = mount.get("Source").and_then(serde_json::Value::as_str);
        if source.is_some_and(live_mount_source_is_sensitive) {
            state = merge_live_state(state, LiveContainerState::Unsafe);
        }
        let destination = mount.get("Destination").and_then(serde_json::Value::as_str);
        let writable = mount.get("RW").and_then(serde_json::Value::as_bool);
        match (source, destination, writable) {
            (Some(source), Some(destination), Some(writable)) => {
                observed_mounts.push(DeclaredMount {
                    source: source.to_owned(),
                    destination: destination.to_owned(),
                    writable,
                });
            }
            _ => {
                complete = false;
                state = merge_live_state(state, LiveContainerState::Unknown);
            }
        }
    }
    observed_mounts.sort();
    let mut expected_mounts = declared_mounts.to_vec();
    expected_mounts.sort();
    let set_state = if complete {
        if observed_mounts == expected_mounts {
            LiveContainerState::Secure
        } else {
            LiveContainerState::Unsafe
        }
    } else if observed_mounts
        .iter()
        .any(|mount| !expected_mounts.contains(mount))
    {
        LiveContainerState::Unsafe
    } else {
        LiveContainerState::Unknown
    };
    merge_live_state(state, set_state)
}

fn live_capabilities_security(
    container: &serde_json::Map<String, serde_json::Value>,
    host: Option<&serde_json::Map<String, serde_json::Value>>,
    backend: &str,
) -> LiveContainerState {
    if backend == "podman" {
        return ["EffectiveCaps", "BoundingCaps"]
            .into_iter()
            .map(
                |field| match container.get(field).and_then(serde_json::Value::as_array) {
                    Some(values) if values.is_empty() => LiveContainerState::Secure,
                    Some(_) => LiveContainerState::Unsafe,
                    None => LiveContainerState::Unknown,
                },
            )
            .fold(LiveContainerState::Secure, merge_live_state);
    }
    let Some(host) = host else {
        return LiveContainerState::Unknown;
    };
    let cap_add = match host.get("CapAdd") {
        Some(serde_json::Value::Null) => LiveContainerState::Secure,
        Some(serde_json::Value::Array(values)) if values.is_empty() => LiveContainerState::Secure,
        Some(serde_json::Value::Array(_)) => LiveContainerState::Unsafe,
        _ => LiveContainerState::Unknown,
    };
    let cap_drop = match host.get("CapDrop").and_then(serde_json::Value::as_array) {
        Some(values) => {
            let mut state = LiveContainerState::Secure;
            let mut has_all = false;
            for value in values {
                match value.as_str() {
                    Some(value) => has_all |= value.eq_ignore_ascii_case("all"),
                    None => state = merge_live_state(state, LiveContainerState::Unknown),
                }
            }
            if !has_all {
                state = merge_live_state(state, LiveContainerState::Unsafe);
            }
            state
        }
        None => LiveContainerState::Unknown,
    };
    merge_live_state(cap_add, cap_drop)
}

fn live_apparmor_security(
    container: &serde_json::Map<String, serde_json::Value>,
    backend: &str,
) -> LiveContainerState {
    match (
        backend,
        container
            .get("AppArmorProfile")
            .and_then(serde_json::Value::as_str),
    ) {
        ("docker", Some("docker-default")) => LiveContainerState::Secure,
        ("docker", Some(_)) => LiveContainerState::Unsafe,
        ("podman", Some(profile)) if !profile.is_empty() && profile != "unconfined" => {
            LiveContainerState::Secure
        }
        ("podman", Some(_)) => LiveContainerState::Unsafe,
        _ => LiveContainerState::Unknown,
    }
}

fn live_observed_security_drift(
    container: &serde_json::Map<String, serde_json::Value>,
    backend: &str,
    expected_network: &str,
    declared_mounts: &[DeclaredMount],
    declared_tmpfs: &BTreeMap<String, u64>,
    declared_live_resources: &DeclaredLiveResources,
) -> LiveContainerState {
    let network_settings = container
        .get("NetworkSettings")
        .and_then(serde_json::Value::as_object);
    let host = container
        .get("HostConfig")
        .and_then(serde_json::Value::as_object);
    let mut state = live_network_attachment_security(network_settings, backend, expected_network);
    state = merge_live_state(state, live_port_bindings_security(host, network_settings));

    if let Some(host) = host {
        let tmpfs = match host.get("Tmpfs") {
            Some(serde_json::Value::Object(tmpfs)) => {
                live_tmpfs_set_security(tmpfs, declared_tmpfs, backend)
            }
            None | Some(serde_json::Value::Null) => LiveContainerState::Unsafe,
            Some(_) => LiveContainerState::Unknown,
        };
        state = merge_live_state(state, tmpfs);
        state = merge_live_state(
            state,
            live_resource_limits_security(host, backend, declared_live_resources),
        );
    } else {
        state = merge_live_state(state, LiveContainerState::Unknown);
    }

    state = merge_live_state(
        state,
        live_expected_bool_security(host.and_then(|host| host.get("Privileged")), false),
    );
    state = merge_live_state(
        state,
        live_expected_bool_security(host.and_then(|host| host.get("ReadonlyRootfs")), true),
    );
    let network_mode = match host
        .and_then(|host| host.get("NetworkMode"))
        .and_then(serde_json::Value::as_str)
    {
        Some("none") if expected_network == "none" => LiveContainerState::Secure,
        Some("bridge") if expected_network != "none" && backend == "podman" => {
            LiveContainerState::Secure
        }
        Some(network) if expected_network != "none" && network == expected_network => {
            LiveContainerState::Secure
        }
        Some(_) => LiveContainerState::Unsafe,
        None => LiveContainerState::Unknown,
    };
    state = merge_live_state(state, network_mode);

    let user = match container
        .get("Config")
        .and_then(serde_json::Value::as_object)
        .and_then(|config| config.get("User"))
        .and_then(serde_json::Value::as_str)
    {
        Some(user) if live_user_is_non_root(user) => LiveContainerState::Secure,
        Some(_) => LiveContainerState::Unsafe,
        None => LiveContainerState::Unknown,
    };
    state = merge_live_state(state, user);

    let runtime = if backend == "podman" {
        container
            .get("OCIRuntime")
            .and_then(serde_json::Value::as_str)
    } else {
        host.and_then(|host| host.get("Runtime"))
            .and_then(serde_json::Value::as_str)
    };
    state = merge_live_state(
        state,
        match runtime {
            Some(runtime) if runtime == "runsc" || runtime.ends_with("/runsc") => {
                LiveContainerState::Secure
            }
            Some(_) => LiveContainerState::Unsafe,
            None => LiveContainerState::Unknown,
        },
    );
    state = merge_live_state(state, live_security_options_security(host));
    state = merge_live_state(
        state,
        live_mounts_security(container.get("Mounts"), declared_mounts),
    );
    state = merge_live_state(state, live_capabilities_security(container, host, backend));
    state = merge_live_state(state, live_apparmor_security(container, backend));
    state
}

fn merge_live_state(
    current: LiveContainerState,
    observed: LiveContainerState,
) -> LiveContainerState {
    match (current, observed) {
        (LiveContainerState::Unsafe, _) | (_, LiveContainerState::Unsafe) => {
            LiveContainerState::Unsafe
        }
        (LiveContainerState::Unknown, _) | (_, LiveContainerState::Unknown) => {
            LiveContainerState::Unknown
        }
        (LiveContainerState::Secure, LiveContainerState::Secure) => LiveContainerState::Secure,
    }
}

fn live_port_bindings_security(
    host: Option<&serde_json::Map<String, serde_json::Value>>,
    network_settings: Option<&serde_json::Map<String, serde_json::Value>>,
) -> LiveContainerState {
    let publish_all = match host
        .and_then(|host| host.get("PublishAllPorts"))
        .and_then(serde_json::Value::as_bool)
    {
        Some(false) => LiveContainerState::Secure,
        Some(true) => LiveContainerState::Unsafe,
        None => LiveContainerState::Unknown,
    };
    [
        publish_all,
        live_port_map_security(host.and_then(|host| host.get("PortBindings"))),
        live_port_map_security(
            network_settings.and_then(|network_settings| network_settings.get("Ports")),
        ),
    ]
    .into_iter()
    .fold(LiveContainerState::Secure, merge_live_state)
}

fn live_port_map_security(value: Option<&serde_json::Value>) -> LiveContainerState {
    let Some(value) = value else {
        return LiveContainerState::Unknown;
    };
    match value {
        serde_json::Value::Null => LiveContainerState::Secure,
        serde_json::Value::Object(bindings) => {
            bindings
                .values()
                .fold(LiveContainerState::Secure, |state, value| {
                    let observed = match value {
                        serde_json::Value::Null => LiveContainerState::Secure,
                        serde_json::Value::Array(values) if values.is_empty() => {
                            LiveContainerState::Secure
                        }
                        serde_json::Value::Array(_) => LiveContainerState::Unsafe,
                        _ => LiveContainerState::Unknown,
                    };
                    merge_live_state(state, observed)
                })
        }
        _ => LiveContainerState::Unknown,
    }
}

fn live_u64_field(
    object: &serde_json::Map<String, serde_json::Value>,
    field: &str,
) -> Result<u64, LiveContainerState> {
    let Some(value) = object.get(field) else {
        return Err(LiveContainerState::Unknown);
    };
    let Some(number) = value.as_number() else {
        return Err(LiveContainerState::Unknown);
    };
    if let Some(value) = number.as_u64() {
        Ok(value)
    } else if number.as_i64().is_some() {
        Err(LiveContainerState::Unsafe)
    } else {
        Err(LiveContainerState::Unknown)
    }
}

fn live_exact_u64_security(
    object: &serde_json::Map<String, serde_json::Value>,
    field: &str,
    expected: u64,
) -> LiveContainerState {
    if object.get(field).is_some_and(serde_json::Value::is_null) {
        return LiveContainerState::Unsafe;
    }
    match live_u64_field(object, field) {
        Ok(value) if value == expected => LiveContainerState::Secure,
        Ok(_) => LiveContainerState::Unsafe,
        Err(state) => state,
    }
}

fn live_resource_limits_security(
    host: &serde_json::Map<String, serde_json::Value>,
    backend: &str,
    declared: &DeclaredLiveResources,
) -> LiveContainerState {
    let mut state = [
        ("Memory", declared.memory_bytes),
        ("MemorySwap", declared.memory_swap_bytes),
        ("PidsLimit", declared.pids_limit),
        ("NanoCpus", declared.nano_cpus),
    ]
    .into_iter()
    .map(|(field, expected)| live_exact_u64_security(host, field, expected))
    .fold(LiveContainerState::Secure, merge_live_state);

    if backend == "podman" {
        for (field, expected) in [
            ("CpuPeriod", 100_000),
            ("CpuQuota", declared.nano_cpus / 10_000),
        ] {
            state = merge_live_state(state, live_exact_u64_security(host, field, expected));
        }
    }
    merge_live_state(
        state,
        live_ulimits_security(host.get("Ulimits"), backend, declared),
    )
}

fn live_ulimits_security(
    value: Option<&serde_json::Value>,
    backend: &str,
    declared: &DeclaredLiveResources,
) -> LiveContainerState {
    let ulimits = match value {
        None => return LiveContainerState::Unknown,
        Some(serde_json::Value::Null) => return LiveContainerState::Unsafe,
        Some(serde_json::Value::Array(ulimits)) => ulimits,
        Some(_) => return LiveContainerState::Unknown,
    };
    let (nofile_name, nproc_name) = if backend == "podman" {
        ("RLIMIT_NOFILE", "RLIMIT_NPROC")
    } else {
        ("nofile", "nproc")
    };
    let mut state = LiveContainerState::Secure;
    let mut names_complete = true;
    let mut nofile_count = 0;
    let mut nproc_count = 0;
    for ulimit in ulimits {
        let Some(ulimit) = ulimit.as_object() else {
            names_complete = false;
            state = merge_live_state(state, LiveContainerState::Unknown);
            continue;
        };
        let Some(name) = ulimit.get("Name").and_then(serde_json::Value::as_str) else {
            names_complete = false;
            state = merge_live_state(state, LiveContainerState::Unknown);
            continue;
        };
        let expected = if name == nofile_name {
            nofile_count += 1;
            Some(declared.nofile)
        } else if name == nproc_name {
            nproc_count += 1;
            Some(declared.nproc)
        } else {
            None
        };
        if let Some(expected) = expected {
            for field in ["Soft", "Hard"] {
                state = merge_live_state(state, live_exact_u64_security(ulimit, field, expected));
            }
        } else {
            for field in ["Soft", "Hard"] {
                let observed = match live_u64_field(ulimit, field) {
                    Ok(_) => LiveContainerState::Secure,
                    Err(state) => state,
                };
                state = merge_live_state(state, observed);
            }
        }
    }
    for count in [nofile_count, nproc_count] {
        let observed = match count {
            0 if names_complete => LiveContainerState::Unsafe,
            0 => LiveContainerState::Unknown,
            1 => LiveContainerState::Secure,
            _ => LiveContainerState::Unknown,
        };
        state = merge_live_state(state, observed);
    }
    state
}

fn live_tmpfs_set_security(
    observed: &serde_json::Map<String, serde_json::Value>,
    declared: &BTreeMap<String, u64>,
    backend: &str,
) -> LiveContainerState {
    if observed.len() != declared.len()
        || declared
            .keys()
            .any(|destination| !observed.contains_key(destination))
    {
        return LiveContainerState::Unsafe;
    }
    declared
        .iter()
        .map(|(destination, expected_size)| {
            observed
                .get(destination)
                .and_then(serde_json::Value::as_str)
                .map_or(LiveContainerState::Unknown, |options| {
                    live_tmpfs_options_security(options, *expected_size, backend)
                })
        })
        .fold(LiveContainerState::Secure, merge_live_state)
}

fn live_tmpfs_options_security(
    options: &str,
    expected_size: u64,
    backend: &str,
) -> LiveContainerState {
    let mut state = LiveContainerState::Secure;
    let mut read_write = false;
    let mut no_suid = false;
    let mut no_dev = false;
    let mut no_exec = false;
    let mut recursive_private = false;
    let mut copy_up = false;
    let mut size_seen = false;
    for option in options.split(',') {
        let already_seen = match option {
            "rw" => std::mem::replace(&mut read_write, true),
            "nosuid" => std::mem::replace(&mut no_suid, true),
            "nodev" => std::mem::replace(&mut no_dev, true),
            "noexec" => std::mem::replace(&mut no_exec, true),
            "rprivate" => std::mem::replace(&mut recursive_private, true),
            "tmpcopyup" => std::mem::replace(&mut copy_up, true),
            "ro" | "suid" | "dev" | "exec" | "private" | "shared" | "rshared" | "slave"
            | "rslave" | "unbindable" | "runbindable" | "notmpcopyup" => {
                state = merge_live_state(state, LiveContainerState::Unsafe);
                false
            }
            value if value.starts_with("size=") => {
                let duplicate = std::mem::replace(&mut size_seen, true);
                state = merge_live_state(
                    state,
                    match parse_live_size(&value[5..]) {
                        Some(size) if size == expected_size => LiveContainerState::Secure,
                        Some(_) => LiveContainerState::Unsafe,
                        None => LiveContainerState::Unknown,
                    },
                );
                duplicate
            }
            _ => {
                state = merge_live_state(state, LiveContainerState::Unknown);
                false
            }
        };
        if already_seen {
            state = merge_live_state(state, LiveContainerState::Unknown);
        }
    }
    let required_options = if read_write
        && no_suid
        && no_dev
        && no_exec
        && size_seen
        && if backend == "podman" {
            recursive_private && copy_up
        } else {
            !recursive_private && !copy_up
        } {
        LiveContainerState::Secure
    } else {
        LiveContainerState::Unsafe
    };
    merge_live_state(state, required_options)
}

fn parse_live_size(value: &str) -> Option<u64> {
    let normalized = value.to_ascii_lowercase();
    let normalized = normalized.strip_suffix('b').unwrap_or(&normalized);
    let (digits, factor) = match normalized.as_bytes().last().copied() {
        Some(b'k') => (&normalized[..normalized.len() - 1], 1024_u64),
        Some(b'm') => (&normalized[..normalized.len() - 1], 1024_u64.pow(2)),
        Some(b'g') => (&normalized[..normalized.len() - 1], 1024_u64.pow(3)),
        Some(b't') => (&normalized[..normalized.len() - 1], 1024_u64.pow(4)),
        _ => (normalized, 1),
    };
    if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    digits.parse::<u64>().ok()?.checked_mul(factor)
}

fn live_broker_network_security(
    network_value: &serde_json::Value,
    backend: &str,
    expected_network: &str,
) -> LiveContainerState {
    let Some(networks) = network_value.as_array() else {
        return LiveContainerState::Unknown;
    };
    if networks.len() > 1 {
        return LiveContainerState::Unsafe;
    }
    let Some(network) = networks.first().and_then(serde_json::Value::as_object) else {
        return LiveContainerState::Unknown;
    };
    let (name_key, internal_key, driver_key) = if backend == "podman" {
        ("name", "internal", "driver")
    } else {
        ("Name", "Internal", "Driver")
    };
    [
        match network.get(name_key).and_then(serde_json::Value::as_str) {
            Some(name) if name == expected_network => LiveContainerState::Secure,
            Some(_) => LiveContainerState::Unsafe,
            None => LiveContainerState::Unknown,
        },
        match network
            .get(internal_key)
            .and_then(serde_json::Value::as_bool)
        {
            Some(true) => LiveContainerState::Secure,
            Some(false) => LiveContainerState::Unsafe,
            None => LiveContainerState::Unknown,
        },
        match network.get(driver_key).and_then(serde_json::Value::as_str) {
            Some("bridge") => LiveContainerState::Secure,
            Some(_) => LiveContainerState::Unsafe,
            None => LiveContainerState::Unknown,
        },
    ]
    .into_iter()
    .fold(LiveContainerState::Secure, merge_live_state)
}

fn live_user_is_non_root(user: &str) -> bool {
    let mut fields = user.split(':');
    let uid = fields.next().unwrap_or_default();
    let gid = fields.next().unwrap_or_default();
    fields.next().is_none()
        && uid.parse::<u64>().is_ok_and(|value| value > 0)
        && gid.parse::<u64>().is_ok_and(|value| value > 0)
}

fn live_mount_source_is_sensitive(source: &str) -> bool {
    [
        "/",
        "/dev",
        "/etc",
        "/home",
        "/proc",
        "/root",
        "/run",
        "/sys",
        "/var/lib/containers",
        "/var/lib/docker",
        "/var/lib/tentaflake-worker-state-volumes",
        "/var/lib/tentaflake-workspace-volumes",
        "/var/run",
    ]
    .iter()
    .any(|root| source == *root || source.starts_with(&format!("{root}/")))
        || source.ends_with(".sock")
}

fn host_finding(
    id: &'static str,
    severity: &'static str,
    explanation: &'static str,
    remediation: &'static str,
) -> Finding {
    Finding {
        id,
        severity,
        agent: None,
        explanation,
        remediation,
    }
}

fn agent_finding(
    id: &'static str,
    severity: &'static str,
    agent: &SecurityAgent,
    explanation: &'static str,
    remediation: &'static str,
) -> Finding {
    Finding {
        id,
        severity,
        agent: Some(agent.name.clone()),
        explanation,
        remediation,
    }
}

fn check_agent(
    findings: &mut Vec<Finding>,
    condition: bool,
    id: &'static str,
    severity: &'static str,
    agent: &SecurityAgent,
    explanation: &'static str,
    remediation: &'static str,
) {
    if !condition {
        findings.push(agent_finding(id, severity, agent, explanation, remediation));
    }
}

fn load_agents(path: &Path) -> Result<Vec<Agent>, String> {
    let text = fs::read_to_string(path)
        .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
    parse_agents(&text)
}

fn parse_agents(text: &str) -> Result<Vec<Agent>, String> {
    text.lines()
        .filter(|line| !line.trim().is_empty() && !line.starts_with('#'))
        .enumerate()
        .map(|(index, line)| {
            let fields: Vec<&str> = line.split('\t').collect();
            if fields.len() != 5 || fields.iter().any(|field| field.is_empty()) {
                return Err(format!(
                    "invalid agent record on line {}: expected five non-empty tab-separated fields",
                    index + 1
                ));
            }
            Ok(Agent {
                runtime: fields[0].into(),
                name: fields[1].into(),
                container: fields[2].into(),
                unit: fields[3].into(),
                state_dir: PathBuf::from(fields[4]),
            })
        })
        .collect()
}

fn find_agent<'a>(agents: &'a [Agent], query: Option<&String>) -> Result<&'a Agent, String> {
    let query = query.ok_or_else(|| {
        let names = agents
            .iter()
            .map(|agent| agent.name.as_str())
            .collect::<Vec<_>>()
            .join(" ");
        format!("missing agent name; available: {names}")
    })?;
    agents
        .iter()
        .find(|agent| &agent.name == query || &agent.container == query)
        .ok_or_else(|| format!("unknown agent `{query}`"))
}

fn status(config: &Config, agents: &[Agent], mode: OutputMode) -> Result<u8, String> {
    let rows: Vec<(&Agent, String)> = agents
        .iter()
        .map(|agent| (agent, unit_state(&agent.unit)))
        .collect();

    if mode.json {
        print!(
            "{{\"host\":\"{}\",\"backend\":\"{}\",\"security_profile\":\"{}\",\"agents\":[",
            json_escape(if mode.hide {
                "redacted"
            } else {
                &config.host_name
            }),
            json_escape(&config.backend),
            json_escape(&config.security_profile)
        );
        for (index, (agent, state)) in rows.iter().enumerate() {
            if index != 0 {
                print!(",");
            }
            let name = if mode.hide {
                format!("agent-{}", index + 1)
            } else {
                agent.name.clone()
            };
            print!(
                "{{\"name\":\"{}\",\"runtime\":\"{}\",\"state\":\"{}\"}}",
                json_escape(&name),
                json_escape(&agent.runtime),
                json_escape(state)
            );
        }
        println!("]}}");
        return Ok(0);
    }

    let color = io::stdout().is_terminal() && env::var_os("NO_COLOR").is_none();
    let host = if mode.hide {
        "redacted"
    } else {
        &config.host_name
    };
    println!(
        "tentaflake {host} · {} · security {}",
        config.backend, config.security_profile
    );
    if config.security_profile == "dev" {
        println!("  WARNING: dev is not a security boundary for untrusted 24/7 agents");
    }
    if rows.is_empty() {
        println!("  no agents defined");
    }
    for (index, (agent, state)) in rows.iter().enumerate() {
        let marker = match state.as_str() {
            "active" if color => "\u{1b}[32m●\u{1b}[0m",
            "failed" if color => "\u{1b}[31m●\u{1b}[0m",
            "active" => "●",
            _ => "○",
        };
        let name = if mode.hide {
            format!("agent-{}", index + 1)
        } else {
            agent.name.clone()
        };
        println!("  {marker} {name:<20} {:<10} {state}", agent.runtime);
    }
    Ok(0)
}

fn health(config: &Config, agents: &[Agent], mode: OutputMode) -> Result<u8, String> {
    if mode.json {
        return doctor(config, agents, mode);
    }
    let host = if mode.hide {
        "redacted"
    } else {
        &config.host_name
    };
    println!("tentaflake health · {host}");
    if let Ok(load) = fs::read_to_string("/proc/loadavg") {
        println!(
            "  load: {}",
            load.split_whitespace()
                .take(3)
                .collect::<Vec<_>>()
                .join(" ")
        );
    }
    let disk = output("df", &["-P", "/"])?;
    if let Some(line) = String::from_utf8_lossy(&disk.stdout).lines().nth(1) {
        println!("  disk: {line}");
    }
    let failed = agents
        .iter()
        .filter(|agent| unit_state(&agent.unit) == "failed")
        .count();
    println!("  agents: {} total, {failed} failed", agents.len());
    Ok(if failed == 0 { 0 } else { 1 })
}

fn doctor(config: &Config, agents: &[Agent], mode: OutputMode) -> Result<u8, String> {
    let failed_units = output("systemctl", &["--failed", "--no-legend", "--plain"])?;
    let failed_units_text = String::from_utf8_lossy(&failed_units.stdout)
        .trim()
        .to_owned();
    let disk = output("df", &["-P", "/"])?;
    let disk_pct = String::from_utf8_lossy(&disk.stdout)
        .lines()
        .nth(1)
        .and_then(|line| line.split_whitespace().nth(4))
        .and_then(|value| value.trim_end_matches('%').parse::<u8>().ok());
    let agent_failures: Vec<&Agent> = agents
        .iter()
        .filter(|agent| unit_state(&agent.unit) == "failed")
        .collect();
    let mut problems = usize::from(!failed_units_text.is_empty()) + agent_failures.len();
    if disk_pct.is_some_and(|value| value >= 90) {
        problems += 1;
    }

    if mode.json {
        println!(
            "{{\"host\":\"{}\",\"problems\":{},\"failed_systemd_units\":{},\"disk_percent\":{},\"failed_agents\":[{}]}}",
            json_escape(if mode.hide {
                "redacted"
            } else {
                &config.host_name
            }),
            problems,
            if failed_units_text.is_empty() {
                "false"
            } else {
                "true"
            },
            disk_pct.map_or("null".into(), |value| value.to_string()),
            agent_failures
                .iter()
                .enumerate()
                .map(|(index, agent)| {
                    let name = if mode.hide {
                        format!("agent-{}", index + 1)
                    } else {
                        agent.name.clone()
                    };
                    format!("\"{}\"", json_escape(&name))
                })
                .collect::<Vec<_>>()
                .join(",")
        );
    } else {
        println!("Tentaflake doctor — {}", config.host_name);
        finding(failed_units_text.is_empty(), "no failed systemd units");
        finding(
            disk_pct.is_none_or(|value| value < 90),
            &format!(
                "root disk usage: {}%",
                disk_pct.map_or("?".into(), |v| v.to_string())
            ),
        );
        for agent in agents {
            let state = unit_state(&agent.unit);
            finding(
                state != "failed",
                &format!("agent {} is {state}", agent.name),
            );
        }
        println!("{problems} problem(s) found");
    }
    Ok(if problems == 0 { 0 } else { 1 })
}

fn finding(ok: bool, message: &str) {
    println!("  {} {message}", if ok { "✓" } else { "✗" });
}

fn stats(config: &Config, agents: &[Agent]) -> Result<u8, String> {
    if agents.is_empty() {
        println!("no agents defined");
        return Ok(0);
    }
    let mut command = backend_command(config);
    command.arg("stats").arg("--no-stream");
    for agent in agents {
        command.arg(&agent.container);
    }
    exec(command)
}

fn logs(agents: &[Agent], args: &[String]) -> Result<u8, String> {
    let agent = find_agent(agents, args.first())?;
    let mut command = Command::new("journalctl");
    command.arg("-u").arg(&agent.unit).arg("-f");
    command.args(&args[1..]);
    exec(command)
}

fn lifecycle(action: &str, agents: &[Agent], args: &[String]) -> Result<u8, String> {
    let agent = find_agent(agents, args.first())?;
    let mut command = Command::new("sudo");
    command.arg("systemctl").arg(action).arg(&agent.unit);
    if action == "stop" {
        for mode in ["llm", "fetch"] {
            let unit = format!("tentaflake-broker-{mode}-{}.service", agent.container);
            if systemd_unit_exists(&unit) {
                command.arg(unit);
            }
        }
    }
    exec(command)
}

fn systemd_unit_exists(unit: &str) -> bool {
    Command::new("systemctl")
        .args(["show", "--property=LoadState", "--value", unit])
        .output()
        .is_ok_and(|output| output.status.success() && output.stdout != b"not-found\n")
}

fn container_shell(config: &Config, agents: &[Agent], args: &[String]) -> Result<u8, String> {
    let agent = find_agent(agents, args.first())?;
    let mut command = backend_command(config);
    command.args(["exec", "-it", &agent.container, "sh"]);
    exec(command)
}

fn container_exec(config: &Config, agents: &[Agent], args: &[String]) -> Result<u8, String> {
    let agent = find_agent(agents, args.first())?;
    let command_args = args[1..].strip_prefix(&["--".into()]).unwrap_or(&args[1..]);
    if command_args.is_empty() {
        return Err("missing command after agent name".into());
    }
    let mut command = backend_command(config);
    command.arg("exec").arg(&agent.container).args(command_args);
    exec(command)
}

fn backup(agents: &[Agent], args: &[String]) -> Result<u8, String> {
    let agent = find_agent(agents, args.first())?;
    if !agent.state_dir.is_dir() {
        return Err(format!(
            "state directory does not exist: {}",
            agent.state_dir.display()
        ));
    }
    let parent = agent
        .state_dir
        .parent()
        .ok_or("state directory has no parent")?;
    let base = agent
        .state_dir
        .file_name()
        .ok_or("state directory has no basename")?;
    let stamp = output("date", &["-u", "+%Y%m%dT%H%M%SZ"])?;
    let stamp = String::from_utf8_lossy(&stamp.stdout).trim().to_owned();
    let archive = format!("tentaflake-{}-{stamp}.tar.gz", agent.name);
    let status = Command::new("sudo")
        .arg("tar")
        .arg("czf")
        .arg(&archive)
        .arg("-C")
        .arg(parent)
        .arg(base)
        .status()
        .map_err(|error| format!("cannot execute backup: {error}"))?;
    if !status.success() {
        return Ok(status.code().unwrap_or(1) as u8);
    }
    let uid = current_id("-u")?;
    let gid = current_id("-g")?;
    let owner = format!("{uid}:{gid}");
    let status = Command::new("sudo")
        .args(["chown", &owner, &archive])
        .status()
        .map_err(|error| format!("cannot restore backup ownership: {error}"))?;
    if !status.success() {
        return Ok(status.code().unwrap_or(1) as u8);
    }
    let status = Command::new("chmod")
        .args(["0600", &archive])
        .status()
        .map_err(|error| format!("cannot secure backup permissions: {error}"))?;
    if !status.success() {
        return Ok(status.code().unwrap_or(1) as u8);
    }
    println!("backup written: {archive}");
    Ok(0)
}

fn current_id(flag: &str) -> Result<String, String> {
    let result = output("id", &[flag])?;
    if !result.status.success() {
        return Err(format!("`id {flag}` failed"));
    }
    Ok(String::from_utf8_lossy(&result.stdout).trim().to_owned())
}

fn rebuild(config: &Config) -> Result<u8, String> {
    let target = format!("{}#{}", config.flake_dir.display(), config.host_name);
    let mut command = Command::new("sudo");
    command.args(["nixos-rebuild", "switch", "--flake", &target]);
    exec(command)
}

fn update(config: &Config) -> Result<u8, String> {
    let mut command = Command::new("sudo");
    command.args(["nix", "flake", "update", "--flake"]);
    command.arg(&config.flake_dir);
    let status = command
        .status()
        .map_err(|error| format!("cannot execute nix flake update: {error}"))?;
    if !status.success() {
        return Ok(status.code().unwrap_or(1) as u8);
    }
    print!("Rebuild now to apply the update? [y/N] ");
    io::stdout().flush().map_err(|error| error.to_string())?;
    let mut answer = String::new();
    io::stdin()
        .read_line(&mut answer)
        .map_err(|error| error.to_string())?;
    if matches!(answer.trim(), "y" | "Y" | "yes" | "YES") {
        rebuild(config)
    } else {
        println!("not rebuilding; apply later with `tentaflake rebuild`");
        Ok(0)
    }
}

fn backend_passthrough(config: &Config, args: &[&str]) -> Result<u8, String> {
    let mut command = backend_command(config);
    command.args(args);
    exec(command)
}

fn backend_command(config: &Config) -> Command {
    if config.security_profile == "dev" {
        Command::new(&config.backend)
    } else {
        let mut command = Command::new("sudo");
        command.arg(&config.backend);
        command
    }
}

fn exec(mut command: Command) -> Result<u8, String> {
    let program = command.get_program().to_string_lossy().into_owned();
    let error = command.exec();
    Err(format!("cannot execute {program}: {error}"))
}

fn output(program: &str, args: &[&str]) -> Result<Output, String> {
    Command::new(program)
        .args(args)
        .output()
        .map_err(|error| format!("cannot execute {program}: {error}"))
}

fn output_with_timeout(command: &mut Command, timeout: Duration) -> Result<Output, String> {
    let program = command.get_program().to_string_lossy().into_owned();
    let mut child = command
        .spawn()
        .map_err(|error| format!("cannot execute {program}: {error}"))?;
    let deadline = Instant::now() + timeout;
    loop {
        match child
            .try_wait()
            .map_err(|error| format!("cannot wait for {program}: {error}"))?
        {
            Some(_) => {
                return child
                    .wait_with_output()
                    .map_err(|error| format!("cannot collect {program} output: {error}"));
            }
            None if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!(
                    "{program} exceeded the {timeout:?} inspection timeout"
                ));
            }
            None => std::thread::sleep(Duration::from_millis(10)),
        }
    }
}

fn unit_state(unit: &str) -> String {
    Command::new("systemctl")
        .args(["is-active", unit])
        .output()
        .ok()
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .map(|state| state.trim().to_owned())
        .filter(|state| !state.is_empty())
        .unwrap_or_else(|| "unknown".into())
}

fn json_escape(input: &str) -> String {
    let mut escaped = String::with_capacity(input.len());
    for ch in input.chars() {
        match ch {
            '"' => escaped.push_str("\\\""),
            '\\' => escaped.push_str("\\\\"),
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            '\t' => escaped.push_str("\\t"),
            value if value.is_control() => {
                use std::fmt::Write as _;
                let _ = write!(escaped, "\\u{:04x}", value as u32);
            }
            value => escaped.push(value),
        }
    }
    escaped
}

fn print_help(backend: &str) {
    println!(
        "Tentaflake — manage declarative agent containers (backend: {backend})\n\n\
         USAGE\n\
           tentaflake [status] [--hide] [--json]\n\
           tentaflake logs <name> [journalctl args]\n\
           tentaflake restart|start|stop <name>\n\
           tentaflake shell <name>\n\
           tentaflake exec <name> -- <command>\n\
           tentaflake ps|stats|health|doctor\n\
           tentaflake doctor --security [--json]\n\
           tentaflake backup <name>\n\
           tentaflake rebuild|update\n\n\
         Agent definitions are declarative: edit agents.json or my-agents.nix, then rebuild."
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_config_and_defaults_agents_path() {
        let config =
            parse_config("backend=podman\nflake_dir=/etc/nixos\nhost_name=agent-host\n").unwrap();
        assert_eq!(config.backend, "podman");
        assert_eq!(config.agents_file, PathBuf::from(DEFAULT_AGENTS));
    }

    #[test]
    fn rejects_unknown_backend() {
        let result = parse_config("backend=lxc\nflake_dir=/tmp\nhost_name=test\n");
        assert!(
            result
                .unwrap_err()
                .contains("unsupported container backend")
        );
    }

    #[test]
    fn rejects_unknown_security_profile() {
        let result = parse_config(
            "backend=docker\nflake_dir=/tmp\nhost_name=test\nsecurity_profile=unsafe\n",
        );
        assert!(result.unwrap_err().contains("unsupported security profile"));
    }

    #[test]
    fn parses_agent_records() {
        let agents = parse_agents(
            "hermes\tcoding\thermes-coding\tdocker-hermes-coding.service\t/var/lib/hermes-coding\n",
        )
        .unwrap();
        assert_eq!(agents.len(), 1);
        assert_eq!(agents[0].name, "coding");
        assert_eq!(agents[0].state_dir, PathBuf::from("/var/lib/hermes-coding"));
    }

    #[test]
    fn rejects_malformed_agent_records() {
        assert!(parse_agents("hermes\tmissing\tfields\n").is_err());
    }

    #[test]
    fn finds_agent_by_name_or_container() {
        let agents = parse_agents(
            "zeroclaw\tassistant\tzeroclaw-assistant\tdocker-zeroclaw-assistant.service\t/var/lib/zeroclaw-assistant\n",
        )
        .unwrap();
        assert_eq!(
            find_agent(&agents, Some(&"assistant".into())).unwrap().name,
            "assistant"
        );
        assert_eq!(
            find_agent(&agents, Some(&"zeroclaw-assistant".into()))
                .unwrap()
                .name,
            "assistant"
        );
    }

    #[test]
    fn escapes_json_control_characters() {
        assert_eq!(json_escape("a\"b\\c\n"), "a\\\"b\\\\c\\n");
    }

    #[test]
    fn canonicalizes_exact_github_https_remotes() {
        assert_eq!(
            canonical_github_remote("https://GitHub.com/Owner/Repo.git").unwrap(),
            "https://github.com/owner/repo"
        );
        assert_eq!(
            canonical_github_remote("https://github.com/owner/repo").unwrap(),
            "https://github.com/owner/repo"
        );
    }

    #[test]
    fn rejects_malicious_remote_variants() {
        for remote in [
            "http://github.com/owner/repo",
            "ssh://git@github.com/owner/repo",
            "git@github.com:owner/repo.git",
            "https://github.com.evil.test/owner/repo",
            "https://github.com@evil.test/owner/repo",
            "https://github.com/owner/repo/extra",
            "https://github.com/owner/../repo",
            "https://github.com/owner/repo.git.git",
            "https://github.com/owner/repo?x=1",
            "https://github.com/owner/repo#main",
            "https://github.com/owner/repo/",
            "https://github.com/owner%2frepo/x",
        ] {
            assert!(
                canonical_github_remote(remote).is_err(),
                "accepted malicious remote: {remote}"
            );
        }
    }

    #[test]
    fn remote_check_requires_an_exact_allowlist_match() {
        let allowed = "https://github.com/example/allowed.git".to_owned();
        assert_eq!(
            remote_check(&[
                "https://github.com/EXAMPLE/allowed".to_owned(),
                allowed.clone(),
            ])
            .unwrap(),
            0
        );
        assert_eq!(
            remote_check(&["https://github.com/example/other".to_owned(), allowed]).unwrap(),
            1
        );
    }

    #[test]
    fn parses_security_manifest_and_reports_fail_closed_broker_gap() {
        let state = parse_security_state(
            "host\tbalanced\tfalse\tfalse\ttrue\ttrue\ttrue\ttrue\t36\n\
             agent\tcoding\tbalanced\ttrue\ttrue\ttrue\ttrue\ttrue\ttrue\ttrue\ttrue\ttrue\ttrue\ttrue\ttrue\ttrue\ttrue\ttrue\ttrue\ttrue\tfalse\tfalse\t-\tfalse\t-\n",
        )
        .unwrap();
        let findings = security_findings(&state);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].id, "TFSEC-013");
        assert_eq!(findings[0].severity, "warning");
    }

    #[test]
    fn security_findings_have_stable_ids_for_unsafe_fixture() {
        let state = parse_security_state(
            "host\tdev\ttrue\tfalse\tfalse\tfalse\tfalse\tfalse\t36\n\
             agent\tcoding\tdev\tfalse\tfalse\tfalse\tfalse\tfalse\tfalse\tfalse\tfalse\tfalse\tfalse\tfalse\tfalse\tfalse\tfalse\tfalse\tfalse\tfalse\tfalse\tfalse\t-\tfalse\t-\n",
        )
        .unwrap();
        let ids: Vec<&str> = security_findings(&state)
            .iter()
            .map(|finding| finding.id)
            .collect();
        for expected in [
            "TFSEC-001",
            "TFSEC-002",
            "TFSEC-003",
            "TFSEC-004",
            "TFSEC-005",
            "TFSEC-006",
            "TFSEC-007",
            "TFSEC-008",
            "TFSEC-009",
            "TFSEC-010",
            "TFSEC-011",
            "TFSEC-012",
            "TFSEC-014",
            "TFSEC-015",
        ] {
            assert!(ids.contains(&expected), "missing finding {expected}");
        }
    }

    #[test]
    fn secure_profile_requires_disposable_worker() {
        let state = parse_security_state(
            "host\tbalanced\tfalse\ttrue\ttrue\ttrue\ttrue\ttrue\t36\n\
             agent\tcoding\tbalanced\ttrue\ttrue\ttrue\ttrue\ttrue\ttrue\ttrue\ttrue\ttrue\ttrue\ttrue\ttrue\tfalse\ttrue\ttrue\ttrue\ttrue\ttrue\tfalse\t-\ttrue\t10.0.0.1:7811\n",
        )
        .unwrap();
        let findings = security_findings(&state);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].id, "TFSEC-020");
        assert_eq!(findings[0].severity, "high");
    }

    #[test]
    fn secure_profile_requires_workspace_quota() {
        let state = parse_security_state(
            "host\tbalanced\tfalse\ttrue\ttrue\ttrue\ttrue\ttrue\t36\n\
             agent\tcoding\tbalanced\ttrue\ttrue\ttrue\ttrue\ttrue\ttrue\ttrue\ttrue\ttrue\ttrue\ttrue\ttrue\ttrue\tfalse\ttrue\ttrue\ttrue\ttrue\tfalse\t-\ttrue\t10.0.0.1:7811\n",
        )
        .unwrap();
        let findings = security_findings(&state);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].id, "TFSEC-021");
        assert_eq!(findings[0].severity, "high");
    }

    #[test]
    fn secure_profile_requires_seccomp_and_apparmor() {
        let state = parse_security_state(
            "host\tbalanced\tfalse\ttrue\ttrue\ttrue\ttrue\ttrue\t36\n\
             agent\tcoding\tbalanced\ttrue\ttrue\ttrue\ttrue\ttrue\ttrue\ttrue\ttrue\ttrue\ttrue\ttrue\ttrue\ttrue\ttrue\tfalse\tfalse\ttrue\ttrue\tfalse\t-\ttrue\t10.0.0.1:7811\n",
        )
        .unwrap();
        let ids: Vec<_> = security_findings(&state)
            .iter()
            .map(|finding| finding.id)
            .collect();
        assert_eq!(ids, ["TFSEC-022", "TFSEC-023"]);
    }

    #[test]
    fn rejects_malformed_security_manifest() {
        assert!(
            parse_security_state("host\tbalanced\tmaybe\tfalse\ttrue\ttrue\ttrue\tfalse\t36\n")
                .is_err()
        );
        assert!(parse_security_state("agent\tonly\ttwo\n").is_err());
    }

    #[test]
    fn secure_profile_reports_missing_image_signature_gate() {
        let state = parse_security_state(
            "host\tbalanced\tfalse\ttrue\ttrue\ttrue\ttrue\ttrue\t36\n\
             agent\tcoding\tbalanced\ttrue\ttrue\ttrue\ttrue\ttrue\ttrue\ttrue\ttrue\ttrue\ttrue\ttrue\ttrue\ttrue\ttrue\ttrue\ttrue\tfalse\ttrue\tfalse\t-\ttrue\t10.0.0.1:7811\n",
        )
        .unwrap();
        let findings = security_findings(&state);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].id, "TFSEC-024");
        assert_eq!(findings[0].severity, "warning");
    }

    #[test]
    fn parses_live_security_evidence_without_assuming_unknown_is_green() {
        assert_eq!(
            parse_disk_percent(
                "Filesystem 1024-blocks Used Available Capacity Mounted on\n/dev/vda 100 91 9 91% /\n"
            ),
            Some(91)
        );
        assert_eq!(parse_disk_percent("malformed\n"), None);

        let funnel: serde_json::Value =
            serde_json::from_str(r#"{"TCP":{"443":{"HTTPS":true}},"AllowFunnel":{"443":true}}"#)
                .unwrap();
        assert!(json_has_truthy_key(&funnel, "funnel"));
        assert!(json_is_nonempty(&funnel));
        assert!(!json_is_nonempty(&serde_json::json!({})));
    }

    #[test]
    fn broker_health_requires_an_explicit_ready_response() {
        fn serve_once(response: &'static [u8]) -> SocketAddr {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let address = listener.local_addr().unwrap();
            std::thread::spawn(move || {
                let (mut stream, _) = listener.accept().unwrap();
                let mut request = [0_u8; 1024];
                let _ = stream.read(&mut request).unwrap();
                stream.write_all(response).unwrap();
            });
            address
        }

        let ready = serve_once(
            b"HTTP/1.1 200 OK\r\nContent-Length: 18\r\nConnection: close\r\n\r\n{\"status\":\"ready\"}",
        );
        assert_eq!(probe_broker_health(ready), BrokerHealth::Ready);

        let unhealthy = serve_once(
            b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 20\r\nConnection: close\r\n\r\n{\"error\":\"audit\"}",
        );
        assert_eq!(probe_broker_health(unhealthy), BrokerHealth::Unhealthy);
    }

    #[test]
    fn broker_health_probes_preserve_one_result_per_endpoint() {
        let endpoints = (0..=MAX_PARALLEL_BROKER_HEALTH_PROBES)
            .map(|_| "127.0.0.1:0".parse::<SocketAddr>().unwrap())
            .collect::<Vec<_>>();
        let result = probe_broker_health_bounded(&endpoints);
        assert_eq!(result.len(), endpoints.len());
        assert!(
            result
                .iter()
                .all(|health| *health == BrokerHealth::Unavailable)
        );
    }

    #[test]
    fn live_inspection_command_times_out_and_reaps_the_child() {
        let mut command = Command::new("sh");
        command.args(["-c", "while :; do :; done"]);
        let error = output_with_timeout(&mut command, Duration::from_millis(10)).unwrap_err();
        assert!(error.contains("inspection timeout"), "{error}");
    }

    #[test]
    fn broker_manifest_modes_must_match_the_network_label() {
        let state = parse_security_state(
            "host\tbalanced\tfalse\ttrue\ttrue\ttrue\ttrue\ttrue\t36\n\
             agent\tcoding\tbalanced\ttrue\ttrue\ttrue\ttrue\ttrue\ttrue\ttrue\ttrue\ttrue\ttrue\ttrue\ttrue\ttrue\ttrue\ttrue\ttrue\ttrue\ttrue\tfalse\t-\tfalse\t-\n",
        )
        .unwrap();
        let findings = security_findings(&state);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].id, "TFSEC-037");
        assert_eq!(findings[0].severity, "critical");
    }

    #[test]
    fn security_inspection_targets_the_manifest_container_name() {
        let state = parse_security_state(
            "host\tbalanced\tfalse\tfalse\ttrue\ttrue\ttrue\ttrue\t36\n\\
             agent\thermes-fixture\tbalanced\ttrue\ttrue\ttrue\ttrue\ttrue\ttrue\ttrue\ttrue\ttrue\ttrue\ttrue\ttrue\ttrue\ttrue\ttrue\ttrue\ttrue\tfalse\tfalse\t-\tfalse\t-\n",
        )
        .unwrap();
        let agent = &state.agents[0];

        assert_eq!(agent.name, "hermes-fixture");
        assert!(agent.declared_ports_absent.is_none());
        assert!(agent.declared_live_resources.is_none());
        assert_eq!(
            live_container_security_from_manifest(
                agent,
                Some(&serde_json::json!([])),
                "docker",
                Some("none"),
            ),
            LiveContainerState::Unknown
        );
        assert_eq!(
            inspect_arguments_for_agent("docker", agent),
            ["-n", "docker", "inspect", "hermes-fixture"]
        );
    }

    #[test]
    fn parses_current_live_evidence_manifest_fields() {
        let state = parse_security_state(concat!(
            "host\tbalanced\tfalse\tfalse\ttrue\ttrue\ttrue\ttrue\t36\n",
            "agent\thermes-fixture\tbalanced\ttrue\ttrue\ttrue\ttrue\ttrue\ttrue\ttrue\ttrue\ttrue\ttrue\ttrue\ttrue\ttrue\ttrue\ttrue\ttrue\ttrue\tfalse\tfalse\t-\tfalse\t-",
            "\tnone\t[]\t",
            r#"{"/run":67108864,"/tmp":268435456,"/var/tmp":268435456}"#,
            "\ttrue\t",
            r#"{"memoryBytes":2147483648,"memorySwapBytes":2147483648,"nanoCpus":2000000000,"pidsLimit":512,"nofile":4096,"nproc":512}"#,
            "\n"
        ))
        .unwrap();
        let agent = &state.agents[0];

        assert_eq!(agent.broker_network.as_deref(), Some("none"));
        assert_eq!(
            agent.declared_mounts.as_deref(),
            Some(&[] as &[DeclaredMount])
        );
        assert_eq!(agent.declared_tmpfs, Some(test_declared_tmpfs()));
        assert_eq!(agent.declared_ports_absent, Some(true));
        assert_eq!(
            agent.declared_live_resources,
            Some(test_declared_live_resources())
        );

        let mut ports_declared = agent.clone();
        ports_declared.declared_ports_absent = Some(false);
        assert_eq!(
            live_container_security_from_manifest(&ports_declared, None, "docker", Some("none"),),
            LiveContainerState::Unsafe
        );
    }

    #[test]
    fn dev_manifest_omits_strict_live_desired_fields() {
        let state = parse_security_state(
            "host\tdev\ttrue\tfalse\tfalse\tfalse\tfalse\tfalse\t36\n\
             agent\tdev-fixture\tdev\tfalse\tfalse\tfalse\tfalse\tfalse\tfalse\tfalse\tfalse\tfalse\tfalse\tfalse\tfalse\tfalse\tfalse\tfalse\tfalse\tfalse\tfalse\tfalse\t-\tfalse\t-\t-\t-\t-\t-\t-\n",
        )
        .unwrap();
        let agent = &state.agents[0];

        assert!(agent.broker_network.is_none());
        assert!(agent.declared_mounts.is_none());
        assert!(agent.declared_tmpfs.is_none());
        assert!(agent.declared_ports_absent.is_none());
        assert!(agent.declared_live_resources.is_none());
        assert_eq!(
            live_container_security_from_manifest(
                agent,
                Some(&serde_json::json!([])),
                "docker",
                Some("none"),
            ),
            LiveContainerState::Unknown
        );
    }

    #[test]
    fn declared_mount_manifest_is_closed_and_typed() {
        assert_eq!(
            parse_declared_mounts(
                r#"["/var/lib/hermes-coding:/home/hermes:rw","/nix/store/config:/config:ro"]"#,
                2,
            )
            .unwrap(),
            Some(vec![
                DeclaredMount {
                    source: "/nix/store/config".into(),
                    destination: "/config".into(),
                    writable: false,
                },
                DeclaredMount {
                    source: "/var/lib/hermes-coding".into(),
                    destination: "/home/hermes".into(),
                    writable: true,
                },
            ])
        );
        assert_eq!(parse_declared_mounts("-", 2).unwrap(), None);
        assert!(parse_declared_mounts(r#"["relative:/target:ro"]"#, 2).is_err());
        assert!(parse_declared_mounts(r#"["/source:/target:shared"]"#, 2).is_err());
    }

    fn test_declared_tmpfs() -> BTreeMap<String, u64> {
        BTreeMap::from([
            ("/run".into(), 64 * 1024 * 1024),
            ("/tmp".into(), 256 * 1024 * 1024),
            ("/var/tmp".into(), 256 * 1024 * 1024),
        ])
    }

    fn test_declared_live_resources() -> DeclaredLiveResources {
        DeclaredLiveResources {
            memory_bytes: 2 * 1024 * 1024 * 1024,
            memory_swap_bytes: 2 * 1024 * 1024 * 1024,
            nano_cpus: 2_000_000_000,
            pids_limit: 512,
            nofile: 4096,
            nproc: 512,
        }
    }

    #[test]
    fn declared_tmpfs_manifest_is_closed_and_typed() {
        assert_eq!(
            parse_declared_tmpfs(
                r#"{"/run":67108864,"/tmp":268435456,"/var/tmp":268435456}"#,
                2,
            )
            .unwrap(),
            Some(test_declared_tmpfs())
        );
        assert_eq!(parse_declared_tmpfs("-", 2).unwrap(), None);
        assert!(parse_declared_tmpfs(r#"{"/run":1,"/tmp":1}"#, 2).is_err());
        assert!(parse_declared_tmpfs(r#"{"/run":1,"/tmp":1,"/var/tmp":0}"#, 2,).is_err());
    }

    #[test]
    fn declared_live_resource_manifest_is_closed_and_typed() {
        let encoded = r#"{"memoryBytes":2147483648,"memorySwapBytes":2147483648,"nanoCpus":2000000000,"pidsLimit":512,"nofile":4096,"nproc":512}"#;
        assert_eq!(
            parse_declared_live_resources(encoded, 2).unwrap(),
            Some(test_declared_live_resources())
        );
        assert_eq!(parse_declared_live_resources("-", 2).unwrap(), None);
        assert!(
            parse_declared_live_resources(
                r#"{"memoryBytes":2147483648,"memorySwapBytes":2147483648,"nanoCpus":2000000000,"pidsLimit":512,"nofile":4096,"nproc":512,"extra":1}"#,
                2,
            )
            .is_err()
        );
        assert!(
            parse_declared_live_resources(
                r#"{"memoryBytes":2147483648,"memorySwapBytes":2147483648,"nanoCpus":2000000001,"pidsLimit":512,"nofile":4096,"nproc":512}"#,
                2,
            )
            .is_err()
        );
        assert!(
            parse_declared_live_resources(
                r#"{"memoryBytes":2147483648,"memorySwapBytes":2147483648,"nanoCpus":2000000000,"pidsLimit":512,"nofile":0,"nproc":512}"#,
                2,
            )
            .is_err()
        );
    }

    #[test]
    fn rejects_insecure_live_container_inspect_state() {
        let declared_mounts = vec![DeclaredMount {
            source: "/var/lib/hermes-coding".into(),
            destination: "/home/hermes".into(),
            writable: true,
        }];
        let declared_tmpfs = test_declared_tmpfs();
        let declared_live_resources = test_declared_live_resources();
        let mut inspect = serde_json::json!([{
            "AppArmorProfile": "docker-default",
            "State": { "Running": true },
            "Config": { "User": "10000:10000" },
            "NetworkSettings": {
                "Networks": { "none": {} },
                "Ports": {}
            },
            "HostConfig": {
                "Privileged": false,
                "PublishAllPorts": false,
                "PortBindings": {},
                "ReadonlyRootfs": true,
                "NetworkMode": "none",
                "Runtime": "runsc",
                "CapAdd": null,
                "CapDrop": ["ALL"],
                "SecurityOpt": ["no-new-privileges"],
                "Memory": 2147483648_u64,
                "MemorySwap": 2147483648_i64,
                "NanoCpus": 2000000000_i64,
                "PidsLimit": 512,
                "Ulimits": [
                    { "Name": "nofile", "Soft": 4096, "Hard": 4096 },
                    { "Name": "nproc", "Soft": 512, "Hard": 512 }
                ],
                "Tmpfs": {
                    "/run": "rw,nosuid,nodev,noexec,size=64m",
                    "/tmp": "rw,nosuid,nodev,noexec,size=256m",
                    "/var/tmp": "rw,nosuid,nodev,noexec,size=256m"
                }
            },
            "Mounts": [
                {
                    "Type": "bind",
                    "Source": "/var/lib/hermes-coding",
                    "Destination": "/home/hermes",
                    "RW": true
                }
            ]
        }]);
        assert_eq!(
            live_container_security(
                &inspect,
                "docker",
                "none",
                &declared_mounts,
                &declared_tmpfs,
                &declared_live_resources,
            ),
            LiveContainerState::Secure
        );
        let mut empty_none_network = inspect.clone();
        empty_none_network[0]["NetworkSettings"]["Networks"] = serde_json::json!({});
        assert_eq!(
            live_container_security(
                &empty_none_network,
                "docker",
                "none",
                &declared_mounts,
                &declared_tmpfs,
                &declared_live_resources,
            ),
            LiveContainerState::Secure
        );

        let mut multiple_containers = inspect.clone();
        multiple_containers
            .as_array_mut()
            .unwrap()
            .push(serde_json::json!({}));
        assert_eq!(
            live_container_security(
                &multiple_containers,
                "docker",
                "none",
                &declared_mounts,
                &declared_tmpfs,
                &declared_live_resources,
            ),
            LiveContainerState::Unknown
        );

        let mut published_all = inspect.clone();
        published_all[0]["HostConfig"]["PublishAllPorts"] = serde_json::json!(true);
        assert_eq!(
            live_container_security(
                &published_all,
                "docker",
                "none",
                &declared_mounts,
                &declared_tmpfs,
                &declared_live_resources,
            ),
            LiveContainerState::Unsafe
        );
        let binding = serde_json::json!({
            "8080/tcp": [{ "HostIp": "0.0.0.0", "HostPort": "8080" }]
        });
        for (scope, field) in [("HostConfig", "PortBindings"), ("NetworkSettings", "Ports")] {
            let mut published_port = inspect.clone();
            published_port[0][scope][field] = binding.clone();
            assert_eq!(
                live_container_security(
                    &published_port,
                    "docker",
                    "none",
                    &declared_mounts,
                    &declared_tmpfs,
                    &declared_live_resources,
                ),
                LiveContainerState::Unsafe,
                "published port in {scope}.{field} was not rejected"
            );
        }
        let mut exposed_unpublished = inspect.clone();
        exposed_unpublished[0]["HostConfig"]["PortBindings"]["80/tcp"] = serde_json::Value::Null;
        exposed_unpublished[0]["NetworkSettings"]["Ports"]["80/tcp"] = serde_json::json!([]);
        assert_eq!(
            live_container_security(
                &exposed_unpublished,
                "docker",
                "none",
                &declared_mounts,
                &declared_tmpfs,
                &declared_live_resources,
            ),
            LiveContainerState::Secure
        );
        for (scope, field) in [("HostConfig", "PortBindings"), ("NetworkSettings", "Ports")] {
            let mut missing_port_field = inspect.clone();
            missing_port_field[0][scope]
                .as_object_mut()
                .unwrap()
                .remove(field);
            assert_eq!(
                live_container_security(
                    &missing_port_field,
                    "docker",
                    "none",
                    &declared_mounts,
                    &declared_tmpfs,
                    &declared_live_resources,
                ),
                LiveContainerState::Unknown,
                "missing {scope}.{field} was not treated as unknown"
            );
        }
        let mut malformed_port_field = inspect.clone();
        malformed_port_field[0]["NetworkSettings"]["Ports"]["80/tcp"] =
            serde_json::json!("unexpected");
        assert_eq!(
            live_container_security(
                &malformed_port_field,
                "docker",
                "none",
                &declared_mounts,
                &declared_tmpfs,
                &declared_live_resources,
            ),
            LiveContainerState::Unknown
        );

        let mut missing_publish_all = inspect.clone();
        missing_publish_all[0]["HostConfig"]
            .as_object_mut()
            .unwrap()
            .remove("PublishAllPorts");
        assert_eq!(
            live_container_security(
                &missing_publish_all,
                "docker",
                "none",
                &declared_mounts,
                &declared_tmpfs,
                &declared_live_resources,
            ),
            LiveContainerState::Unknown
        );
        let mut missing_publish_with_binding = missing_publish_all.clone();
        missing_publish_with_binding[0]["NetworkSettings"]["Ports"] = binding.clone();
        assert_eq!(
            live_container_security(
                &missing_publish_with_binding,
                "docker",
                "none",
                &declared_mounts,
                &declared_tmpfs,
                &declared_live_resources,
            ),
            LiveContainerState::Unsafe
        );

        for (field, value) in [
            ("Memory", serde_json::json!(2147483649_u64)),
            ("MemorySwap", serde_json::json!(2147483649_u64)),
            ("NanoCpus", serde_json::json!(2000000001_u64)),
            ("PidsLimit", serde_json::json!(513_u64)),
        ] {
            let mut drifted_resource = inspect.clone();
            drifted_resource[0]["HostConfig"][field] = value;
            assert_eq!(
                live_container_security(
                    &drifted_resource,
                    "docker",
                    "none",
                    &declared_mounts,
                    &declared_tmpfs,
                    &declared_live_resources,
                ),
                LiveContainerState::Unsafe,
                "resource drift in {field} was not rejected"
            );
        }
        let mut missing_memory_with_pids_drift = inspect.clone();
        missing_memory_with_pids_drift[0]["HostConfig"]
            .as_object_mut()
            .unwrap()
            .remove("Memory");
        missing_memory_with_pids_drift[0]["HostConfig"]["PidsLimit"] = serde_json::json!(513);
        assert_eq!(
            live_container_security(
                &missing_memory_with_pids_drift,
                "docker",
                "none",
                &declared_mounts,
                &declared_tmpfs,
                &declared_live_resources,
            ),
            LiveContainerState::Unsafe
        );

        let mut drifted_nofile = inspect.clone();
        drifted_nofile[0]["HostConfig"]["Ulimits"][0]["Soft"] = serde_json::json!(4097);
        assert_eq!(
            live_container_security(
                &drifted_nofile,
                "docker",
                "none",
                &declared_mounts,
                &declared_tmpfs,
                &declared_live_resources,
            ),
            LiveContainerState::Unsafe
        );
        let mut missing_ulimits = inspect.clone();
        missing_ulimits[0]["HostConfig"]
            .as_object_mut()
            .unwrap()
            .remove("Ulimits");
        assert_eq!(
            live_container_security(
                &missing_ulimits,
                "docker",
                "none",
                &declared_mounts,
                &declared_tmpfs,
                &declared_live_resources,
            ),
            LiveContainerState::Unknown
        );
        let mut missing_ulimits_privileged = missing_ulimits.clone();
        missing_ulimits_privileged[0]["HostConfig"]["Privileged"] = serde_json::json!(true);
        assert_eq!(
            live_container_security(
                &missing_ulimits_privileged,
                "docker",
                "none",
                &declared_mounts,
                &declared_tmpfs,
                &declared_live_resources,
            ),
            LiveContainerState::Unsafe
        );
        for field in ["PidsLimit", "Ulimits"] {
            let mut null_resource_evidence = inspect.clone();
            null_resource_evidence[0]["HostConfig"][field] = serde_json::Value::Null;
            assert_eq!(
                live_container_security(
                    &null_resource_evidence,
                    "docker",
                    "none",
                    &declared_mounts,
                    &declared_tmpfs,
                    &declared_live_resources,
                ),
                LiveContainerState::Unsafe,
                "explicit null {field} evidence was not rejected"
            );
        }

        let mut false_nnp_without_options = inspect.clone();
        false_nnp_without_options[0]["HostConfig"]
            .as_object_mut()
            .unwrap()
            .remove("SecurityOpt");
        false_nnp_without_options[0]["HostConfig"]["NoNewPrivileges"] = serde_json::json!(false);
        assert_eq!(
            live_container_security(
                &false_nnp_without_options,
                "docker",
                "none",
                &declared_mounts,
                &declared_tmpfs,
                &declared_live_resources,
            ),
            LiveContainerState::Unsafe
        );
        let mut true_nnp_without_options = false_nnp_without_options.clone();
        true_nnp_without_options[0]["HostConfig"]["NoNewPrivileges"] = serde_json::json!(true);
        assert_eq!(
            live_container_security(
                &true_nnp_without_options,
                "docker",
                "none",
                &declared_mounts,
                &declared_tmpfs,
                &declared_live_resources,
            ),
            LiveContainerState::Unknown
        );
        let mut null_security_options = inspect.clone();
        null_security_options[0]["HostConfig"]["SecurityOpt"] = serde_json::Value::Null;
        assert_eq!(
            live_container_security(
                &null_security_options,
                "docker",
                "none",
                &declared_mounts,
                &declared_tmpfs,
                &declared_live_resources,
            ),
            LiveContainerState::Unsafe
        );
        for option in ["no-new-privileges:false", "seccomp:unconfined"] {
            let mut unsafe_security_option = inspect.clone();
            unsafe_security_option[0]["HostConfig"]["SecurityOpt"] =
                serde_json::json!(["no-new-privileges", option]);
            assert_eq!(
                live_container_security(
                    &unsafe_security_option,
                    "docker",
                    "none",
                    &declared_mounts,
                    &declared_tmpfs,
                    &declared_live_resources,
                ),
                LiveContainerState::Unsafe,
                "unsafe security option was not rejected: {option}"
            );
        }

        let mut missing_tmpfs = inspect.clone();
        missing_tmpfs[0]["HostConfig"]["Tmpfs"]
            .as_object_mut()
            .unwrap()
            .remove("/run");
        assert_eq!(
            live_container_security(
                &missing_tmpfs,
                "docker",
                "none",
                &declared_mounts,
                &declared_tmpfs,
                &declared_live_resources,
            ),
            LiveContainerState::Unsafe
        );
        let mut extra_tmpfs = inspect.clone();
        extra_tmpfs[0]["HostConfig"]["Tmpfs"]["/cache"] =
            serde_json::json!("rw,nosuid,nodev,noexec,size=1m");
        assert_eq!(
            live_container_security(
                &extra_tmpfs,
                "docker",
                "none",
                &declared_mounts,
                &declared_tmpfs,
                &declared_live_resources,
            ),
            LiveContainerState::Unsafe
        );
        for options in [
            "rw,nosuid,nodev,exec,size=64m",
            "rw,suid,nodev,noexec,size=64m",
            "rw,nosuid,noexec,size=64m",
            "rw,nosuid,nodev,noexec,size=65m",
        ] {
            let mut drifted_tmpfs = inspect.clone();
            drifted_tmpfs[0]["HostConfig"]["Tmpfs"]["/run"] = serde_json::json!(options);
            assert_eq!(
                live_container_security(
                    &drifted_tmpfs,
                    "docker",
                    "none",
                    &declared_mounts,
                    &declared_tmpfs,
                    &declared_live_resources,
                ),
                LiveContainerState::Unsafe,
                "tmpfs drift was not rejected: {options}"
            );
        }
        let mut malformed_tmpfs = inspect.clone();
        malformed_tmpfs[0]["HostConfig"]["Tmpfs"]["/run"] =
            serde_json::json!("rw,nosuid,nodev,noexec,mystery,size=64m");
        assert_eq!(
            live_container_security(
                &malformed_tmpfs,
                "docker",
                "none",
                &declared_mounts,
                &declared_tmpfs,
                &declared_live_resources,
            ),
            LiveContainerState::Unknown
        );
        let mut unknown_then_unsafe_tmpfs = inspect.clone();
        unknown_then_unsafe_tmpfs[0]["HostConfig"]["Tmpfs"]["/run"] =
            serde_json::json!(["unrecognized"]);
        unknown_then_unsafe_tmpfs[0]["HostConfig"]["Tmpfs"]["/tmp"] =
            serde_json::json!("rw,nosuid,nodev,exec,size=256m");
        assert_eq!(
            live_container_security(
                &unknown_then_unsafe_tmpfs,
                "docker",
                "none",
                &declared_mounts,
                &declared_tmpfs,
                &declared_live_resources,
            ),
            LiveContainerState::Unsafe
        );
        let mut unknown_option_then_exec = inspect.clone();
        unknown_option_then_exec[0]["HostConfig"]["Tmpfs"]["/run"] =
            serde_json::json!("mystery,exec,rw,nosuid,nodev,noexec,size=64m");
        assert_eq!(
            live_container_security(
                &unknown_option_then_exec,
                "docker",
                "none",
                &declared_mounts,
                &declared_tmpfs,
                &declared_live_resources,
            ),
            LiveContainerState::Unsafe
        );
        let mut null_tmpfs = inspect.clone();
        null_tmpfs[0]["HostConfig"]["Tmpfs"] = serde_json::Value::Null;
        assert_eq!(
            live_container_security(
                &null_tmpfs,
                "docker",
                "none",
                &declared_mounts,
                &declared_tmpfs,
                &declared_live_resources,
            ),
            LiveContainerState::Unsafe
        );

        inspect[0]["State"]["Running"] = serde_json::json!(false);
        assert_eq!(
            live_container_security(
                &inspect,
                "docker",
                "none",
                &declared_mounts,
                &declared_tmpfs,
                &declared_live_resources,
            ),
            LiveContainerState::Unknown
        );
        inspect[0]["State"]["Running"] = serde_json::json!(true);
        let mut missing_state = inspect.clone();
        missing_state[0].as_object_mut().unwrap().remove("State");
        assert_eq!(
            live_container_security(
                &missing_state,
                "docker",
                "none",
                &declared_mounts,
                &declared_tmpfs,
                &declared_live_resources,
            ),
            LiveContainerState::Unknown
        );

        inspect[0]["HostConfig"]["NetworkMode"] = serde_json::json!("tf-unreviewed");
        assert_eq!(
            live_container_security(
                &inspect,
                "docker",
                "none",
                &declared_mounts,
                &declared_tmpfs,
                &declared_live_resources,
            ),
            LiveContainerState::Unsafe
        );
        inspect[0]["HostConfig"]["NetworkMode"] = serde_json::json!("none");
        inspect[0]["HostConfig"]["Privileged"] = serde_json::json!(true);
        assert_eq!(
            live_container_security(
                &inspect,
                "docker",
                "none",
                &declared_mounts,
                &declared_tmpfs,
                &declared_live_resources,
            ),
            LiveContainerState::Unsafe
        );
        inspect[0]["HostConfig"]["Privileged"] = serde_json::json!(false);
        let mut missing_readonly_privileged = inspect.clone();
        missing_readonly_privileged[0]["HostConfig"]
            .as_object_mut()
            .unwrap()
            .remove("ReadonlyRootfs");
        missing_readonly_privileged[0]["HostConfig"]["Privileged"] = serde_json::json!(true);
        assert_eq!(
            live_container_security(
                &missing_readonly_privileged,
                "docker",
                "none",
                &declared_mounts,
                &declared_tmpfs,
                &declared_live_resources,
            ),
            LiveContainerState::Unsafe
        );

        inspect[0]["Config"]["User"] = serde_json::json!("0:10000");
        assert_eq!(
            live_container_security(
                &inspect,
                "docker",
                "none",
                &declared_mounts,
                &declared_tmpfs,
                &declared_live_resources,
            ),
            LiveContainerState::Unsafe
        );
        inspect[0]["Config"]["User"] = serde_json::json!("10000:10000");

        inspect[0]["NetworkSettings"]["Networks"]["unexpected"] = serde_json::json!({});
        assert_eq!(
            live_container_security(
                &inspect,
                "docker",
                "none",
                &declared_mounts,
                &declared_tmpfs,
                &declared_live_resources,
            ),
            LiveContainerState::Unsafe
        );
        inspect[0]["NetworkSettings"]["Networks"]
            .as_object_mut()
            .unwrap()
            .remove("unexpected");

        inspect[0]["Mounts"][0]["Destination"] = serde_json::json!("/unexpected");
        assert_eq!(
            live_container_security(
                &inspect,
                "docker",
                "none",
                &declared_mounts,
                &declared_tmpfs,
                &declared_live_resources,
            ),
            LiveContainerState::Unsafe
        );
        inspect[0]["Mounts"][0]["Destination"] = serde_json::json!("/home/hermes");
        inspect[0]["Mounts"][0]["RW"] = serde_json::json!(false);
        assert_eq!(
            live_container_security(
                &inspect,
                "docker",
                "none",
                &declared_mounts,
                &declared_tmpfs,
                &declared_live_resources,
            ),
            LiveContainerState::Unsafe
        );
        inspect[0]["Mounts"][0]["RW"] = serde_json::json!(true);

        inspect[0]["Mounts"][0]["Source"] = serde_json::json!("/var/run/docker.sock");
        assert_eq!(
            live_container_security(
                &inspect,
                "docker",
                "none",
                &declared_mounts,
                &declared_tmpfs,
                &declared_live_resources,
            ),
            LiveContainerState::Unsafe
        );
    }

    #[test]
    fn parses_podman_live_container_security_state() {
        let declared_mounts = vec![DeclaredMount {
            source: "/var/lib/zeroclaw-assistant".into(),
            destination: "/zeroclaw-data".into(),
            writable: true,
        }];
        let declared_tmpfs = test_declared_tmpfs();
        let declared_live_resources = test_declared_live_resources();
        let mut inspect = serde_json::json!([{
            "AppArmorProfile": "containers-default-0.57.0",
            "OCIRuntime": "/nix/store/fixture/bin/runsc",
            "EffectiveCaps": [],
            "BoundingCaps": [],
            "State": { "Running": true },
            "Config": { "User": "65534:65534" },
            "NetworkSettings": {
                "Networks": { "tf-zeroclaw-assistant": {} },
                "Ports": {}
            },
            "HostConfig": {
                "Privileged": false,
                "PublishAllPorts": false,
                "PortBindings": {},
                "ReadonlyRootfs": true,
                "NetworkMode": "bridge",
                "CapAdd": null,
                "CapDrop": ["ALL"],
                "SecurityOpt": ["no-new-privileges"],
                "Memory": 2147483648_u64,
                "MemorySwap": 2147483648_i64,
                "CpuPeriod": 100000_i64,
                "CpuQuota": 200000_i64,
                "NanoCpus": 2000000000_i64,
                "PidsLimit": 512,
                "Ulimits": [
                    { "Name": "RLIMIT_NOFILE", "Soft": 4096, "Hard": 4096 },
                    { "Name": "RLIMIT_NPROC", "Soft": 512, "Hard": 512 }
                ],
                "Tmpfs": {
                    "/run": "rw,nosuid,nodev,noexec,size=64m,rprivate,tmpcopyup",
                    "/tmp": "rw,nosuid,nodev,noexec,size=256m,rprivate,tmpcopyup",
                    "/var/tmp": "rw,nosuid,nodev,noexec,size=256m,rprivate,tmpcopyup"
                }
            },
            "Mounts": [
                {
                    "Type": "bind",
                    "Source": "/var/lib/zeroclaw-assistant",
                    "Destination": "/zeroclaw-data",
                    "RW": true
                }
            ]
        }]);
        assert_eq!(
            live_container_security(
                &inspect,
                "podman",
                "tf-zeroclaw-assistant",
                &declared_mounts,
                &declared_tmpfs,
                &declared_live_resources,
            ),
            LiveContainerState::Secure
        );
        let mut missing_podman_propagation = inspect.clone();
        missing_podman_propagation[0]["HostConfig"]["Tmpfs"]["/run"] =
            serde_json::json!("rw,nosuid,nodev,noexec,size=64m,tmpcopyup");
        assert_eq!(
            live_container_security(
                &missing_podman_propagation,
                "podman",
                "tf-zeroclaw-assistant",
                &declared_mounts,
                &declared_tmpfs,
                &declared_live_resources,
            ),
            LiveContainerState::Unsafe
        );
        let mut conflicting_podman_propagation = inspect.clone();
        conflicting_podman_propagation[0]["HostConfig"]["Tmpfs"]["/run"] =
            serde_json::json!("rw,nosuid,nodev,noexec,size=64m,shared,tmpcopyup");
        assert_eq!(
            live_container_security(
                &conflicting_podman_propagation,
                "podman",
                "tf-zeroclaw-assistant",
                &declared_mounts,
                &declared_tmpfs,
                &declared_live_resources,
            ),
            LiveContainerState::Unsafe
        );

        let mut missing_named_networks = inspect.clone();
        missing_named_networks[0]["NetworkSettings"]
            .as_object_mut()
            .unwrap()
            .remove("Networks");
        assert_eq!(
            live_container_security(
                &missing_named_networks,
                "podman",
                "tf-zeroclaw-assistant",
                &declared_mounts,
                &declared_tmpfs,
                &declared_live_resources,
            ),
            LiveContainerState::Unknown
        );
        let mut additional_named_network = inspect.clone();
        additional_named_network[0]["NetworkSettings"]["Networks"]["unexpected"] =
            serde_json::json!({});
        assert_eq!(
            live_container_security(
                &additional_named_network,
                "podman",
                "tf-zeroclaw-assistant",
                &declared_mounts,
                &declared_tmpfs,
                &declared_live_resources,
            ),
            LiveContainerState::Unsafe
        );
        let mut isolated_podman = inspect.clone();
        isolated_podman[0]["HostConfig"]["NetworkMode"] = serde_json::json!("none");
        isolated_podman[0]["NetworkSettings"]
            .as_object_mut()
            .unwrap()
            .remove("Networks");
        assert_eq!(
            live_container_security(
                &isolated_podman,
                "podman",
                "none",
                &declared_mounts,
                &declared_tmpfs,
                &declared_live_resources,
            ),
            LiveContainerState::Secure
        );

        let mut inconsistent_podman_cpu = inspect.clone();
        inconsistent_podman_cpu[0]["HostConfig"]["CpuQuota"] = serde_json::json!(199999);
        assert_eq!(
            live_container_security(
                &inconsistent_podman_cpu,
                "podman",
                "tf-zeroclaw-assistant",
                &declared_mounts,
                &declared_tmpfs,
                &declared_live_resources,
            ),
            LiveContainerState::Unsafe
        );
        let mut drifted_podman_nproc = inspect.clone();
        drifted_podman_nproc[0]["HostConfig"]["Ulimits"][1]["Hard"] = serde_json::json!(513);
        assert_eq!(
            live_container_security(
                &drifted_podman_nproc,
                "podman",
                "tf-zeroclaw-assistant",
                &declared_mounts,
                &declared_tmpfs,
                &declared_live_resources,
            ),
            LiveContainerState::Unsafe
        );

        inspect[0]["EffectiveCaps"] = serde_json::json!(["CAP_NET_RAW"]);
        assert_eq!(
            live_container_security(
                &inspect,
                "podman",
                "tf-zeroclaw-assistant",
                &declared_mounts,
                &declared_tmpfs,
                &declared_live_resources,
            ),
            LiveContainerState::Unsafe
        );
        inspect[0]["EffectiveCaps"] = serde_json::json!([]);
        inspect[0]
            .as_object_mut()
            .unwrap()
            .remove("AppArmorProfile");
        assert_eq!(
            live_container_security(
                &inspect,
                "podman",
                "tf-zeroclaw-assistant",
                &declared_mounts,
                &declared_tmpfs,
                &declared_live_resources,
            ),
            LiveContainerState::Unknown
        );
    }

    #[test]
    fn broker_network_inspect_must_be_exact_internal_bridge() {
        let docker = serde_json::json!([{
            "Name": "tf-coding",
            "Internal": true,
            "Driver": "bridge"
        }]);
        assert_eq!(
            live_broker_network_security(&docker, "docker", "tf-coding"),
            LiveContainerState::Secure
        );
        assert_eq!(
            network_inspect_arguments("docker", "tf-coding"),
            ["-n", "docker", "network", "inspect", "tf-coding"]
        );

        let mut external = docker.clone();
        external[0]["Internal"] = serde_json::json!(false);
        assert_eq!(
            live_broker_network_security(&external, "docker", "tf-coding"),
            LiveContainerState::Unsafe
        );
        let mut missing_internal = docker.clone();
        missing_internal[0]
            .as_object_mut()
            .unwrap()
            .remove("Internal");
        assert_eq!(
            live_broker_network_security(&missing_internal, "docker", "tf-coding"),
            LiveContainerState::Unknown
        );
        let mut missing_name_external = docker.clone();
        missing_name_external[0]
            .as_object_mut()
            .unwrap()
            .remove("Name");
        missing_name_external[0]["Internal"] = serde_json::json!(false);
        assert_eq!(
            live_broker_network_security(&missing_name_external, "docker", "tf-coding"),
            LiveContainerState::Unsafe
        );
        let mut missing_name_wrong_driver = docker.clone();
        missing_name_wrong_driver[0]
            .as_object_mut()
            .unwrap()
            .remove("Name");
        missing_name_wrong_driver[0]["Driver"] = serde_json::json!("host");
        assert_eq!(
            live_broker_network_security(&missing_name_wrong_driver, "docker", "tf-coding"),
            LiveContainerState::Unsafe
        );
        let mut wrong_name = docker.clone();
        wrong_name[0]["Name"] = serde_json::json!("tf-attacker");
        assert_eq!(
            live_broker_network_security(&wrong_name, "docker", "tf-coding"),
            LiveContainerState::Unsafe
        );
        let mut multiple = docker.clone();
        multiple.as_array_mut().unwrap().push(serde_json::json!({
            "Name": "tf-coding",
            "Internal": true,
            "Driver": "bridge"
        }));
        assert_eq!(
            live_broker_network_security(&multiple, "docker", "tf-coding"),
            LiveContainerState::Unsafe
        );

        let podman = serde_json::json!([{
            "name": "tf-coding",
            "internal": true,
            "driver": "bridge"
        }]);
        assert_eq!(
            live_broker_network_security(&podman, "podman", "tf-coding"),
            LiveContainerState::Secure
        );
    }
}
