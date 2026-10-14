//! Protected credential and explicit public CA files. Callers authenticate
//! their own directory ancestry; this reader brackets the held and named file.
use std::fs::{File, Metadata};
use std::io::Read;
use std::os::fd::AsRawFd;
use std::os::unix::fs::MetadataExt;
use std::path::Path;
use tentaflake_admission::protected_path::open_at;

pub(super) const MAX_SECRET: usize = 4096;
pub(super) const MAX_CA: usize = 64 * 1024;

#[derive(Clone, Copy, Debug)]
pub(super) struct Error;

pub(super) fn secret_shape(secret: &[u8]) -> bool {
    (32..=MAX_SECRET).contains(&secret.len()) && secret.iter().all(u8::is_ascii_graphic)
}

pub(super) fn absolute_path(path: &Path) -> bool {
    path.to_str().is_some_and(|value| {
        value.starts_with('/')
            && value.len() <= 4096
            && value.bytes().all(|b| b.is_ascii_graphic())
            && value
                .split('/')
                .skip(1)
                .all(|part| !part.is_empty() && part != "." && part != "..")
    })
}

pub(super) fn runtime_path(path: &Path) -> bool {
    absolute_path(path) && !path.starts_with("/nix/store")
}

fn metadata(file: &File, owner: u32) -> Result<Metadata, Error> {
    let metadata = file.metadata().map_err(|_| Error)?;
    if !metadata.is_file()
        || metadata.uid() != owner
        || metadata.nlink() != 1
        || !matches!(metadata.mode() & 0o7777, 0o400 | 0o600)
        || !(32..=MAX_SECRET as u64 + 1).contains(&metadata.len())
    {
        return Err(Error);
    }
    Ok(metadata)
}

fn same_file(left: &Metadata, right: &Metadata) -> bool {
    left.dev() == right.dev()
        && left.ino() == right.ino()
        && left.uid() == right.uid()
        && left.gid() == right.gid()
        && left.mode() == right.mode()
        && left.len() == right.len()
        && left.mtime() == right.mtime()
        && left.mtime_nsec() == right.mtime_nsec()
        && left.ctime() == right.ctime()
        && left.ctime_nsec() == right.ctime_nsec()
}

pub(super) fn read_at(
    directory: &File,
    name: &std::ffi::OsStr,
    owner: u32,
    reopen: impl FnOnce() -> Result<File, Error>,
) -> Result<Vec<u8>, Error> {
    let mut bytes = read_checked_at(
        directory,
        name,
        MAX_SECRET as u64 + 1,
        |file| metadata(file, owner),
        reopen,
    )?;
    if bytes.last() == Some(&b'\n') {
        bytes.pop();
    }
    if !secret_shape(&bytes) {
        return Err(Error);
    }
    Ok(bytes)
}

/// Public CA material may be readable by others, but never writable by them.
pub(super) fn read_ca_at(
    directory: &File,
    name: &std::ffi::OsStr,
    owner: u32,
    reopen: impl FnOnce() -> Result<File, Error>,
) -> Result<Vec<u8>, Error> {
    read_checked_at(
        directory,
        name,
        MAX_CA as u64,
        |file| {
            let value = file.metadata().map_err(|_| Error)?;
            if !value.is_file()
                || ![0, owner].contains(&value.uid())
                || value.nlink() != 1
                || value.mode() & 0o7022 != 0
                || !(1..=MAX_CA as u64).contains(&value.len())
            {
                return Err(Error);
            }
            Ok(value)
        },
        reopen,
    )
}

fn read_checked_at(
    directory: &File,
    name: &std::ffi::OsStr,
    limit: u64,
    metadata: impl Fn(&File) -> Result<Metadata, Error>,
    reopen: impl FnOnce() -> Result<File, Error>,
) -> Result<Vec<u8>, Error> {
    let file = open_at(directory.as_raw_fd(), name, libc::O_RDONLY).map_err(|_| Error)?;
    let before = metadata(&file)?;
    let mut bytes = Vec::new();
    (&file)
        .take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| Error)?;
    let current_directory = reopen()?;
    let old = directory.metadata().map_err(|_| Error)?;
    let current = current_directory.metadata().map_err(|_| Error)?;
    if old.dev() != current.dev() || old.ino() != current.ino() {
        return Err(Error);
    }
    let named = open_at(current_directory.as_raw_fd(), name, libc::O_PATH).map_err(|_| Error)?;
    if !same_file(&before, &metadata(&file)?) || !same_file(&before, &metadata(&named)?) {
        return Err(Error);
    }
    if bytes.len() as u64 != before.len() || bytes.len() as u64 > limit {
        return Err(Error);
    }
    Ok(bytes)
}
