//! Read-only inventory of the two fixed quota-image directories.
//!
//! The caller must fence every quota creator and privileged writer throughout
//! scanning and ledger initialization. Metadata checks detect observed changes;
//! they do not replace that fence, verify Btrfs, or establish physical capacity.

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::ffi::{CStr, CString, OsStr, OsString};
use std::fs::{File, Metadata};
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, IntoRawFd, RawFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::path::{Component, Path, PathBuf};
use tentaflake_admission::ledger::StorageInventory;
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
    scan_at(specs, Path::new(VOLUME_ROOT), 0, false)
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

fn open_at(directory: RawFd, name: &OsStr, flags: i32) -> Result<File, String> {
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

fn protected_directory(
    file: &File,
    owner_uid: u32,
    ancestor: bool,
    fixture: bool,
) -> Result<Metadata, String> {
    let meta = metadata(file)?;
    // Only private fixtures may traverse the root-owned sticky temporary root.
    let sticky_fixture = fixture && ancestor && meta.uid() == 0 && meta.mode() & 0o1000 != 0;
    if !meta.is_dir()
        || (meta.uid() != owner_uid && !(ancestor && meta.uid() == 0))
        || (meta.mode() & 0o022 != 0 && !sticky_fixture)
    {
        return Err("unprotected quota directory ancestry".into());
    }
    Ok(meta)
}

fn open_root(root: &Path, owner_uid: u32, fixture: bool) -> Result<File, String> {
    if !root.is_absolute() {
        return Err("quota root must be absolute".into());
    }
    let mut directory = open_at(
        libc::AT_FDCWD,
        OsStr::new("/"),
        libc::O_RDONLY | libc::O_DIRECTORY,
    )?;
    protected_directory(&directory, owner_uid, true, fixture)?;
    for component in root.components() {
        match component {
            Component::RootDir => {}
            Component::Normal(name) => {
                directory = open_at(
                    directory.as_raw_fd(),
                    name,
                    libc::O_RDONLY | libc::O_DIRECTORY,
                )?;
                protected_directory(&directory, owner_uid, true, fixture)?;
            }
            _ => return Err("invalid quota root components".into()),
        }
    }
    protected_directory(&directory, owner_uid, false, fixture)?;
    Ok(directory)
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

fn inspect_image(directory: &File, name: &OsStr, owner_uid: u32) -> Result<Metadata, String> {
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
    let readable = open_at(directory.as_raw_fd(), name, libc::O_RDONLY)?;
    if stamp(&meta) != stamp(&metadata(&readable)?) {
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

fn scan_at(
    specs: &[VolumeSpec],
    root: &Path,
    owner_uid: u32,
    fixture: bool,
) -> Result<StorageInventory, String> {
    let specs = declared_slots(specs, root)?;
    let directory = open_root(root, owner_uid, fixture)?;
    let before = metadata(&directory)?;
    let mut directories = vec![ScannedDirectory {
        file: directory,
        before,
        images: Vec::new(),
    }];
    let mut allocations = BTreeMap::new();
    // Exactly the workspace root and, when present, its state child.
    for index in 0..2 {
        if index == directories.len() {
            break;
        }
        let kind = if index == 0 {
            StorageKind::Workspace
        } else {
            StorageKind::State
        };
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
                let before = protected_directory(&file, owner_uid, false, fixture)?;
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
            let Some(slot) = name_text
                .strip_suffix(".img")
                .filter(|name| valid_slot_name(name))
            else {
                return Err("unexpected quota inventory entry".into());
            };
            if allocations.len() >= MAX_STORAGE_ALLOCATIONS {
                return Err("quota inventory exceeds allocation limit".into());
            }
            let meta = inspect_image(&directories[index].file, &name, owner_uid)?;
            let id = slot_id(kind, slot);
            let binding = specs
                .get(&id)
                .filter(|spec| spec.bytes == meta.len())
                .map(|spec| StorageBinding {
                    instance_id: spec.instance_id.clone(),
                    owner_id: spec.owner_id.clone(),
                    revision: spec.binding_revision.clone(),
                });
            allocations.insert(
                id.clone(),
                StorageAllocation {
                    id,
                    kind,
                    bytes: meta.len(),
                    binding,
                    state: StorageState::Unknown,
                },
            );
            directories[index].images.push((name, meta));
        }
    }
    for directory in &directories {
        for (name, before) in &directory.images {
            if stamp(before) != stamp(&inspect_image(&directory.file, name, owner_uid)?) {
                return Err("quota image changed during inventory".into());
            }
        }
        if stamp(&directory.before) != stamp(&metadata(&directory.file)?) {
            return Err("quota directory changed during inventory".into());
        }
    }
    // Also detect replacement of the fixed root while its original FD was held.
    if stamp(&directories[0].before) != stamp(&metadata(&open_root(root, owner_uid, fixture)?)?) {
        return Err("quota inventory root changed".into());
    }
    Ok(StorageInventory {
        complete: true,
        allocations: allocations.into_values().collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::{self, DirBuilder, OpenOptions};
    use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt, symlink};
    use std::sync::atomic::{AtomicU64, Ordering};

    struct Fixture(PathBuf);

    impl Fixture {
        fn new() -> Self {
            static SEQUENCE: AtomicU64 = AtomicU64::new(0);
            let root = std::env::temp_dir().join(format!(
                "tentaflake-quota-inventory-{}-{}",
                std::process::id(),
                SEQUENCE.fetch_add(1, Ordering::Relaxed)
            ));
            DirBuilder::new().mode(0o700).create(&root).unwrap();
            Self(root)
        }

        fn image(&self, kind: StorageKind, name: &str, bytes: u64) -> PathBuf {
            let path = image_path(&self.0, kind, name);
            if kind == StorageKind::State && !self.0.join("state").exists() {
                DirBuilder::new()
                    .mode(0o700)
                    .create(self.0.join("state"))
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
                image_path: image_path(&self.0, kind, name),
                mount_path: PathBuf::from(format!("/var/lib/{name}/{kind:?}")),
                bytes: MIN_STORAGE_BYTES,
                owner_uid: 1000,
                owner_gid: 1000,
                binding_revision: "binding-1".into(),
            }
        }

        fn scan(&self, specs: &[VolumeSpec]) -> Result<StorageInventory, String> {
            scan_at(specs, &self.0, unsafe { libc::geteuid() }, true)
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).unwrap();
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
        assert!(scan_at(&[], &fixture.0.join("absent"), uid, true).is_err());
        DirBuilder::new()
            .mode(0o700)
            .create(fixture.0.join("child"))
            .unwrap();
        fs::set_permissions(&fixture.0, fs::Permissions::from_mode(0o777)).unwrap();
        assert!(scan_at(&[], &fixture.0.join("child"), uid, true).is_err());
        fs::set_permissions(&fixture.0, fs::Permissions::from_mode(0o700)).unwrap();
        assert!(scan_at(&[], &fixture.0, uid.wrapping_add(1), true).is_err());
        symlink(fixture.0.join("child"), fixture.0.join("alias")).unwrap();
        assert!(scan_at(&[], &fixture.0.join("alias"), uid, true).is_err());
    }

    #[test]
    fn refuses_partial_unknown_and_nonregular_entries() {
        for name in ["agent-a.img.new", "notes", "bad_name.img", "UPPER.img"] {
            let fixture = Fixture::new();
            fs::write(fixture.0.join(name), b"x").unwrap();
            assert!(fixture.scan(&[]).is_err(), "accepted {name}");
        }
        let fixture = Fixture::new();
        DirBuilder::new()
            .mode(0o700)
            .create(fixture.0.join("nested"))
            .unwrap();
        assert!(fixture.scan(&[]).is_err());
        fs::remove_dir(fixture.0.join("nested")).unwrap();
        symlink("missing", fixture.0.join("agent-a.img")).unwrap();
        assert!(fixture.scan(&[]).is_err());
        fs::remove_file(fixture.0.join("agent-a.img")).unwrap();
        symlink("missing", fixture.0.join("state")).unwrap();
        assert!(fixture.scan(&[]).is_err());
    }

    #[test]
    fn refuses_fifo_and_directory_images_without_reading_them() {
        let fixture = Fixture::new();
        let fifo = fixture.0.join("agent-a.img");
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
        fs::set_permissions(fixture.0.join("state"), fs::Permissions::from_mode(0o777)).unwrap();
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
        fs::hard_link(&image, fixture.0.join("agent-b.img")).unwrap();
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
                2 => spec.image_path = fixture.0.join("../agent-a.img"),
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
        assert_eq!(fs::read_dir(&fixture.0).unwrap().count(), 0);
    }
}
