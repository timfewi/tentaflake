use std::fs::{self, OpenOptions};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::Path;
use std::time::{Instant, SystemTime, UNIX_EPOCH};
use tentaflake_workloads::*;

fn load_policy() -> Result<Policy> {
    // /etc's NixOS link is allowed only when its final immutable store target passes the descriptor walk.
    let path = fs::canonicalize("/etc/tentaflake-workloads.json")?;
    if !path.starts_with("/nix/store") {
        return Err(invalid());
    }
    let mut p: Policy = serde_json::from_slice(&bounded_read(secure_open(&path, false)?, 65_536)?)
        .map_err(|_| invalid())?;
    p.docker = fs::canonicalize(&p.docker)?
        .to_str()
        .ok_or_else(invalid)?
        .into();
    p.systemctl = fs::canonicalize(&p.systemctl)?
        .to_str()
        .ok_or_else(invalid)?
        .into();
    validate_policy(&p)?;
    Ok(p)
}
fn run() -> Result<()> {
    if unsafe { libc::geteuid() } != 0 {
        return Err(invalid());
    }
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.len() != 1 || !["--initialize", "--run", "--once"].contains(&args[0].as_str()) {
        return Err(invalid());
    }
    let state_dir = secure_open(Path::new(STATE), true)?;
    if state_dir.metadata()?.mode() & 0o077 != 0 {
        return Err(invalid());
    }
    let runtime_dir = secure_open(Path::new(RUNTIME), true)?;
    if runtime_dir.metadata()?.mode() & 0o777 != 0o755 {
        return Err(invalid());
    }
    let lock_path = format!("/proc/self/fd/{}/lock", state_dir.as_raw_fd());
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(lock_path)?;
    if lock.metadata()?.uid() != 0
        || !lock.metadata()?.is_file()
        || lock.metadata()?.mode() & 0o077 != 0
        || unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0
    {
        return Err(invalid());
    }
    let authority_path = format!("{STATE}/authority.json");
    if args[0] == "--initialize" {
        match fs::symlink_metadata(&authority_path) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
            _ => return Err(invalid()),
        }
        // Operator-only bootstrap. The normal service never creates/reset missing authority.
        return atomic_json(STATE, "authority.json", &Authority::empty(), 0o600);
    }
    let p = load_policy()?;
    let authority_file = secure_open(Path::new(&authority_path), false)?;
    if authority_file.metadata()?.mode() & 0o077 != 0 {
        return Err(invalid());
    }
    let mut authority: Authority =
        serde_json::from_slice(&bounded_read(authority_file, 262_144)?).map_err(|_| invalid())?;
    authority.validate()?;
    // Revoke removed policy entries before any observation; no unbounded directory enumeration.
    let retired: Vec<_> = authority
        .entries
        .keys()
        .filter(|id| !p.bindings.iter().any(|b| &b.agent_id == *id))
        .cloned()
        .collect();
    for id in retired {
        let path = format!("/proc/self/fd/{}/{id}.json", runtime_dir.as_raw_fd());
        match fs::remove_file(path) {
            Ok(()) => (),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
            Err(e) => return Err(e),
        }
        runtime_dir.sync_all()?;
        authority.entries.remove(&id);
    }
    atomic_json(STATE, "authority.json", &authority, 0o600)?;
    loop {
        let started = Instant::now();
        // OBSERVATION_COMMANDS bounded commands for all bindings; the lease is
        // stamped after the second verification, not at the later publication.
        let identities = observe_all(&p);
        let observed = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| invalid())?
            .as_millis();
        let observed = u64::try_from(observed).map_err(|_| invalid())?;
        for (b, identity) in p.bindings.iter().zip(identities) {
            let previous_counter = authority.counter;
            let generation = authority.assign(b, &identity)?;
            // Persist the allocation before exposing it, including an unavailable transition.
            if authority.counter != previous_counter {
                atomic_json(STATE, "authority.json", &authority, 0o600)?;
            }
            let manifest = Manifest::new(b, generation, observed, identity);
            atomic_json(RUNTIME, &format!("{}.json", b.agent_id), &manifest, 0o444)?;
        }
        if args[0] == "--once" {
            return Ok(());
        }
        // Cycles start at most every CYCLE; a slow cycle starts the next at once.
        std::thread::sleep(CYCLE.saturating_sub(started.elapsed()));
    }
}
fn main() {
    if run().is_err() {
        // No raw engine output, paths, process contents, policy scope or measurements in logs.
        eprintln!("tentaflake-workloads: authority unavailable; operator reconciliation required");
        std::process::exit(1);
    }
}
