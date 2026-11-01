//! Nonblocking local telemetry publication: one deadline covers all socket I/O.
use super::{ActivityConfig, peer_uid};
use serde_json::Value;
use std::io::{Read, Write};
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::net::UnixStream;
use std::time::{Duration, Instant};

type Result<T, E = &'static str> = std::result::Result<T, E>;
fn ready(stream: &UnixStream, events: libc::c_short, deadline: Instant) -> Result<()> {
    loop {
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .ok_or("activity deadline")?;
        let mut poll = libc::pollfd {
            fd: stream.as_raw_fd(),
            events,
            revents: 0,
        };
        let milliseconds = remaining.as_millis().min(i32::MAX as u128) as i32;
        if milliseconds == 0 {
            return Err("activity deadline");
        }
        let status = unsafe { libc::poll(&mut poll, 1, milliseconds) };
        if status > 0 && poll.revents & events != 0 {
            return Ok(());
        }
        if status < 0 && std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
            continue;
        }
        return Err("activity unavailable");
    }
}
fn connect(path: &str, deadline: Instant) -> Result<UnixStream> {
    let mut address: libc::sockaddr_un = unsafe { std::mem::zeroed() };
    let bytes = path.as_bytes();
    if !path.starts_with('/') || bytes.contains(&0) || bytes.len() >= address.sun_path.len() {
        return Err("activity path");
    }
    address.sun_family = libc::AF_UNIX as libc::sa_family_t;
    for (target, source) in address.sun_path.iter_mut().zip(bytes) {
        *target = *source as libc::c_char;
    }
    let fd = unsafe {
        libc::socket(
            libc::AF_UNIX,
            libc::SOCK_STREAM | libc::SOCK_NONBLOCK | libc::SOCK_CLOEXEC,
            0,
        )
    };
    if fd < 0 {
        return Err("activity unavailable");
    }
    let stream = unsafe { UnixStream::from_raw_fd(fd) };
    let status = unsafe {
        libc::connect(
            fd,
            &address as *const _ as *const libc::sockaddr,
            std::mem::size_of_val(&address) as libc::socklen_t,
        )
    };
    if status != 0 {
        // AF_UNIX full backlog returns EAGAIN: fail immediately, without retry.
        if std::io::Error::last_os_error().raw_os_error() != Some(libc::EINPROGRESS) {
            return Err("activity unavailable");
        }
        ready(&stream, libc::POLLOUT, deadline)?;
        if stream
            .take_error()
            .map_err(|_| "activity unavailable")?
            .is_some()
        {
            return Err("activity unavailable");
        }
    }
    if Instant::now() >= deadline {
        return Err("activity deadline");
    }
    Ok(stream)
}
fn send(stream: &mut UnixStream, mut bytes: &[u8], deadline: Instant) -> Result<()> {
    while !bytes.is_empty() {
        ready(stream, libc::POLLOUT, deadline)?;
        match stream.write(bytes) {
            Ok(0) => return Err("activity closed"),
            Ok(n) => bytes = &bytes[n..],
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted
                ) => {}
            Err(_) => return Err("activity unavailable"),
        }
    }
    Ok(())
}
fn receive(stream: &mut UnixStream, mut bytes: &mut [u8], deadline: Instant) -> Result<()> {
    while !bytes.is_empty() {
        ready(stream, libc::POLLIN, deadline)?;
        match stream.read(bytes) {
            Ok(0) => return Err("activity closed"),
            Ok(n) => bytes = &mut bytes[n..],
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted
                ) => {}
            Err(_) => return Err("activity unavailable"),
        }
    }
    Ok(())
}
pub(super) fn publish(config: &ActivityConfig, message: &Value, budget: Duration) -> Result<()> {
    let deadline = Instant::now()
        .checked_add(budget)
        .ok_or("activity deadline")?;
    let bytes = serde_json::to_vec(message).map_err(|_| "activity unavailable")?;
    if bytes.len() > 65536 {
        return Err("activity bounds");
    }
    let mut stream = connect(&config.socket, deadline)?;
    if peer_uid(&stream)? != config.server_uid {
        return Err("activity peer denied");
    }
    send(&mut stream, &(bytes.len() as u32).to_be_bytes(), deadline)?;
    send(&mut stream, &bytes, deadline)?;
    let mut header = [0; 4];
    receive(&mut stream, &mut header, deadline)?;
    let length = u32::from_be_bytes(header) as usize;
    if length > 65536 {
        return Err("activity bounds");
    }
    let mut bytes = vec![0; length];
    receive(&mut stream, &mut bytes, deadline)?;
    let response: Value = serde_json::from_slice(&bytes).map_err(|_| "activity unavailable")?;
    if response["version"] != 1 || response["status"] != "ok" {
        return Err("activity rejected");
    }
    Ok(())
}
