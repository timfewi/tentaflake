use std::ffi::CString;
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Component, Path};

pub type Result<T, E = String> = std::result::Result<T, E>;
pub const MAX_DOCUMENT: usize = 262_144;
pub const MAX_STATE: usize = 32 * 1024 * 1024;
fn err() -> String {
    "private filesystem operation refused".into()
}
fn name(value: &str) -> Result<CString> {
    CString::new(value).map_err(|_| err())
}

/// Walk every component through directory descriptors; never follow a symlink.
pub fn open_path(path: &Path, directory: bool) -> Result<File> {
    if !path.is_absolute() {
        return Err(err());
    }
    let mut file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open("/")
        .map_err(|_| err())?;
    let parts: Vec<_> = path.components().collect();
    for (i, component) in parts.iter().enumerate().skip(1) {
        let Component::Normal(part) = component else {
            return Err(err());
        };
        let c = CString::new(part.as_encoded_bytes()).map_err(|_| err())?;
        let flags = libc::O_RDONLY
            | libc::O_NOFOLLOW
            | libc::O_CLOEXEC
            | libc::O_NONBLOCK
            | if i + 1 < parts.len() || directory {
                libc::O_DIRECTORY
            } else {
                0
            };
        let fd = unsafe { libc::openat(file.as_raw_fd(), c.as_ptr(), flags) };
        if fd < 0 {
            return Err(err());
        }
        file = unsafe { File::from_raw_fd(fd) };
    }
    let meta = file.metadata().map_err(|_| err())?;
    if (directory && !meta.is_dir()) || (!directory && !meta.is_file()) {
        return Err(err());
    }
    Ok(file)
}
pub fn read_file(path: &Path, limit: usize) -> Result<Vec<u8>> {
    read_bounded(open_path(path, false)?, limit)
}
pub fn read_bounded(file: File, limit: usize) -> Result<Vec<u8>> {
    let mut data = Vec::new();
    file.take((limit + 1) as u64)
        .read_to_end(&mut data)
        .map_err(|_| err())?;
    if data.len() > limit {
        return Err("document exceeds limit".into());
    }
    Ok(data)
}

/// Path-based child exec/candidate creation is safe only below non-replaceable ancestors.
/// Root-owned sticky temporary roots are permitted; another UID cannot rename our child.
pub fn secure_ancestry(path: &Path) -> Result<()> {
    let uid = unsafe { libc::geteuid() };
    let mut prefix = std::path::PathBuf::from("/");
    for component in path.components().skip(1) {
        let Component::Normal(part) = component else {
            return Err(err());
        };
        prefix.push(part);
        let metadata = open_path(&prefix, true)?.metadata().map_err(|_| err())?;
        let root_sticky = metadata.uid() == 0 && metadata.mode() & 0o1000 != 0;
        if (metadata.uid() != 0 && metadata.uid() != uid)
            || (metadata.mode() & 0o022 != 0 && !root_sticky)
        {
            return Err(err());
        }
    }
    Ok(())
}

pub struct PrivateDir {
    pub dir: File,
    _lock: File,
}
impl PrivateDir {
    pub fn lock(path: &Path) -> Result<Self> {
        secure_ancestry(path)?;
        let dir = open_path(path, true)?;
        let m = dir.metadata().map_err(|_| err())?;
        if m.uid() != unsafe { libc::geteuid() } || m.mode() & 0o077 != 0 {
            return Err(err());
        }
        let c = name("writer.lock")?;
        let fd = unsafe {
            libc::openat(
                dir.as_raw_fd(),
                c.as_ptr(),
                libc::O_RDWR
                    | libc::O_CREAT
                    | libc::O_NOFOLLOW
                    | libc::O_CLOEXEC
                    | libc::O_NONBLOCK,
                0o600,
            )
        };
        if fd < 0 {
            return Err(err());
        }
        let lock = unsafe { File::from_raw_fd(fd) };
        let m = lock.metadata().map_err(|_| err())?;
        if !m.is_file()
            || m.nlink() != 1
            || m.uid() != unsafe { libc::geteuid() }
            || m.mode() & 0o077 != 0
        {
            return Err(err());
        }
        if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            return Err("state already has a writer".into());
        }
        Ok(Self { dir, _lock: lock })
    }
    pub fn read(&self, filename: &str, limit: usize) -> Result<Option<Vec<u8>>> {
        let c = name(filename)?;
        let fd = unsafe {
            libc::openat(
                self.dir.as_raw_fd(),
                c.as_ptr(),
                libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK,
            )
        };
        if fd < 0 {
            if std::io::Error::last_os_error().raw_os_error() == Some(libc::ENOENT) {
                return Ok(None);
            }
            return Err(err());
        }
        let file = unsafe { File::from_raw_fd(fd) };
        let m = file.metadata().map_err(|_| err())?;
        if !m.is_file()
            || m.nlink() != 1
            || m.uid() != unsafe { libc::geteuid() }
            || m.mode() & 0o077 != 0
        {
            return Err(err());
        }
        read_bounded(file, limit).map(Some)
    }
    pub fn atomic(&self, filename: &str, bytes: &[u8]) -> Result<()> {
        if bytes.len() > MAX_STATE {
            return Err("state capacity exhausted".into());
        }
        let temporary = name("receipt.pending")?;
        // A leftover temporary is never a dispatched request or recovery authority.
        unsafe {
            libc::unlinkat(self.dir.as_raw_fd(), temporary.as_ptr(), 0);
        }
        let fd = unsafe {
            libc::openat(
                self.dir.as_raw_fd(),
                temporary.as_ptr(),
                libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW | libc::O_CLOEXEC,
                0o600,
            )
        };
        if fd < 0 {
            return Err(err());
        }
        let mut file = unsafe { File::from_raw_fd(fd) };
        file.write_all(bytes)
            .and_then(|_| file.sync_all())
            .map_err(|_| err())?;
        let target = name(filename)?;
        if unsafe {
            libc::renameat(
                self.dir.as_raw_fd(),
                temporary.as_ptr(),
                self.dir.as_raw_fd(),
                target.as_ptr(),
            )
        } != 0
        {
            return Err(err());
        }
        self.dir.sync_all().map_err(|_| err())
    }
}
