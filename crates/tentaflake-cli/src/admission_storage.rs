//! Protected quota roots, inventory and admission-time backing headroom.
//!
//! The caller must fence every quota creator and privileged writer throughout
//! scanning and ledger initialization. Metadata checks detect observed changes;
//! they do not replace that fence or verify Btrfs. The creation-space check is
//! a conservative current-space guard, not a reservation of physical extents.

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::ffi::{CStr, CString, OsStr, OsString};
use std::fs::{File, Metadata};
use std::io;
use std::os::fd::{AsRawFd, IntoRawFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use tentaflake_admission::ledger::StorageInventory;
pub(crate) use tentaflake_admission::protected_path::open_root;
use tentaflake_admission::protected_path::{open_at, protected_directory};
use tentaflake_admission::{
    MAX_STORAGE_ALLOCATIONS, MIN_STORAGE_BYTES, StorageAllocation, StorageBinding, StorageKind,
    StorageState, identifier,
};

const VOLUME_ROOT: &str = "/var/lib/tentaflake-workspace-volumes";

/// Authenticated by the caller against its immutable host manifest. Logical
/// ownership is explicit and is never inferred from the capsule's Unix IDs.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct VolumeSpec {
    pub instance_id: String,
    pub owner_id: String,
    pub allocation_id: String,
    pub kind: StorageKind,
    pub image_path: PathBuf,
    pub mount_path: PathBuf,
    pub bytes: u64,
    pub owner_uid: u32,
    pub owner_gid: u32,
    pub binding_revision: String,
}

/// Validate the immutable declaration shape without opening any host paths.
/// The caller separately authenticates its owner and binding to the Instance.
pub(crate) fn validate_specs(specs: &[VolumeSpec]) -> Result<(), String> {
    declared_slots(specs, Path::new(VOLUME_ROOT)).map(|_| ())
}

/// Import every existing slot, including removed definitions and size drift.
/// Missing roots are errors, absent desired images create no row, and every
/// imported row remains Unknown until separately verified by a trusted host.
pub fn scan_inventory(specs: &[VolumeSpec]) -> Result<StorageInventory, String> {
    if unsafe { libc::geteuid() } != 0 {
        return Err("quota inventory requires root".into());
    }
    let root = Path::new(VOLUME_ROOT);
    scan_at(specs, root, 0, || open_root(root, 0))
}

/// Explicit bootstrap only. Create the two fixed roots without repairing any
/// existing entry. The caller must fence other privileged quota mutators.
pub fn ensure_backing_roots() -> Result<(), String> {
    if unsafe { libc::geteuid() } != 0 {
        return Err("quota root initialization requires root".into());
    }
    ensure_roots_at(Path::new(VOLUME_ROOT), 0, |path| open_root(path, 0))
}

/// New slots add their full charge; reserved slots must already have exact
/// Unknown bindings. Neither mode permits an existing final or staging image.
#[derive(Clone, Copy)]
pub enum CreationTargets<'a> {
    New(&'a [VolumeSpec]),
    Reserved(&'a [VolumeSpec]),
}

impl<'a> CreationTargets<'a> {
    fn specs(self) -> &'a [VolumeSpec] {
        match self {
            Self::New(specs) | Self::Reserved(specs) => specs,
        }
    }
}

/// Call once for every slot in the transaction, under the outer creator fence
/// and ledger guard against its exact retained snapshot. Every retained byte
/// stays charged, including missing, orphaned and Unknown images. Unrelated
/// host writes can consume space after this observation; no allocated blocks
/// or shared extents receive credit. This inspection grants no creation permit.
pub fn check_creation_space(
    specs: &[VolumeSpec],
    retained: &[StorageAllocation],
    targets: CreationTargets<'_>,
    headroom: u64,
) -> Result<(), String> {
    if unsafe { libc::geteuid() } != 0 {
        return Err("quota creation-space inspection requires root".into());
    }
    check_creation_at(
        specs,
        retained,
        targets,
        headroom,
        Path::new(VOLUME_ROOT),
        0,
        || open_root(Path::new(VOLUME_ROOT), 0),
    )
}

fn valid_slot_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 63
        && (name.as_bytes()[0].is_ascii_lowercase() || name.as_bytes()[0].is_ascii_digit())
        && name
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

fn slot_id(kind: StorageKind, name: &str) -> String {
    let prefix = match kind {
        StorageKind::Workspace => "workspace",
        StorageKind::State => "state",
    };
    format!("{prefix}:{name}")
}

fn image_path(root: &Path, kind: StorageKind, name: &str) -> PathBuf {
    let directory = match kind {
        StorageKind::Workspace => root.to_owned(),
        StorageKind::State => root.join("state"),
    };
    directory.join(format!("{name}.img"))
}

fn valid_mount_path(path: &Path) -> bool {
    let Some(path) = path.to_str() else {
        return false;
    };
    path.len() <= 4096
        && path.starts_with("/var/lib/")
        && path
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"/._+-".contains(&byte))
        && path[1..]
            .split('/')
            .all(|part| !part.is_empty() && part != "." && part != "..")
}

fn declared_slots<'a>(
    specs: &'a [VolumeSpec],
    root: &Path,
) -> Result<BTreeMap<String, &'a VolumeSpec>, String> {
    if specs.len() > MAX_STORAGE_ALLOCATIONS {
        return Err("too many declared quota volumes".into());
    }
    let mut slots = BTreeMap::new();
    let mut mounts = BTreeMap::new();
    for spec in specs {
        let expected_path = image_path(root, spec.kind, &spec.instance_id);
        if !valid_slot_name(&spec.instance_id)
            || !identifier(&spec.owner_id)
            || !identifier(&spec.binding_revision)
            || spec.allocation_id != slot_id(spec.kind, &spec.instance_id)
            // Compare bytes, not normalized Path components, to reject aliases.
            || spec.image_path.as_os_str() != expected_path.as_os_str()
            || !valid_mount_path(&spec.mount_path)
            || spec.mount_path.starts_with(root)
            || spec.bytes < MIN_STORAGE_BYTES
            || mounts.insert(spec.mount_path.as_path(), spec).is_some()
            || slots.insert(spec.allocation_id.clone(), spec).is_some()
        {
            return Err("invalid or duplicate quota volume declaration".into());
        }
    }
    for spec in specs {
        for path in spec.mount_path.ancestors().skip(1) {
            if let Some(parent) = mounts.get(path)
                && (parent.instance_id != spec.instance_id || parent.kind == StorageKind::Workspace)
            {
                return Err("overlapping quota mount declarations".into());
            }
        }
    }
    Ok(slots)
}

fn metadata(file: &File) -> Result<Metadata, String> {
    file.metadata()
        .map_err(|_| "cannot inspect quota inventory entry".into())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct PoolIdentity {
    device: u64,
    mount_id: u64,
}

fn pool_identity(file: &File) -> Result<PoolIdentity, String> {
    let mut stat = std::mem::MaybeUninit::<libc::statx>::zeroed();
    if unsafe {
        libc::statx(
            file.as_raw_fd(),
            c"".as_ptr(),
            libc::AT_EMPTY_PATH | libc::AT_SYMLINK_NOFOLLOW,
            libc::STATX_MNT_ID,
            stat.as_mut_ptr(),
        )
    } != 0
    {
        return Err("cannot identify quota backing mount".into());
    }
    let stat = unsafe { stat.assume_init() };
    if stat.stx_mask & libc::STATX_MNT_ID == 0 {
        return Err("quota backing mount identity is unavailable".into());
    }
    Ok(PoolIdentity {
        device: metadata(file)?.dev(),
        mount_id: stat.stx_mnt_id,
    })
}

fn require_same_pool(expected: PoolIdentity, actual: PoolIdentity) -> Result<(), String> {
    if expected != actual {
        return Err(
            "quota backing entries must share one mount; nested and bind mounts are refused".into(),
        );
    }
    Ok(())
}

fn ensure_directory(parent: &File, name: &OsStr, owner_uid: u32) -> Result<File, String> {
    let encoded = CString::new(name.as_bytes()).map_err(|_| "invalid quota directory")?;
    let created = unsafe { libc::mkdirat(parent.as_raw_fd(), encoded.as_ptr(), 0o700) } == 0;
    if !created && io::Error::last_os_error().raw_os_error() != Some(libc::EEXIST) {
        return Err("cannot create quota backing directory".into());
    }
    let directory = open_at(parent.as_raw_fd(), name, libc::O_RDONLY | libc::O_DIRECTORY)?;
    protected_directory(&directory, owner_uid)?;
    if created {
        // Only our new inode is chmodded; an existing entry is never repaired.
        // This also establishes 0700 when the caller has an unusually strict umask.
        if unsafe { libc::fchmod(directory.as_raw_fd(), 0o700) } != 0 {
            return Err("cannot protect new quota backing directory".into());
        }
        directory
            .sync_all()
            .map_err(|_| "cannot sync quota directory")?;
        parent
            .sync_all()
            .map_err(|_| "cannot sync quota directory parent")?;
    }
    Ok(directory)
}

fn ensure_roots_at(
    root: &Path,
    owner_uid: u32,
    open_path: impl Fn(&Path) -> Result<File, String>,
) -> Result<(), String> {
    let parent = open_path(root.parent().ok_or("invalid quota root")?)?;
    protected_directory(&parent, owner_uid)?;
    let directory = ensure_directory(
        &parent,
        root.file_name().ok_or("invalid quota root")?,
        owner_uid,
    )?;
    let state = ensure_directory(&directory, OsStr::new("state"), owner_uid)?;
    let pool = pool_identity(&directory)?;
    require_same_pool(pool, pool_identity(&state)?)?;
    let current = open_path(root)?;
    protected_directory(&current, owner_uid)?;
    require_same_pool(pool, pool_identity(&current)?)?;
    if stamp(&metadata(&directory)?) != stamp(&metadata(&current)?) {
        return Err("quota backing root changed during initialization".into());
    }
    Ok(())
}

fn stamp(meta: &Metadata) -> [u64; 11] {
    [
        meta.dev(),
        meta.ino(),
        u64::from(meta.mode()),
        meta.nlink(),
        u64::from(meta.uid()),
        u64::from(meta.gid()),
        meta.len(),
        meta.mtime() as u64,
        meta.mtime_nsec() as u64,
        meta.ctime() as u64,
        meta.ctime_nsec() as u64,
    ]
}

fn image_unchanged(before: &Metadata, after: &Metadata, quiescent: bool) -> bool {
    let before = stamp(before);
    let after = stamp(after);
    // Inner-filesystem writes change timestamps but cannot change the protected
    // backing file's identity, ownership, permissions, links or logical length.
    if quiescent {
        before == after
    } else {
        before[..7] == after[..7]
    }
}

fn inspect_image(
    directory: &File,
    name: &OsStr,
    owner_uid: u32,
    pool: PoolIdentity,
    quiescent: bool,
) -> Result<Metadata, String> {
    // Inspect through O_PATH first: opening a device node for reading can have
    // effects even when the subsequent regular-file check would reject it.
    let image = open_at(directory.as_raw_fd(), name, libc::O_PATH)?;
    let meta = metadata(&image)?;
    if !meta.is_file()
        || meta.uid() != owner_uid
        || meta.mode() & 0o7777 != 0o600
        || meta.nlink() != 1
        || meta.len() == 0
    {
        return Err("unsafe or empty quota image".into());
    }
    require_same_pool(pool, pool_identity(&image)?)?;
    let readable = open_at(directory.as_raw_fd(), name, libc::O_RDONLY)?;
    require_same_pool(pool, pool_identity(&readable)?)?;
    if !image_unchanged(&meta, &metadata(&readable)?, quiescent) {
        return Err("quota image changed during inventory".into());
    }
    Ok(meta)
}

struct DirectoryStream(*mut libc::DIR);

impl Drop for DirectoryStream {
    fn drop(&mut self) {
        unsafe { libc::closedir(self.0) };
    }
}

impl DirectoryStream {
    fn new(directory: &File) -> Result<Self, String> {
        // A separate open description avoids sharing enumeration offsets.
        let fd = open_at(
            directory.as_raw_fd(),
            OsStr::new("."),
            libc::O_RDONLY | libc::O_DIRECTORY,
        )?
        .into_raw_fd();
        let stream = unsafe { libc::fdopendir(fd) };
        if stream.is_null() {
            unsafe { libc::close(fd) };
            return Err("cannot enumerate quota directory".into());
        }
        Ok(Self(stream))
    }

    fn next(&mut self) -> Result<Option<OsString>, String> {
        // readdir uses null for both EOF and error; errno distinguishes them.
        unsafe { *libc::__errno_location() = 0 };
        let entry = unsafe { libc::readdir(self.0) };
        if entry.is_null() {
            return if unsafe { *libc::__errno_location() } == 0 {
                Ok(None)
            } else {
                Err("incomplete quota directory enumeration".into())
            };
        }
        let name = unsafe { CStr::from_ptr((*entry).d_name.as_ptr()) };
        Ok(Some(OsStr::from_bytes(name.to_bytes()).to_owned()))
    }
}

struct ScannedDirectory {
    file: File,
    before: Metadata,
    images: Vec<(OsString, Metadata)>,
}

fn scan_pool(
    owner_uid: u32,
    quiescent: bool,
    reopen_root: impl Fn() -> Result<File, String>,
) -> Result<Vec<ScannedDirectory>, String> {
    let directory = reopen_root()?;
    let pool = pool_identity(&directory)?;
    let before = protected_directory(&directory, owner_uid)?;
    let mut directories = vec![ScannedDirectory {
        file: directory,
        before,
        images: Vec::new(),
    }];
    let mut image_count = 0;
    // Exactly the workspace root and, when present, its state child.
    for index in 0..2 {
        if index == directories.len() {
            break;
        }
        let mut stream = DirectoryStream::new(&directories[index].file)?;
        let mut names = BTreeSet::new();
        while let Some(name) = stream.next()? {
            if name == "." || name == ".." {
                continue;
            }
            if names.len() > MAX_STORAGE_ALLOCATIONS || !names.insert(name.clone()) {
                return Err("excessive or duplicate quota directory entries".into());
            }
            if index == 0 && name == "state" {
                let file = open_at(
                    directories[index].file.as_raw_fd(),
                    &name,
                    libc::O_RDONLY | libc::O_DIRECTORY,
                )?;
                let before = protected_directory(&file, owner_uid)?;
                require_same_pool(pool, pool_identity(&file)?)?;
                directories.push(ScannedDirectory {
                    file,
                    before,
                    images: Vec::new(),
                });
                continue;
            }
            let Some(name_text) = name.to_str() else {
                return Err("invalid quota image name".into());
            };
            if name_text.ends_with(".new") {
                return Err("unfinished quota image creation requires reconciliation".into());
            }
            if name_text
                .strip_suffix(".img")
                .filter(|name| valid_slot_name(name))
                .is_none()
            {
                return Err("unexpected quota inventory entry".into());
            }
            if image_count >= MAX_STORAGE_ALLOCATIONS {
                return Err("quota inventory exceeds allocation limit".into());
            }
            let meta = inspect_image(&directories[index].file, &name, owner_uid, pool, quiescent)?;
            image_count += 1;
            directories[index].images.push((name, meta));
        }
    }
    for directory in &directories {
        for (name, before) in &directory.images {
            if !image_unchanged(
                before,
                &inspect_image(&directory.file, name, owner_uid, pool, quiescent)?,
                quiescent,
            ) {
                return Err("quota image changed during inventory".into());
            }
        }
        if stamp(&directory.before) != stamp(&metadata(&directory.file)?) {
            return Err("quota directory changed during inventory".into());
        }
    }
    // Also detect replacement of the fixed root while its original FD was held.
    let current = reopen_root()?;
    protected_directory(&current, owner_uid)?;
    require_same_pool(pool, pool_identity(&current)?)?;
    if stamp(&directories[0].before) != stamp(&metadata(&current)?) {
        return Err("quota inventory root changed".into());
    }
    if directories.len() == 2 {
        let state = open_at(
            current.as_raw_fd(),
            OsStr::new("state"),
            libc::O_RDONLY | libc::O_DIRECTORY,
        )?;
        require_same_pool(pool, pool_identity(&state)?)?;
        if stamp(&directories[1].before) != stamp(&metadata(&state)?) {
            return Err("quota inventory state directory changed".into());
        }
    }
    Ok(directories)
}

fn observed_slots(
    directories: &[ScannedDirectory],
) -> impl Iterator<Item = (String, StorageKind, &Metadata)> {
    directories
        .iter()
        .enumerate()
        .flat_map(|(index, directory)| {
            let kind = if index == 0 {
                StorageKind::Workspace
            } else {
                StorageKind::State
            };
            directory.images.iter().map(move |(name, meta)| {
                let slot = name
                    .to_str()
                    .expect("validated slot name")
                    .strip_suffix(".img")
                    .expect("validated image suffix");
                (slot_id(kind, slot), kind, meta)
            })
        })
}

fn scan_at(
    specs: &[VolumeSpec],
    root: &Path,
    owner_uid: u32,
    reopen_root: impl Fn() -> Result<File, String>,
) -> Result<StorageInventory, String> {
    let specs = declared_slots(specs, root)?;
    let directories = scan_pool(owner_uid, true, reopen_root)?;
    let allocations: BTreeMap<_, _> = observed_slots(&directories)
        .map(|(id, kind, meta)| {
            let binding = specs
                .get(&id)
                .filter(|spec| spec.bytes == meta.len())
                .map(|spec| StorageBinding {
                    instance_id: spec.instance_id.clone(),
                    owner_id: spec.owner_id.clone(),
                    revision: spec.binding_revision.clone(),
                });
            (
                id.clone(),
                StorageAllocation {
                    id,
                    kind,
                    bytes: meta.len(),
                    binding,
                    state: StorageState::Unknown,
                },
            )
        })
        .collect();
    Ok(StorageInventory {
        complete: true,
        allocations: allocations.into_values().collect(),
    })
}

fn required_creation_bytes(
    retained: &[StorageAllocation],
    targets: CreationTargets<'_>,
    headroom: u64,
) -> Result<u64, String> {
    let specs = targets.specs();
    let new_count = match targets {
        CreationTargets::New(_) => specs.len(),
        CreationTargets::Reserved(_) => 0,
    };
    if specs.is_empty() || specs.len() > 2 || retained.len() > MAX_STORAGE_ALLOCATIONS - new_count {
        return Err("invalid quota creation charge".into());
    }
    let mut allocations = BTreeMap::new();
    let mut total = headroom;
    for allocation in retained {
        if allocation.bytes == 0
            || !identifier(&allocation.id)
            || allocations
                .insert(allocation.id.as_str(), allocation)
                .is_some()
        {
            return Err("invalid retained quota allocation".into());
        }
        total = total
            .checked_add(allocation.bytes)
            .ok_or("quota backing charge overflow")?;
    }
    let mut ids = BTreeSet::new();
    for spec in specs {
        if spec.bytes == 0 || !identifier(&spec.allocation_id) || !ids.insert(&spec.allocation_id) {
            return Err("invalid or duplicate quota creation target".into());
        }
        match targets {
            CreationTargets::New(_) => {
                if allocations.contains_key(spec.allocation_id.as_str()) {
                    return Err("new quota target already has a retained allocation".into());
                }
                total = total
                    .checked_add(spec.bytes)
                    .ok_or("quota backing charge overflow")?;
            }
            CreationTargets::Reserved(_) => {
                if !allocations
                    .get(spec.allocation_id.as_str())
                    .is_some_and(|allocation| {
                        allocation.state == StorageState::Unknown
                            && allocation.kind == spec.kind
                            && allocation.bytes == spec.bytes
                            && allocation.binding.as_ref().is_some_and(|binding| {
                                binding.instance_id == spec.instance_id
                                    && binding.owner_id == spec.owner_id
                                    && binding.revision == spec.binding_revision
                            })
                    })
                {
                    return Err("reserved quota target lacks its exact Unknown binding".into());
                }
            }
        }
    }
    Ok(total)
}

fn available_bytes(blocks: u64, fragment_size: u64) -> Result<u64, String> {
    if fragment_size == 0 {
        return Err("quota backing fragment size is unavailable".into());
    }
    blocks
        .checked_mul(fragment_size)
        .ok_or_else(|| "quota backing space overflow".into())
}

fn backing_available(directory: &File) -> Result<u64, String> {
    let mut stat = std::mem::MaybeUninit::<libc::statvfs>::zeroed();
    if unsafe { libc::fstatvfs(directory.as_raw_fd(), stat.as_mut_ptr()) } != 0 {
        return Err("cannot observe available quota backing space".into());
    }
    let stat = unsafe { stat.assume_init() };
    if stat.f_flag & libc::ST_RDONLY != 0 {
        return Err("quota backing filesystem is read-only".into());
    }
    available_bytes(stat.f_bavail, stat.f_frsize)
}

fn check_creation_at(
    specs: &[VolumeSpec],
    retained: &[StorageAllocation],
    targets: CreationTargets<'_>,
    headroom: u64,
    root: &Path,
    owner_uid: u32,
    reopen_root: impl Fn() -> Result<File, String>,
) -> Result<(), String> {
    let (directory, required) = creation_pool(
        specs,
        retained,
        targets,
        headroom,
        root,
        owner_uid,
        reopen_root,
    )?;
    if backing_available(&directory)? < required {
        return Err("insufficient backing headroom for all retained and new quota bytes".into());
    }
    Ok(())
}

fn creation_pool(
    specs: &[VolumeSpec],
    retained: &[StorageAllocation],
    targets: CreationTargets<'_>,
    headroom: u64,
    root: &Path,
    owner_uid: u32,
    reopen_root: impl Fn() -> Result<File, String>,
) -> Result<(File, u64), String> {
    let required = required_creation_bytes(retained, targets, headroom)?;
    let declared = declared_slots(specs, root)?;
    if targets
        .specs()
        .iter()
        .any(|target| declared.get(&target.allocation_id).copied() != Some(target))
    {
        return Err("creation requires exact declared quota targets".into());
    }
    let mut directories = scan_pool(owner_uid, false, reopen_root)?;
    if directories.len() != 2 {
        return Err("quota backing roots require explicit bootstrap".into());
    }
    let retained: BTreeMap<_, _> = retained
        .iter()
        .map(|allocation| (allocation.id.as_str(), allocation))
        .collect();
    for (id, kind, meta) in observed_slots(&directories) {
        if targets
            .specs()
            .iter()
            .any(|target| target.allocation_id == id)
        {
            return Err("quota creation target already has a final image".into());
        }
        if !retained
            .get(id.as_str())
            .is_some_and(|allocation| allocation.kind == kind && allocation.bytes == meta.len())
        {
            return Err("actual quota image is not represented by its retained allocation".into());
        }
    }
    Ok((directories.remove(0).file, required))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::fs::{self, DirBuilder, OpenOptions};
    use std::os::unix::fs::{DirBuilderExt, FileExt, OpenOptionsExt, PermissionsExt, symlink};
    use std::sync::atomic::{AtomicU64, Ordering};
    use tentaflake_admission::protected_path::open_relative;

    struct Fixture {
        root: PathBuf,
        anchor_path: PathBuf,
        anchor: File,
    }

    impl Fixture {
        fn new() -> Self {
            static SEQUENCE: AtomicU64 = AtomicU64::new(0);
            let anchor_path = std::env::temp_dir().join(format!(
                "tentaflake-quota-inventory-{}-{}",
                std::process::id(),
                SEQUENCE.fetch_add(1, Ordering::Relaxed)
            ));
            DirBuilder::new().mode(0o700).create(&anchor_path).unwrap();
            let anchor = open_at(
                libc::AT_FDCWD,
                anchor_path.as_os_str(),
                libc::O_RDONLY | libc::O_DIRECTORY,
            )
            .unwrap();
            protected_directory(&anchor, unsafe { libc::geteuid() }).unwrap();
            let root = anchor_path.join("volumes");
            DirBuilder::new().mode(0o700).create(&root).unwrap();
            Self {
                root,
                anchor_path,
                anchor,
            }
        }

        fn image(&self, kind: StorageKind, name: &str, bytes: u64) -> PathBuf {
            let path = image_path(&self.root, kind, name);
            if kind == StorageKind::State && !self.root.join("state").exists() {
                DirBuilder::new()
                    .mode(0o700)
                    .create(self.root.join("state"))
                    .unwrap();
            }
            let file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&path)
                .unwrap();
            file.set_len(bytes).unwrap();
            path
        }

        fn spec(&self, kind: StorageKind, name: &str) -> VolumeSpec {
            VolumeSpec {
                instance_id: name.into(),
                owner_id: "host-root".into(),
                allocation_id: slot_id(kind, name),
                kind,
                image_path: image_path(&self.root, kind, name),
                mount_path: PathBuf::from(format!("/var/lib/{name}/{kind:?}")),
                bytes: MIN_STORAGE_BYTES,
                owner_uid: 1000,
                owner_gid: 1000,
                binding_revision: "binding-1".into(),
            }
        }

        fn scan(&self, specs: &[VolumeSpec]) -> Result<StorageInventory, String> {
            self.scan_path(specs, &self.root, unsafe { libc::geteuid() })
        }

        fn open(&self, root: &Path, owner_uid: u32) -> Result<File, String> {
            let relative = root
                .strip_prefix(&self.anchor_path)
                .map_err(|_| "quota fixture path is outside its anchor")?;
            open_relative(&self.anchor, relative, owner_uid)
        }

        fn scan_path(
            &self,
            specs: &[VolumeSpec],
            root: &Path,
            owner_uid: u32,
        ) -> Result<StorageInventory, String> {
            scan_at(specs, root, owner_uid, || self.open(root, owner_uid))
        }
        fn initialize(&self) {
            ensure_roots_at(&self.root, unsafe { libc::geteuid() }, |path| {
                self.open(path, unsafe { libc::geteuid() })
            })
            .unwrap();
        }

        fn creation_pool(
            &self,
            retained: &[StorageAllocation],
            new: &VolumeSpec,
        ) -> Result<File, String> {
            self.target_pool(retained, CreationTargets::New(std::slice::from_ref(new)), 0)
                .map(|(directory, _)| directory)
        }

        fn target_pool(
            &self,
            retained: &[StorageAllocation],
            targets: CreationTargets<'_>,
            headroom: u64,
        ) -> Result<(File, u64), String> {
            creation_pool(
                targets.specs(),
                retained,
                targets,
                headroom,
                &self.root,
                unsafe { libc::geteuid() },
                || self.open(&self.root, unsafe { libc::geteuid() }),
            )
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.anchor_path).unwrap();
        }
    }

    fn reserved(spec: &VolumeSpec) -> StorageAllocation {
        StorageAllocation {
            id: spec.allocation_id.clone(),
            kind: spec.kind,
            bytes: spec.bytes,
            binding: Some(StorageBinding {
                instance_id: spec.instance_id.clone(),
                owner_id: spec.owner_id.clone(),
                revision: spec.binding_revision.clone(),
            }),
            state: StorageState::Unknown,
        }
    }

    #[test]
    fn imports_exact_bindings_orphans_and_tiny_images_as_unknown() {
        let fixture = Fixture::new();
        fixture.image(StorageKind::Workspace, "agent-a", MIN_STORAGE_BYTES);
        fixture.image(StorageKind::State, "agent-a", MIN_STORAGE_BYTES);
        fixture.image(StorageKind::Workspace, "removed", 1);
        let specs = [
            fixture.spec(StorageKind::Workspace, "agent-a"),
            fixture.spec(StorageKind::State, "agent-a"),
        ];
        let inventory = fixture.scan(&specs).unwrap();
        assert!(inventory.complete);
        assert_eq!(inventory.allocations.len(), 3);
        assert!(
            inventory
                .allocations
                .iter()
                .all(|row| row.state == StorageState::Unknown)
        );
        assert_eq!(inventory.allocations[0].id, "state:agent-a");
        assert_eq!(
            inventory.allocations[0].binding.as_ref().unwrap().owner_id,
            "host-root"
        );
        assert_eq!(
            inventory.allocations[1]
                .binding
                .as_ref()
                .unwrap()
                .instance_id,
            "agent-a"
        );
        assert_eq!(inventory.allocations[2].bytes, 1);
        assert!(inventory.allocations[2].binding.is_none());
    }

    #[test]
    fn size_drift_is_orphaned_and_missing_desired_images_are_not_fabricated() {
        let fixture = Fixture::new();
        fixture.image(StorageKind::Workspace, "agent-a", MIN_STORAGE_BYTES - 1);
        let inventory = fixture
            .scan(&[
                fixture.spec(StorageKind::Workspace, "agent-a"),
                fixture.spec(StorageKind::State, "agent-a"),
            ])
            .unwrap();
        assert_eq!(inventory.allocations.len(), 1);
        assert_eq!(inventory.allocations[0].bytes, MIN_STORAGE_BYTES - 1);
        assert!(inventory.allocations[0].binding.is_none());
        assert_eq!(inventory.allocations[0].state, StorageState::Unknown);
    }

    #[test]
    fn requires_existing_protected_root_and_ancestors() {
        let fixture = Fixture::new();
        let uid = unsafe { libc::geteuid() };
        assert!(
            fixture
                .scan_path(&[], &fixture.root.join("absent"), uid)
                .is_err()
        );
        DirBuilder::new()
            .mode(0o700)
            .create(fixture.root.join("child"))
            .unwrap();
        fs::set_permissions(&fixture.root, fs::Permissions::from_mode(0o777)).unwrap();
        assert!(
            fixture
                .scan_path(&[], &fixture.root.join("child"), uid)
                .is_err()
        );
        fs::set_permissions(&fixture.root, fs::Permissions::from_mode(0o700)).unwrap();
        assert!(
            fixture
                .scan_path(&[], &fixture.root, uid.wrapping_add(1))
                .is_err()
        );
        symlink(fixture.root.join("child"), fixture.root.join("alias")).unwrap();
        assert!(
            fixture
                .scan_path(&[], &fixture.root.join("alias"), uid)
                .is_err()
        );
    }

    #[test]
    fn requires_protected_anchor_without_sticky_directory_exception() {
        let fixture = Fixture::new();
        for mode in [0o777, 0o1777] {
            fs::set_permissions(&fixture.anchor_path, fs::Permissions::from_mode(mode)).unwrap();
            assert_eq!(
                fixture.scan(&[]).err().as_deref(),
                Some("unprotected quota directory ancestry")
            );
        }
        fs::set_permissions(&fixture.anchor_path, fs::Permissions::from_mode(0o700)).unwrap();
        assert!(fixture.scan(&[]).is_ok());
    }

    #[test]
    fn relative_traversal_refuses_escape_and_non_directory_anchors() {
        let fixture = Fixture::new();
        let uid = unsafe { libc::geteuid() };
        assert!(open_relative(&fixture.anchor, Path::new("volumes"), uid).is_ok());
        for path in ["/volumes", "../volumes", "volumes/../volumes", "./volumes"] {
            assert!(
                open_relative(&fixture.anchor, Path::new(path), uid).is_err(),
                "accepted relative path {path}"
            );
        }
        let image = fixture.image(StorageKind::Workspace, "agent-a", 1);
        let file = File::open(image).unwrap();
        assert!(open_relative(&file, Path::new(""), uid).is_err());
    }

    #[test]
    fn absolute_traversal_keeps_production_root_policy() {
        let root = fs::metadata("/").unwrap();
        let result = open_root(Path::new("/"), 0);
        if root.uid() == 0 && root.mode() & 0o022 == 0 {
            assert!(result.is_ok());
        } else {
            assert_eq!(
                result.err().as_deref(),
                Some("unprotected quota directory ancestry")
            );
        }
        assert!(open_root(Path::new("relative"), 0).is_err());
    }

    #[test]
    fn final_reopen_refuses_replaced_root_or_symlink() {
        for replace_with_symlink in [false, true] {
            let fixture = Fixture::new();
            let uid = unsafe { libc::geteuid() };
            let opens = Cell::new(0);
            let result = scan_at(&[], &fixture.root, uid, || {
                opens.set(opens.get() + 1);
                if opens.get() == 2 {
                    let previous = fixture.anchor_path.join("previous-volumes");
                    fs::rename(&fixture.root, &previous).unwrap();
                    if replace_with_symlink {
                        symlink(previous, &fixture.root).unwrap();
                    } else {
                        DirBuilder::new().mode(0o700).create(&fixture.root).unwrap();
                    }
                }
                fixture.open(&fixture.root, uid)
            });
            assert_eq!(opens.get(), 2);
            if replace_with_symlink {
                assert!(result.is_err());
            } else {
                assert_eq!(
                    result.err().as_deref(),
                    Some("quota inventory root changed")
                );
            }
        }
    }

    #[test]
    fn refuses_partial_unknown_and_nonregular_entries() {
        for name in ["agent-a.img.new", "notes", "bad_name.img", "UPPER.img"] {
            let fixture = Fixture::new();
            fs::write(fixture.root.join(name), b"x").unwrap();
            assert!(fixture.scan(&[]).is_err(), "accepted {name}");
        }
        let fixture = Fixture::new();
        DirBuilder::new()
            .mode(0o700)
            .create(fixture.root.join("nested"))
            .unwrap();
        assert!(fixture.scan(&[]).is_err());
        fs::remove_dir(fixture.root.join("nested")).unwrap();
        symlink("missing", fixture.root.join("agent-a.img")).unwrap();
        assert!(fixture.scan(&[]).is_err());
        fs::remove_file(fixture.root.join("agent-a.img")).unwrap();
        symlink("missing", fixture.root.join("state")).unwrap();
        assert!(fixture.scan(&[]).is_err());
    }

    #[test]
    fn refuses_fifo_and_directory_images_without_reading_them() {
        let fixture = Fixture::new();
        let fifo = fixture.root.join("agent-a.img");
        let name = CString::new(fifo.as_os_str().as_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
        assert!(fixture.scan(&[]).is_err());
        fs::remove_file(&fifo).unwrap();
        DirBuilder::new().mode(0o700).create(&fifo).unwrap();
        assert!(fixture.scan(&[]).is_err());
    }

    #[test]
    fn refuses_unprotected_state_directory() {
        let fixture = Fixture::new();
        fixture.image(StorageKind::State, "agent-a", 1);
        fs::set_permissions(
            fixture.root.join("state"),
            fs::Permissions::from_mode(0o777),
        )
        .unwrap();
        assert!(fixture.scan(&[]).is_err());
    }

    #[test]
    fn refuses_empty_hardlinked_or_exposed_images() {
        let fixture = Fixture::new();
        let image = fixture.image(StorageKind::Workspace, "agent-a", 0);
        assert!(fixture.scan(&[]).is_err());
        OpenOptions::new()
            .write(true)
            .open(&image)
            .unwrap()
            .set_len(1)
            .unwrap();
        fs::set_permissions(&image, fs::Permissions::from_mode(0o640)).unwrap();
        assert!(fixture.scan(&[]).is_err());
        fs::set_permissions(&image, fs::Permissions::from_mode(0o600)).unwrap();
        fs::hard_link(&image, fixture.root.join("agent-b.img")).unwrap();
        assert!(fixture.scan(&[]).is_err());
    }

    #[test]
    fn validates_declared_identity_kind_paths_sizes_and_duplicates() {
        let fixture = Fixture::new();
        let original = fixture.spec(StorageKind::Workspace, "agent-a");
        assert!(fixture.scan(&[original.clone(), original.clone()]).is_err());
        for change in 0..8 {
            let mut spec = original.clone();
            match change {
                0 => spec.kind = StorageKind::State,
                1 => spec.allocation_id = "workspace:other".into(),
                2 => spec.image_path = fixture.root.join("../agent-a.img"),
                3 => spec.owner_id = "bad owner".into(),
                4 => spec.binding_revision = "x".repeat(129),
                5 => spec.bytes = MIN_STORAGE_BYTES - 1,
                6 => spec.mount_path = PathBuf::from("/var/lib/../escape"),
                7 => spec.instance_id = "bad_name".into(),
                _ => unreachable!(),
            }
            assert!(fixture.scan(&[spec]).is_err(), "accepted mutation {change}");
        }
        assert!(
            fixture
                .scan(&vec![original; MAX_STORAGE_ALLOCATIONS + 1])
                .is_err()
        );
    }

    #[test]
    fn declarations_are_closed_and_camel_case() {
        let fixture = Fixture::new();
        let spec = fixture.spec(StorageKind::Workspace, "agent-a");
        let mut value = serde_json::to_value(&spec).unwrap();
        assert_eq!(value["ownerId"], "host-root");
        value["ready"] = true.into();
        assert!(serde_json::from_value::<VolumeSpec>(value).is_err());
    }

    #[test]
    fn shape_validation_does_not_require_live_images() {
        let fixture = Fixture::new();
        let mut spec = fixture.spec(StorageKind::Workspace, "agent-a");
        spec.image_path = image_path(Path::new(VOLUME_ROOT), spec.kind, &spec.instance_id);
        assert!(validate_specs(&[spec.clone()]).is_ok());
        spec.image_path = PathBuf::from("/var/lib/tentaflake-workspace-volumes/./agent-a.img");
        assert!(validate_specs(&[spec]).is_err());
    }

    #[test]
    fn mount_overlap_matches_quota_ownership_and_nesting_rules() {
        let fixture = Fixture::new();
        let mut workspace = fixture.spec(StorageKind::Workspace, "agent-a");
        workspace.mount_path = PathBuf::from("/var/lib/agent-a/workspace");
        let mut state = fixture.spec(StorageKind::State, "agent-a");
        state.mount_path = PathBuf::from("/var/lib/agent-a");
        assert!(fixture.scan(&[workspace.clone(), state.clone()]).is_ok());
        assert!(fixture.scan(&[state.clone(), workspace.clone()]).is_ok());

        state.mount_path = workspace.mount_path.join("state");
        assert!(fixture.scan(&[workspace.clone(), state.clone()]).is_err());
        assert!(fixture.scan(&[state, workspace.clone()]).is_err());

        let mut other = fixture.spec(StorageKind::Workspace, "agent-b");
        other.mount_path = workspace.mount_path.join("child");
        assert!(fixture.scan(&[workspace.clone(), other.clone()]).is_err());
        assert!(fixture.scan(&[other.clone(), workspace.clone()]).is_err());
        other.mount_path = workspace.mount_path.clone();
        assert!(fixture.scan(&[workspace.clone(), other.clone()]).is_err());

        // A lexical prefix is not a filesystem ancestor.
        other.mount_path = PathBuf::from("/var/lib/agent-a/workspace-other");
        assert!(fixture.scan(&[workspace, other]).is_ok());
    }

    #[test]
    fn empty_existing_root_is_complete_without_creating_declared_images() {
        let fixture = Fixture::new();
        let inventory = fixture
            .scan(&[fixture.spec(StorageKind::Workspace, "agent-a")])
            .unwrap();
        assert!(inventory.complete);
        assert!(inventory.allocations.is_empty());
        assert_eq!(fs::read_dir(&fixture.root).unwrap().count(), 0);
    }

    #[test]
    fn bootstrap_creates_only_private_roots_and_leaves_existing_modes_unchanged() {
        let fixture = Fixture::new();
        let root = fixture.root.join("volumes");
        let uid = unsafe { libc::geteuid() };
        ensure_roots_at(&root, uid, |path| fixture.open(path, uid)).unwrap();
        for path in [&root, &root.join("state")] {
            assert_eq!(fs::metadata(path).unwrap().mode() & 0o7777, 0o700);
        }
        assert_eq!(fs::read_dir(&root).unwrap().count(), 1);
        assert_eq!(fs::read_dir(root.join("state")).unwrap().count(), 0);
        fs::set_permissions(&root, fs::Permissions::from_mode(0o750)).unwrap();
        ensure_roots_at(&root, uid, |path| fixture.open(path, uid)).unwrap();
        assert_eq!(fs::metadata(&root).unwrap().mode() & 0o7777, 0o750);
    }

    #[test]
    fn bootstrap_reopens_root_after_creating_directories() {
        let fixture = Fixture::new();
        let root = fixture.root.join("volumes");
        let uid = unsafe { libc::geteuid() };
        let opens = Cell::new(0);
        let result = ensure_roots_at(&root, uid, |path| {
            opens.set(opens.get() + 1);
            if opens.get() == 2 {
                fs::rename(&root, fixture.root.join("previous-volumes")).unwrap();
                DirBuilder::new().mode(0o700).create(&root).unwrap();
            }
            fixture.open(path, uid)
        });
        assert_eq!(opens.get(), 2);
        assert_eq!(
            result.unwrap_err(),
            "quota backing root changed during initialization"
        );
    }

    #[test]
    fn bootstrap_refuses_unsafe_existing_entries_without_repair() {
        let fixture = Fixture::new();
        let uid = unsafe { libc::geteuid() };
        let root = fixture.root.join("volumes");
        fs::write(&root, b"preserve").unwrap();
        assert!(ensure_roots_at(&root, uid, |path| fixture.open(path, uid)).is_err());
        assert_eq!(fs::read(&root).unwrap(), b"preserve");
        fs::remove_file(&root).unwrap();
        symlink(&fixture.root, &root).unwrap();
        assert!(ensure_roots_at(&root, uid, |path| fixture.open(path, uid)).is_err());
        assert!(!fixture.root.join("state").exists());
        fs::remove_file(&root).unwrap();
        ensure_roots_at(&root, uid, |path| fixture.open(path, uid)).unwrap();
        let state = root.join("state");
        fs::set_permissions(&state, fs::Permissions::from_mode(0o777)).unwrap();
        assert!(ensure_roots_at(&root, uid, |path| fixture.open(path, uid)).is_err());
        assert_eq!(fs::metadata(&state).unwrap().mode() & 0o7777, 0o777);
        fs::set_permissions(&state, fs::Permissions::from_mode(0o700)).unwrap();
        fs::set_permissions(&fixture.root, fs::Permissions::from_mode(0o777)).unwrap();
        assert!(ensure_roots_at(&root, uid, |path| fixture.open(path, uid)).is_err());
        fs::set_permissions(&fixture.root, fs::Permissions::from_mode(0o700)).unwrap();
        assert!(
            ensure_roots_at(&root, uid.wrapping_add(1), |path| fixture
                .open(path, uid.wrapping_add(1)))
            .is_err()
        );
    }

    #[test]
    fn charges_every_retained_kind_and_state_without_block_credit() {
        let fixture = Fixture::new();
        fixture.image(StorageKind::Workspace, "agent-a", 7);
        fixture.image(StorageKind::State, "orphan", 11);
        let mut retained = fixture.scan(&[]).unwrap().allocations;
        retained[0].state = StorageState::Present;
        let new = fixture.spec(StorageKind::Workspace, "new");
        let targets = CreationTargets::New(std::slice::from_ref(&new));
        assert_eq!(
            required_creation_bytes(&retained, targets, 17).unwrap(),
            MIN_STORAGE_BYTES + 35
        );
        assert!(required_creation_bytes(&retained, targets, u64::MAX).is_err());
        retained[0].bytes = u64::MAX;
        assert!(required_creation_bytes(&retained, targets, 0).is_err());
        retained[0].bytes = 0;
        assert!(required_creation_bytes(&retained, targets, 0).is_err());
        retained[0] = retained[1].clone();
        assert!(required_creation_bytes(&retained, targets, 0).is_err());
        assert_eq!(available_bytes(7, 4096).unwrap(), 28672);
        assert_eq!(available_bytes(0, 4096).unwrap(), 0);
        assert!(available_bytes(u64::MAX, 4096).is_err());
        assert!(available_bytes(1, 0).is_err());
    }

    #[test]
    fn creation_requires_complete_actual_accounting_but_keeps_missing_charges() {
        let fixture = Fixture::new();
        fixture.initialize();
        fixture.image(StorageKind::Workspace, "old", MIN_STORAGE_BYTES);
        fixture.image(StorageKind::State, "orphan", MIN_STORAGE_BYTES);
        let mut retained = fixture.scan(&[]).unwrap().allocations;
        let new = fixture.spec(StorageKind::Workspace, "new");
        assert!(fixture.creation_pool(&[], &new).is_err());
        assert!(fixture.creation_pool(&retained[..1], &new).is_err());
        assert!(fixture.creation_pool(&retained, &new).is_ok());
        fs::remove_file(image_path(&fixture.root, StorageKind::State, "orphan")).unwrap();
        assert!(fixture.creation_pool(&retained, &new).is_ok());
        assert_eq!(
            required_creation_bytes(
                &retained,
                CreationTargets::New(std::slice::from_ref(&new)),
                19
            )
            .unwrap(),
            3 * MIN_STORAGE_BYTES + 19,
        );
        retained
            .iter_mut()
            .find(|allocation| allocation.kind == StorageKind::Workspace)
            .unwrap()
            .bytes += 1;
        assert!(fixture.creation_pool(&retained, &new).is_err());
        assert!(!new.image_path.exists());
    }

    #[test]
    fn creation_never_replays_a_retained_slot_or_touches_staging() {
        let fixture = Fixture::new();
        fixture.initialize();
        let new = fixture.spec(StorageKind::Workspace, "new");
        let retained = StorageAllocation {
            id: new.allocation_id.clone(),
            kind: new.kind,
            bytes: new.bytes,
            binding: None,
            state: StorageState::Unknown,
        };
        assert!(
            fixture
                .creation_pool(std::slice::from_ref(&retained), &new)
                .is_err()
        );
        let mut changed = new.clone();
        changed.bytes += 1;
        assert!(
            creation_pool(
                std::slice::from_ref(&new),
                &[],
                CreationTargets::New(std::slice::from_ref(&changed)),
                0,
                &fixture.root,
                unsafe { libc::geteuid() },
                || fixture.open(&fixture.root, unsafe { libc::geteuid() }),
            )
            .is_err()
        );
        let staging = fixture.root.join("new.img.new");
        fs::write(&staging, b"partial").unwrap();
        assert!(fixture.creation_pool(&[], &new).is_err());
        assert_eq!(fs::read(&staging).unwrap(), b"partial");
        assert!(!new.image_path.exists());
        fs::remove_file(&staging).unwrap();
        fixture.image(new.kind, "new", new.bytes);
        assert!(fixture.creation_pool(&[], &new).is_err());
        assert!(fixture.creation_pool(&[retained], &new).is_err());
        assert_eq!(fs::metadata(&new.image_path).unwrap().len(), new.bytes);
    }

    #[test]
    fn new_pair_and_reserved_pair_charge_the_same_complete_pool_once() {
        let fixture = Fixture::new();
        fixture.initialize();
        let orphan = fixture.image(StorageKind::Workspace, "orphan", 19);
        let retained = fixture.scan(&[]).unwrap().allocations;
        let targets = [
            fixture.spec(StorageKind::Workspace, "new"),
            fixture.spec(StorageKind::State, "new"),
        ];
        let (_, fresh_charge) = fixture
            .target_pool(&retained, CreationTargets::New(&targets), 23)
            .unwrap();
        assert_eq!(fresh_charge, 2 * MIN_STORAGE_BYTES + 42);
        let mut queued = retained.clone();
        queued.extend(targets.iter().map(reserved));
        let (_, queued_charge) = fixture
            .target_pool(&queued, CreationTargets::Reserved(&targets), 23)
            .unwrap();
        assert_eq!(queued_charge, fresh_charge);
        assert!(
            fixture
                .target_pool(&queued, CreationTargets::New(&targets), 23)
                .is_err()
        );
        // A real orphan cannot be hidden by otherwise exact target reservations.
        assert!(
            fixture
                .target_pool(&queued[1..], CreationTargets::Reserved(&targets), 23)
                .is_err()
        );
        fs::remove_file(orphan).unwrap();
        assert_eq!(
            fixture
                .target_pool(&queued, CreationTargets::Reserved(&targets), 23)
                .unwrap()
                .1,
            fresh_charge
        );
        assert!(
            required_creation_bytes(&queued, CreationTargets::Reserved(&targets), u64::MAX)
                .is_err()
        );
        assert!(targets.iter().all(|target| !target.image_path.exists()));
    }

    #[test]
    fn reserved_creation_requires_exact_unique_unknown_bindings() {
        let fixture = Fixture::new();
        fixture.initialize();
        let targets = [
            fixture.spec(StorageKind::Workspace, "new"),
            fixture.spec(StorageKind::State, "new"),
        ];
        let rows: Vec<_> = targets.iter().map(reserved).collect();
        assert!(
            fixture
                .target_pool(&rows, CreationTargets::Reserved(&targets), 0)
                .is_ok()
        );
        for case in 0..10 {
            let mut invalid = rows.clone();
            match case {
                0 => {
                    invalid.pop();
                }
                1 => invalid[0].state = StorageState::Present,
                2 => invalid[0].kind = StorageKind::State,
                3 => invalid[0].bytes += 1,
                4 => invalid[0].binding = None,
                5 => invalid[0].binding.as_mut().unwrap().instance_id = "other".into(),
                6 => invalid[0].binding.as_mut().unwrap().owner_id = "other".into(),
                7 => invalid[0].binding.as_mut().unwrap().revision = "other".into(),
                8 => invalid.push(invalid[0].clone()),
                9 => invalid[0].id = "workspace:other".into(),
                _ => unreachable!(),
            }
            assert!(
                fixture
                    .target_pool(&invalid, CreationTargets::Reserved(&targets), 0)
                    .is_err(),
                "case {case}"
            );
        }
        for targets in [CreationTargets::New(&[]), CreationTargets::Reserved(&[])] {
            assert!(fixture.target_pool(&rows, targets, 0).is_err());
        }
        let duplicates = [targets[0].clone(), targets[0].clone()];
        assert!(
            fixture
                .target_pool(&[], CreationTargets::New(&duplicates), 0)
                .is_err()
        );
        assert!(
            fixture
                .target_pool(&rows, CreationTargets::Reserved(&duplicates), 0)
                .is_err()
        );
        let mut altered = targets.clone();
        altered[0].owner_uid += 1;
        assert!(
            creation_pool(
                &targets,
                &rows,
                CreationTargets::Reserved(&altered),
                0,
                &fixture.root,
                unsafe { libc::geteuid() },
                || fixture.open(&fixture.root, unsafe { libc::geteuid() })
            )
            .is_err()
        );
    }

    #[test]
    fn reserved_targets_refuse_final_and_staging_images_without_touching_them() {
        let fixture = Fixture::new();
        fixture.initialize();
        let targets = [
            fixture.spec(StorageKind::Workspace, "new"),
            fixture.spec(StorageKind::State, "new"),
        ];
        let rows: Vec<_> = targets.iter().map(reserved).collect();
        for target in &targets {
            let path = fixture.image(target.kind, "new", target.bytes);
            let before = fs::metadata(&path).unwrap();
            assert!(
                fixture
                    .target_pool(&rows, CreationTargets::Reserved(&targets), 0)
                    .is_err()
            );
            assert!(image_unchanged(
                &before,
                &fs::metadata(&path).unwrap(),
                true
            ));
            let staging = path.with_extension("img.new");
            fs::write(&staging, b"partial").unwrap();
            assert!(
                fixture
                    .target_pool(&rows, CreationTargets::Reserved(&targets), 0)
                    .is_err()
            );
            fs::remove_file(&path).unwrap();
            assert!(
                fixture
                    .target_pool(&rows, CreationTargets::Reserved(&targets), 0)
                    .is_err()
            );
            assert_eq!(fs::read(&staging).unwrap(), b"partial");
            assert!(!path.exists());
            fs::remove_file(staging).unwrap();
        }
        assert!(
            fixture
                .target_pool(&rows, CreationTargets::Reserved(&targets), 0)
                .is_ok()
        );
    }

    #[test]
    fn creation_counts_only_new_slots_against_the_inventory_limit() {
        let fixture = Fixture::new();
        let targets = [
            fixture.spec(StorageKind::Workspace, "new"),
            fixture.spec(StorageKind::State, "new"),
        ];
        let mut rows: Vec<_> = (0..MAX_STORAGE_ALLOCATIONS - 2)
            .map(|index| StorageAllocation {
                id: format!("workspace:old-{index}"),
                kind: StorageKind::Workspace,
                bytes: 1,
                binding: None,
                state: StorageState::Unknown,
            })
            .collect();
        let expected = (MAX_STORAGE_ALLOCATIONS - 2) as u64 + 2 * MIN_STORAGE_BYTES;
        assert_eq!(
            required_creation_bytes(&rows, CreationTargets::New(&targets), 0).unwrap(),
            expected
        );
        rows.extend(targets.iter().map(reserved));
        assert_eq!(
            required_creation_bytes(&rows, CreationTargets::Reserved(&targets), 0).unwrap(),
            expected
        );
        let other = fixture.spec(StorageKind::Workspace, "other");
        assert!(
            required_creation_bytes(&rows, CreationTargets::New(std::slice::from_ref(&other)), 0)
                .is_err()
        );
        rows.push(reserved(&other));
        assert!(required_creation_bytes(&rows, CreationTargets::Reserved(&targets), 0).is_err());
    }

    #[test]
    fn live_image_writes_are_allowed_but_size_and_permissions_are_not() {
        let fixture = Fixture::new();
        fixture.initialize();
        let path = fixture.image(StorageKind::Workspace, "old", MIN_STORAGE_BYTES);
        let retained = fixture.scan(&[]).unwrap().allocations;
        let before = fs::metadata(&path).unwrap();
        let file = OpenOptions::new().write(true).open(&path).unwrap();
        file.write_at(b"updated", 0).unwrap();
        file.set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_secs(1))
            .unwrap();
        let after = fs::metadata(&path).unwrap();
        assert!(image_unchanged(&before, &after, false));
        assert!(!image_unchanged(&before, &after, true));
        let new = fixture.spec(StorageKind::State, "new");
        assert!(fixture.creation_pool(&retained, &new).is_ok());
        file.set_len(MIN_STORAGE_BYTES + 1).unwrap();
        assert!(fixture.creation_pool(&retained, &new).is_err());
        file.set_len(MIN_STORAGE_BYTES).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o640)).unwrap();
        assert!(fixture.creation_pool(&retained, &new).is_err());
    }

    #[test]
    fn pool_identity_rejects_bind_mounts_even_when_device_matches() {
        let fixture = Fixture::new();
        fixture.initialize();
        let root = fixture
            .open(&fixture.root, unsafe { libc::geteuid() })
            .unwrap();
        let identity = pool_identity(&root).unwrap();
        let state = open_at(
            root.as_raw_fd(),
            OsStr::new("state"),
            libc::O_RDONLY | libc::O_DIRECTORY,
        )
        .unwrap();
        assert_eq!(identity, pool_identity(&state).unwrap());
        assert!(
            require_same_pool(
                identity,
                PoolIdentity {
                    mount_id: identity.mount_id.wrapping_add(1),
                    ..identity
                },
            )
            .is_err()
        );
        assert!(
            require_same_pool(
                identity,
                PoolIdentity {
                    device: identity.device.wrapping_add(1),
                    ..identity
                },
            )
            .is_err()
        );
        let new = fixture.spec(StorageKind::Workspace, "new");
        assert!(
            check_creation_at(
                std::slice::from_ref(&new),
                &[],
                CreationTargets::New(std::slice::from_ref(&new)),
                u64::MAX - new.bytes,
                &fixture.root,
                unsafe { libc::geteuid() },
                || fixture.open(&fixture.root, unsafe { libc::geteuid() }),
            )
            .is_err()
        );
        assert!(!new.image_path.exists());
    }
}
