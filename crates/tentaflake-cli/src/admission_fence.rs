//! Outer host mutation fence. Unlike the transactional ledger lock, this lock
//! must survive supervisor death while a trusted helper still holds its FD.

use crate::admission_storage::open_root;
use std::ffi::CString;
use std::fs::{File, Metadata};
use std::io;
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

const FENCE_PATH: &str = "/var/lib/tentaflake-admission.lock";
const LOCK_TIMEOUT: Duration = Duration::from_secs(5);

/// Closing this descriptor releases only this owner's reference. Never issue
/// LOCK_UN: that would also unlock the description inherited by live helpers.
#[must_use = "retain the host fence throughout the supervised mutation"]
#[derive(Debug)]
pub(crate) struct HostFence {
    file: File,
}

pub(crate) fn exclusive() -> Result<HostFence, String> {
    if unsafe { libc::geteuid() } != 0 {
        return Err("host admission fence requires root".into());
    }
    acquire(Path::new(FENCE_PATH), 0, false, LOCK_TIMEOUT)
}

impl HostFence {
    /// Explicitly pass this fence to one trusted mutator command and its
    /// descendants. The parent's FDs remain CLOEXEC; only the duplicate in the
    /// forked child has CLOEXEC cleared immediately before exec.
    ///
    /// Command owns a duplicate until it is dropped, even after spawn or failure.
    /// Helpers must preserve inherited FDs. This provides no execution authority
    /// by itself; the caller still owns ledger admission and service supervision.
    pub(crate) fn inherit(&self, command: &mut Command) -> Result<(), String> {
        let file = self
            .file
            .try_clone()
            .map_err(|_| "cannot duplicate host admission fence")?;
        // SAFETY: the owned File keeps the descriptor valid for every spawn of
        // this Command. The post-fork callback only uses async-signal-safe fcntl.
        unsafe {
            command.pre_exec(move || {
                let flags = libc::fcntl(file.as_raw_fd(), libc::F_GETFD);
                if flags < 0
                    || libc::fcntl(file.as_raw_fd(), libc::F_SETFD, flags & !libc::FD_CLOEXEC) < 0
                {
                    return Err(io::Error::last_os_error());
                }
                Ok(())
            });
        }
        Ok(())
    }
}

fn inspect(file: &File, owner_uid: u32) -> Result<Metadata, String> {
    let metadata = file
        .metadata()
        .map_err(|_| "cannot inspect host admission fence")?;
    if !metadata.is_file()
        || metadata.uid() != owner_uid
        || metadata.mode() & 0o7777 != 0o600
        || metadata.nlink() != 1
    {
        return Err("host admission fence must be a protected single-link regular file".into());
    }
    Ok(metadata)
}

fn open_at(parent: &File, name: &CString, flags: i32) -> Result<File, io::Error> {
    let fd = unsafe {
        libc::openat(
            parent.as_raw_fd(),
            name.as_ptr(),
            flags | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC,
            0o600,
        )
    };
    if fd < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(unsafe { File::from_raw_fd(fd) })
    }
}

fn identity_matches(left: &Metadata, right: &Metadata) -> bool {
    left.dev() == right.dev() && left.ino() == right.ino()
}

fn verify_path(parent: &File, name: &CString, file: &File, owner_uid: u32) -> Result<(), String> {
    let current =
        open_at(parent, name, libc::O_PATH).map_err(|_| "cannot resolve host admission fence")?;
    if !identity_matches(&inspect(file, owner_uid)?, &inspect(&current, owner_uid)?) {
        return Err("host admission fence path changed".into());
    }
    Ok(())
}

fn acquire(
    path: &Path,
    owner_uid: u32,
    fixture: bool,
    timeout: Duration,
) -> Result<HostFence, String> {
    let deadline = Instant::now() + timeout;
    let parent_path = path.parent().ok_or("invalid host admission fence path")?;
    let parent = open_root(parent_path, owner_uid, fixture)?;
    let name = CString::new(
        path.file_name()
            .ok_or("invalid host admission fence path")?
            .as_bytes(),
    )
    .map_err(|_| "invalid host admission fence name")?;
    let file = match open_at(&parent, &name, libc::O_RDWR | libc::O_CREAT | libc::O_EXCL) {
        Ok(file) => {
            // Only a newly created inode gets its mode established. Existing
            // paths are never repaired, replaced, truncated or unlinked.
            if unsafe { libc::fchmod(file.as_raw_fd(), 0o600) } != 0 {
                return Err("cannot protect new host admission fence".into());
            }
            inspect(&file, owner_uid)?;
            file.sync_all()
                .map_err(|_| "cannot sync host admission fence")?;
            parent
                .sync_all()
                .map_err(|_| "cannot sync host admission fence parent")?;
            file
        }
        Err(error) if error.raw_os_error() == Some(libc::EEXIST) => {
            // Inspect without opening devices or FIFOs for I/O first.
            let before = open_at(&parent, &name, libc::O_PATH)
                .map_err(|_| "cannot inspect existing host admission fence")?;
            let before = inspect(&before, owner_uid)?;
            let file = open_at(&parent, &name, libc::O_RDWR)
                .map_err(|_| "cannot open existing host admission fence")?;
            if !identity_matches(&before, &inspect(&file, owner_uid)?) {
                return Err("host admission fence changed while opening".into());
            }
            file
        }
        Err(_) => return Err("cannot create host admission fence".into()),
    };
    loop {
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
            break;
        }
        let error = io::Error::last_os_error();
        if !matches!(error.raw_os_error(), Some(libc::EWOULDBLOCK | libc::EINTR)) {
            return Err("cannot acquire host admission fence".into());
        }
        let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
            return Err(
                "host admission fence is busy; another mutation or helper still owns it".into(),
            );
        };
        std::thread::sleep(remaining.min(Duration::from_millis(20)));
    }
    // Recheck after waiting: neither a renamed ancestor nor a replaced lock
    // inode may create a second locking domain under the fixed path.
    let current_parent = open_root(parent_path, owner_uid, fixture)?;
    if !identity_matches(
        &parent
            .metadata()
            .map_err(|_| "cannot inspect fence parent")?,
        &current_parent
            .metadata()
            .map_err(|_| "cannot inspect fence parent")?,
    ) {
        return Err("host admission fence parent changed".into());
    }
    verify_path(&current_parent, &name, &file, owner_uid)?;
    Ok(HostFence { file })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::{self, DirBuilder};
    use std::io::{Read, Write};
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt, symlink};
    use std::path::PathBuf;
    use std::process::{Child, Stdio};
    use std::sync::atomic::{AtomicU64, Ordering};

    struct Fixture(PathBuf);

    impl Fixture {
        fn new() -> Self {
            static SEQUENCE: AtomicU64 = AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!(
                "tentaflake-fence-{}-{}",
                std::process::id(),
                SEQUENCE.fetch_add(1, Ordering::Relaxed),
            ));
            DirBuilder::new().mode(0o700).create(&path).unwrap();
            Self(path)
        }

        fn path(&self) -> PathBuf {
            self.0.join("admission.lock")
        }

        fn acquire(&self) -> Result<HostFence, String> {
            acquire(
                &self.path(),
                unsafe { libc::geteuid() },
                true,
                Duration::ZERO,
            )
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    struct ChildGuard(Child);

    impl Drop for ChildGuard {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    #[test]
    fn lock_excludes_other_open_descriptions_without_removing_or_truncating_file() {
        let fixture = Fixture::new();
        let guard = fixture.acquire().unwrap();
        let metadata = fs::metadata(fixture.path()).unwrap();
        assert_eq!(metadata.mode() & 0o7777, 0o600);
        assert_eq!(metadata.nlink(), 1);
        assert!(fixture.acquire().unwrap_err().contains("busy"));
        drop(guard);
        assert_eq!(metadata.ino(), fs::metadata(fixture.path()).unwrap().ino());
        fs::write(fixture.path(), b"preserve").unwrap();
        drop(fixture.acquire().unwrap());
        assert_eq!(fs::read(fixture.path()).unwrap(), b"preserve");
    }

    #[test]
    fn refuses_unsafe_existing_file_and_ancestry_without_repair() {
        let fixture = Fixture::new();
        drop(fixture.acquire().unwrap());
        fs::set_permissions(fixture.path(), fs::Permissions::from_mode(0o640)).unwrap();
        assert!(fixture.acquire().is_err());
        assert_eq!(fs::metadata(fixture.path()).unwrap().mode() & 0o7777, 0o640);
        fs::set_permissions(fixture.path(), fs::Permissions::from_mode(0o600)).unwrap();
        let alias = fixture.0.join("alias");
        fs::hard_link(fixture.path(), &alias).unwrap();
        assert!(fixture.acquire().is_err());
        fs::remove_file(&alias).unwrap();
        fs::set_permissions(&fixture.0, fs::Permissions::from_mode(0o777)).unwrap();
        assert!(fixture.acquire().is_err());
        fs::set_permissions(&fixture.0, fs::Permissions::from_mode(0o700)).unwrap();
        fs::rename(fixture.path(), &alias).unwrap();
        symlink(&alias, fixture.path()).unwrap();
        assert!(fixture.acquire().is_err());
        assert!(
            fs::symlink_metadata(fixture.path())
                .unwrap()
                .file_type()
                .is_symlink()
        );
        fs::remove_file(fixture.path()).unwrap();
        let name = CString::new(fixture.path().as_os_str().as_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
        assert!(fixture.acquire().is_err());
    }

    #[test]
    fn path_verification_refuses_replaced_lock_inode() {
        let fixture = Fixture::new();
        let guard = fixture.acquire().unwrap();
        fs::rename(fixture.path(), fixture.0.join("old.lock")).unwrap();
        fs::write(fixture.path(), b"").unwrap();
        fs::set_permissions(fixture.path(), fs::Permissions::from_mode(0o600)).unwrap();
        let uid = unsafe { libc::geteuid() };
        let parent = open_root(&fixture.0, uid, true).unwrap();
        let name = CString::new("admission.lock").unwrap();
        assert!(verify_path(&parent, &name, &guard.file, uid).is_err());
    }

    #[test]
    fn failed_spawn_retains_only_the_commands_owned_duplicate() {
        let fixture = Fixture::new();
        let guard = fixture.acquire().unwrap();
        let mut command = Command::new(fixture.0.join("missing-helper"));
        guard.inherit(&mut command).unwrap();
        drop(guard);
        assert!(command.spawn().is_err());
        assert!(fixture.acquire().unwrap_err().contains("busy"));
        drop(command);
        drop(fixture.acquire().unwrap());
    }

    #[test]
    fn exec_child_keeps_fence_after_parent_drops_every_copy() {
        let fixture = Fixture::new();
        let guard = fixture.acquire().unwrap();
        let ready = fixture.0.join("ready");
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .args(["--exact", "admission_fence::tests::inherited_helper"])
            .env("TENTAFLAKE_FENCE_TEST_CHILD", &ready)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        guard.inherit(&mut command).unwrap();
        assert_ne!(
            unsafe { libc::fcntl(guard.file.as_raw_fd(), libc::F_GETFD) } & libc::FD_CLOEXEC,
            0
        );
        let mut child = ChildGuard(command.spawn().unwrap());
        drop(command);
        drop(guard);
        let deadline = Instant::now() + Duration::from_secs(5);
        while !ready.exists() {
            assert!(
                child.0.try_wait().unwrap().is_none(),
                "helper exited before readiness"
            );
            assert!(Instant::now() < deadline, "helper did not become ready");
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(fixture.acquire().unwrap_err().contains("busy"));
        child.0.stdin.take().unwrap().write_all(b"x").unwrap();
        while child.0.try_wait().unwrap().is_none() {
            assert!(Instant::now() < deadline, "helper did not exit");
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(child.0.wait().unwrap().success());
        drop(fixture.acquire().unwrap());
    }

    #[test]
    fn inherited_helper() {
        let Some(ready) = std::env::var_os("TENTAFLAKE_FENCE_TEST_CHILD") else {
            return;
        };
        fs::write(ready, b"ready").unwrap();
        let mut byte = [0];
        io::stdin().read_exact(&mut byte).unwrap();
    }
}
