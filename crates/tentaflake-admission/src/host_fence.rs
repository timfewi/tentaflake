//! Outer host mutation fence. Unlike the transactional ledger lock, this lock
//! must survive supervisor death while a trusted helper still holds its FD.

use crate::protected_path::open_root;
use std::ffi::CString;
use std::fs::{File, Metadata};
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, RawFd};
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
pub struct HostFence {
    file: File,
}

pub fn exclusive() -> Result<HostFence, String> {
    if crate::effective_uid() != 0 {
        return Err("host admission fence requires root".into());
    }
    let path = Path::new(FENCE_PATH);
    acquire(path, 0, LOCK_TIMEOUT, || {
        open_root(path.parent().ok_or("invalid host admission fence path")?, 0)
    })
}

impl HostFence {
    /// Explicitly pass this fence to one trusted mutator command and its
    /// descendants. The parent's FDs remain CLOEXEC; only the duplicate in the
    /// forked child has CLOEXEC cleared immediately before exec.
    ///
    /// Command owns a duplicate until it is dropped, even after spawn or failure.
    /// The returned number is that duplicate in the child, suitable for an
    /// explicit private argument. It is not a borrowed descriptor to close.
    /// Helpers must preserve inherited FDs. This provides no execution authority
    /// by itself; the caller still owns ledger admission and service supervision.
    pub fn inherit(&self, command: &mut Command) -> Result<RawFd, String> {
        let file = self
            .file
            .try_clone()
            .map_err(|_| "cannot duplicate host admission fence")?;
        let inherited_fd = file.as_raw_fd();
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
        Ok(inherited_fd)
    }

    /// Adopt a trusted child's inherited descriptor as synchronization only.
    /// The guard that launched this child must authenticate installed policy and
    /// ledger authority separately. This does not establish a dispatch grant.
    ///
    /// The owned descriptor becomes CLOEXEC immediately. Further trusted
    /// mutators receive their own explicit duplicate through `inherit`.
    pub fn adopt(file: File) -> Result<Self, String> {
        if crate::effective_uid() != 0 {
            return Err("host admission fence requires root".into());
        }
        let path = Path::new(FENCE_PATH);
        adopt_at(file, path, 0, || {
            open_root(path.parent().ok_or("invalid host admission fence path")?, 0)
        })
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
    // SAFETY: The held parent and NUL-terminated name remain live; the create mode is supplied.
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
        // SAFETY: openat returned a new descriptor; File takes its sole ownership.
        Ok(unsafe { File::from_raw_fd(fd) })
    }
}

fn identity_matches(left: &Metadata, right: &Metadata) -> bool {
    left.dev() == right.dev() && left.ino() == right.ino()
}

fn open_existing(parent: &File, name: &CString, owner_uid: u32) -> Result<Option<File>, String> {
    // Inspect before opening for I/O, so an unexpected device or FIFO cannot
    // acquire side effects. Only genuine absence permits a later create.
    let before = match open_at(parent, name, libc::O_PATH) {
        Ok(file) => file,
        Err(error) if error.raw_os_error() == Some(libc::ENOENT) => return Ok(None),
        Err(_) => return Err("cannot inspect existing host admission fence".into()),
    };
    let before = inspect(&before, owner_uid)?;
    let file = open_at(parent, name, libc::O_RDWR)
        .map_err(|_| "cannot open existing host admission fence")?;
    if !identity_matches(&before, &inspect(&file, owner_uid)?) {
        return Err("host admission fence changed while opening".into());
    }
    Ok(Some(file))
}

fn verify_path(parent: &File, name: &CString, file: &File, owner_uid: u32) -> Result<(), String> {
    let current =
        open_at(parent, name, libc::O_PATH).map_err(|_| "cannot resolve host admission fence")?;
    if !identity_matches(&inspect(file, owner_uid)?, &inspect(&current, owner_uid)?) {
        return Err("host admission fence path changed".into());
    }
    Ok(())
}

fn fence_name(path: &Path) -> Result<CString, String> {
    CString::new(
        path.file_name()
            .ok_or("invalid host admission fence path")?
            .as_bytes(),
    )
    .map_err(|_| "invalid host admission fence name".into())
}

fn verify_after_lock(
    parent: &File,
    name: &CString,
    file: &File,
    owner_uid: u32,
    reopen_parent: impl Fn() -> Result<File, String>,
) -> Result<(), String> {
    let current_parent = reopen_parent()?;
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
    verify_path(&current_parent, name, file, owner_uid)
}

fn adopt_at(
    file: File,
    path: &Path,
    owner_uid: u32,
    reopen_parent: impl Fn() -> Result<File, String>,
) -> Result<HostFence, String> {
    let fd = file.as_raw_fd();
    // SAFETY: file holds this descriptor; F_GETFD takes no pointer argument.
    let descriptor_flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
    if descriptor_flags < 0
        // SAFETY: file remains live and F_SETFD receives integer descriptor flags.
        || unsafe { libc::fcntl(fd, libc::F_SETFD, descriptor_flags | libc::FD_CLOEXEC) } < 0
    {
        return Err("cannot protect inherited host admission fence".into());
    }
    // SAFETY: file holds this descriptor; F_GETFL takes no pointer argument.
    let open_flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if open_flags < 0 || open_flags & libc::O_ACCMODE != libc::O_RDWR {
        return Err("inherited host admission fence is not read-write".into());
    }
    inspect(&file, owner_uid)?;
    let parent = reopen_parent()?;
    let name = fence_name(path)?;
    verify_path(&parent, &name, &file, owner_uid)?;
    // flock is attached to the open file description. An inherited duplicate
    // confirms the parent's exclusive lock; an unlocked exact descriptor can
    // establish synchronization, but never authenticates policy or authority.
    // SAFETY: file keeps fd live; flock takes only a descriptor and lock flags.
    if unsafe { libc::flock(fd, libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        return Err("inherited host admission fence is not exclusive".into());
    }
    verify_after_lock(&parent, &name, &file, owner_uid, reopen_parent)?;
    Ok(HostFence { file })
}

fn acquire(
    path: &Path,
    owner_uid: u32,
    timeout: Duration,
    reopen_parent: impl Fn() -> Result<File, String>,
) -> Result<HostFence, String> {
    let deadline = Instant::now() + timeout;
    let parent = reopen_parent()?;
    let name = fence_name(path)?;
    // Sandboxed callers may write only the existing lock inode while its
    // directory stays read-only. Do not request creation on that path.
    let file = if let Some(file) = open_existing(&parent, &name, owner_uid)? {
        file
    } else {
        match open_at(&parent, &name, libc::O_RDWR | libc::O_CREAT | libc::O_EXCL) {
            Ok(file) => {
                // Only a newly created inode gets its mode established. Existing
                // paths are never repaired, replaced, truncated or unlinked.
                // SAFETY: file owns the newly created descriptor; fchmod takes an integer mode.
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
                // Another owner created it after our absence observation. Retry
                // the checked existing path once; disappearance is not a new create.
                open_existing(&parent, &name, owner_uid)?
                    .ok_or("host admission fence disappeared after competing creation")?
            }
            Err(_) => return Err("cannot create host admission fence".into()),
        }
    };
    loop {
        // SAFETY: file remains live throughout the nonblocking flock call.
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
    verify_after_lock(&parent, &name, &file, owner_uid, reopen_parent)?;
    Ok(HostFence { file })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protected_path::open_relative;
    use std::cell::Cell;
    use std::fs::{self, DirBuilder, OpenOptions};
    use std::io::{Read, Write};
    use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt, symlink};
    use std::path::PathBuf;
    use std::process::{Child, Stdio};
    use std::sync::atomic::{AtomicU64, Ordering};

    struct Fixture {
        root: PathBuf,
        anchor_path: PathBuf,
        anchor: File,
    }

    impl Fixture {
        fn new() -> Self {
            static SEQUENCE: AtomicU64 = AtomicU64::new(0);
            let anchor_path = std::env::temp_dir().join(format!(
                "tentaflake-fence-{}-{}",
                std::process::id(),
                SEQUENCE.fetch_add(1, Ordering::Relaxed),
            ));
            DirBuilder::new().mode(0o700).create(&anchor_path).unwrap();
            let anchor = OpenOptions::new()
                .read(true)
                .custom_flags(
                    libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC,
                )
                .open(&anchor_path)
                .unwrap();
            let root = anchor_path.join("fences");
            DirBuilder::new().mode(0o700).create(&root).unwrap();
            Self {
                root,
                anchor_path,
                anchor,
            }
        }

        fn path(&self) -> PathBuf {
            self.root.join("admission.lock")
        }

        fn open_parent(&self) -> Result<File, String> {
            open_relative(&self.anchor, Path::new("fences"), crate::effective_uid())
        }

        fn acquire(&self) -> Result<HostFence, String> {
            acquire(&self.path(), crate::effective_uid(), Duration::ZERO, || {
                self.open_parent()
            })
        }

        fn adopt(&self, file: File) -> Result<HostFence, String> {
            adopt_at(file, &self.path(), crate::effective_uid(), || {
                self.open_parent()
            })
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.anchor_path);
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
    fn existing_fence_needs_no_parent_write_permission() {
        let fixture = Fixture::new();
        drop(fixture.acquire().unwrap());
        fs::write(fixture.path(), b"preserve").unwrap();
        let before = fs::metadata(fixture.path()).unwrap();
        fs::set_permissions(&fixture.root, fs::Permissions::from_mode(0o500)).unwrap();

        let guard = acquire(
            &fixture.path(),
            crate::effective_uid(),
            Duration::from_secs(2),
            || fixture.open_parent(),
        );
        let competing = fixture.acquire();
        let absent = acquire(
            &fixture.root.join("absent.lock"),
            crate::effective_uid(),
            Duration::ZERO,
            || fixture.open_parent(),
        );
        let create = fs::write(fixture.root.join("forbidden"), b"");
        let parent_mode = fs::metadata(&fixture.root).map(|meta| meta.mode() & 0o7777);
        // Restore only the test directory before assertions so cleanup also
        // succeeds if an assertion fails. This fixture requires unprivileged
        // test execution; a privileged permission bypass must fail, not skip.
        fs::set_permissions(&fixture.root, fs::Permissions::from_mode(0o700)).unwrap();

        let guard = guard.unwrap();
        assert!(competing.unwrap_err().contains("busy"));
        assert!(absent.is_err());
        assert_eq!(create.unwrap_err().kind(), io::ErrorKind::PermissionDenied);
        assert_eq!(parent_mode.unwrap(), 0o500);
        assert!(identity_matches(&before, &guard.file.metadata().unwrap()));
        assert_eq!(fs::read(fixture.path()).unwrap(), b"preserve");
        assert_eq!(fs::metadata(fixture.path()).unwrap().mode() & 0o7777, 0o600);
    }

    #[test]
    fn refuses_unsafe_existing_file_and_ancestry_without_repair() {
        let fixture = Fixture::new();
        drop(fixture.acquire().unwrap());
        fs::set_permissions(fixture.path(), fs::Permissions::from_mode(0o640)).unwrap();
        assert!(fixture.acquire().is_err());
        assert_eq!(fs::metadata(fixture.path()).unwrap().mode() & 0o7777, 0o640);
        fs::set_permissions(fixture.path(), fs::Permissions::from_mode(0o600)).unwrap();
        let alias = fixture.root.join("alias");
        fs::hard_link(fixture.path(), &alias).unwrap();
        assert!(fixture.acquire().is_err());
        fs::remove_file(&alias).unwrap();
        fs::set_permissions(&fixture.root, fs::Permissions::from_mode(0o777)).unwrap();
        assert!(fixture.acquire().is_err());
        fs::set_permissions(&fixture.root, fs::Permissions::from_mode(0o700)).unwrap();
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
        // SAFETY: name is a live NUL-terminated fixture path; mkfifo takes an integer mode.
        assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
        assert!(fixture.acquire().is_err());
    }

    #[test]
    fn path_verification_refuses_replaced_lock_inode() {
        let fixture = Fixture::new();
        let guard = fixture.acquire().unwrap();
        fs::rename(fixture.path(), fixture.root.join("old.lock")).unwrap();
        fs::write(fixture.path(), b"").unwrap();
        fs::set_permissions(fixture.path(), fs::Permissions::from_mode(0o600)).unwrap();
        let uid = crate::effective_uid();
        let parent = fixture.open_parent().unwrap();
        let name = CString::new("admission.lock").unwrap();
        assert!(verify_path(&parent, &name, &guard.file, uid).is_err());
    }

    #[test]
    fn final_parent_reopen_refuses_replacement_or_symlink() {
        for replace_with_symlink in [false, true] {
            let fixture = Fixture::new();
            let opens = Cell::new(0);
            let result = acquire(
                &fixture.path(),
                crate::effective_uid(),
                Duration::ZERO,
                || {
                    opens.set(opens.get() + 1);
                    if opens.get() == 2 {
                        let previous = fixture.anchor_path.join("previous-fences");
                        fs::rename(&fixture.root, &previous).unwrap();
                        if replace_with_symlink {
                            symlink(previous, &fixture.root).unwrap();
                        } else {
                            DirBuilder::new().mode(0o700).create(&fixture.root).unwrap();
                        }
                    }
                    fixture.open_parent()
                },
            );
            assert_eq!(opens.get(), 2);
            if replace_with_symlink {
                assert!(result.is_err());
            } else {
                assert_eq!(result.unwrap_err(), "host admission fence parent changed");
            }
        }
    }

    #[test]
    fn refuses_unprotected_anchor_before_creating_lock() {
        let fixture = Fixture::new();
        fs::set_permissions(&fixture.anchor_path, fs::Permissions::from_mode(0o1777)).unwrap();
        assert!(fixture.acquire().is_err());
        assert!(!fixture.path().exists());
        fs::set_permissions(&fixture.anchor_path, fs::Permissions::from_mode(0o700)).unwrap();
        drop(fixture.acquire().unwrap());
    }

    #[test]
    fn failed_spawn_retains_only_the_commands_owned_duplicate() {
        let fixture = Fixture::new();
        let guard = fixture.acquire().unwrap();
        let mut command = Command::new(fixture.root.join("missing-helper"));
        guard.inherit(&mut command).unwrap();
        drop(guard);
        assert!(command.spawn().is_err());
        assert!(fixture.acquire().unwrap_err().contains("busy"));
        drop(command);
        // Other test threads can fork while this FD exists. Their inherited
        // CLOEXEC copies legitimately survive until exec; a short bounded wait
        // distinguishes that window from a leaked command-owned duplicate.
        drop(
            acquire(
                &fixture.path(),
                crate::effective_uid(),
                Duration::from_secs(2),
                || fixture.open_parent(),
            )
            .unwrap(),
        );
    }

    #[test]
    fn adoption_preserves_same_description_and_sets_cloexec() {
        let fixture = Fixture::new();
        let original = fixture.acquire().unwrap();
        let file = original.file.try_clone().unwrap();
        assert_eq!(
            // SAFETY: file owns this descriptor; F_SETFD receives integer flags.
            unsafe { libc::fcntl(file.as_raw_fd(), libc::F_SETFD, 0) },
            0
        );
        let adopted = fixture.adopt(file).unwrap();
        assert_ne!(
            // SAFETY: adopted owns this descriptor; F_GETFD takes no pointer argument.
            unsafe { libc::fcntl(adopted.file.as_raw_fd(), libc::F_GETFD) } & libc::FD_CLOEXEC,
            0
        );
        drop(original);
        assert!(fixture.acquire().unwrap_err().contains("busy"));
        drop(adopted);
        drop(
            acquire(
                &fixture.path(),
                crate::effective_uid(),
                Duration::from_secs(2),
                || fixture.open_parent(),
            )
            .unwrap(),
        );
    }

    #[test]
    fn adoption_requires_exclusivity_and_can_lock_an_unheld_exact_description() {
        let fixture = Fixture::new();
        let original = fixture.acquire().unwrap();
        let open = || {
            OpenOptions::new()
                .read(true)
                .write(true)
                .open(fixture.path())
                .unwrap()
        };
        assert!(fixture.adopt(open()).unwrap_err().contains("not exclusive"));
        drop(original);
        // Concurrent test forks can still hold the first description until
        // exec. A fresh file tests genuinely unheld adoption without assuming
        // that dropping the parent's reference released every inherited copy.
        let unheld = Fixture::new();
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(unheld.path())
            .unwrap();
        let adopted = unheld.adopt(file).unwrap();
        assert!(unheld.acquire().unwrap_err().contains("busy"));
        drop(adopted);
    }

    #[test]
    fn adoption_refuses_foreign_modes_links_owner_and_path_without_repair() {
        let fixture = Fixture::new();
        let original = fixture.acquire().unwrap();
        assert!(
            fixture
                .adopt(File::open(fixture.path()).unwrap())
                .unwrap_err()
                .contains("not read-write")
        );
        assert!(
            adopt_at(
                original.file.try_clone().unwrap(),
                &fixture.path(),
                crate::effective_uid().wrapping_add(1),
                || fixture.open_parent()
            )
            .is_err()
        );
        fs::set_permissions(fixture.path(), fs::Permissions::from_mode(0o640)).unwrap();
        assert!(fixture.adopt(original.file.try_clone().unwrap()).is_err());
        assert_eq!(fs::metadata(fixture.path()).unwrap().mode() & 0o7777, 0o640);
        fs::set_permissions(fixture.path(), fs::Permissions::from_mode(0o600)).unwrap();
        let alias = fixture.root.join("alias");
        fs::hard_link(fixture.path(), &alias).unwrap();
        assert!(fixture.adopt(original.file.try_clone().unwrap()).is_err());
        fs::remove_file(&alias).unwrap();
        fs::rename(fixture.path(), &alias).unwrap();
        symlink(&alias, fixture.path()).unwrap();
        assert!(fixture.adopt(original.file.try_clone().unwrap()).is_err());
        fs::remove_file(fixture.path()).unwrap();
        fs::write(fixture.path(), []).unwrap();
        fs::set_permissions(fixture.path(), fs::Permissions::from_mode(0o600)).unwrap();
        assert!(
            fixture
                .adopt(original.file.try_clone().unwrap())
                .unwrap_err()
                .contains("path changed")
        );
        assert_eq!(
            fs::metadata(&alias).unwrap().ino(),
            original.file.metadata().unwrap().ino()
        );
    }

    #[test]
    fn adoption_reopens_protected_parent_after_locking() {
        let fixture = Fixture::new();
        let original = fixture.acquire().unwrap();
        let opens = Cell::new(0);
        let result = adopt_at(
            original.file.try_clone().unwrap(),
            &fixture.path(),
            crate::effective_uid(),
            || {
                opens.set(opens.get() + 1);
                if opens.get() == 2 {
                    fs::rename(&fixture.root, fixture.anchor_path.join("old-fences")).unwrap();
                    DirBuilder::new().mode(0o700).create(&fixture.root).unwrap();
                }
                fixture.open_parent()
            },
        );
        assert_eq!(opens.get(), 2);
        assert_eq!(result.unwrap_err(), "host admission fence parent changed");
    }

    #[test]
    fn exec_child_keeps_fence_after_parent_drops_every_copy() {
        check_exec_child(false);
    }

    #[test]
    fn exec_child_adopts_same_inherited_descriptor() {
        check_exec_child(true);
    }

    fn check_exec_child(adopt: bool) {
        let fixture = Fixture::new();
        let guard = fixture.acquire().unwrap();
        let ready = fixture.root.join("ready");
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .args(["--exact", "host_fence::tests::inherited_helper"])
            .env("TENTAFLAKE_FENCE_TEST_CHILD", &ready)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let inherited_fd = guard.inherit(&mut command).unwrap();
        command.env("TENTAFLAKE_FENCE_TEST_FD", inherited_fd.to_string());
        if adopt {
            command.env("TENTAFLAKE_FENCE_TEST_ADOPT", "1");
        }
        assert_ne!(
            // SAFETY: guard owns this descriptor; F_GETFD takes no pointer argument.
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
        let inherited_fd: RawFd = std::env::var("TENTAFLAKE_FENCE_TEST_FD")
            .unwrap()
            .parse()
            .unwrap();
        assert_eq!(
            // SAFETY: F_GETFD only inspects the inherited descriptor and reports invalid descriptors.
            unsafe { libc::fcntl(inherited_fd, libc::F_GETFD) } & libc::FD_CLOEXEC,
            0
        );
        let ready = PathBuf::from(ready);
        let _adopted = if std::env::var_os("TENTAFLAKE_FENCE_TEST_ADOPT").is_some() {
            let root = ready.parent().unwrap();
            let anchor = OpenOptions::new()
                .read(true)
                .custom_flags(
                    libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC,
                )
                .open(root.parent().unwrap())
                .unwrap();
            // The parent deliberately supplied this live owned descriptor by
            // inherit; no unvalidated integer becomes a File in production.
            // SAFETY: The test parent transferred this live inherited descriptor; no File owns it yet.
            let file = unsafe { File::from_raw_fd(inherited_fd) };
            Some(
                adopt_at(
                    file,
                    &root.join("admission.lock"),
                    crate::effective_uid(),
                    || open_relative(&anchor, Path::new("fences"), crate::effective_uid()),
                )
                .unwrap(),
            )
        } else {
            None
        };
        fs::write(ready, b"ready").unwrap();
        let mut byte = [0];
        io::stdin().read_exact(&mut byte).unwrap();
    }
}
