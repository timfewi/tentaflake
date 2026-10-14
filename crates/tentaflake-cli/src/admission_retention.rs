//! Retain immutable authority closures before their first durable ledger reference.
//! Nix owns GC synchronization; a hand-made symlink is not registration evidence.
use crate::admission_host::{load_plan, store_path};
use crate::admission_runtime::authority;
use std::ffi::{CString, OsStr};
use std::fs::File;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;
use tentaflake_admission::host_fence::HostFence;
use tentaflake_admission::ledger::PublishedAuthority;
use tentaflake_admission::protected_path::{open_at, open_root, protected_directory};

const PARENT: &str = "/nix/var/nix/gcroots";
const DIRECTORY: &str = "tentaflake";

fn object(manifest: &Path) -> Result<PathBuf, String> {
    if !store_path(manifest) {
        return Err("retained manifest must be a normalized immutable store file".into());
    }
    let object: PathBuf = manifest.components().take(4).collect();
    if object.as_os_str().as_bytes().ends_with(b".drv")
        || !object
            .file_name()
            .expect("validated store object")
            .as_bytes()
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || b"+._-".contains(byte))
    {
        return Err("manifest retention never realizes derivations".into());
    }
    Ok(object)
}

fn directory(parent: &File, owner: u32) -> Result<File, String> {
    protected_directory(parent, owner)?;
    let name = CString::new(DIRECTORY).expect("fixed name");
    let created = unsafe { libc::mkdirat(parent.as_raw_fd(), name.as_ptr(), 0o700) } == 0;
    if !created && io::Error::last_os_error().raw_os_error() != Some(libc::EEXIST) {
        return Err("cannot create manifest retention directory".into());
    }
    let directory = open_at(
        parent.as_raw_fd(),
        OsStr::new(DIRECTORY),
        libc::O_RDONLY | libc::O_DIRECTORY,
    )?;
    let metadata = protected_directory(&directory, owner)?;
    if created {
        if unsafe { libc::fchmod(directory.as_raw_fd(), 0o700) } != 0 {
            return Err("cannot protect new manifest retention directory".into());
        }
        directory
            .sync_all()
            .map_err(|_| "cannot sync retention directory")?;
        parent
            .sync_all()
            .map_err(|_| "cannot sync retention parent")?;
    } else if metadata.mode() & 0o7777 != 0o700 {
        return Err("existing manifest retention directory must have mode 0700".into());
    }
    Ok(directory)
}

fn inspect(directory: &File, name: &OsStr, target: &Path, owner: u32) -> Result<bool, String> {
    let name = CString::new(name.as_bytes()).map_err(|_| "invalid retention root name")?;
    let fd = unsafe {
        libc::openat(
            directory.as_raw_fd(),
            name.as_ptr(),
            libc::O_PATH | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        )
    };
    if fd < 0 {
        return if io::Error::last_os_error().raw_os_error() == Some(libc::ENOENT) {
            Ok(false)
        } else {
            Err("cannot inspect manifest retention root".into())
        };
    }
    let link = unsafe { File::from_raw_fd(fd) };
    let metadata = link
        .metadata()
        .map_err(|_| "cannot inspect retention metadata")?;
    if !metadata.file_type().is_symlink() || metadata.uid() != owner || metadata.nlink() != 1 {
        return Err("manifest retention root must be an owned single-link symlink".into());
    }
    let mut bytes = [0_u8; 4097];
    let length = unsafe {
        libc::readlinkat(
            link.as_raw_fd(),
            c"".as_ptr(),
            bytes.as_mut_ptr().cast(),
            bytes.len(),
        )
    };
    if length < 0 || &bytes[..length as usize] != target.as_os_str().as_bytes() {
        return Err("manifest retention root has an unexpected target".into());
    }
    Ok(true)
}

fn retain_at(
    parent: &File,
    root: &Path,
    target: &Path,
    owner: u32,
    register: impl FnOnce(&Path, &Path) -> Result<(), String>,
) -> Result<(), String> {
    let directory = directory(parent, owner)?;
    let name = target
        .file_name()
        .ok_or("missing retained store object name")?;
    inspect(&directory, name, target, owner)?;
    // Even a matching existing symlink uses Nix's registration protocol. Never
    // replace a mismatched entry or remove a root after uncertain registration.
    register(&root.join(name), target)?;
    let reopened = open_at(
        parent.as_raw_fd(),
        OsStr::new(DIRECTORY),
        libc::O_RDONLY | libc::O_DIRECTORY,
    )?;
    let before = protected_directory(&directory, owner)?;
    let after = protected_directory(&reopened, owner)?;
    if (before.dev(), before.ino()) != (after.dev(), after.ino())
        || after.mode() & 0o7777 != 0o700
        || !inspect(&reopened, name, target, owner)?
    {
        return Err("manifest retention root changed during registration".into());
    }
    reopened
        .sync_all()
        .map_err(|_| "cannot sync registered manifest root")?;
    parent
        .sync_all()
        .map_err(|_| "cannot sync registered manifest parent")?;
    Ok(())
}

/// Caller authenticates current installation/publication under this fence and
/// rechecks them before committing. Candidate authority may not yet be installed.
/// Retention is conservative and never grants runtime or resource authority.
pub(crate) fn retain(
    expected: &PublishedAuthority,
    nix_store: &Path,
    _fence: &HostFence,
) -> Result<(), String> {
    let manifest = Path::new(&expected.manifest_path);
    let target = object(manifest)?;
    if !store_path(nix_store) || nix_store.file_name() != Some(OsStr::new("nix-store")) {
        return Err("manifest retention requires the declared immutable Nix tool".into());
    }
    let verify = || -> Result<(), String> {
        if authority(&load_plan(manifest)?, manifest) != *expected {
            return Err("retained manifest does not match its recorded authority".into());
        }
        Ok(())
    };
    verify()?;
    let parent = open_root(Path::new(PARENT), 0)?;
    retain_at(
        &parent,
        &Path::new(PARENT).join(DIRECTORY),
        &target,
        0,
        |root, target| {
            let mut command = Command::new(nix_store);
            command
                .env_clear()
                .env("NIX_REMOTE", "daemon")
                .stdin(Stdio::null())
                .args([
                    "--option",
                    "substitute",
                    "false",
                    "--option",
                    "substituters",
                    "",
                    "--option",
                    "builders",
                    "",
                    "--option",
                    "max-jobs",
                    "0",
                    "--add-root",
                ])
                .arg(root)
                .arg("--realise")
                .arg(target);
            let output = crate::output_with_timeout(&mut command, Duration::from_secs(10))
                .map_err(
                    |_| "manifest root registration did not finish; preserve any created root",
                )?;
            if !output.status.success() {
                return Err("manifest root registration failed; preserve any created root".into());
            }
            Ok(())
        },
    )?;
    // A privileged directory replacement or manifest change is never success.
    let reopened = open_root(Path::new(PARENT), 0)?;
    let before = parent
        .metadata()
        .map_err(|_| "cannot inspect retention parent")?;
    let after = reopened
        .metadata()
        .map_err(|_| "cannot inspect retention parent")?;
    if (before.dev(), before.ino()) != (after.dev(), after.ino()) {
        return Err("manifest retention ancestry changed".into());
    }
    verify()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::{PermissionsExt, symlink};
    use std::sync::atomic::{AtomicU64, Ordering};

    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!(
                "tentaflake-retention-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path).unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
            Self(path)
        }
        fn run(
            &self,
            register: impl FnOnce(&Path, &Path) -> Result<(), String>,
        ) -> Result<(), String> {
            let directory = File::open(&self.0).unwrap();
            retain_at(
                &directory,
                &self.0.join(DIRECTORY),
                &self.0.join("manifest"),
                unsafe { libc::geteuid() },
                register,
            )
        }
        fn root(&self) -> PathBuf {
            self.0.join(DIRECTORY).join("manifest")
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    fn register(root: &Path, target: &Path) -> Result<(), String> {
        symlink(target, root).map_err(|_| "fixture registration failed".into())
    }

    #[test]
    fn exact_roots_survive_registration_failures_and_repeat_without_replacement() {
        let f = Fixture::new();
        fs::write(f.0.join("manifest"), b"immutable fixture").unwrap();
        assert!(
            f.run(|root, target| {
                register(root, target)?;
                Err("lost reply".into())
            })
            .is_err()
        );
        assert_eq!(fs::read_link(f.root()).unwrap(), f.0.join("manifest"));
        let before = fs::symlink_metadata(f.root()).unwrap().ino();
        f.run(|_, _| Ok(())).unwrap();
        assert_eq!(before, fs::symlink_metadata(f.root()).unwrap().ino());
    }

    #[test]
    fn conflicting_files_and_links_refuse_before_registration_without_repair() {
        for link in [false, true] {
            let f = Fixture::new();
            fs::create_dir(f.0.join(DIRECTORY)).unwrap();
            fs::set_permissions(f.0.join(DIRECTORY), fs::Permissions::from_mode(0o700)).unwrap();
            if link {
                symlink(f.0.join("foreign"), f.root()).unwrap();
            } else {
                fs::write(f.root(), b"owned data").unwrap();
            }
            assert!(
                f.run(|_, _| panic!("conflict reached registration"))
                    .is_err()
            );
            assert!(fs::symlink_metadata(f.root()).is_ok());
        }
    }

    #[test]
    fn unsafe_root_directory_and_replacement_during_registration_refuse() {
        for mode in [0o755, 0o770, 0o1777] {
            let f = Fixture::new();
            fs::create_dir(f.0.join(DIRECTORY)).unwrap();
            fs::set_permissions(f.0.join(DIRECTORY), fs::Permissions::from_mode(mode)).unwrap();
            assert!(
                f.run(|_, _| panic!("unsafe root reached registration"))
                    .is_err()
            );
        }
        let f = Fixture::new();
        assert!(
            f.run(|root, target| {
                fs::rename(f.0.join(DIRECTORY), f.0.join("old")).unwrap();
                fs::create_dir(f.0.join(DIRECTORY)).unwrap();
                fs::set_permissions(f.0.join(DIRECTORY), fs::Permissions::from_mode(0o700))
                    .unwrap();
                register(root, target)
            })
            .is_err()
        );
        // Registration uncertainty never removes either directory or its root.
        assert!(f.0.join("old").is_dir());
        assert!(fs::symlink_metadata(f.root()).is_ok());
    }

    #[test]
    fn manifest_objects_reject_derivations_aliases_and_non_store_paths() {
        let base = "/nix/store/00000000000000000000000000000000-plan";
        assert_eq!(
            object(Path::new(&format!("{base}/admission.json"))).unwrap(),
            Path::new(base)
        );
        for path in [
            format!("{base}.drv"),
            format!("{base}/../plan"),
            "/tmp/manifest".into(),
            format!("{base}^out"),
        ] {
            assert!(object(Path::new(&path)).is_err(), "{path}");
        }
    }
}
