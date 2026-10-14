//! Authenticated local Research startup prerequisites. These observations do not
//! reserve job capacity or establish provider, browser, or Internet health.
use crate::admission_host::{HostPlan, open_store_file, store_path};
use crate::admission_systemd::{self, UnitObservation};
use serde::Deserialize;
use std::collections::BTreeSet;
use std::ffi::CString;
use std::fs::{self, File};
use std::io::Read;
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::fs::{FileTypeExt, MetadataExt};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(crate) struct ResearchSpec {
    pub schema_version: u32,
    pub client: ClientSpec,
    pub relay: RelaySpec,
    pub service: ProcessSpec,
    pub egress: ProcessSpec,
    pub controller: ProcessSpec,
    pub control_path: PathBuf,
    pub vpn_interface: String,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(crate) struct ClientSpec {
    pub executable: PathBuf,
    pub closure_paths: Vec<PathBuf>,
    pub socket_destination: PathBuf,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(crate) struct RelaySpec {
    pub socket_unit: String,
    pub socket_path: PathBuf,
    pub template: ProcessSpec,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(crate) struct ProcessSpec {
    pub unit: String,
    pub executable: PathBuf,
    pub argv: Vec<String>,
    pub uid: u32,
    pub root_directory: PathBuf,
    pub slice: String,
    pub bind_read_only_paths: Vec<String>,
    pub configuration: Option<Configuration>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(crate) struct Configuration {
    pub source: PathBuf,
    pub destination: PathBuf,
}

pub(crate) struct ReadyResearch {
    pub generation: String,
    pub observed_at: u64,
    pub valid_until: u64,
    source: Source,
}

impl ReadyResearch {
    pub(crate) fn same_source(&self, other: &Self) -> bool {
        self.generation == other.generation && self.source == other.source
    }
}

#[derive(PartialEq, Eq)]
struct Source {
    spec: ResearchSpec,
    service: ProcessIdentity,
    egress: ProcessIdentity,
    controller: ProcessIdentity,
    service_socket: ListenerIdentity,
    egress_socket: ListenerIdentity,
    control_directory: (u64, u64),
    socket: UnitObservation,
    socket_node: (u64, u64),
    interface_index: u32,
    policy: String,
    lease_generation: String,
}

#[derive(PartialEq, Eq)]
struct ProcessIdentity {
    unit: UnitObservation,
    ticks: u64,
    netns: (u64, u64),
    mountns: (u64, u64),
    root: (u64, u64),
    bindings: Vec<(u64, u64)>,
}

#[derive(PartialEq, Eq)]
struct ListenerIdentity {
    unit: UnitObservation,
    inode: u64,
    node: (u64, u64),
}

fn normalized(path: &Path) -> bool {
    let Some(value) = path.to_str() else {
        return false;
    };
    value.len() <= 4096
        && value.starts_with('/')
        && !value.bytes().any(|b| b.is_ascii_control())
        && value[1..].split('/').all(|p| !matches!(p, "" | "." | ".."))
}

fn validate_process(
    spec: &ProcessSpec,
    unit: &str,
    binary: &str,
    uid: Option<u32>,
) -> Result<(), String> {
    if spec.unit != unit
        || !store_path(&spec.executable)
        || spec.executable.file_name() != Some(binary.as_ref())
        || !store_path(&spec.root_directory)
        || spec.slice != "agent-research.slice"
        || uid.is_some_and(|uid| spec.uid != uid)
        || spec.uid == u32::MAX
        || spec.argv.first() != spec.executable.to_str().map(|v| v.to_owned()).as_ref()
        || spec.argv.len() > 16
        || spec
            .argv
            .iter()
            .any(|s| s.len() > 4096 || s.bytes().any(|b| b.is_ascii_control()))
        || spec.bind_read_only_paths.is_empty()
        || spec.bind_read_only_paths.len() > 128
        || spec
            .bind_read_only_paths
            .iter()
            .collect::<BTreeSet<_>>()
            .len()
            != spec.bind_read_only_paths.len()
    {
        return Err("invalid Research process declaration".into());
    }
    if let Some(config) = &spec.configuration
        && (!store_path(&config.source)
            || config.destination != Path::new("/etc/research/config.json")
            || !spec.bind_read_only_paths.contains(&format!(
                "{}:{}",
                config.source.display(),
                config.destination.display()
            )))
    {
        return Err("invalid Research configuration binding".into());
    }
    for binding in &spec.bind_read_only_paths {
        let (source, destination) = binding.split_once(':').unwrap_or((binding, binding));
        if !normalized(Path::new(source))
            || !normalized(Path::new(destination))
            || (source != destination
                && spec.configuration.as_ref().is_none_or(|c| {
                    c.source != Path::new(source) || c.destination != Path::new(destination)
                }))
        {
            return Err("invalid Research read-only binding".into());
        }
    }
    Ok(())
}

pub(crate) fn validate(id: &str, spec: &ResearchSpec) -> Result<(), String> {
    if id.is_empty()
        || id.len() > 63
        || !id.as_bytes()[0].is_ascii_alphanumeric()
        || !id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        || spec.schema_version != 1
        || spec.control_path != Path::new("/run/agent-research-egress/control/state.json")
        || spec.relay.socket_unit != format!("tentaflake-research-{id}.socket")
        || spec.relay.socket_path != Path::new(&format!("/run/tentaflake-research/{id}/socket"))
        || spec.client.socket_destination != Path::new("/run/tentaflake-research/socket")
        || !store_path(&spec.client.executable)
        || spec.client.executable.file_name() != Some("research-client".as_ref())
        || spec.client.closure_paths.is_empty()
        || spec.client.closure_paths.len() > 64
        || spec
            .client
            .closure_paths
            .iter()
            .any(|p| !store_path(p) || p.parent() != Some(Path::new("/nix/store")))
        || spec
            .client
            .closure_paths
            .iter()
            .collect::<BTreeSet<_>>()
            .len()
            != spec.client.closure_paths.len()
        || !spec
            .client
            .closure_paths
            .iter()
            .any(|p| spec.client.executable.starts_with(p))
        || spec.vpn_interface.is_empty()
        || spec.vpn_interface.len() >= libc::IFNAMSIZ
        || !spec
            .vpn_interface
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b))
    {
        return Err("invalid Research transport declaration".into());
    }
    validate_process(
        &spec.service,
        "agent-research.service",
        "research-service",
        None,
    )?;
    validate_process(
        &spec.egress,
        "agent-research-egress.service",
        "research-egress",
        None,
    )?;
    validate_process(
        &spec.controller,
        "agent-research-egress-control.service",
        "research-egress-control",
        Some(0),
    )?;
    validate_process(
        &spec.relay.template,
        &format!("tentaflake-research-{id}@.service"),
        "socat",
        None,
    )?;
    let uids = [spec.service.uid, spec.egress.uid, spec.relay.template.uid];
    if uids.contains(&0)
        || uids.iter().collect::<BTreeSet<_>>().len() != 3
        || spec.service.configuration.is_none()
        || spec.egress.configuration.is_none()
        || spec.controller.configuration.is_some()
        || spec.relay.template.configuration.is_some()
        || spec.service.argv[1..]
            != [
                "--listen-fd",
                "3",
                "--temporary-directory",
                "/tmp/research",
                "--public-only",
                "--config",
                "/etc/research/config.json",
            ]
        || spec.egress.argv[1..] != ["--listen-fd", "3", "--config", "/etc/research/config.json"]
        || spec.relay.template.argv[1..] != ["STDIO", "UNIX-CONNECT:/run/agent-research/socket"]
        || spec.controller.argv.len() != 7
        || spec.controller.argv[1] != "--input"
        || !normalized(Path::new(&spec.controller.argv[2]))
        || spec.controller.argv[3] != "--output"
        || Path::new(&spec.controller.argv[4]) != spec.control_path
        || spec.controller.argv[5] != "--drain-seconds"
        || spec.controller.argv[6]
            .parse::<u32>()
            .ok()
            .is_none_or(|n| !(1..=300).contains(&n) || n.to_string() != spec.controller.argv[6])
    {
        return Err("Research process roles differ from the supported transport".into());
    }
    let input_parent = Path::new(&spec.controller.argv[2])
        .parent()
        .and_then(Path::to_str)
        .ok_or("invalid Research observation directory")?;
    for (process, required, optional) in [
        (
            &spec.service,
            vec!["/run/agent-research", "/run/agent-research-egress"],
            Some("/run/agent-research-searxng"),
        ),
        (&spec.egress, vec!["/run/agent-research-egress"], None),
        (&spec.controller, vec![input_parent], None),
        (&spec.relay.template, vec!["/run/agent-research"], None),
    ] {
        let config = process
            .configuration
            .as_ref()
            .map(|c| format!("{}:{}", c.source.display(), c.destination.display()));
        if required.iter().any(|path| {
            !process
                .bind_read_only_paths
                .iter()
                .any(|binding| binding == path)
        }) || process.bind_read_only_paths.iter().any(|binding| {
            !store_path(Path::new(binding))
                && !required.contains(&binding.as_str())
                && optional != Some(binding.as_str())
                && config.as_ref() != Some(binding)
        }) {
            return Err("Research binding is outside its declared process role".into());
        }
    }
    Ok(())
}

fn bounded(path: &Path, limit: u64) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    File::open(path)
        .map_err(|_| "Research observation unavailable")?
        .take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "Research observation unavailable")?;
    if bytes.len() as u64 > limit {
        return Err("Research observation exceeded its bound".into());
    }
    Ok(bytes)
}

fn identity(path: &Path) -> Result<(u64, u64), String> {
    let m = fs::metadata(path).map_err(|_| "Research object unavailable")?;
    Ok((m.dev(), m.ino()))
}

fn remaining(deadline: Instant) -> Result<Duration, String> {
    deadline
        .checked_duration_since(Instant::now())
        .filter(|d| !d.is_zero())
        .ok_or_else(|| "Research observation deadline exceeded".into())
}

fn now() -> Result<u64, String> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .map_err(|_| "Research clock unavailable".into())
}

fn ticks(root: &Path) -> Result<u64, String> {
    let bytes = bounded(&root.join("stat"), 4096)?;
    std::str::from_utf8(&bytes)
        .ok()
        .and_then(|s| s.rsplit_once(')'))
        .and_then(|(_, s)| s.split_whitespace().nth(19))
        .and_then(|s| s.parse().ok())
        .filter(|n| *n > 0)
        .ok_or_else(|| "Research process identity unavailable".into())
}

fn readonly_mount(mounts: &str, path: &str) -> bool {
    let mut selected = None;
    let mut seen = BTreeSet::new();
    for line in mounts.lines() {
        let fields: Vec<_> = line.split_whitespace().collect();
        if fields.len() < 10
            || !fields.contains(&"-")
            || fields[0].parse::<u64>().ok().is_none_or(|id| id == 0)
        {
            return false;
        }
        let mount = fields[4];
        let ro = fields[5]
            .split(',')
            .filter(|option| *option == "ro")
            .count();
        let rw = fields[5]
            .split(',')
            .filter(|option| *option == "rw")
            .count();
        if ro + rw != 1 {
            return false;
        }
        // The projected binding grammar has no escaped path components. Refuse
        // ambiguous stacked mounts and any child mount replacing part of it.
        if Path::new(mount).starts_with(path) && mount != path {
            return false;
        }
        if path == mount || Path::new(path).starts_with(mount) {
            if !seen.insert(mount) {
                return false;
            }
            if selected.is_none_or(|(length, _)| mount.len() >= length) {
                selected = Some((mount.len(), ro == 1));
            }
        }
    }
    selected.is_some_and(|(_, ro)| ro)
}

fn process(
    plan: &HostPlan,
    spec: &ProcessSpec,
    unit_name: &str,
    private_network: bool,
    deadline: Instant,
) -> Result<ProcessIdentity, String> {
    remaining(deadline)?;
    let unit = admission_systemd::inspect_unit_until(&plan.tools.busctl, unit_name, deadline)?;
    let service = unit
        .service
        .as_ref()
        .ok_or("Research service metadata unavailable")?;
    if unit.id != unit_name
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
        || service.exec_start[0].path != spec.argv[0]
        || service.exec_start[0].argv != spec.argv
        || service.kill_mode != "control-group"
        || service.control_group != format!("/agent.slice/agent-research.slice/{unit_name}")
    {
        return Err("Research process unit is not ready".into());
    }
    let root = PathBuf::from(format!("/proc/{}", service.main_pid));
    let start = ticks(&root)?;
    let status = bounded(&root.join("status"), 16384)?;
    let status = std::str::from_utf8(&status).map_err(|_| "invalid Research process status")?;
    let uid = status
        .lines()
        .find_map(|line| line.strip_prefix("Uid:"))
        .ok_or("Research process UID missing")?;
    if uid.split_whitespace().collect::<Vec<_>>() != vec![spec.uid.to_string(); 4]
        || fs::read_link(root.join("exe")).map_err(|_| "Research executable unavailable")?
            != spec.executable
        || bounded(&root.join("cmdline"), 65536)?
            != format!("{}\0", spec.argv.join("\0")).as_bytes()
        || bounded(&root.join("cgroup"), 4096)?
            != format!("0::{}\n", service.control_group).as_bytes()
    {
        return Err("Research process identity differs from declaration".into());
    }
    let host_net = identity(Path::new("/proc/1/ns/net"))?;
    let netns = identity(&root.join("ns/net"))?;
    let mountns = identity(&root.join("ns/mnt"))?;
    let root_identity = identity(&root.join("root"))?;
    if identity(Path::new("/proc/self/ns/net"))? != host_net
        || (netns == host_net) == private_network
        || root_identity != identity(&spec.root_directory)?
        || mountns == identity(Path::new("/proc/1/ns/mnt"))?
    {
        return Err("Research process namespaces or root differ from declaration".into());
    }
    let mount_bytes = bounded(&root.join("mountinfo"), 1024 * 1024)?;
    let mounts =
        std::str::from_utf8(&mount_bytes).map_err(|_| "invalid Research mount observation")?;
    let mut bindings = Vec::new();
    for binding in &spec.bind_read_only_paths {
        let (source, destination) = binding.split_once(':').unwrap_or((binding, binding));
        let observed = identity(&root.join("root").join(destination.trim_start_matches('/')))?;
        if observed != identity(Path::new(source))? || !readonly_mount(mounts, destination) {
            return Err("Research read-only binding differs from declaration".into());
        }
        bindings.push(observed);
    }
    if let Some(config) = &spec.configuration {
        let file = open_store_file(&config.source)?;
        let m = file
            .metadata()
            .map_err(|_| "Research configuration unavailable")?;
        if identity(
            &root.join("root").join(
                config
                    .destination
                    .strip_prefix("/")
                    .map_err(|_| "invalid config path")?,
            ),
        )? != (m.dev(), m.ino())
        {
            return Err("Research effective configuration differs from declaration".into());
        }
    }
    if ticks(&root)? != start {
        return Err("Research process changed during observation".into());
    }
    remaining(deadline)?;
    Ok(ProcessIdentity {
        unit,
        ticks: start,
        netns,
        mountns,
        root: root_identity,
        bindings,
    })
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Lease {
    version: u32,
    generation: String,
    mode: String,
    valid_until: u64,
    region: Option<String>,
}

fn valid_uuid(value: &str) -> bool {
    value.len() == 36
        && value.bytes().enumerate().all(|(i, b)| {
            if [8, 13, 18, 23].contains(&i) {
                b == b'-'
            } else {
                b.is_ascii_digit() || (b'a'..=b'f').contains(&b)
            }
        })
        && value.bytes().any(|b| b != b'0' && b != b'-')
}

fn parse_lease(bytes: &[u8], timestamp: u64) -> Result<Lease, String> {
    let lease: Lease =
        serde_json::from_slice(bytes).map_err(|_| "invalid Research controller lease")?;
    if lease.version != 1
        || !valid_uuid(&lease.generation)
        || lease.mode != "ready"
        || lease.valid_until <= timestamp
        || lease.valid_until > timestamp.saturating_add(3)
        || lease
            .region
            .as_ref()
            .is_some_and(|s| s.len() != 2 || !s.bytes().all(|b| b.is_ascii_uppercase()))
    {
        return Err("Research controller has no fresh ready lease".into());
    }
    Ok(lease)
}

fn lease(spec: &ResearchSpec, expected_directory: (u64, u64)) -> Result<Lease, String> {
    let parent = tentaflake_admission::protected_path::open_root(
        spec.control_path
            .parent()
            .ok_or("invalid Research lease path")?,
        0,
    )?;
    let directory = parent
        .metadata()
        .map_err(|_| "Research control directory unavailable")?;
    if (directory.dev(), directory.ino()) != expected_directory {
        return Err("Research control directory changed".into());
    }
    let fd = unsafe {
        libc::openat(
            parent.as_raw_fd(),
            c"state.json".as_ptr(),
            libc::O_RDONLY | libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_NONBLOCK,
        )
    };
    if fd < 0 {
        return Err("Research controller lease unavailable".into());
    }
    let mut file = unsafe { File::from_raw_fd(fd) };
    let before = file
        .metadata()
        .map_err(|_| "Research controller lease unavailable")?;
    if !before.is_file()
        || before.uid() != 0
        || before.mode() & 0o022 != 0
        || before.nlink() != 1
        || before.len() > 4096
    {
        return Err("Research controller lease is not protected".into());
    }
    let mut bytes = Vec::new();
    (&mut file)
        .take(4097)
        .read_to_end(&mut bytes)
        .map_err(|_| "Research controller lease read failed")?;
    let after = file
        .metadata()
        .map_err(|_| "Research controller lease unavailable")?;
    if bytes.len() > 4096
        || (
            before.len(),
            before.mtime(),
            before.mtime_nsec(),
            before.ctime(),
            before.ctime_nsec(),
        ) != (
            after.len(),
            after.mtime(),
            after.mtime_nsec(),
            after.ctime(),
            after.ctime_nsec(),
        )
    {
        return Err("Research controller lease changed during read".into());
    }
    parse_lease(&bytes, now()?)
}

fn socket_node(spec: &ResearchSpec) -> Result<(File, (u64, u64)), String> {
    let parent = tentaflake_admission::protected_path::open_root(
        spec.relay
            .socket_path
            .parent()
            .ok_or("invalid relay path")?,
        0,
    )?;
    let fd = unsafe {
        libc::openat(
            parent.as_raw_fd(),
            c"socket".as_ptr(),
            libc::O_PATH | libc::O_CLOEXEC | libc::O_NOFOLLOW,
        )
    };
    if fd < 0 {
        return Err("Research relay socket unavailable".into());
    }
    let file = unsafe { File::from_raw_fd(fd) };
    let m = file
        .metadata()
        .map_err(|_| "Research relay socket unavailable")?;
    if !m.file_type().is_socket()
        || m.uid() != 0
        || m.gid() != 0
        || m.mode() & 0o7777 != 0o666
        || m.nlink() != 1
    {
        return Err("Research relay socket differs from declaration".into());
    }
    Ok((parent, (m.dev(), m.ino())))
}

fn connect(parent: &File, deadline: Instant) -> Result<UnixStream, String> {
    remaining(deadline)?;
    let path = format!("/proc/self/fd/{}/socket", parent.as_raw_fd());
    let mut address: libc::sockaddr_un = unsafe { std::mem::zeroed() };
    address.sun_family = libc::AF_UNIX as libc::sa_family_t;
    for (out, byte) in address.sun_path.iter_mut().zip(path.bytes()) {
        *out = byte as libc::c_char;
    }
    let fd = unsafe {
        libc::socket(
            libc::AF_UNIX,
            libc::SOCK_STREAM | libc::SOCK_CLOEXEC | libc::SOCK_NONBLOCK,
            0,
        )
    };
    if fd < 0 {
        return Err("Research relay connection unavailable".into());
    }
    let stream = unsafe { UnixStream::from_raw_fd(fd) };
    let result = unsafe {
        libc::connect(
            fd,
            (&address as *const libc::sockaddr_un).cast(),
            std::mem::size_of_val(&address) as libc::socklen_t,
        )
    };
    if result != 0 {
        if std::io::Error::last_os_error().raw_os_error() != Some(libc::EINPROGRESS) {
            return Err("Research relay connection failed".into());
        }
        let timeout = remaining(deadline)?.min(Duration::from_secs(2)).as_millis() as i32;
        let mut poll = libc::pollfd {
            fd,
            events: libc::POLLOUT,
            revents: 0,
        };
        if unsafe { libc::poll(&mut poll, 1, timeout) } != 1 || poll.revents != libc::POLLOUT {
            return Err("Research relay connection timed out".into());
        }
        let mut error: libc::c_int = 0;
        let mut length = std::mem::size_of_val(&error) as libc::socklen_t;
        if unsafe {
            libc::getsockopt(
                fd,
                libc::SOL_SOCKET,
                libc::SO_ERROR,
                (&mut error as *mut libc::c_int).cast(),
                &mut length,
            )
        } != 0
            || error != 0
        {
            return Err("Research relay connection failed".into());
        }
    }
    stream
        .set_nonblocking(false)
        .map_err(|_| "Research relay connection configuration failed")?;
    remaining(deadline)?;
    Ok(stream)
}

fn relay_process(
    plan: &HostPlan,
    id: &str,
    spec: &ResearchSpec,
    peer_inode: u32,
    deadline: Instant,
) -> Result<ProcessIdentity, String> {
    // Activation is asynchronous. Only this bounded exact-template inventory is
    // polled; no global process search and no replay of a dispatched operation.
    let activation_deadline = deadline.min(Instant::now() + Duration::from_secs(2));
    loop {
        remaining(activation_deadline)?;
        let mut found = None;
        for name in
            admission_systemd::list_relay_units_until(&plan.tools.busctl, id, activation_deadline)?
        {
            remaining(activation_deadline)?;
            let unit = admission_systemd::inspect_unit_until(
                &plan.tools.busctl,
                &name,
                activation_deadline,
            )?;
            let Some(service) = &unit.service else {
                continue;
            };
            let root = PathBuf::from(format!("/proc/{}/fd", service.main_pid));
            let socket = format!("socket:[{peer_inode}]");
            if fs::read_link(root.join("0")).ok().as_deref() != Some(Path::new(&socket))
                || fs::read_link(root.join("1")).ok().as_deref() != Some(Path::new(&socket))
            {
                continue;
            }
            let observed = process(plan, &spec.relay.template, &name, true, activation_deadline)?;
            if observed.unit != unit || found.replace(observed).is_some() {
                return Err("Research relay connection has ambiguous process ownership".into());
            }
        }
        if let Some(found) = found {
            return Ok(found);
        }
        std::thread::sleep(Duration::from_millis(10).min(remaining(activation_deadline)?));
    }
}

fn interface(spec: &ResearchSpec) -> Result<u32, String> {
    let name = CString::new(spec.vpn_interface.as_bytes())
        .map_err(|_| "invalid Research VPN interface")?;
    let index = unsafe { libc::if_nametoindex(name.as_ptr()) };
    let bytes = bounded(
        &Path::new("/sys/class/net")
            .join(&spec.vpn_interface)
            .join("flags"),
        32,
    )?;
    let flags = std::str::from_utf8(&bytes)
        .ok()
        .and_then(|s| s.trim().strip_prefix("0x"))
        .and_then(|s| u32::from_str_radix(s, 16).ok())
        .ok_or("Research VPN interface flags unavailable")?;
    if index == 0 || flags & libc::IFF_UP as u32 == 0 {
        return Err("Research VPN interface is not up".into());
    }
    Ok(index)
}

fn listening_inode(bytes: &[u8], path: &str) -> Result<u64, String> {
    let text = std::str::from_utf8(bytes).map_err(|_| "invalid Research Unix listener table")?;
    let mut lines = text.lines();
    if lines.next() != Some("Num       RefCount Protocol Flags    Type St Inode Path") {
        return Err("invalid Research Unix listener table".into());
    }
    let mut found = None;
    for line in lines {
        let fields: Vec<_> = line.split_whitespace().collect();
        if fields.get(7).copied() != Some(path) {
            continue;
        }
        // Accepted server sockets retain the listener pathname in /proc. They
        // are normal while a relay is held open, and are not extra listeners.
        if fields.len() == 8 && fields[2..6] == ["00000000", "00000000", "0001", "03"] {
            continue;
        }
        let inode = fields
            .get(6)
            .and_then(|s| s.parse::<u64>().ok())
            .filter(|n| *n != 0)
            .ok_or("invalid Research Unix listener identity")?;
        if fields.len() != 8
            || fields[2] != "00000000"
            || fields[3] != "00010000"
            || fields[4] != "0001"
            || fields[5] != "01"
            || found.replace(inode).is_some()
        {
            return Err("Research Unix listener identity is ambiguous".into());
        }
    }
    found.ok_or_else(|| "Research Unix listener unavailable".into())
}

fn listener(
    plan: &HostPlan,
    process: &ProcessIdentity,
    socket_name: &str,
    path: &str,
    deadline: Instant,
) -> Result<ListenerIdentity, String> {
    remaining(deadline)?;
    let socket =
        admission_systemd::inspect_socket_until(&plan.tools.busctl, socket_name, deadline)?;
    if socket.unit.load_state != "loaded"
        || socket.unit.active_state != "active"
        || socket.unit.sub_state != "running"
        || socket.unit.need_daemon_reload
        || socket.unit.job.id != 0
        || socket.unit.invocation_id == [0; 16]
        || socket.accept
        || socket.listen != [("Stream".into(), path.into())]
        || socket.triggers != [process.unit.id.clone()]
    {
        return Err("Research backend socket unit is not ready".into());
    }
    let parent = tentaflake_admission::protected_path::open_root(
        Path::new(path)
            .parent()
            .ok_or("invalid Research backend socket path")?,
        0,
    )?;
    let fd = unsafe {
        libc::openat(
            parent.as_raw_fd(),
            c"socket".as_ptr(),
            libc::O_PATH | libc::O_CLOEXEC | libc::O_NOFOLLOW,
        )
    };
    if fd < 0 {
        return Err("Research backend socket unavailable".into());
    }
    let file = unsafe { File::from_raw_fd(fd) };
    let node = file
        .metadata()
        .map_err(|_| "Research backend socket unavailable")?;
    if !node.file_type().is_socket() || node.uid() != 0 || node.nlink() != 1 {
        return Err("Research backend socket has no protected identity".into());
    }
    let inode = listening_inode(&bounded(Path::new("/proc/1/net/unix"), 1024 * 1024)?, path)?;
    let pid = process
        .unit
        .service
        .as_ref()
        .ok_or("Research backend process unavailable")?
        .main_pid;
    if fs::read_link(format!("/proc/{pid}/fd/3"))
        .map_err(|_| "Research inherited listener unavailable")?
        != Path::new(&format!("socket:[{inode}]"))
        || identity(&PathBuf::from(format!("/proc/{pid}/root")).join(path.trim_start_matches('/')))?
            != (node.dev(), node.ino())
    {
        return Err("Research backend process does not own the declared listener".into());
    }
    Ok(ListenerIdentity {
        unit: socket.unit,
        inode,
        node: (node.dev(), node.ino()),
    })
}

fn observe(plan: &HostPlan, spec: &ResearchSpec, deadline: Instant) -> Result<Source, String> {
    let service = process(plan, &spec.service, &spec.service.unit, true, deadline)?;
    let egress = process(plan, &spec.egress, &spec.egress.unit, false, deadline)?;
    let controller = process(
        plan,
        &spec.controller,
        &spec.controller.unit,
        true,
        deadline,
    )?;
    let service_socket = listener(
        plan,
        &service,
        "agent-research.socket",
        "/run/agent-research/socket",
        deadline,
    )?;
    let egress_socket = listener(
        plan,
        &egress,
        "agent-research-egress.socket",
        "/run/agent-research-egress/socket",
        deadline,
    )?;
    let control_path = spec
        .control_path
        .parent()
        .ok_or("invalid Research control directory")?;
    let control_directory = identity(control_path)?;
    let controller_pid = controller
        .unit
        .service
        .as_ref()
        .ok_or("Research controller unavailable")?
        .main_pid;
    if identity(
        &PathBuf::from(format!("/proc/{controller_pid}/root")).join(
            control_path
                .strip_prefix("/")
                .map_err(|_| "invalid control directory")?,
        ),
    )? != control_directory
    {
        return Err("Research controller output binding differs from declaration".into());
    }
    remaining(deadline)?;
    let socket = admission_systemd::inspect_socket_until(
        &plan.tools.busctl,
        &spec.relay.socket_unit,
        deadline,
    )?;
    if socket.unit.load_state != "loaded"
        || socket.unit.active_state != "active"
        || socket.unit.sub_state != "listening"
        || socket.unit.need_daemon_reload
        || socket.unit.job.id != 0
        || socket.unit.invocation_id == [0; 16]
        || !socket.accept
        || socket.max_connections != 4
        || socket.listen
            != [(
                "Stream".into(),
                spec.relay.socket_path.to_string_lossy().into_owned(),
            )]
    {
        return Err("Research relay socket unit is not ready".into());
    }
    let (_, socket_node) = socket_node(spec)?;
    Ok(Source {
        spec: spec.clone(),
        service,
        egress,
        controller,
        service_socket,
        egress_socket,
        control_directory,
        socket: socket.unit,
        socket_node,
        interface_index: interface(spec)?,
        policy: String::new(),
        lease_generation: String::new(),
    })
}

pub(crate) fn collect(
    plan: &HostPlan,
    id: &str,
    spec: &ResearchSpec,
) -> Result<ReadyResearch, String> {
    if unsafe { libc::geteuid() } != 0 {
        return Err("Research collection requires root".into());
    }
    validate(id, spec)?;
    let deadline = Instant::now() + Duration::from_secs(20);
    let mut before = observe(plan, spec, deadline)?;
    let (parent, node) = socket_node(spec)?;
    if node != before.socket_node {
        return Err("Research relay socket changed".into());
    }
    let mut stream = connect(&parent, deadline)?;
    let peer = crate::admission_unix::peer_identity(&stream)?;
    let relay = relay_process(plan, id, spec, peer.peer_inode, deadline)?;
    let initial_lease = lease(spec, before.control_directory)?;
    let report = crate::admission_research_rpc::probe(
        &mut stream,
        spec.relay.template.uid,
        &initial_lease.generation,
        deadline,
    )?;
    let mut after = observe(plan, spec, deadline)?;
    let relay_after = relay_process(plan, id, spec, peer.peer_inode, deadline)?;
    let final_lease = lease(spec, after.control_directory)?;
    if peer != crate::admission_unix::peer_identity(&stream)?
        || relay != relay_after
        || initial_lease.generation != final_lease.generation
        || initial_lease.region != final_lease.region
    {
        return Err("Research connection or controller generation changed during probe".into());
    }
    before.policy = report.policy_sha256.clone();
    after.policy = report.policy_sha256;
    before.lease_generation = initial_lease.generation;
    after.lease_generation = final_lease.generation;
    if before != after {
        return Err("Research prerequisites changed during readiness probe".into());
    }
    remaining(deadline)?;
    let valid_until = report.valid_until.min(final_lease.valid_until);
    let completed_at = now()?;
    if report.observed_at > completed_at || valid_until <= completed_at {
        return Err("Research readiness expired before collection completed".into());
    }
    let invocation = |unit: &UnitObservation| {
        unit.invocation_id
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>()
    };
    let generation = format!(
        "{}.{}.{}",
        after.lease_generation,
        invocation(&after.service.unit),
        invocation(&after.egress.unit)
    );
    Ok(ReadyResearch {
        generation,
        observed_at: report.observed_at,
        valid_until,
        source: after,
    })
}

#[cfg(test)]
#[path = "admission_research_tests.rs"]
pub(crate) mod tests;
