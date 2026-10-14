//! Descriptor-relative protected path traversal shared by host fence and quota inventory.
//! The caller selects the trusted owner/anchor; this grants no mutation authority.
use std::ffi::{CString, OsStr};
use std::fs::{File, Metadata};
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, RawFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::path::{Component, Path};

/// Open existing metadata or a readable entry without following its final
/// symlink. Callers authenticate the parent and inspect the returned type.
pub fn open_at(directory: RawFd, name: &OsStr, flags: i32) -> Result<File, String> {
    if flags != libc::O_RDONLY
        && flags != libc::O_PATH
        && flags != (libc::O_RDONLY | libc::O_DIRECTORY)
    {
        return Err("unsupported protected path open flags".into());
    }
    let name = CString::new(name.as_bytes()).map_err(|_| "invalid quota path")?;
    let fd = unsafe {
        libc::openat(
            directory,
            name.as_ptr(),
            flags | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC,
        )
    };
    if fd < 0 {
        return Err(format!(
            "cannot open quota inventory entry: {}",
            io::Error::last_os_error()
        ));
    }
    Ok(unsafe { File::from_raw_fd(fd) })
}

fn metadata(file: &File) -> Result<Metadata, String> {
    file.metadata()
        .map_err(|_| "cannot inspect quota inventory entry".into())
}

pub fn protected_directory(file: &File, owner_uid: u32) -> Result<Metadata, String> {
    let meta = metadata(file)?;
    if !meta.is_dir() || meta.uid() != owner_uid || meta.mode() & 0o022 != 0 {
        return Err("unprotected quota directory ancestry".into());
    }
    Ok(meta)
}

/// The caller chooses the authority anchor; every descriptor, including that
/// anchor, must satisfy the same ownership and mode checks. Production anchors
/// at `/`; filesystem fixtures can instead supply their own private directory.
pub fn open_relative(anchor: &File, relative: &Path, owner_uid: u32) -> Result<File, String> {
    protected_directory(anchor, owner_uid)?;
    let mut directory = anchor
        .try_clone()
        .map_err(|_| "cannot duplicate quota directory anchor")?;
    for component in relative.components() {
        let Component::Normal(name) = component else {
            return Err("invalid quota root components".into());
        };
        directory = open_at(
            directory.as_raw_fd(),
            name,
            libc::O_RDONLY | libc::O_DIRECTORY,
        )?;
        protected_directory(&directory, owner_uid)?;
    }
    Ok(directory)
}

pub fn open_root(root: &Path, owner_uid: u32) -> Result<File, String> {
    if !root.is_absolute() {
        return Err("quota root must be absolute".into());
    }
    let anchor = open_at(
        libc::AT_FDCWD,
        OsStr::new("/"),
        libc::O_RDONLY | libc::O_DIRECTORY,
    )?;
    open_relative(
        &anchor,
        root.strip_prefix("/").map_err(|_| "invalid quota root")?,
        owner_uid,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_flags_cannot_create_or_modify_entries() {
        for flags in [
            libc::O_RDWR,
            libc::O_WRONLY,
            libc::O_RDONLY | libc::O_CREAT,
            libc::O_RDONLY | libc::O_TRUNC,
            libc::O_TMPFILE | libc::O_RDWR,
        ] {
            assert_eq!(
                open_at(-1, OsStr::new("entry"), flags).unwrap_err(),
                "unsupported protected path open flags"
            );
        }
    }
}
