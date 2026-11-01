//! Helpers shared by the CLI integration tests.

use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

/// Install an executable fixture script without ever holding a writable
/// descriptor to it in this multi-threaded test process. Otherwise a concurrent
/// test's fork can inherit that descriptor until its exec, and running the
/// script fails with ETXTBSY ("Text file busy"), making tests flaky.
pub fn write_executable(path: &Path, shell: &str, script: &str) {
    let mut child = Command::new(shell)
        .args([
            "-c",
            r#"umask 077 && cat > "$1" && chmod 700 "$1""#,
            "write-executable",
        ])
        .arg(path)
        .stdin(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(format!("#!{shell}\n{script}\n").as_bytes())
        .unwrap();
    assert!(child.wait().unwrap().success(), "write {}", path.display());
}
