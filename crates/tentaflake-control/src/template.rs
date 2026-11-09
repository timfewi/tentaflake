use crate::{
    digest,
    storage::{Result, open_path, read_file},
};
use serde::Deserialize;
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Builder {
    pub template: PathBuf,
    pub template_hash: String,
    pub nix: PathBuf,
    pub target: String,
    pub deadline_seconds: u64,
}
pub fn store_path(path: &Path) -> bool {
    path.is_absolute()
        && path.starts_with("/nix/store")
        && path.components().all(|c| {
            matches!(
                c,
                std::path::Component::RootDir | std::path::Component::Normal(_)
            )
        })
        && path.components().count() >= 4
}
pub fn output_path(path: &str) -> bool {
    let Some(item) = path.strip_prefix("/nix/store/") else {
        return false;
    };
    let Some((hash, name)) = item.split_once('-') else {
        return false;
    };
    hash.len() == 32
        && hash
            .bytes()
            .all(|b| b"0123456789abcdfghijklmnpqrsvwxyz".contains(&b))
        && !name.is_empty()
        && name.len() <= 211
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"+-._?=".contains(&b))
}
pub fn validate(builder: &Builder, inputs: &str) -> Result<()> {
    if !store_path(&builder.template)
        || !store_path(&builder.nix)
        || builder.target.is_empty()
        || builder.target.len() > 64
        || !builder
            .target
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
        || !(1..=3600).contains(&builder.deadline_seconds)
    {
        return Err("invalid fixed build configuration".into());
    }
    let executable = open_path(&builder.nix, false)?;
    let m = executable.metadata().map_err(|_| "builder unavailable")?;
    if m.uid() != 0 || m.mode() & 0o222 != 0 || m.mode() & 0o111 == 0 {
        return Err("mutable or non-executable builder refused".into());
    }
    if fingerprint(&builder.template)? != builder.template_hash
        || digest(&read_file(
            &builder.template.join("flake.lock"),
            1024 * 1024,
        )?) != inputs
    {
        return Err("reviewed build inputs changed".into());
    }
    Ok(())
}
fn tree(
    root: &Path,
    relative: &Path,
    files: &mut Vec<(PathBuf, Vec<u8>, u32)>,
    bytes: &mut usize,
    entries: &mut usize,
    owner: u32,
) -> Result<()> {
    let path = root.join(relative);
    let dir = open_path(&path, true)?;
    let m = dir.metadata().map_err(|_| "template unavailable")?;
    if m.uid() != owner || m.mode() & 0o222 != 0 {
        return Err("mutable template refused".into());
    }
    let mut children: Vec<_> = fs::read_dir(&path)
        .map_err(|_| "template unavailable")?
        .take(4097)
        .collect::<std::io::Result<Vec<_>>>()
        .map_err(|_| "template unavailable")?;
    children.sort_by_key(|e| e.file_name());
    for child in children {
        *entries += 1;
        if *entries > 4096 {
            return Err("template exceeds limits".into());
        }
        let name = child.file_name();
        let name = name.to_str().ok_or("non UTF-8 template path")?;
        if name.starts_with('.')
            || matches!(
                name,
                "secrets" | "private" | "runtime" | "state" | "target" | "node_modules"
            )
        {
            return Err("private or runtime template entry refused".into());
        }
        let rel = relative.join(name);
        if rel.components().count() > 32 || files.len() >= 4096 {
            return Err("template exceeds limits".into());
        }
        let m = child.metadata().map_err(|_| "template unavailable")?;
        if child
            .file_type()
            .map_err(|_| "template unavailable")?
            .is_symlink()
            || m.uid() != owner
            || m.mode() & 0o222 != 0
        {
            return Err("mutable or linked template refused".into());
        }
        if m.is_dir() {
            tree(root, &rel, files, bytes, entries, owner)?;
        } else if m.is_file() {
            let data = read_file(&root.join(&rel), 32 * 1024 * 1024 - *bytes)?;
            *bytes += data.len();
            files.push((rel, data, m.mode() & 0o111));
        } else {
            return Err("special template entry refused".into());
        }
    }
    Ok(())
}
fn inventory(root: &Path, owner: u32) -> Result<Vec<(PathBuf, Vec<u8>, u32)>> {
    if !store_path(root) {
        return Err("template must be immutable store source".into());
    }
    let mut files = Vec::new();
    tree(root, Path::new(""), &mut files, &mut 0, &mut 0, owner)?;
    if !files.iter().any(|(p, _, _)| p == Path::new("flake.nix"))
        || !files.iter().any(|(p, _, _)| p == Path::new("flake.lock"))
        || !files
            .iter()
            .any(|(p, _, _)| p == Path::new("tentaflake.toml"))
    {
        return Err("template lacks required flake sources".into());
    }
    Ok(files)
}
fn hash_inventory(files: &[(PathBuf, Vec<u8>, u32)]) -> String {
    let mut hash = Sha256::new();
    for (p, data, mode) in files {
        let p = p.as_os_str().as_encoded_bytes();
        hash.update((p.len() as u64).to_be_bytes());
        hash.update(p);
        hash.update(mode.to_be_bytes());
        hash.update((data.len() as u64).to_be_bytes());
        hash.update(data);
    }
    crate::hex(&hash.finalize())
}
pub fn fingerprint(root: &Path) -> Result<String> {
    Ok(hash_inventory(&inventory(root, 0)?))
}
/// Compute a digest without opening deployment state or granting build authority.
/// Nix sandboxes may map immutable input ownership to nobody; all entries must
/// still share the root's observed owner and remain read-only. Deployment
/// validate() and materialize() continue to require the real store's root UID.
pub fn fingerprint_for_review(root: &Path) -> Result<String> {
    let owner = open_path(root, true)?
        .metadata()
        .map_err(|_| "template unavailable")?
        .uid();
    Ok(hash_inventory(&inventory(root, owner)?))
}
pub fn materialize(builder: &Builder, destination: &Path, toml: &str) -> Result<()> {
    let files = inventory(&builder.template, 0)?;
    if hash_inventory(&files) != builder.template_hash {
        return Err("template drift".into());
    }
    fs::create_dir(destination).map_err(|_| "candidate already exists or unavailable")?;
    fs::set_permissions(destination, fs::Permissions::from_mode(0o700))
        .map_err(|_| "candidate unavailable")?;
    for (relative, bytes, mode) in files {
        let target = destination.join(&relative);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).map_err(|_| "candidate unavailable")?;
        }
        let bytes = if relative == Path::new("tentaflake.toml") {
            toml.as_bytes()
        } else {
            &bytes
        };
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o400 | mode)
            .custom_flags(libc::O_NOFOLLOW)
            .open(target)
            .map_err(|_| "candidate unavailable")?;
        file.write_all(bytes)
            .and_then(|_| file.sync_all())
            .map_err(|_| "candidate unavailable")?;
    }
    open_path(destination, true)?
        .sync_all()
        .map_err(|_| "candidate unavailable".into())
}
