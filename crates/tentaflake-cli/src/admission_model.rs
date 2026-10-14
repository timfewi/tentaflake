//! Local model transport prerequisites, not provider acceptance or model budget.
//! Secret environment values and open credentials never enter serialized evidence.
use crate::admission_host::{HostPlan, store_path};
use crate::admission_systemd::{UnitObservation, inspect_unit};
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::io::{Read, Seek, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpStream};
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

const MAX_OUTPUT: usize = 1024 * 1024;
const MAX_ENVIRONMENT: u64 = 8192;

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(crate) struct ModelSpec {
    pub schema_version: u32,
    pub unit: String,
    pub executable: PathBuf,
    pub configuration: PathBuf,
    pub environment_file: PathBuf,
    pub network: ModelNetwork,
    pub port: u16,
    pub firewall_table: String,
    pub firewall_ports: Vec<u16>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(crate) struct ModelNetwork {
    pub name: String,
    pub bridge: String,
    pub subnet: String,
    pub gateway: String,
}

#[derive(Clone, Copy)]
pub(crate) enum Stage<'a> {
    BeforeCreate,
    Created(&'a str),
    Running(&'a str),
}

pub(crate) struct ReadyModel {
    pub generation: String,
    pub network_id: String,
    pub environment: BTreeMap<String, String>,
    pub environment_file: File,
    source: Source,
}

impl ReadyModel {
    pub(crate) fn same_source(&self, other: &Self) -> bool {
        self.source == other.source
            && self.environment == other.environment
            && self.generation == other.generation
            && self.network_id == other.network_id
    }
}

#[derive(PartialEq, Eq)]
struct Source {
    spec: ModelSpec,
    unit: UnitObservation,
    process_start: u64,
    process_uid: u32,
    netns: (u64, u64),
    socket_inode: u64,
    environment: FileIdentity,
    ifindex: u32,
    firewall: FirewallIdentity,
}

#[derive(Clone, PartialEq, Eq)]
struct FileIdentity {
    device: u64,
    inode: u64,
    uid: u32,
    gid: u32,
    mode: u32,
    links: u64,
    bytes: u64,
    modified: (i64, i64),
    changed: (i64, i64),
}

impl From<&fs::Metadata> for FileIdentity {
    fn from(value: &fs::Metadata) -> Self {
        Self {
            device: value.dev(),
            inode: value.ino(),
            uid: value.uid(),
            gid: value.gid(),
            mode: value.mode(),
            links: value.nlink(),
            bytes: value.len(),
            modified: (value.mtime(), value.mtime_nsec()),
            changed: (value.ctime(), value.ctime_nsec()),
        }
    }
}

fn hex(value: &str, length: usize) -> bool {
    value.len() == length
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 63
        && value
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        && value.as_bytes()[0].is_ascii_alphanumeric()
}

pub(crate) fn subnet(spec: &ModelSpec) -> Result<Ipv4Addr, String> {
    let address = spec
        .network
        .subnet
        .strip_suffix("/30")
        .and_then(|s| s.parse::<Ipv4Addr>().ok())
        .ok_or("invalid model subnet")?;
    if !address.is_private()
        || u32::from(address) & 3 != 0
        || spec.network.subnet != format!("{address}/30")
        || spec.network.gateway != Ipv4Addr::from(u32::from(address) + 1).to_string()
    {
        return Err("invalid model subnet or gateway".into());
    }
    Ok(address)
}

pub(crate) fn validate(id: &str, spec: &ModelSpec) -> Result<(), String> {
    let bridge_hash = spec.network.bridge.strip_prefix("tfb-").unwrap_or_default();
    if !name(id)
        || spec.schema_version != 1
        || spec.unit != format!("tentaflake-broker-llm-{id}.service")
        || !store_path(&spec.executable)
        || spec.executable.file_name() != Some("tentaflake-broker".as_ref())
        || !store_path(&spec.configuration)
        || spec.executable.as_os_str().len() > 4096
        || spec.configuration.as_os_str().len() > 4096
        || spec.environment_file.as_os_str()
            != format!("/run/tentaflake-broker/{id}/agent.env").as_str()
        || !name(&spec.network.name)
        || !hex(bridge_hash, 8)
        || spec.firewall_table != format!("tentaflake-broker-{bridge_hash}")
        || spec.port == 0
        || spec.firewall_ports.is_empty()
        || spec.firewall_ports.len() > 2
        || !spec.firewall_ports.contains(&spec.port)
        || spec.firewall_ports.contains(&0)
        || spec.firewall_ports.windows(2).any(|p| p[0] >= p[1])
    {
        return Err("invalid model transport declaration".into());
    }
    subnet(spec)?;
    Ok(())
}

fn environment_values(bytes: &[u8], spec: &ModelSpec) -> Result<BTreeMap<String, String>, String> {
    let text = std::str::from_utf8(bytes).map_err(|_| "invalid model environment")?;
    if bytes.len() > MAX_ENVIRONMENT as usize
        || !text.ends_with('\n')
        || text.contains(['\0', '\r'])
    {
        return Err("invalid model environment".into());
    }
    let mut values = BTreeMap::new();
    for line in text.strip_suffix('\n').unwrap_or_default().split('\n') {
        let (key, value) = line.split_once('=').ok_or("invalid model environment")?;
        if values.insert(key.to_string(), value.to_string()).is_some() {
            return Err("duplicate model environment key".into());
        }
    }
    let url = format!("http://{}:{}/v1", spec.network.gateway, spec.port);
    if values.len() != 4
        || values
            .get("TENTAFLAKE_BROKER_TOKEN")
            .is_none_or(|value| !hex(value, 64))
        || values.get("OPENAI_API_KEY") != values.get("TENTAFLAKE_BROKER_TOKEN")
        || values.get("OPENAI_BASE_URL") != Some(&url)
        || values.get("TENTAFLAKE_LLM_BASE_URL") != Some(&url)
    {
        return Err("model environment does not match the declared transport".into());
    }
    Ok(values)
}

fn environment_at(
    parent: &File,
    owner: u32,
    spec: &ModelSpec,
) -> Result<(File, FileIdentity, BTreeMap<String, String>), String> {
    // Recheck the supplied anchor using the same traversal as production. Tests
    // supply an explicitly owned descriptor, never a weaker filesystem policy.
    let parent = crate::admission_storage::open_relative(parent, Path::new(""), owner)?;
    let fd = unsafe {
        libc::openat(
            parent.as_raw_fd(),
            c"agent.env".as_ptr(),
            libc::O_RDONLY | libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_NONBLOCK,
        )
    };
    if fd < 0 {
        return Err("model environment is unavailable".into());
    }
    let mut file = unsafe { File::from_raw_fd(fd) };
    let metadata = file
        .metadata()
        .map_err(|_| "model environment is unavailable")?;
    if !metadata.is_file()
        || metadata.uid() != owner
        || metadata.mode() & 0o7777 != 0o400
        || metadata.nlink() != 1
        || metadata.len() == 0
        || metadata.len() > MAX_ENVIRONMENT
    {
        return Err("model environment is not a private owned regular file".into());
    }
    let identity = FileIdentity::from(&metadata);
    let mut bytes = Vec::new();
    (&mut file)
        .take(MAX_ENVIRONMENT + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "model environment read failed")?;
    let values = environment_values(&bytes, spec)?;
    if identity != FileIdentity::from(&file.metadata().map_err(|_| "model environment changed")?) {
        return Err("model environment changed while reading".into());
    }
    file.rewind()
        .map_err(|_| "cannot rewind model environment")?;
    Ok((file, identity, values))
}

fn read_bounded(path: &Path, limit: u64) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    File::open(path)
        .map_err(|_| "model observation unavailable")?
        .take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "model observation unavailable")?;
    if bytes.len() as u64 > limit {
        return Err("model observation exceeded its bound".into());
    }
    Ok(bytes)
}

fn remaining(deadline: Instant) -> Result<Duration, String> {
    deadline
        .checked_duration_since(Instant::now())
        .filter(|d| !d.is_zero())
        .ok_or_else(|| "model observation deadline exceeded".into())
}

fn read_json(tool: &Path, args: &[&str], deadline: Instant) -> Result<Value, String> {
    let mut command = Command::new(tool);
    command.env_clear().args(args);
    let output = crate::output_with_timeout(
        &mut command,
        remaining(deadline)?.min(Duration::from_secs(2)),
    )
    .map_err(|_| "model observation command failed")?;
    if !output.status.success() || output.stdout.len() > MAX_OUTPUT {
        return Err("model observation command failed or exceeded its bound".into());
    }
    serde_json::from_slice(&output.stdout).map_err(|_| "invalid model observation JSON".into())
}

fn exact_keys(value: &Value, required: &[&str], optional: &[&str]) -> bool {
    value.as_object().is_some_and(|object| {
        required.iter().all(|key| object.contains_key(*key))
            && object
                .keys()
                .all(|key| required.contains(&key.as_str()) || optional.contains(&key.as_str()))
    })
}

fn empty_object(value: &Value) -> bool {
    value.is_null() || value.as_object().is_some_and(|v| v.is_empty())
}

fn network(value: &Value, id: &str, spec: &ModelSpec, stage: Stage<'_>) -> Result<String, String> {
    let entries = value
        .as_array()
        .filter(|v| v.len() == 1)
        .ok_or("ambiguous model network")?;
    let network = &entries[0];
    if !exact_keys(
        network,
        &[
            "Name",
            "Id",
            "Created",
            "Scope",
            "Driver",
            "EnableIPv6",
            "IPAM",
            "Internal",
            "Attachable",
            "Ingress",
            "ConfigFrom",
            "ConfigOnly",
            "Containers",
            "Options",
            "Labels",
        ],
        &["EnableIPv4"],
    ) || network["Name"] != spec.network.name
        || network["Scope"] != "local"
        || network["Driver"] != "bridge"
        || network["Internal"] != true
        || network["EnableIPv6"] != false
        || network.get("EnableIPv4").is_some_and(|v| v != true)
        || network["Attachable"] != false
        || network["Ingress"] != false
        || network["ConfigOnly"] != false
        || network["ConfigFrom"] != json!({"Network":""})
        || !empty_object(&network["Labels"])
        || network["Options"] != json!({"com.docker.network.bridge.name":spec.network.bridge})
        || !exact_keys(&network["IPAM"], &["Driver", "Options", "Config"], &[])
        || network["IPAM"]["Driver"] != "default"
        || !empty_object(&network["IPAM"]["Options"])
    {
        return Err("model network settings differ from declaration".into());
    }
    let network_id = network["Id"]
        .as_str()
        .filter(|s| hex(s, 64))
        .ok_or("invalid model network identity")?;
    let ipam = network["IPAM"]["Config"]
        .as_array()
        .filter(|v| v.len() == 1)
        .ok_or("ambiguous model IPAM")?;
    if !exact_keys(
        &ipam[0],
        &["Subnet", "Gateway"],
        &["IPRange", "AuxiliaryAddresses"],
    ) || ipam[0]["Subnet"] != spec.network.subnet
        || ipam[0]["Gateway"] != spec.network.gateway
        || ipam[0].get("IPRange").is_some_and(|v| v != "")
        || ipam[0]
            .get("AuxiliaryAddresses")
            .is_some_and(|v| !empty_object(v))
    {
        return Err("model IPAM differs from declaration".into());
    }
    let peers = network["Containers"]
        .as_object()
        .ok_or("invalid model network peers")?;
    let cid = match stage {
        Stage::BeforeCreate => None,
        Stage::Created(cid) | Stage::Running(cid) => Some(cid),
    };
    if cid.is_some_and(|cid| !hex(cid, 64))
        || peers.len() > usize::from(cid.is_some())
        || matches!(stage, Stage::Running(_)) && peers.len() != 1
    {
        return Err("model network has unexpected peers".into());
    }
    for (peer_id, peer) in peers {
        let address = Ipv4Addr::from(u32::from(subnet(spec)?) + 2);
        if Some(peer_id.as_str()) != cid
            || !exact_keys(
                peer,
                &[
                    "Name",
                    "EndpointID",
                    "MacAddress",
                    "IPv4Address",
                    "IPv6Address",
                ],
                &[],
            )
            || peer["Name"] != id
            || peer["IPv4Address"] != format!("{address}/30")
            || peer["IPv6Address"] != ""
            || peer["EndpointID"].as_str().is_none_or(|s| !hex(s, 64))
            || peer["MacAddress"]
                .as_str()
                .is_none_or(|s| s.split(':').count() != 6 || !s.split(':').all(|p| hex(p, 2)))
        {
            return Err("model network endpoint differs from owned container".into());
        }
    }
    Ok(network_id.into())
}

fn bridge(value: &Value, spec: &ModelSpec) -> Result<u32, String> {
    let entries = value
        .as_array()
        .filter(|v| v.len() == 1)
        .ok_or("ambiguous model bridge")?;
    let bridge = &entries[0];
    let index = bridge["ifindex"]
        .as_u64()
        .and_then(|n| u32::try_from(n).ok())
        .filter(|n| *n > 0)
        .ok_or("invalid model bridge index")?;
    let flags = bridge["flags"]
        .as_array()
        .ok_or("missing model bridge flags")?;
    let addresses = bridge["addr_info"]
        .as_array()
        .ok_or("missing model bridge addresses")?;
    if bridge["ifname"] != spec.network.bridge
        || bridge["link_type"] != "ether"
        || bridge["linkinfo"]["info_kind"] != "bridge"
        || !flags.contains(&json!("UP"))
        || addresses.len() != 1
        || addresses[0]["family"] != "inet"
        || addresses[0]["local"] != spec.network.gateway
        || addresses[0]["prefixlen"] != 30
        || addresses[0]["scope"] != "global"
    {
        return Err("live model bridge differs from declaration".into());
    }
    Ok(index)
}

#[derive(PartialEq, Eq)]
struct FirewallIdentity {
    table: u64,
    chains: BTreeMap<String, u64>,
    rules: BTreeMap<String, Vec<u64>>,
}

fn handle(value: &Value) -> Result<u64, String> {
    value["handle"]
        .as_u64()
        .filter(|n| *n > 0)
        .ok_or_else(|| "missing firewall object identity".into())
}

fn nft_entries(value: &Value) -> Result<&Vec<Value>, String> {
    if !exact_keys(value, &["nftables"], &[]) {
        return Err("invalid firewall observation".into());
    }
    value["nftables"]
        .as_array()
        .filter(|v| v.len() <= 4096)
        .ok_or_else(|| "unbounded firewall observation".into())
}

fn metainfo(value: &Value) -> bool {
    exact_keys(
        value,
        &["version", "release_name", "json_schema_version"],
        &[],
    ) && value["version"].as_str().is_some_and(|v| v.len() <= 64)
        && value["release_name"]
            .as_str()
            .is_some_and(|v| v.len() <= 128)
        && value["json_schema_version"] == 1
}

fn no_flowtables(value: &Value) -> Result<(), String> {
    let mut metadata = false;
    for object in nft_entries(value)? {
        if !exact_keys(object, &["metainfo"], &[]) || metadata || !metainfo(&object["metainfo"]) {
            return Err("flowtable acceleration or unknown firewall object is present".into());
        }
        metadata = true;
    }
    Ok(())
}

fn firewall(value: &Value, spec: &ModelSpec) -> Result<FirewallIdentity, String> {
    let mut identity = FirewallIdentity {
        table: 0,
        chains: BTreeMap::new(),
        rules: BTreeMap::new(),
    };
    let bridge_match =
        json!({"match":{"op":"==","left":{"meta":{"key":"iifname"}},"right":spec.network.bridge}});
    let source = spec
        .network
        .subnet
        .strip_suffix("/30")
        .ok_or("invalid model subnet")?;
    let allow = vec![
        bridge_match.clone(),
        json!({"match":{"op":"==","left":{"payload":{"protocol":"ip","field":"saddr"}},"right":{"prefix":{"addr":source,"len":30}}}}),
        json!({"match":{"op":"==","left":{"payload":{"protocol":"ip","field":"daddr"}},"right":spec.network.gateway}}),
        json!({"match":{"op":"==","left":{"payload":{"protocol":"tcp","field":"dport"}},"right":{"set":spec.firewall_ports}}}),
        json!({"accept":null}),
    ];
    let drop = vec![bridge_match, json!({"drop":null})];
    let mut expressions: BTreeMap<String, Vec<Vec<Value>>> = BTreeMap::new();
    let mut metadata = false;
    for object in nft_entries(value)? {
        let map = object
            .as_object()
            .filter(|v| v.len() == 1)
            .ok_or("invalid firewall object")?;
        let (kind, item) = map.iter().next().ok_or("missing firewall object")?;
        if kind == "metainfo" {
            if metadata || !metainfo(item) {
                return Err("invalid firewall metadata".into());
            }
            metadata = true;
            continue;
        }
        if item["family"] != "inet" {
            return Err("unexpected model firewall family".into());
        }
        match kind.as_str() {
            "table" => {
                if identity.table != 0
                    || !exact_keys(item, &["family", "name", "handle"], &[])
                    || item["name"] != spec.firewall_table
                {
                    return Err("unexpected model firewall table".into());
                }
                identity.table = handle(item)?;
            }
            "chain" => {
                let chain = item["name"].as_str().ok_or("missing model chain name")?;
                if !exact_keys(
                    item,
                    &[
                        "family", "table", "name", "handle", "type", "hook", "prio", "policy",
                    ],
                    &[],
                ) || !matches!(chain, "input" | "forward")
                    || item["table"] != spec.firewall_table
                    || item["type"] != "filter"
                    || item["hook"] != chain
                    || item["prio"] != -10
                    || item["policy"] != "accept"
                    || identity
                        .chains
                        .insert(chain.into(), handle(item)?)
                        .is_some()
                {
                    return Err("unexpected model firewall chain".into());
                }
            }
            "rule" => {
                let chain = item["chain"]
                    .as_str()
                    .filter(|s| matches!(*s, "input" | "forward"))
                    .ok_or("unexpected model rule chain")?;
                if !exact_keys(item, &["family", "table", "chain", "handle", "expr"], &[])
                    || item["table"] != spec.firewall_table
                {
                    return Err("unexpected model firewall rule".into());
                }
                let mut expr = item["expr"]
                    .as_array()
                    .filter(|v| v.len() <= 8)
                    .ok_or("invalid model firewall expressions")?
                    .clone();
                // nft may render a singleton anonymous port set as a scalar.
                if spec.firewall_ports.len() == 1
                    && expr
                        .get(3)
                        .is_some_and(|v| v["match"]["right"] == spec.firewall_ports[0])
                {
                    expr[3]["match"]["right"] = json!({"set":spec.firewall_ports});
                }
                identity
                    .rules
                    .entry(chain.into())
                    .or_default()
                    .push(handle(item)?);
                expressions.entry(chain.into()).or_default().push(expr);
            }
            _ => return Err("unexpected model firewall object".into()),
        }
    }
    if identity.table == 0
        || identity.chains.len() != 2
        || expressions
            != BTreeMap::from([
                ("input".into(), vec![allow, drop.clone()]),
                ("forward".into(), vec![drop]),
            ])
    {
        return Err("model firewall rules differ from declaration".into());
    }
    let handles: Vec<_> = identity.rules.values().flatten().collect();
    if handles.iter().collect::<BTreeSet<_>>().len() != handles.len() {
        return Err("duplicate model rule identity".into());
    }
    Ok(identity)
}

fn process_ticks(bytes: &[u8]) -> Result<u64, String> {
    std::str::from_utf8(bytes)
        .ok()
        .and_then(|s| s.rsplit_once(')'))
        .and_then(|(_, tail)| tail.split_whitespace().nth(19))
        .and_then(|v| v.parse::<u64>().ok())
        .filter(|v| *v > 0)
        .ok_or_else(|| "invalid broker process identity".into())
}

fn process(unit: &UnitObservation, spec: &ModelSpec) -> Result<(u64, u32, (u64, u64)), String> {
    let service = unit
        .service
        .as_ref()
        .ok_or("missing broker service observation")?;
    let expected = vec![
        spec.executable.to_string_lossy().into_owned(),
        "--config".into(),
        spec.configuration.to_string_lossy().into_owned(),
    ];
    if unit.id != spec.unit
        || unit.load_state != "loaded"
        || unit.active_state != "active"
        || unit.sub_state != "running"
        || unit.job.id != 0
        || unit.need_daemon_reload
        || unit.invocation_id == [0; 16]
        || service.main_pid == 0
        || service.control_pid != 0
        || service.exec_start.len() != 1
        || service.exec_start[0].ignore_failure
        || service.exec_start[0].path != expected[0]
        || service.exec_start[0].argv != expected
        || service.control_group != format!("/system.slice/{}", spec.unit)
    {
        return Err("exact broker unit is not ready".into());
    }
    let root = PathBuf::from(format!("/proc/{}", service.main_pid));
    let uid = fs::metadata(&root)
        .map_err(|_| "broker process disappeared")?
        .uid();
    let start = process_ticks(&read_bounded(&root.join("stat"), 4096)?)?;
    if fs::read_link(root.join("exe")).map_err(|_| "broker executable unavailable")?
        != spec.executable
    {
        return Err("broker executable differs from declaration".into());
    }
    let cmdline = read_bounded(&root.join("cmdline"), 8192)?;
    let mut argv = expected.join("\0").into_bytes();
    argv.push(0);
    if cmdline != argv
        || read_bounded(&root.join("cgroup"), 4096)?
            != format!("0::{}\n", service.control_group).as_bytes()
    {
        return Err("broker process arguments or cgroup differ from declaration".into());
    }
    let host = fs::metadata("/proc/1/ns/net").map_err(|_| "host network namespace unavailable")?;
    let own =
        fs::metadata("/proc/self/ns/net").map_err(|_| "collector network namespace unavailable")?;
    let netns =
        fs::metadata(root.join("ns/net")).map_err(|_| "broker network namespace unavailable")?;
    if (netns.dev(), netns.ino()) != (host.dev(), host.ino())
        || (own.dev(), own.ino()) != (host.dev(), host.ino())
    {
        return Err("broker does not share the host network namespace".into());
    }
    Ok((start, uid, (netns.dev(), netns.ino())))
}

fn listen_inodes(
    bytes: &[u8],
    ipv6: bool,
    gateway: Ipv4Addr,
    port: u16,
) -> Result<Vec<(u64, u32)>, String> {
    let text = std::str::from_utf8(bytes).map_err(|_| "invalid kernel listener observation")?;
    let mut lines = text.lines();
    if lines
        .next()
        .is_none_or(|s| !s.contains("local_address") || !s.contains("inode"))
    {
        return Err("missing kernel socket header".into());
    }
    let mut found = Vec::new();
    let address = format!("{:08X}", u32::from_le_bytes(gateway.octets()));
    let mapped = format!("0000000000000000FFFF0000{address}");
    for (index, line) in lines.enumerate() {
        if index >= 8192 || line.len() > 1024 {
            return Err("kernel socket inventory exceeds bound".into());
        }
        let fields: Vec<_> = line.split_whitespace().collect();
        if fields.len() < 10 {
            return Err("truncated kernel socket inventory".into());
        }
        let (local, local_port) = fields[1]
            .split_once(':')
            .ok_or("invalid kernel socket address")?;
        if local.len() != if ipv6 { 32 } else { 8 } || !local.bytes().all(|v| v.is_ascii_hexdigit())
        {
            return Err("invalid kernel socket address".into());
        }
        let local_port =
            u16::from_str_radix(local_port, 16).map_err(|_| "invalid kernel socket port")?;
        if fields[3] != "0A" || local_port != port {
            continue;
        }
        if local.bytes().all(|v| v == b'0') || ipv6 && local.eq_ignore_ascii_case(&mapped) {
            return Err("wildcard or IPv6 alias broker listener is present".into());
        }
        if !ipv6 && local.eq_ignore_ascii_case(&address) {
            let inode = fields[9]
                .parse::<u64>()
                .ok()
                .filter(|n| *n > 0)
                .ok_or("invalid broker socket inode")?;
            let uid = fields[7]
                .parse::<u32>()
                .map_err(|_| "invalid broker socket owner")?;
            found.push((inode, uid));
        }
    }
    Ok(found)
}

fn listener(pid: u32, uid: u32, spec: &ModelSpec) -> Result<u64, String> {
    let gateway = spec
        .network
        .gateway
        .parse()
        .map_err(|_| "invalid broker gateway")?;
    let found = listen_inodes(
        &read_bounded(Path::new("/proc/net/tcp"), MAX_OUTPUT as u64)?,
        false,
        gateway,
        spec.port,
    )?;
    listen_inodes(
        &read_bounded(Path::new("/proc/net/tcp6"), MAX_OUTPUT as u64)?,
        true,
        gateway,
        spec.port,
    )?;
    if found.len() != 1 || found[0].1 != uid {
        return Err("broker listener is absent or ambiguous".into());
    }
    let expected = format!("socket:[{}]", found[0].0);
    let mut owned = false;
    for (index, entry) in fs::read_dir(format!("/proc/{pid}/fd"))
        .map_err(|_| "broker descriptor inventory unavailable")?
        .enumerate()
    {
        if index >= 4096 {
            return Err("broker descriptor inventory exceeds bound".into());
        }
        let entry = entry.map_err(|_| "broker descriptor inventory incomplete")?;
        match fs::read_link(entry.path()) {
            Ok(link) => {
                if link.as_os_str() == expected.as_str() {
                    owned = true;
                }
            }
            // Concurrent completed client connections may close their FDs.
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return Err("broker descriptor inventory unavailable".into()),
        }
    }
    if !owned {
        return Err("broker does not own the exact listener".into());
    }
    Ok(found[0].0)
}

fn observe(
    plan: &HostPlan,
    id: &str,
    spec: &ModelSpec,
    stage: Stage<'_>,
    deadline: Instant,
) -> Result<ReadyModel, String> {
    remaining(deadline)?;
    let unit = inspect_unit(&plan.tools.busctl, &spec.unit)
        .map_err(|_| "broker unit observation failed")?;
    let (process_start, process_uid, netns) = process(&unit, spec)?;
    let socket_inode = listener(
        unit.service
            .as_ref()
            .ok_or("missing broker process")?
            .main_pid,
        process_uid,
        spec,
    )?;
    let parent = crate::admission_storage::open_root(
        spec.environment_file
            .parent()
            .ok_or("invalid environment path")?,
        0,
    )
    .map_err(|_| "model environment ancestry is untrusted")?;
    let (environment_file, environment_identity, environment) = environment_at(&parent, 0, spec)?;
    let network_id = network(
        &read_json(
            &plan.tools.docker,
            &[
                "--host=unix:///run/docker.sock",
                "--config=/var/empty",
                "network",
                "inspect",
                &spec.network.name,
            ],
            deadline,
        )?,
        id,
        spec,
        stage,
    )?;
    let ip = plan
        .tools
        .ip
        .as_ref()
        .ok_or("model IP observation tool unavailable")?;
    let nft = plan
        .tools
        .nft
        .as_ref()
        .ok_or("model firewall observation tool unavailable")?;
    if !store_path(ip)
        || ip.file_name() != Some("ip".as_ref())
        || !store_path(nft)
        || nft.file_name() != Some("nft".as_ref())
    {
        return Err("invalid model observation tool".into());
    }
    let ifindex = bridge(
        &read_json(
            ip,
            &[
                "-j",
                "-details",
                "address",
                "show",
                "dev",
                &spec.network.bridge,
            ],
            deadline,
        )?,
        spec,
    )?;
    for family in ["ip", "ip6", "inet"] {
        no_flowtables(&read_json(
            nft,
            &["--json", "--numeric", "list", "flowtables", family],
            deadline,
        )?)?;
    }
    let firewall = firewall(
        &read_json(
            nft,
            &[
                "--json",
                "--numeric",
                "list",
                "table",
                "inet",
                &spec.firewall_table,
            ],
            deadline,
        )?,
        spec,
    )?;
    let invocation: String = unit
        .invocation_id
        .iter()
        .map(|v| format!("{v:02x}"))
        .collect();
    let generation = format!("{invocation}.{network_id}.{ifindex:x}.{:x}", firewall.table);
    remaining(deadline)?;
    Ok(ReadyModel {
        generation,
        network_id,
        environment,
        environment_file,
        source: Source {
            spec: spec.clone(),
            unit,
            process_start,
            process_uid,
            netns,
            socket_inode,
            environment: environment_identity,
            ifindex,
            firewall,
        },
    })
}

fn probe(
    spec: &ModelSpec,
    environment: &BTreeMap<String, String>,
    deadline: Instant,
) -> Result<(), String> {
    let endpoint = SocketAddr::from((
        spec.network
            .gateway
            .parse::<Ipv4Addr>()
            .map_err(|_| "invalid model gateway")?,
        spec.port,
    ));
    let deadline = deadline.min(Instant::now() + Duration::from_secs(2));
    let mut stream = TcpStream::connect_timeout(&endpoint, remaining(deadline)?)
        .map_err(|_| "authenticated broker readiness unavailable")?;
    stream
        .set_write_timeout(Some(remaining(deadline)?))
        .map_err(|_| "broker probe deadline unavailable")?;
    let token = environment
        .get("TENTAFLAKE_BROKER_TOKEN")
        .ok_or("model credential unavailable")?;
    let request = format!(
        "GET /readyz HTTP/1.1\r\nHost: {endpoint}\r\nAuthorization: Bearer {token}\r\nConnection: close\r\n\r\n"
    );
    stream
        .write_all(request.as_bytes())
        .map_err(|_| "authenticated broker readiness unavailable")?;
    let mut bytes = Vec::new();
    let mut chunk = [0; 1024];
    loop {
        stream
            .set_read_timeout(Some(remaining(deadline)?))
            .map_err(|_| "broker probe deadline unavailable")?;
        match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(length) if bytes.len() + length <= 8192 => bytes.extend_from_slice(&chunk[..length]),
            _ => return Err("authenticated broker readiness unavailable".into()),
        }
    }
    if !crate::admission_evidence::broker_response_ready(&bytes) {
        return Err("authenticated broker is not ready".into());
    }
    Ok(())
}

pub(crate) fn collect(
    plan: &HostPlan,
    id: &str,
    spec: &ModelSpec,
    stage: Stage<'_>,
) -> Result<ReadyModel, String> {
    if unsafe { libc::geteuid() } != 0 {
        return Err("model collection requires root".into());
    }
    validate(id, spec)?;
    let deadline = Instant::now() + Duration::from_secs(20);
    let before = observe(plan, id, spec, stage, deadline)?;
    probe(spec, &before.environment, deadline)?;
    let after = observe(plan, id, spec, stage, deadline)?;
    if !before.same_source(&after) {
        return Err("model prerequisites changed during readiness probe".into());
    }
    Ok(after)
}

#[cfg(test)]
pub(crate) fn fixture(spec: &ModelSpec) -> ReadyModel {
    use crate::admission_systemd::UnitJob;
    let file = File::open("/dev/null").unwrap();
    ReadyModel {
        generation: "a".repeat(32),
        network_id: "b".repeat(64),
        environment: BTreeMap::from([
            ("OPENAI_API_KEY".into(), "c".repeat(64)),
            ("TENTAFLAKE_BROKER_TOKEN".into(), "c".repeat(64)),
            (
                "OPENAI_BASE_URL".into(),
                format!("http://{}:{}/v1", spec.network.gateway, spec.port),
            ),
            (
                "TENTAFLAKE_LLM_BASE_URL".into(),
                format!("http://{}:{}/v1", spec.network.gateway, spec.port),
            ),
        ]),
        source: Source {
            spec: spec.clone(),
            unit: UnitObservation {
                id: spec.unit.clone(),
                load_state: "loaded".into(),
                active_state: "active".into(),
                sub_state: "running".into(),
                job: UnitJob {
                    id: 0,
                    path: "/".into(),
                },
                fragment_path: "fixture".into(),
                need_daemon_reload: false,
                invocation_id: [1; 16],
                service: None,
                mount: None,
            },
            process_start: 1,
            process_uid: 1,
            netns: (1, 1),
            socket_inode: 1,
            environment: FileIdentity::from(&file.metadata().unwrap()),
            ifindex: 1,
            firewall: FirewallIdentity {
                table: 1,
                chains: BTreeMap::new(),
                rules: BTreeMap::new(),
            },
        },
        environment_file: file,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::{PermissionsExt, symlink};
    use std::sync::atomic::{AtomicU64, Ordering};

    fn spec() -> ModelSpec {
        ModelSpec {
            schema_version: 1,
            unit: "tentaflake-broker-llm-generic-test.service".into(),
            executable: "/nix/store/00000000000000000000000000000000-broker/bin/tentaflake-broker"
                .into(),
            configuration: "/nix/store/00000000000000000000000000000000-broker.json".into(),
            environment_file: "/run/tentaflake-broker/generic-test/agent.env".into(),
            network: ModelNetwork {
                name: "tf-generic-test".into(),
                bridge: "tfb-12345678".into(),
                subnet: "10.203.20.0/30".into(),
                gateway: "10.203.20.1".into(),
            },
            port: 7810,
            firewall_table: "tentaflake-broker-12345678".into(),
            firewall_ports: vec![7810],
        }
    }

    fn environment(spec: &ModelSpec) -> Vec<u8> {
        fixture(spec)
            .environment
            .iter()
            .map(|(k, v)| format!("{k}={v}\n"))
            .collect::<String>()
            .into_bytes()
    }

    struct Directory {
        path: PathBuf,
        file: File,
    }
    impl Directory {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let root = std::env::var_os("CARGO_TARGET_TMPDIR")
                .map(PathBuf::from)
                .unwrap_or_else(|| {
                    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/test-tmp")
                });
            fs::create_dir_all(&root).unwrap();
            let path = root.join(format!(
                "model-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path).unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
            let file = File::open(&path).unwrap();
            Self { path, file }
        }
        fn write(&self, bytes: &[u8]) {
            let path = self.path.join("agent.env");
            fs::write(&path, bytes).unwrap();
            fs::set_permissions(path, fs::Permissions::from_mode(0o400)).unwrap();
        }
    }
    impl Drop for Directory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    #[test]
    fn descriptor_and_environment_refuse_ambiguous_authority() {
        let original = spec();
        validate("generic-test", &original).unwrap();
        let changes: [fn(&mut ModelSpec); 8] = [
            |v| v.schema_version = 2,
            |v| v.firewall_ports = vec![7810, 7810],
            |v| v.firewall_ports = vec![7811],
            |v| v.environment_file = "/run/tentaflake-broker/foreign/agent.env".into(),
            |v| v.network.gateway = "10.203.20.2".into(),
            |v| v.network.subnet = "8.8.8.0/30".into(),
            |v| v.unit = "foreign.service".into(),
            |v| v.firewall_table = "tentaflake-broker-deadbeef".into(),
        ];
        for change in changes {
            let mut value = original.clone();
            change(&mut value);
            assert!(validate("generic-test", &value).is_err());
        }
        let mut wire = json!({"schemaVersion":1,"unit":original.unit,"executable":original.executable,
            "configuration":original.configuration,"environmentFile":original.environment_file,
            "network":{"name":original.network.name,"bridge":original.network.bridge,"subnet":original.network.subnet,"gateway":original.network.gateway},
            "port":original.port,"firewallTable":original.firewall_table,"firewallPorts":original.firewall_ports});
        assert!(serde_json::from_value::<ModelSpec>(wire.clone()).is_ok());
        wire["secret"] = json!("forbidden");
        assert!(serde_json::from_value::<ModelSpec>(wire).is_err());
        let bytes = environment(&original);
        assert!(environment_values(&bytes, &original).is_ok());
        for suffix in [
            "OPENAI_API_KEY=duplicate\n",
            "TENTAFLAKE_FETCH_URL=http://10.203.20.1:7811/v1/fetch\n",
            "\n",
        ] {
            let mut bad = bytes.clone();
            bad.extend_from_slice(suffix.as_bytes());
            assert!(environment_values(&bad, &original).is_err());
        }
        let bad = String::from_utf8(bytes)
            .unwrap()
            .replace(":7810/", ":7811/");
        assert!(environment_values(bad.as_bytes(), &original).is_err());
    }

    #[test]
    fn environment_uses_owned_anchor_and_preserves_descriptor_at_start() {
        let directory = Directory::new();
        let spec = spec();
        directory.write(&environment(&spec));
        let uid = unsafe { libc::geteuid() };
        let (mut opened, identity, values) = environment_at(&directory.file, uid, &spec).unwrap();
        let mut bytes = Vec::new();
        opened.read_to_end(&mut bytes).unwrap();
        assert!(values == environment_values(&bytes, &spec).unwrap());
        assert!(identity == FileIdentity::from(&opened.metadata().unwrap()));
        assert!(environment_at(&directory.file, uid.wrapping_add(1), &spec).is_err());
        let path = directory.path.join("agent.env");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o440)).unwrap();
        assert!(environment_at(&directory.file, uid, &spec).is_err());
        fs::set_permissions(&path, fs::Permissions::from_mode(0o400)).unwrap();
        fs::hard_link(&path, directory.path.join("alias")).unwrap();
        assert!(environment_at(&directory.file, uid, &spec).is_err());
        fs::remove_file(&path).unwrap();
        symlink("alias", &path).unwrap();
        assert!(environment_at(&directory.file, uid, &spec).is_err());
        fs::remove_file(&path).unwrap();
        assert_eq!(
            unsafe { libc::mkfifoat(directory.file.as_raw_fd(), c"agent.env".as_ptr(), 0o400) },
            0
        );
        assert!(environment_at(&directory.file, uid, &spec).is_err());
        fs::remove_file(&path).unwrap();
        directory.write(&vec![b'x'; 8193]);
        assert!(environment_at(&directory.file, uid, &spec).is_err());
        fs::remove_file(&path).unwrap();
        directory.write(&environment(&spec));
        fs::set_permissions(&directory.path, fs::Permissions::from_mode(0o777)).unwrap();
        assert!(environment_at(&directory.file, uid, &spec).is_err());
    }

    #[test]
    fn source_comparison_includes_private_content_and_process_identity() {
        let spec = spec();
        let base = fixture(&spec);
        assert!(base.same_source(&fixture(&spec)));
        let mut changed = fixture(&spec);
        changed
            .environment
            .insert("OPENAI_API_KEY".into(), "d".repeat(64));
        assert!(!base.same_source(&changed));
        let changes: [fn(&mut ReadyModel); 6] = [
            |v| v.source.process_start += 1,
            |v| v.source.socket_inode += 1,
            |v| v.source.environment.inode += 1,
            |v| v.source.firewall.table += 1,
            |v| v.source.unit.invocation_id = [2; 16],
            |v| v.network_id = "e".repeat(64),
        ];
        for change in changes {
            let mut changed = fixture(&spec);
            change(&mut changed);
            assert!(!base.same_source(&changed));
        }
    }

    fn network_fixture(spec: &ModelSpec) -> Value {
        json!([{"Name":spec.network.name,"Id":"b".repeat(64),"Created":"fixture","Scope":"local","Driver":"bridge",
            "EnableIPv6":false,"IPAM":{"Driver":"default","Options":null,"Config":[{"Subnet":spec.network.subnet,"Gateway":spec.network.gateway}]},
            "Internal":true,"Attachable":false,"Ingress":false,"ConfigFrom":{"Network":""},"ConfigOnly":false,"Containers":{},
            "Options":{"com.docker.network.bridge.name":spec.network.bridge},"Labels":{}}])
    }

    #[test]
    fn stage_requires_exact_owned_network_endpoint() {
        let spec = spec();
        let mut value = network_fixture(&spec);
        let cid = "a".repeat(64);
        assert!(network(&value, "generic-test", &spec, Stage::BeforeCreate).is_ok());
        assert!(network(&value, "generic-test", &spec, Stage::Created(&cid)).is_ok());
        assert!(network(&value, "generic-test", &spec, Stage::Running(&cid)).is_err());
        value[0]["Containers"][&cid] = json!({"Name":"generic-test","EndpointID":"c".repeat(64),"MacAddress":"02:42:0a:cb:14:02","IPv4Address":"10.203.20.2/30","IPv6Address":""});
        assert!(network(&value, "generic-test", &spec, Stage::Running(&cid)).is_ok());
        assert!(network(&value, "generic-test", &spec, Stage::BeforeCreate).is_err());
        assert!(
            network(
                &value,
                "generic-test",
                &spec,
                Stage::Created(&"d".repeat(64))
            )
            .is_err()
        );
        value[0]["Containers"][&cid]["IPv4Address"] = json!("10.203.20.3/30");
        assert!(network(&value, "generic-test", &spec, Stage::Running(&cid)).is_err());
        for field in ["EnableIPv6", "Attachable", "Ingress"] {
            let mut bad = network_fixture(&spec);
            bad[0][field] = json!(true);
            assert!(network(&bad, "generic-test", &spec, Stage::BeforeCreate).is_err());
        }
        let mut bad = network_fixture(&spec);
        bad[0]["IPAM"]["Config"]
            .as_array_mut()
            .unwrap()
            .push(json!({"Subnet":"10.0.0.0/8"}));
        assert!(network(&bad, "generic-test", &spec, Stage::BeforeCreate).is_err());
    }

    fn firewall_fixture() -> Value {
        // Pinned owner rule projection; actual kernel rendering is a separate VM
        // check. These mutations test the accepted semantic boundary, not liveness.
        serde_json::from_str(r#"{"nftables":[
            {"metainfo":{"version":"1.1.6","release_name":"fixture","json_schema_version":1}},
            {"table":{"family":"inet","name":"tentaflake-broker-12345678","handle":5}},
            {"chain":{"family":"inet","table":"tentaflake-broker-12345678","name":"input","handle":1,"type":"filter","hook":"input","prio":-10,"policy":"accept"}},
            {"rule":{"family":"inet","table":"tentaflake-broker-12345678","chain":"input","handle":2,"expr":[
                {"match":{"op":"==","left":{"meta":{"key":"iifname"}},"right":"tfb-12345678"}},
                {"match":{"op":"==","left":{"payload":{"protocol":"ip","field":"saddr"}},"right":{"prefix":{"addr":"10.203.20.0","len":30}}}},
                {"match":{"op":"==","left":{"payload":{"protocol":"ip","field":"daddr"}},"right":"10.203.20.1"}},
                {"match":{"op":"==","left":{"payload":{"protocol":"tcp","field":"dport"}},"right":{"set":[7810]}}},
                {"accept":null}]}},
            {"rule":{"family":"inet","table":"tentaflake-broker-12345678","chain":"input","handle":3,"expr":[
                {"match":{"op":"==","left":{"meta":{"key":"iifname"}},"right":"tfb-12345678"}},{"drop":null}]}},
            {"chain":{"family":"inet","table":"tentaflake-broker-12345678","name":"forward","handle":4,"type":"filter","hook":"forward","prio":-10,"policy":"accept"}},
            {"rule":{"family":"inet","table":"tentaflake-broker-12345678","chain":"forward","handle":6,"expr":[
                {"match":{"op":"==","left":{"meta":{"key":"iifname"}},"right":"tfb-12345678"}},{"drop":null}]}}
        ]}"#).unwrap()
    }

    #[test]
    fn firewall_requires_the_complete_restrictive_owner_table() {
        let spec = spec();
        let value = firewall_fixture();
        assert!(firewall(&value, &spec).is_ok());
        let mut scalar = value.clone();
        scalar["nftables"][3]["rule"]["expr"][3]["match"]["right"] = json!(7810);
        assert!(firewall(&scalar, &spec).is_ok());
        let changes: [fn(&mut Value); 8] = [
            |v| v["nftables"][2]["chain"]["prio"] = json!(0),
            |v| v["nftables"][3]["rule"]["expr"][0]["match"]["right"] = json!("foreign"),
            |v| v["nftables"][3]["rule"]["expr"][1]["match"]["right"]["prefix"]["len"] = json!(24),
            |v| v["nftables"][3]["rule"]["expr"][3]["match"]["right"] = json!({"set":[7810,9999]}),
            |v| v["nftables"][4]["rule"]["expr"][1] = json!({"accept":null}),
            |v| v["nftables"].as_array_mut().unwrap().swap(3, 4),
            |v| v["nftables"][1]["table"]["flags"] = json!(["dormant"]),
            |v| {
                v["nftables"].as_array_mut().unwrap().pop();
            },
        ];
        for change in changes {
            let mut bad = value.clone();
            change(&mut bad);
            assert!(firewall(&bad, &spec).is_err());
        }
        assert!(no_flowtables(&json!({"nftables":[]})).is_ok());
        assert!(no_flowtables(&json!({"nftables":[{"flowtable":{"family":"ip","table":"other","name":"offload"}}]})).is_err());
        assert!(no_flowtables(&json!({"nftables":[],"ignored":true})).is_err());
    }

    #[test]
    fn bridge_requires_exact_live_gateway_and_type() {
        let spec = spec();
        let value = json!([{"ifindex":4,"ifname":spec.network.bridge,"flags":["UP","NO-CARRIER"],"link_type":"ether",
            "linkinfo":{"info_kind":"bridge"},"addr_info":[{"family":"inet","local":spec.network.gateway,"prefixlen":30,"scope":"global"}]}]);
        assert_eq!(bridge(&value, &spec).unwrap(), 4);
        let mut bad = value.clone();
        bad[0]["linkinfo"]["info_kind"] = json!("dummy");
        assert!(bridge(&bad, &spec).is_err());
        let mut bad = value.clone();
        bad[0]["addr_info"][0]["local"] = json!("10.203.20.2");
        assert!(bridge(&bad, &spec).is_err());
        let mut bad = value;
        bad[0]["addr_info"]
            .as_array_mut()
            .unwrap()
            .push(json!({"family":"inet6"}));
        assert!(bridge(&bad, &spec).is_err());
    }

    #[test]
    fn real_listener_must_belong_to_observed_process() {
        let socket = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let mut spec = spec();
        spec.network.gateway = "127.0.0.1".into();
        spec.port = socket.local_addr().unwrap().port();
        assert!(listener(std::process::id(), unsafe { libc::geteuid() }, &spec).is_ok());
        assert!(
            listener(
                std::process::id(),
                unsafe { libc::geteuid() }.wrapping_add(1),
                &spec
            )
            .is_err()
        );
        drop(socket);
        assert!(listener(std::process::id(), unsafe { libc::geteuid() }, &spec).is_err());
        let header = "sl local_address rem_address st tx_queue rx_queue tr tm->when retrnsmt uid timeout inode\n";
        let row =
            "0: 00000000:1E82 00000000:0000 0A 00000000:00000000 00:00000000 00000000 1000 0 123\n";
        assert!(
            listen_inodes(
                format!("{header}{row}").as_bytes(),
                false,
                Ipv4Addr::new(10, 203, 20, 1),
                7810
            )
            .is_err()
        );
        assert!(listen_inodes(b"truncated", false, Ipv4Addr::LOCALHOST, 7810).is_err());
    }

    #[test]
    fn readiness_probe_authenticates_and_refuses_unready_without_source_change() {
        let socket = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let mut spec = spec();
        spec.network.gateway = "127.0.0.1".into();
        spec.port = socket.local_addr().unwrap().port();
        let ready = fixture(&spec);
        let server = std::thread::spawn(move || {
            for (status, body, extra_headers) in [
                ("200 OK", r#"{"status":"ready"}"#, ""),
                ("401 Unauthorized", r#"{"status":"ready"}"#, ""),
                ("503 Service Unavailable", r#"{"status":"ready"}"#, ""),
                ("200 OK", r#"{"status":"unready"}"#, ""),
                (
                    "200 OK",
                    r#"{"status":"ready","diagnostic":"private-fixture"}"#,
                    "",
                ),
                ("200 OK", r#"{"status":"ready"}"#, "Content-Length: 18\r\n"),
            ] {
                let (mut stream, _) = socket.accept().unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(2)))
                    .unwrap();
                let mut request = Vec::new();
                let mut buffer = [0; 1024];
                while !request.windows(4).any(|part| part == b"\r\n\r\n") {
                    let length = stream.read(&mut buffer).unwrap();
                    assert!(length > 0);
                    request.extend_from_slice(&buffer[..length]);
                    assert!(request.len() <= 8192);
                }
                let request = std::str::from_utf8(&request).unwrap();
                assert!(request.starts_with("GET /readyz HTTP/1.1\r\n"));
                assert!(request.ends_with("\r\n\r\n"));
                assert!(
                    request.contains(&format!("\r\nAuthorization: Bearer {}\r\n", "c".repeat(64)))
                );
                write!(stream,"HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n{extra_headers}Connection: close\r\n\r\n{body}",body.len()).unwrap();
            }
        });
        assert!(
            probe(
                &spec,
                &ready.environment,
                Instant::now() + Duration::from_secs(2)
            )
            .is_ok()
        );
        for _ in 0..5 {
            assert!(ready.same_source(&fixture(&spec)));
            let error = probe(
                &spec,
                &ready.environment,
                Instant::now() + Duration::from_secs(2),
            )
            .unwrap_err();
            assert!(!error.contains(&"c".repeat(64)));
            assert!(!error.contains("private-fixture"));
        }
        server.join().unwrap();
    }

    #[test]
    fn readiness_probe_deadline_bounds_a_silent_endpoint() {
        let socket = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let mut spec = spec();
        spec.network.gateway = "127.0.0.1".into();
        spec.port = socket.local_addr().unwrap().port();
        let ready = fixture(&spec);
        let (release, wait) = std::sync::mpsc::channel();
        let server = std::thread::spawn(move || {
            let (_stream, _) = socket.accept().unwrap();
            // Keep the connection open until the client has enforced its own
            // deadline, with an independent bound if that behavior regresses.
            let _ = wait.recv_timeout(Duration::from_secs(2));
        });
        let started = Instant::now();
        let error = probe(
            &spec,
            &ready.environment,
            started + Duration::from_millis(100),
        )
        .unwrap_err();
        let elapsed = started.elapsed();
        let _ = release.send(());
        server.join().unwrap();
        assert!(elapsed < Duration::from_secs(1));
        assert!(!error.contains(&"c".repeat(64)));
    }
}
