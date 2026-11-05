use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::os::unix::process::CommandExt;
use std::path::{Component, Path};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

pub const MAX_GENERATION: u64 = 9_007_199_254_740_991;
pub const STATE: &str = "/var/lib/tentaflake-workloads";
pub const RUNTIME: &str = "/run/tentaflake-workloads";
/// Consumers' maximum manifest lease.
pub const MANIFEST_LEASE: Duration = Duration::from_secs(10);
/// Cycles start at most this often.
pub const CYCLE: Duration = Duration::from_secs(2);
pub const COMMAND_TIMEOUT: Duration = Duration::from_secs(1);
/// Bounded commands per cycle, whatever the binding count: unit state and
/// container identity, each read before and after the kernel checks.
pub const OBSERVATION_COMMANDS: u32 = 4;
/// Output cap per binding for one batched command (one inspect line or unit block).
const BYTES_PER_BINDING: usize = 256;
// Even when every command runs to its deadline, a cycle plus the pacing delay
// stays inside the lease; slow commands make bindings unavailable, never stale.
const _: () = assert!(
    OBSERVATION_COMMANDS as u64 * COMMAND_TIMEOUT.as_secs() + CYCLE.as_secs()
        < MANIFEST_LEASE.as_secs()
);
pub type Result<T, E = io::Error> = std::result::Result<T, E>;
pub fn invalid() -> io::Error {
    io::Error::other("workload authority unavailable")
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Binding {
    pub tenant_id: String,
    pub agent_id: String,
    pub workload_id: String,
    pub policy_revision: String,
    pub unit: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Policy {
    pub version: u32,
    pub docker: String,
    pub systemctl: String,
    pub bindings: Vec<Binding>,
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Identity {
    pub kernel_boot_id: String,
    pub container_id: String,
    pub pid: u32,
    pub start_ticks: u64,
    pub cgroup: String,
    pub cgroup_inode: u64,
}
#[derive(Serialize, Deserialize, Debug)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub version: u32,
    pub state: String,
    pub tenant_id: String,
    pub agent_id: String,
    pub workload_id: String,
    pub policy_revision: String,
    pub unit: String,
    pub generation: u64,
    pub verified_at_ms: u64,
    pub kernel_boot_id: Option<String>,
    pub container_id: Option<String>,
    pub pid: Option<u32>,
    pub start_ticks: Option<u64>,
    pub cgroup: Option<String>,
    pub cgroup_inode: Option<u64>,
}
impl Manifest {
    pub fn new(b: &Binding, generation: u64, now: u64, identity: Option<Identity>) -> Self {
        let live = identity.is_some();
        Self {
            version: 1,
            state: if live { "live" } else { "unavailable" }.into(),
            tenant_id: b.tenant_id.clone(),
            agent_id: b.agent_id.clone(),
            workload_id: b.workload_id.clone(),
            policy_revision: b.policy_revision.clone(),
            unit: b.unit.clone(),
            generation,
            verified_at_ms: now,
            kernel_boot_id: identity.as_ref().map(|v| v.kernel_boot_id.clone()),
            container_id: identity.as_ref().map(|v| v.container_id.clone()),
            pid: identity.as_ref().map(|v| v.pid),
            start_ticks: identity.as_ref().map(|v| v.start_ticks),
            cgroup: identity.as_ref().map(|v| v.cgroup.clone()),
            cgroup_inode: identity.map(|v| v.cgroup_inode),
        }
    }
}
pub fn valid_id(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 64
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
        && s != "."
        && s != ".."
}
pub fn validate_policy(p: &Policy) -> Result<()> {
    if p.version != 1 {
        return Err(invalid());
    }
    for executable in [&p.docker, &p.systemctl] {
        if !executable.starts_with("/nix/store/") {
            return Err(invalid());
        }
        let file = secure_open(Path::new(executable), false)?;
        if !file.metadata()?.is_file() {
            return Err(invalid());
        }
    }
    let mut agents = std::collections::BTreeSet::new();
    let mut scopes = std::collections::BTreeSet::new();
    for b in &p.bindings {
        if ![
            &b.agent_id,
            &b.tenant_id,
            &b.workload_id,
            &b.policy_revision,
        ]
        .into_iter()
        .all(|s| valid_id(s))
            || !b.agent_id.starts_with("agent-")
            || b.unit != format!("docker-{}.service", b.agent_id)
            || !agents.insert(&b.agent_id)
            || !scopes.insert((&b.tenant_id, &b.workload_id))
        {
            return Err(invalid());
        }
    }
    Ok(())
}

// Walk descriptors, never follow symlinks; writable ancestors cannot swap a target.
pub fn secure_open(path: &Path, directory: bool) -> Result<File> {
    if !path.is_absolute() {
        return Err(invalid());
    }
    let mut parts = path.components().enumerate().peekable();
    if parts.next() != Some((0, Component::RootDir)) {
        return Err(invalid());
    }
    let mut current = File::open("/")?;
    while let Some((depth, part)) = parts.next() {
        let Component::Normal(name) = part else {
            return Err(invalid());
        };
        let name = std::ffi::CString::new(name.as_encoded_bytes()).map_err(|_| invalid())?;
        let is_dir = parts.peek().is_some() || directory;
        let flags = libc::O_RDONLY
            | libc::O_CLOEXEC
            | libc::O_NOFOLLOW
            | if is_dir {
                libc::O_DIRECTORY
            } else {
                libc::O_NONBLOCK
            };
        let fd = unsafe { libc::openat(current.as_raw_fd(), name.as_ptr(), flags) };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        current = unsafe { File::from_raw_fd(fd) };
        let meta = current.metadata()?;
        // NixOS uses root-owned, sticky 1775 /nix/store. Its group cannot replace
        // the root-owned children checked below; no other writable ancestor is allowed.
        let sticky_store = depth == 2
            && path.starts_with("/nix/store")
            && is_dir
            && meta.mode() & 0o1002 == 0o1000;
        if meta.uid() != 0
            || (meta.mode() & 0o022 != 0 && !sticky_store)
            || (!is_dir && !meta.is_file())
        {
            return Err(invalid());
        }
    }
    Ok(current)
}
pub fn bounded_read(mut file: File, limit: usize) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    (&mut file).take(limit as u64 + 1).read_to_end(&mut bytes)?;
    if bytes.len() > limit {
        return Err(invalid());
    }
    Ok(bytes)
}
fn valid_json_filename(name: &str) -> bool {
    name.strip_suffix(".json").is_some_and(valid_id)
}

pub fn atomic_json<T: Serialize>(directory: &str, name: &str, value: &T, mode: u32) -> Result<()> {
    if !valid_json_filename(name) {
        return Err(invalid());
    }
    let dir = secure_open(Path::new(directory), true)?;
    let target = format!("/proc/self/fd/{}/{}", dir.as_raw_fd(), name);
    let temp = format!("{target}.pending");
    // A previous interrupted publication requires operator cleanup, never follows or overwrites it.
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(mode)
        .custom_flags(libc::O_NOFOLLOW)
        .open(&temp)?;
    let bytes = serde_json::to_vec(value).map_err(|_| invalid())?;
    if bytes.len() > 262_144 {
        return Err(invalid());
    }
    file.write_all(&bytes)?;
    file.set_permissions(fs::Permissions::from_mode(mode))?;
    file.sync_all()?;
    fs::rename(&temp, &target)?;
    dir.sync_all()
}

pub fn parse_stat(text: &str, expected_pid: u32) -> Result<u64> {
    let (pid, rest) = text.split_once(" (").ok_or_else(invalid)?;
    if pid.parse::<u32>().ok() != Some(expected_pid) {
        return Err(invalid());
    }
    let (_, fields) = rest.rsplit_once(") ").ok_or_else(invalid)?;
    let start = fields
        .split_whitespace()
        .nth(19)
        .ok_or_else(invalid)?
        .parse::<u64>()
        .map_err(|_| invalid())?;
    if start == 0 {
        return Err(invalid());
    }
    Ok(start)
}
pub fn parse_cgroup(text: &str, container: &str) -> Result<String> {
    let mut lines = text.lines();
    let path = lines
        .next()
        .and_then(|s| s.strip_prefix("0::"))
        .ok_or_else(invalid)?;
    if lines.next().is_some()
        || path
            .strip_prefix("/system.slice/docker-")
            .and_then(|s| s.strip_suffix(".scope"))
            != Some(container)
    {
        return Err(invalid());
    }
    Ok(path.into())
}
fn valid_container_id(id: &str) -> bool {
    id.len() == 64
        && id
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
/// One batched `container inspect` line per found container:
/// `/<name> <id> <pid> <running>`. Missing containers are simply absent; any
/// malformed, foreign, duplicate or truncated line rejects the whole batch.
pub fn parse_inspect_all(text: &str, names: &[&str]) -> Result<BTreeMap<String, (String, u32)>> {
    if !text.is_empty() && !text.ends_with('\n') {
        return Err(invalid());
    }
    let mut seen = std::collections::BTreeSet::new();
    let mut running = BTreeMap::new();
    for line in text.lines() {
        let values: Vec<&str> = line.split(' ').collect();
        let [name, id, pid, state] = values[..] else {
            return Err(invalid());
        };
        let name = name.strip_prefix('/').ok_or_else(invalid)?;
        if !names.contains(&name) || !seen.insert(name) || !valid_container_id(id) {
            return Err(invalid());
        }
        let pid = pid.parse::<u32>().map_err(|_| invalid())?;
        match state {
            "true" if pid > 1 => {
                running.insert(name.to_owned(), (id.to_owned(), pid));
            }
            "false" => (),
            _ => return Err(invalid()),
        }
    }
    Ok(running)
}
/// Batched `systemctl show --property=Id,ActiveState`: blank-line separated
/// blocks. Returns the active requested units; anything unexpected rejects all.
pub fn parse_active_units(
    text: &str,
    units: &[&str],
) -> Result<std::collections::BTreeSet<String>> {
    let mut seen = std::collections::BTreeSet::new();
    let mut active = std::collections::BTreeSet::new();
    for block in text.split("\n\n").filter(|block| !block.trim().is_empty()) {
        let (mut id, mut state) = (None, None);
        for line in block.lines() {
            let slot = match line.split_once('=') {
                Some(("Id", value)) => (&mut id, value),
                Some(("ActiveState", value)) => (&mut state, value),
                _ => return Err(invalid()),
            };
            if slot.0.replace(slot.1).is_some() {
                return Err(invalid());
            }
        }
        let (Some(id), Some(state)) = (id, state) else {
            return Err(invalid());
        };
        if !units.contains(&id) || !seen.insert(id) {
            return Err(invalid());
        }
        if state == "active" {
            active.insert(id.to_owned());
        }
    }
    Ok(active)
}

// No caller-controlled command or argument surface. Only the two observer operations use this.
fn bounded_command(
    executable: &str,
    args: &[&str],
    timeout: Duration,
    limit: usize,
    require_success: bool,
) -> Result<String> {
    let mut child = Command::new(executable)
        .args(args)
        .env_clear()
        .env("PATH", "/no-path")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn()?;
    let mut pipe = child.stdout.take().ok_or_else(invalid)?;
    let flags = unsafe { libc::fcntl(pipe.as_raw_fd(), libc::F_GETFL) };
    if flags < 0
        || unsafe { libc::fcntl(pipe.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0
    {
        let _ = child.kill();
        let _ = child.wait();
        return Err(invalid());
    }
    let started = Instant::now();
    let result = (|| {
        let mut bytes = Vec::new();
        loop {
            let mut buffer = [0; 512];
            match pipe.read(&mut buffer) {
                Ok(n) => {
                    bytes.extend_from_slice(&buffer[..n]);
                    if bytes.len() > limit {
                        return Err(invalid());
                    }
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => (),
                Err(e) => return Err(e),
            }
            if let Some(status) = child.try_wait()? {
                if require_success && !status.success() {
                    return Err(invalid());
                }
                // Drain only bounded bytes, including output delivered just before exit.
                loop {
                    match pipe.read(&mut buffer) {
                        Ok(0) => break,
                        Ok(n) => {
                            bytes.extend_from_slice(&buffer[..n]);
                            if bytes.len() > limit {
                                return Err(invalid());
                            }
                        }
                        Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
                        Err(e) => return Err(e),
                    }
                }
                return String::from_utf8(bytes).map_err(|_| invalid());
            }
            if started.elapsed() >= timeout {
                return Err(invalid());
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    })();
    if result.is_err() {
        unsafe {
            libc::kill(-(child.id() as i32), libc::SIGKILL);
        }
        let _ = child.wait();
    }
    result
}
fn output_limit(bindings: usize) -> usize {
    BYTES_PER_BINDING * bindings.max(1)
}
fn inspect_all(p: &Policy) -> Result<BTreeMap<String, (String, u32)>> {
    let names: Vec<&str> = p.bindings.iter().map(|b| b.agent_id.as_str()).collect();
    let mut args = vec![
        "--host=unix:///run/docker.sock",
        "container",
        "inspect",
        "--format",
        "{{.Name}} {{.Id}} {{.State.Pid}} {{.State.Running}}",
        "--",
    ];
    args.extend(&names);
    // Docker exits non-zero when any name is missing but still reports the rest.
    let output = bounded_command(
        &p.docker,
        &args,
        COMMAND_TIMEOUT,
        output_limit(names.len()),
        false,
    )?;
    parse_inspect_all(&output, &names)
}
fn active_units(p: &Policy) -> Result<std::collections::BTreeSet<String>> {
    let units: Vec<&str> = p.bindings.iter().map(|b| b.unit.as_str()).collect();
    let mut args = vec!["show", "--property=Id,ActiveState", "--"];
    args.extend(&units);
    let output = bounded_command(
        &p.systemctl,
        &args,
        COMMAND_TIMEOUT,
        output_limit(units.len()),
        true,
    )?;
    parse_active_units(&output, &units)
}
fn proc_text(path: &str) -> Result<String> {
    let file = File::open(path)?;
    let mut stat = std::mem::MaybeUninit::<libc::statfs>::uninit();
    if unsafe { libc::fstatfs(file.as_raw_fd(), stat.as_mut_ptr()) } != 0
        || unsafe { stat.assume_init() }.f_type != 0x9fa0
    {
        return Err(invalid());
    }
    String::from_utf8(bounded_read(file, 8192)?).map_err(|_| invalid())
}
fn kernel_identity(container_id: String, pid: u32) -> Result<Identity> {
    // Refuse user namespaces and daemon remapping, including rootless runtimes.
    let target_namespace = fs::metadata(format!("/proc/{pid}/ns/user"))?;
    let initial_namespace = fs::metadata("/proc/1/ns/user")?;
    if (target_namespace.dev(), target_namespace.ino())
        != (initial_namespace.dev(), initial_namespace.ino())
        || !proc_text(&format!("/proc/{pid}/uid_map"))?
            .split_whitespace()
            .eq(["0", "0", "4294967295"])
    {
        return Err(invalid());
    }
    let start_ticks = parse_stat(&proc_text(&format!("/proc/{pid}/stat"))?, pid)?;
    let cgroup = parse_cgroup(&proc_text(&format!("/proc/{pid}/cgroup"))?, &container_id)?;
    let group = secure_open(Path::new(&format!("/sys/fs/cgroup{cgroup}")), true)?;
    let mut stat = std::mem::MaybeUninit::<libc::statfs>::uninit();
    if unsafe { libc::fstatfs(group.as_raw_fd(), stat.as_mut_ptr()) } != 0 {
        return Err(invalid());
    }
    if unsafe { stat.assume_init() }.f_type != 0x6367_7270 {
        return Err(invalid());
    }
    let cgroup_inode = group.metadata()?.ino();
    let kernel_boot_id = proc_text("/proc/sys/kernel/random/boot_id")?
        .trim()
        .to_owned();
    if kernel_boot_id.len() != 36
        || !kernel_boot_id.bytes().enumerate().all(|(i, b)| {
            if [8, 13, 18, 23].contains(&i) {
                b == b'-'
            } else {
                b.is_ascii_digit() || (b'a'..=b'f').contains(&b)
            }
        })
    {
        return Err(invalid());
    }
    if parse_stat(&proc_text(&format!("/proc/{pid}/stat"))?, pid)? != start_ticks
        || parse_cgroup(&proc_text(&format!("/proc/{pid}/cgroup"))?, &container_id)? != cgroup
    {
        return Err(invalid());
    }
    Ok(Identity {
        kernel_boot_id,
        container_id,
        pid,
        start_ticks,
        cgroup,
        cgroup_inode,
    })
}
/// Observe every binding with exactly `OBSERVATION_COMMANDS` bounded commands,
/// independent of the binding count. Unit state and container identity are read
/// before and after the kernel checks; any failure makes the binding unavailable.
pub fn observe_all(p: &Policy) -> Vec<Option<Identity>> {
    observe_batch(p).unwrap_or_else(|_| vec![None; p.bindings.len()])
}
fn observe_batch(p: &Policy) -> Result<Vec<Option<Identity>>> {
    if p.bindings.is_empty() {
        return Ok(Vec::new());
    }
    let active = active_units(p)?;
    let containers = inspect_all(p)?;
    let first: Vec<_> = p
        .bindings
        .iter()
        .map(|b| {
            let (id, pid) = containers
                .get(&b.agent_id)
                .filter(|_| active.contains(&b.unit))?;
            let identity = kernel_identity(id.clone(), *pid).ok()?;
            Some((id.clone(), *pid, identity))
        })
        .collect();
    let containers = inspect_all(p)?;
    let active = active_units(p)?;
    Ok(p.bindings
        .iter()
        .zip(first)
        .map(|(b, first)| {
            let (id, pid, identity) = first?;
            let unchanged = active.contains(&b.unit)
                && containers.get(&b.agent_id) == Some(&(id.clone(), pid))
                && kernel_identity(id, pid).ok().as_ref() == Some(&identity);
            unchanged.then_some(identity)
        })
        .collect())
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Entry {
    pub fingerprint: String,
    pub generation: u64,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Authority {
    pub version: u32,
    pub counter: u64,
    pub entries: BTreeMap<String, Entry>,
}
impl Authority {
    pub fn empty() -> Self {
        Self {
            version: 1,
            counter: 0,
            entries: BTreeMap::new(),
        }
    }
    pub fn validate(&self) -> Result<()> {
        if self.version != 1
            || self.counter > MAX_GENERATION
            || self.entries.iter().any(|(id, e)| {
                !valid_id(id)
                    || e.generation == 0
                    || e.generation > self.counter
                    || e.fingerprint.len() > 8192
            })
        {
            return Err(invalid());
        }
        Ok(())
    }
    pub fn assign(&mut self, binding: &Binding, identity: &Option<Identity>) -> Result<u64> {
        let fingerprint = serde_json::to_string(&(binding, identity)).map_err(|_| invalid())?;
        if let Some(entry) = self.entries.get(&binding.agent_id)
            && entry.fingerprint == fingerprint
        {
            return Ok(entry.generation);
        }
        if self.counter >= MAX_GENERATION {
            return Err(invalid());
        }
        self.counter += 1;
        self.entries.insert(
            binding.agent_id.clone(),
            Entry {
                fingerprint,
                generation: self.counter,
            },
        );
        Ok(self.counter)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn binding() -> Binding {
        Binding {
            tenant_id: "t".into(),
            agent_id: "agent-a".into(),
            workload_id: "w".into(),
            policy_revision: "r1".into(),
            unit: "docker-agent-a.service".into(),
        }
    }
    #[test]
    fn three_hundred_bindings_preserve_policy_and_authority_validation() {
        let executable = fs::canonicalize(on_path("bash")).unwrap();
        let executable = executable.to_str().unwrap();
        let bindings: Vec<_> = (0..300)
            .map(|i| Binding {
                agent_id: format!("agent-{i}"),
                unit: format!("docker-agent-{i}.service"),
                workload_id: format!("w{i}"),
                ..binding()
            })
            .collect();
        let bytes = serde_json::to_vec(&serde_json::json!({
            "version": 1,
            "docker": executable,
            "systemctl": executable,
            "bindings": bindings,
        }))
        .unwrap();
        // The fixed policy loader's byte bound, not a binding-count allowance.
        assert!(bytes.len() <= 65_536);
        let mut policy: Policy = serde_json::from_slice(&bytes).unwrap();
        validate_policy(&policy).unwrap();
        assert_eq!(policy.bindings.len(), 300);
        let mut authority = Authority::empty();
        for b in &policy.bindings {
            authority.assign(b, &None).unwrap();
        }
        authority.validate().unwrap();
        assert_eq!(authority.counter, 300);
        let bytes = serde_json::to_vec(&authority).unwrap();
        // Durable authority reads and atomic publication retain their byte cap.
        assert!(bytes.len() <= 262_144);
        let mut restored: Authority = serde_json::from_slice(&bytes).unwrap();
        restored.validate().unwrap();
        assert_eq!(restored.entries.len(), 300);
        assert!(
            policy
                .bindings
                .iter()
                .all(|b| restored.entries.contains_key(&b.agent_id))
        );

        policy.bindings.push(policy.bindings[0].clone());
        assert!(validate_policy(&policy).is_err());
        let _ = policy.bindings.pop();
        let last = policy.bindings.last().unwrap().clone();
        policy.bindings.last_mut().unwrap().workload_id = "w0".into();
        assert!(validate_policy(&policy).is_err());
        *policy.bindings.last_mut().unwrap() = last.clone();
        policy.bindings.last_mut().unwrap().unit = "docker-other.service".into();
        assert!(validate_policy(&policy).is_err());
        *policy.bindings.last_mut().unwrap() = last;
        policy.bindings.last_mut().unwrap().agent_id = "undeclared".into();
        policy.bindings.last_mut().unwrap().unit = "docker-undeclared.service".into();
        assert!(validate_policy(&policy).is_err());
        restored.entries.get_mut("agent-299").unwrap().generation = 0;
        assert!(restored.validate().is_err());
        restored.entries.get_mut("agent-299").unwrap().generation = 300;
        restored.entries.get_mut("agent-299").unwrap().fingerprint = "x".repeat(8193);
        assert!(restored.validate().is_err());
    }
    #[test]
    fn scope_ids_match_projection_bounds() {
        assert!(valid_id(&"a".repeat(64)));
        assert!(!valid_id(&"a".repeat(65)));
    }
    #[test]
    fn manifest_filenames_preserve_scope_bounds() {
        for length in [53, 54, 55, 56, 57, 58] {
            let agent_id = format!("agent-{}", "a".repeat(length));
            assert!(valid_id(&agent_id));
            assert!(valid_json_filename(&format!("{agent_id}.json")));
        }
        assert!(valid_json_filename("authority.json"));
        for name in [
            format!("{}.json", "a".repeat(65)),
            "../agent-a.json".into(),
            "/agent-a.json".into(),
            ".json".into(),
            "..json".into(),
            "...json".into(),
            "agent-a.json/pending".into(),
            "agent-a".into(),
        ] {
            assert!(!valid_json_filename(&name), "{name}");
        }
    }
    #[test]
    fn batched_inspect_is_closed() {
        let id = "a".repeat(64);
        let names = ["agent-a", "agent-b", "agent-c"];
        // agent-b stopped, agent-c missing (Docker still reports the others).
        let found = parse_inspect_all(
            &format!("/agent-a {id} 42 true\n/agent-b {id} 0 false\n"),
            &names,
        )
        .unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found["agent-a"], (id.clone(), 42));
        assert!(parse_inspect_all("", &names).unwrap().is_empty());
        for s in [
            format!("/agent-a {id} 42 true"),
            format!("/agent-a {id} 0 true\n"),
            format!("/agent-a {id} 42 maybe\n"),
            format!("/agent-a {id} 42 true secret\n"),
            format!("/agent-x {id} 42 true\n"),
            format!("agent-a {id} 42 true\n"),
            format!("/agent-a {id} 42 true\n/agent-a {id} 43 true\n"),
            "/agent-a short 42 true\n".into(),
        ] {
            assert!(parse_inspect_all(&s, &names).is_err(), "{s:?}");
        }
    }
    #[test]
    fn batched_unit_state_is_closed() {
        let units = ["docker-agent-a.service", "docker-agent-b.service"];
        let active = parse_active_units(
            "Id=docker-agent-a.service\nActiveState=active\n\nActiveState=failed\nId=docker-agent-b.service\n",
            &units,
        )
        .unwrap();
        assert_eq!(
            active.into_iter().collect::<Vec<_>>(),
            ["docker-agent-a.service"]
        );
        for s in [
            "Id=docker-agent-a.service\n",
            "Id=docker-other.service\nActiveState=active\n",
            "Id=docker-agent-a.service\nActiveState=active\nMainPID=1\n",
            "Id=docker-agent-a.service\nActiveState=active\nActiveState=active\n",
            "Id=docker-agent-a.service\nActiveState=active\n\nId=docker-agent-a.service\nActiveState=active\n",
        ] {
            assert!(parse_active_units(s, &units).is_err(), "{s:?}");
        }
    }
    fn on_path(name: &str) -> String {
        std::env::split_paths(&std::env::var_os("PATH").unwrap())
            .map(|p| p.join(name))
            .find(|p| p.is_file())
            .expect("test tools include bash and coreutils")
            .to_str()
            .unwrap()
            .to_owned()
    }
    #[test]
    fn three_hundred_bindings_cost_four_commands_inside_the_lease() {
        const BINDINGS: usize = 300;
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/test-tmp")
            .join(format!("workloads-batch-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let log = root.join("calls");
        let (bash, sleep) = (on_path("bash"), on_path("sleep"));
        // Each fake command takes 200 ms; a per-binding design needs 240 s here.
        let tool = |name: &str, line: &str| {
            let path = root.join(name);
            fs::write(
                &path,
                format!(
                    "#!{bash}\nprintf '%s\\n' {name} >> '{}'\n'{sleep}' 0.2\nseen=\nfor a in \"$@\"; do\n  if [ -n \"$seen\" ]; then printf '{line}' \"$a\"; fi\n  if [ \"$a\" = -- ]; then seen=1; fi\ndone\n",
                    log.display()
                ),
            )
            .unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
            path.to_str().unwrap().to_owned()
        };
        let policy = Policy {
            version: 1,
            docker: tool("docker", &format!("/%s {} 4242 true\\n", "0".repeat(64))),
            systemctl: tool("systemctl", "Id=%s\\nActiveState=active\\n\\n"),
            bindings: (0..BINDINGS)
                .map(|i| Binding {
                    agent_id: format!("agent-{i}"),
                    unit: format!("docker-agent-{i}.service"),
                    workload_id: format!("w{i}"),
                    ..binding()
                })
                .collect(),
        };
        let started = Instant::now();
        let identities = observe_all(&policy);
        let elapsed = started.elapsed();
        let calls = fs::read_to_string(&log).unwrap();
        fs::remove_dir_all(&root).unwrap();
        assert_eq!(identities.len(), BINDINGS);
        // The fake PID is not a Docker scope, so kernel verification refuses it.
        assert!(identities.iter().all(Option::is_none));
        assert_eq!(
            calls.lines().count(),
            OBSERVATION_COMMANDS as usize,
            "{calls}"
        );
        assert!(elapsed >= Duration::from_millis(800), "{elapsed:?}");
        assert!(elapsed + CYCLE < MANIFEST_LEASE / 2, "{elapsed:?}");
    }
    #[test]
    fn membership_is_exact() {
        let id = "a".repeat(64);
        assert!(parse_cgroup(&format!("0::/system.slice/docker-{id}.scope\n"), &id).is_ok());
        for s in [
            "0::/",
            "0::/../system.slice",
            "1:cpu:/a\n0::/b",
            "0::/user.slice/a",
            "0::/system.slice/docker-other.scope",
        ] {
            assert!(parse_cgroup(s, &id).is_err());
        }
    }
    #[test]
    fn stat_handles_parentheses() {
        let fields = format!("S {} 99", vec!["0"; 18].join(" "));
        assert_eq!(parse_stat(&format!("42 (a ) b) {fields}"), 42).unwrap(), 99);
        assert!(parse_stat(&format!("43 (a) {fields}"), 42).is_err());
        assert!(parse_stat("42 (a) S", 42).is_err());
    }
    #[test]
    fn generations_never_wrap_and_policy_changes_advance() {
        let mut a = Authority::empty();
        let mut b = binding();
        assert_eq!(a.assign(&b, &None).unwrap(), 1);
        assert_eq!(a.assign(&b, &None).unwrap(), 1);
        b.policy_revision = "r2".into();
        assert_eq!(a.assign(&b, &None).unwrap(), 2);
        a.counter = MAX_GENERATION;
        b.policy_revision = "r3".into();
        assert!(a.assign(&b, &None).is_err());
        assert!(serde_json::from_str::<Authority>("{}").is_err());
    }
    #[test]
    fn paths_and_symlinks_refused() {
        assert!(secure_open(Path::new("relative"), false).is_err());
        assert!(secure_open(Path::new("/proc/self"), true).is_err());
        for id in ["..", ".", "a/b", "a\n", ""] {
            assert!(!valid_id(id));
        }
    }
    #[test]
    fn command_deadline_is_bounded() {
        let executable = std::env::current_exe().unwrap();
        let started = Instant::now();
        assert!(
            bounded_command(
                executable.to_str().unwrap(),
                &["--exact", "tests::timeout_child", "--ignored"],
                Duration::from_millis(40),
                4096,
                true,
            )
            .is_err()
        );
        assert!(started.elapsed() >= Duration::from_millis(40));
        assert!(started.elapsed() < Duration::from_secs(2));
    }
    #[test]
    #[ignore = "bounded-command subprocess fixture"]
    fn timeout_child() {
        std::thread::sleep(Duration::from_secs(3));
    }
    #[test]
    fn every_runtime_identity_transition_advances() {
        let b = binding();
        let mut authority = Authority::empty();
        let mut identity = Identity {
            kernel_boot_id: "00000000-0000-0000-0000-000000000001".into(),
            container_id: "a".repeat(64),
            pid: 42,
            start_ticks: 100,
            cgroup: format!("/system.slice/docker-{}.scope", "a".repeat(64)),
            cgroup_inode: 9,
        };
        assert_eq!(authority.assign(&b, &Some(identity.clone())).unwrap(), 1);
        identity.start_ticks += 1;
        assert_eq!(authority.assign(&b, &Some(identity.clone())).unwrap(), 2);
        identity.cgroup_inode += 1;
        assert_eq!(authority.assign(&b, &Some(identity.clone())).unwrap(), 3);
        identity.kernel_boot_id = "00000000-0000-0000-0000-000000000002".into();
        assert_eq!(authority.assign(&b, &Some(identity.clone())).unwrap(), 4);
        assert_eq!(authority.assign(&b, &None).unwrap(), 5);
        assert_eq!(authority.assign(&b, &Some(identity)).unwrap(), 6);
    }
    #[test]
    fn unavailable_schema_has_no_process_authority() {
        let m = Manifest::new(&binding(), 1, 100, None);
        let v = serde_json::to_value(m).unwrap();
        assert_eq!(v.as_object().unwrap().len(), 15);
        for name in [
            "kernel_boot_id",
            "container_id",
            "pid",
            "start_ticks",
            "cgroup",
            "cgroup_inode",
        ] {
            assert!(v[name].is_null());
        }
    }
}
