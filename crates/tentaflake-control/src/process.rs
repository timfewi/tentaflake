use crate::storage::Result;
use std::io::Read;
use std::os::fd::AsRawFd;
use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

pub struct Output {
    pub success: bool,
    pub stdout: Vec<u8>,
}

/// No shell, inherited environment, unbounded pipe readers or detached descendants.
pub fn run(command: &mut Command, deadline: Duration, limit: usize) -> Result<Vec<u8>> {
    let output = capture(command, deadline, limit)?;
    if output.success {
        Ok(output.stdout)
    } else {
        Err("fixed executable failed".into())
    }
}

/// Capture only bounded stdout and exit status; stderr is counted but never exposed.
pub fn capture(command: &mut Command, deadline: Duration, limit: usize) -> Result<Output> {
    let home = command
        .get_current_dir()
        .unwrap_or(std::path::Path::new("/var/empty"))
        .to_path_buf();
    command
        .env_clear()
        .env("NIX_REMOTE", "daemon")
        .env("HOME", &home)
        .env("XDG_CACHE_HOME", home.join("cache"))
        .env("NIX_CONFIG", "sandbox = true\nfallback = false\n")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0);
    let mut child = command
        .spawn()
        .map_err(|_| "fixed executable unavailable")?;
    let group = child.id() as i32;
    let mut stdout = child.stdout.take().ok_or("missing output pipe")?;
    let mut stderr = child.stderr.take().ok_or("missing error pipe")?;
    for fd in [stdout.as_raw_fd(), stderr.as_raw_fd()] {
        if unsafe { libc::fcntl(fd, libc::F_SETFL, libc::O_NONBLOCK) } < 0 {
            unsafe {
                libc::kill(-group, libc::SIGKILL);
            }
            let _ = child.wait();
            return Err("cannot bound executable output".into());
        }
    }
    let start = Instant::now();
    let mut output = Vec::new();
    let mut total = 0usize;
    let mut buffer = [0u8; 8192];
    let mut exited = None;
    let mut done = [false; 2];
    let result = loop {
        let mut io_error = false;
        for (i, pipe) in [&mut stdout as &mut dyn Read, &mut stderr as &mut dyn Read]
            .into_iter()
            .enumerate()
        {
            // One read per pipe per iteration prevents continuous writers defeating the deadline.
            match pipe.read(&mut buffer) {
                Ok(0) => done[i] = true,
                Ok(n) => {
                    total += n;
                    if i == 0 {
                        output.extend_from_slice(&buffer[..n]);
                    }
                }
                Err(e)
                    if matches!(
                        e.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted
                    ) => {}
                Err(_) => io_error = true,
            }
        }
        if io_error || total > limit {
            break Err("executable output refused".into());
        }
        if start.elapsed() >= deadline {
            break Err("executable deadline exceeded".into());
        }
        if exited.is_none() {
            match child.try_wait() {
                Ok(Some(status)) => {
                    // Terminate inherited pipe holders, then account for both finite remaining pipes.
                    unsafe {
                        libc::kill(-group, libc::SIGKILL);
                    }
                    exited = Some(status.success());
                }
                Ok(None) => (),
                Err(_) => break Err("executable status unavailable".into()),
            }
        }
        if let Some(success) = exited
            && done.iter().all(|v| *v)
        {
            break Ok(Output {
                success,
                stdout: output,
            });
        }
        std::thread::sleep(Duration::from_millis(2));
    };
    unsafe {
        libc::kill(-group, libc::SIGKILL);
    }
    let _ = child.wait();
    result
}
