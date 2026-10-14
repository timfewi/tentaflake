//! Local runtime transport. A client failure never proves daemon cancellation.
use crate::{Config, Result};
use std::io::{self, Read};
use std::os::fd::AsRawFd;
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, Instant};

const COMMAND_TIMEOUT: Duration = Duration::from_secs(10);
const COMMAND_BYTES: usize = 64 * 1024;

pub(super) fn command(config: &Config) -> Command {
    let mut command = Command::new(&config.runtime);
    command.env_clear().stdin(Stdio::null());
    if config.backend == "docker" {
        command.args(["--host=unix:///run/docker.sock", "--config=/var/empty"]);
    } else {
        // Preserve the existing rootful Podman backend, without inherited
        // remote connections or per-user configuration.
        command
            .args(["--remote=false"])
            .env("HOME", "/var/empty")
            .env("XDG_CONFIG_HOME", "/var/empty");
    }
    command
}

pub(super) fn local_image(config: &Config) -> Result<String> {
    let output = output(command(config).args([
        "image",
        "inspect",
        "--format={{.Id}}",
        "--",
        &config.image,
    ]))?;
    if !output.status.success() {
        return Err("worker image is not available in the local runtime".into());
    }
    parse_image_id(&output.stdout)
}

fn parse_image_id(bytes: &[u8]) -> Result<String> {
    let value = std::str::from_utf8(bytes).map_err(|_| "invalid local worker image identity")?;
    let value = value.strip_suffix('\n').unwrap_or(value);
    let digest = value.strip_prefix("sha256:").unwrap_or(value);
    if digest.len() != 64
        || !digest
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err("local worker image must have a complete SHA-256 identity".into());
    }
    Ok(value.to_owned())
}

pub(super) fn container_id(bytes: &[u8]) -> Result<String> {
    let value = std::str::from_utf8(bytes).map_err(|_| "invalid worker container identity")?;
    let value = value.strip_suffix('\n').unwrap_or(value);
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(
            "worker create returned no complete container identity; dispatch intent retained"
                .into(),
        );
    }
    Ok(value.to_owned())
}

pub(super) fn output(command: &mut Command) -> Result<Output> {
    let (output, truncated) = capture(command, COMMAND_BYTES, COMMAND_TIMEOUT)?;
    if truncated {
        return Err("runtime response exceeded its byte limit; outcome remains unknown".into());
    }
    Ok(output)
}

pub(super) fn logs(config: &Config, container: &str) -> Result<(Output, bool)> {
    capture(
        command(config).args(["logs", container]),
        config.max_log_bytes,
        COMMAND_TIMEOUT,
    )
}

struct RunningChild(Child);

impl Drop for RunningChild {
    fn drop(&mut self) {
        // Only this CLI process is owned here. Killing it does not cancel a
        // Docker/Podman daemon operation or authorize releasing its reservation.
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn nonblocking(pipe: &impl AsRawFd) -> Result<()> {
    let flags = unsafe { libc::fcntl(pipe.as_raw_fd(), libc::F_GETFL) };
    if flags < 0
        || unsafe { libc::fcntl(pipe.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0
    {
        return Err("cannot bound runtime response".into());
    }
    Ok(())
}

fn capture(command: &mut Command, limit: usize, timeout: Duration) -> Result<(Output, bool)> {
    let mut child = RunningChild(
        command
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|_| "cannot launch local runtime command")?,
    );
    let mut stdout = child.0.stdout.take().ok_or("missing runtime stdout")?;
    let mut stderr = child.0.stderr.take().ok_or("missing runtime stderr")?;
    nonblocking(&stdout)?;
    nonblocking(&stderr)?;
    let deadline = Instant::now() + timeout;
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let (mut out_eof, mut err_eof) = (false, false);
    let mut status = None;
    let mut remaining = limit;
    loop {
        let mut progressed = false;
        let mut truncated = false;
        for (pipe, bytes, eof) in [
            (&mut stdout as &mut dyn Read, &mut out, &mut out_eof),
            (&mut stderr as &mut dyn Read, &mut err, &mut err_eof),
        ] {
            if *eof {
                continue;
            }
            let mut chunk = [0_u8; 8192];
            match pipe.read(&mut chunk) {
                Ok(0) => *eof = true,
                Ok(n) => {
                    let keep = n.min(remaining);
                    bytes.extend_from_slice(&chunk[..keep]);
                    remaining -= keep;
                    progressed = true;
                    if n > keep {
                        truncated = true;
                        break;
                    }
                }
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                Err(_) => return Err("cannot read runtime response".into()),
            }
        }
        if truncated {
            let _ = child.0.kill();
            let status = child.0.wait().map_err(|_| "cannot reap runtime command")?;
            return Ok((
                Output {
                    status,
                    stdout: out,
                    stderr: err,
                },
                true,
            ));
        }
        if status.is_none() {
            status = child
                .0
                .try_wait()
                .map_err(|_| "cannot observe runtime command")?;
        }
        if out_eof
            && err_eof
            && let Some(status) = status
        {
            return Ok((
                Output {
                    status,
                    stdout: out,
                    stderr: err,
                },
                false,
            ));
        }
        if Instant::now() >= deadline {
            return Err("runtime command timed out; outcome remains unknown".into());
        }
        if !progressed {
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shell(script: &str) -> Command {
        let path = std::env::split_paths(&std::env::var_os("PATH").expect("contributor PATH"))
            .map(|path| path.join("bash"))
            .find(|path| path.is_file())
            .expect("pinned contributor bash");
        let mut command = Command::new(path);
        command
            .env_clear()
            .args(["-c", script])
            .stdin(Stdio::null());
        command
    }

    #[test]
    fn accepts_only_complete_local_image_identities() {
        let id = "a1".repeat(32);
        assert_eq!(
            parse_image_id(format!("sha256:{id}\n").as_bytes()).unwrap(),
            format!("sha256:{id}")
        );
        assert_eq!(parse_image_id(id.as_bytes()).unwrap(), id);
        for invalid in [
            "fixture:latest",
            "sha256:abc",
            "",
            &"A".repeat(64),
            &format!("{id}\n{id}"),
            &format!("{id}\n\n"),
        ] {
            assert!(parse_image_id(invalid.as_bytes()).is_err());
        }
    }

    #[test]
    fn captures_both_pipes_and_preserves_exit_status() {
        let (output, truncated) = capture(
            &mut shell("printf abc; printf def >&2; exit 7"),
            6,
            Duration::from_secs(2),
        )
        .unwrap();
        assert_eq!(output.stdout, b"abc");
        assert_eq!(output.stderr, b"def");
        assert_eq!(output.status.code(), Some(7));
        assert!(!truncated);
    }

    #[test]
    fn caps_combined_output_and_terminates_continuous_writer() {
        let started = Instant::now();
        let (output, truncated) = capture(
            &mut shell("while :; do printf aaaaaaaa; printf bbbbbbbb >&2; done"),
            100,
            Duration::from_secs(2),
        )
        .unwrap();
        assert!(truncated);
        assert_eq!(output.stdout.len() + output.stderr.len(), 100);
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn silent_runtime_has_a_deadline() {
        let started = Instant::now();
        let error = capture(
            &mut shell("while :; do :; done"),
            100,
            Duration::from_millis(50),
        )
        .unwrap_err();
        assert!(error.contains("timed out"));
        assert!(started.elapsed() < Duration::from_secs(2));
    }
}
