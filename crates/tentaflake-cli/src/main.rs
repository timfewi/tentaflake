use std::env;
use std::ffi::OsString;
use std::fs;
use std::io::{self, IsTerminal, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Output};
use std::time::SystemTime;

const DEFAULT_CONFIG: &str = "/etc/tentaflake/cli.conf";
const DEFAULT_AGENTS: &str = "/etc/tentaflake/agents.tsv";

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
            Some("agent") if fields.len() == 25 || fields.len() == 26 => {
                agents.push(SecurityAgent {
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
                })
            }
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

    for agent in &state.agents {
        for endpoint in [agent.llm_broker_endpoint, agent.fetch_broker_endpoint]
            .into_iter()
            .flatten()
        {
            match probe_broker_health(endpoint) {
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
        command.args(["-n", &config.backend, "inspect", &agent.name]);
        match command.output() {
            Ok(result) if result.status.success() => {
                let expected_network = if agent.brokered_egress {
                    agent.broker_network.as_deref()
                } else {
                    Some("none")
                };
                let state = expected_network.map_or(LiveContainerState::Unknown, |network| {
                    serde_json::from_slice::<serde_json::Value>(&result.stdout)
                        .ok()
                        .map_or(LiveContainerState::Unknown, |value| {
                            live_container_security(&value, &config.backend, network)
                        })
                });
                match state {
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

fn live_container_security(
    value: &serde_json::Value,
    backend: &str,
    expected_network: &str,
) -> LiveContainerState {
    let Some(container) = value
        .as_array()
        .and_then(|values| values.first())
        .and_then(serde_json::Value::as_object)
    else {
        return LiveContainerState::Unknown;
    };
    let Some(host) = container
        .get("HostConfig")
        .and_then(serde_json::Value::as_object)
    else {
        return LiveContainerState::Unknown;
    };
    let Some(runtime_config) = container
        .get("Config")
        .and_then(serde_json::Value::as_object)
    else {
        return LiveContainerState::Unknown;
    };

    let Some(privileged) = host.get("Privileged").and_then(serde_json::Value::as_bool) else {
        return LiveContainerState::Unknown;
    };
    let Some(read_only) = host
        .get("ReadonlyRootfs")
        .and_then(serde_json::Value::as_bool)
    else {
        return LiveContainerState::Unknown;
    };
    let Some(network) = host.get("NetworkMode").and_then(serde_json::Value::as_str) else {
        return LiveContainerState::Unknown;
    };
    let Some(user) = runtime_config
        .get("User")
        .and_then(serde_json::Value::as_str)
    else {
        return LiveContainerState::Unknown;
    };
    let runtime = if backend == "podman" {
        container
            .get("OCIRuntime")
            .and_then(serde_json::Value::as_str)
    } else {
        host.get("Runtime").and_then(serde_json::Value::as_str)
    };
    let Some(runtime) = runtime else {
        return LiveContainerState::Unknown;
    };
    let Some(security_options) = host
        .get("SecurityOpt")
        .and_then(serde_json::Value::as_array)
    else {
        return LiveContainerState::Unknown;
    };
    let has_no_new_privileges = security_options.iter().any(|value| {
        value
            .as_str()
            .is_some_and(|value| value.contains("no-new-privileges"))
    }) || host
        .get("NoNewPrivileges")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    let has_unconfined = security_options.iter().any(|value| {
        value.as_str().is_some_and(|value| {
            value.contains("seccomp=unconfined") || value.contains("apparmor=unconfined")
        })
    });
    let memory_limited = host
        .get("Memory")
        .and_then(serde_json::Value::as_u64)
        .is_some_and(|value| value > 0);
    let swap_limited = host
        .get("MemorySwap")
        .and_then(serde_json::Value::as_i64)
        .is_some_and(|value| value > 0);
    let pids_limited = host
        .get("PidsLimit")
        .and_then(serde_json::Value::as_i64)
        .is_some_and(|value| value > 0);
    let cpu_limited = host
        .get("NanoCpus")
        .and_then(serde_json::Value::as_i64)
        .is_some_and(|value| value > 0)
        || host
            .get("CpuQuota")
            .and_then(serde_json::Value::as_i64)
            .is_some_and(|value| value > 0);
    let Some(mounts) = container
        .get("Mounts")
        .and_then(serde_json::Value::as_array)
    else {
        return LiveContainerState::Unknown;
    };
    let mounts_known = mounts.iter().all(|mount| {
        mount
            .get("Source")
            .and_then(serde_json::Value::as_str)
            .is_some()
    });
    if !mounts_known {
        return LiveContainerState::Unknown;
    }
    let mounts_safe = mounts.iter().all(|mount| {
        mount
            .get("Source")
            .and_then(serde_json::Value::as_str)
            .is_some_and(|source| !live_mount_source_is_sensitive(source))
    });
    let runtime_is_runsc = runtime == "runsc" || runtime.ends_with("/runsc");
    let apparmor = container
        .get("AppArmorProfile")
        .and_then(serde_json::Value::as_str);

    let capabilities_secure = if backend == "podman" {
        let effective = container
            .get("EffectiveCaps")
            .and_then(serde_json::Value::as_array);
        let bounding = container
            .get("BoundingCaps")
            .and_then(serde_json::Value::as_array);
        match (effective, bounding) {
            (Some(effective), Some(bounding)) => effective.is_empty() && bounding.is_empty(),
            _ => return LiveContainerState::Unknown,
        }
    } else {
        let cap_add_empty = host
            .get("CapAdd")
            .is_some_and(|value| value.is_null() || value.as_array().is_some_and(Vec::is_empty));
        let cap_drop_all = host
            .get("CapDrop")
            .and_then(serde_json::Value::as_array)
            .is_some_and(|values| {
                values.iter().any(|value| {
                    value
                        .as_str()
                        .is_some_and(|value| value.eq_ignore_ascii_case("all"))
                })
            });
        cap_add_empty && cap_drop_all
    };
    let apparmor_secure = match (backend, apparmor) {
        ("docker", Some("docker-default")) => true,
        ("podman", Some(profile)) => !profile.is_empty() && profile != "unconfined",
        _ => return LiveContainerState::Unknown,
    };

    if privileged
        || !read_only
        || network != expected_network
        || !runtime_is_runsc
        || !live_user_is_non_root(user)
        || !capabilities_secure
        || !has_no_new_privileges
        || has_unconfined
        || !memory_limited
        || !swap_limited
        || !pids_limited
        || !cpu_limited
        || !mounts_safe
        || !apparmor_secure
    {
        LiveContainerState::Unsafe
    } else {
        LiveContainerState::Secure
    }
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
    fn rejects_insecure_live_container_inspect_state() {
        let mut inspect = serde_json::json!([{
            "AppArmorProfile": "docker-default",
            "Config": { "User": "10000:10000" },
            "HostConfig": {
                "Privileged": false,
                "ReadonlyRootfs": true,
                "NetworkMode": "none",
                "Runtime": "runsc",
                "CapAdd": null,
                "CapDrop": ["ALL"],
                "SecurityOpt": ["no-new-privileges"],
                "Memory": 2147483648_u64,
                "MemorySwap": 2147483648_i64,
                "NanoCpus": 2000000000_i64,
                "PidsLimit": 512
            },
            "Mounts": [
                { "Source": "/var/lib/hermes-coding" }
            ]
        }]);
        assert_eq!(
            live_container_security(&inspect, "docker", "none"),
            LiveContainerState::Secure
        );
        inspect[0]["HostConfig"]["NetworkMode"] = serde_json::json!("tf-unreviewed");
        assert_eq!(
            live_container_security(&inspect, "docker", "none"),
            LiveContainerState::Unsafe
        );
        inspect[0]["HostConfig"]["NetworkMode"] = serde_json::json!("none");
        inspect[0]["HostConfig"]["Privileged"] = serde_json::json!(true);
        assert_eq!(
            live_container_security(&inspect, "docker", "none"),
            LiveContainerState::Unsafe
        );
        inspect[0]["HostConfig"]["Privileged"] = serde_json::json!(false);
        inspect[0]["Config"]["User"] = serde_json::json!("0:10000");
        assert_eq!(
            live_container_security(&inspect, "docker", "none"),
            LiveContainerState::Unsafe
        );
        inspect[0]["Config"]["User"] = serde_json::json!("10000:10000");
        inspect[0]["Mounts"][0]["Source"] = serde_json::json!("/var/run/docker.sock");
        assert_eq!(
            live_container_security(&inspect, "docker", "none"),
            LiveContainerState::Unsafe
        );
    }

    #[test]
    fn parses_podman_live_container_security_state() {
        let mut inspect = serde_json::json!([{
            "AppArmorProfile": "containers-default-0.57.0",
            "OCIRuntime": "/nix/store/fixture/bin/runsc",
            "EffectiveCaps": [],
            "BoundingCaps": [],
            "Config": { "User": "65534:65534" },
            "HostConfig": {
                "Privileged": false,
                "ReadonlyRootfs": true,
                "NetworkMode": "tf-zeroclaw-assistant",
                "CapAdd": null,
                "CapDrop": ["ALL"],
                "SecurityOpt": ["no-new-privileges"],
                "Memory": 2147483648_u64,
                "MemorySwap": 2147483648_i64,
                "CpuQuota": 200000_i64,
                "PidsLimit": 512
            },
            "Mounts": [
                { "Source": "/var/lib/zeroclaw-assistant" }
            ]
        }]);
        assert_eq!(
            live_container_security(&inspect, "podman", "tf-zeroclaw-assistant"),
            LiveContainerState::Secure
        );
        inspect[0]["EffectiveCaps"] = serde_json::json!(["CAP_NET_RAW"]);
        assert_eq!(
            live_container_security(&inspect, "podman", "tf-zeroclaw-assistant"),
            LiveContainerState::Unsafe
        );
        inspect[0]["EffectiveCaps"] = serde_json::json!([]);
        inspect[0]
            .as_object_mut()
            .unwrap()
            .remove("AppArmorProfile");
        assert_eq!(
            live_container_security(&inspect, "podman", "tf-zeroclaw-assistant"),
            LiveContainerState::Unknown
        );
    }
}
