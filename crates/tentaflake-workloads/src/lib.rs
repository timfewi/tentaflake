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
    if p.version != 1 || p.bindings.len() > 64 {
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
pub fn parse_inspect(text: &str) -> Result<(String, u32)> {
    let mut values = text.split_whitespace();
    let id = values.next().ok_or_else(invalid)?;
    let pid = values
        .next()
        .ok_or_else(invalid)?
        .parse::<u32>()
        .map_err(|_| invalid())?;
    if id.len() != 64
        || !id
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        || values.next() != Some("true")
        || values.next().is_some()
        || pid <= 1
    {
        return Err(invalid());
    }
    Ok((id.into(), pid))
}

// No caller-controlled command or argument surface. Only the two observer operations use this.
fn bounded_command(executable: &str, args: &[&str], timeout: Duration) -> Result<String> {
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
                    if bytes.len() > 4096 {
                        return Err(invalid());
                    }
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => (),
                Err(e) => return Err(e),
            }
            if let Some(status) = child.try_wait()? {
                if !status.success() {
                    return Err(invalid());
                }
                // Drain only bounded bytes, including output delivered just before exit.
                loop {
                    match pipe.read(&mut buffer) {
                        Ok(0) => break,
                        Ok(n) => {
                            bytes.extend_from_slice(&buffer[..n]);
                            if bytes.len() > 4096 {
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
fn inspect(p: &Policy, b: &Binding) -> Result<(String, u32)> {
    parse_inspect(&bounded_command(
        &p.docker,
        &[
            "--host=unix:///run/docker.sock",
            "container",
            "inspect",
            "--format",
            "{{.Id}} {{.State.Pid}} {{.State.Running}}",
            "--",
            &b.agent_id,
        ],
        Duration::from_secs(1),
    )?)
}
fn unit_active(p: &Policy, b: &Binding) -> Result<()> {
    if bounded_command(
        &p.systemctl,
        &["show", "--property=ActiveState", "--value", "--", &b.unit],
        Duration::from_secs(1),
    )?
    .trim()
        != "active"
    {
        return Err(invalid());
    }
    Ok(())
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
pub fn observe(p: &Policy, b: &Binding) -> Result<Identity> {
    unit_active(p, b)?;
    let (container, pid) = inspect(p, b)?;
    let first = kernel_identity(container.clone(), pid)?;
    if inspect(p, b)? != (container.clone(), pid) {
        return Err(invalid());
    }
    unit_active(p, b)?;
    if kernel_identity(container, pid)? != first {
        return Err(invalid());
    }
    Ok(first)
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
            || self.entries.len() > 64
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
    fn inspect_closed() {
        let id = "a".repeat(64);
        assert_eq!(parse_inspect(&format!("{id} 42 true\n")).unwrap().1, 42);
        for s in [
            format!("{id} 42 false"),
            format!("{id} 0 true"),
            format!("{id} 42 true secret"),
            "short 42 true".into(),
        ] {
            assert!(parse_inspect(&s).is_err());
        }
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
                Duration::from_millis(40)
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
